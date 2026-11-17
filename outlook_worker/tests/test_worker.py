"""Tests for outlook_worker.py using fake COM objects (no Outlook required).

Run:  python -m unittest discover -s outlook_worker/tests -v
"""

import datetime as dt
import json
import os
import subprocess
import sys
import tempfile
import types
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER_DIR = os.path.dirname(HERE)
sys.path.insert(0, WORKER_DIR)

import outlook_worker as w  # noqa: E402


# ----------------------------------------------------------------- fakes

class FakeCollection:
    def __init__(self, items):
        self._items = list(items)

    @property
    def Count(self):
        return len(self._items)

    def Item(self, i):
        return self._items[i - 1]


class FakeItems:
    def __init__(self, items):
        self._items = list(items)
        self.filters = []
        self._i = 0

    def Restrict(self, flt):
        self.filters.append(flt)
        return FakeItems(self._items)

    def Sort(self, key, desc):
        self._items.sort(key=lambda m: m.ReceivedTime, reverse=desc)

    def GetFirst(self):
        self._i = 0
        return self.GetNext()

    def GetNext(self):
        if self._i >= len(self._items):
            return None
        it = self._items[self._i]
        self._i += 1
        return it


class FakeAccessor:
    def __init__(self, props=None):
        self.props = dict(props or {})

    def GetProperty(self, name):
        if name not in self.props:
            raise Exception("property not found")
        return self.props[name]

    def SetProperty(self, name, value):
        self.props[name] = value


class FakeAttachment:
    def __init__(self, path):
        self.path = path
        self.PropertyAccessor = FakeAccessor()


class FakeAttachments:
    def __init__(self):
        self.items = []

    @property
    def Count(self):
        return len(self.items)

    def Add(self, path, kind, pos):
        a = FakeAttachment(path)
        self.items.append(a)
        return a


class FakeMail:
    Class = 43

    def __init__(self, entry_id="E1", subject="Hello", body="Body", received=None, sender="a@b.org", conv="C1", imid="<m1@x>"):
        self.EntryID = entry_id
        self.Subject = subject
        self.Body = body
        self.HTMLBody = "<html><body><p>quoted original</p></body></html>"
        self.ReceivedTime = received or dt.datetime(2026, 11, 16, 9, 30, 0)
        self.SenderName = "Doe, Jane"
        self.SenderEmailAddress = sender
        self.SenderEmailType = "SMTP"
        self.ConversationID = conv
        self.ConversationTopic = subject
        self.Attachments = FakeAttachments()
        self.PropertyAccessor = FakeAccessor({w.PR_INTERNET_MESSAGE_ID: imid})
        self.saved = False
        self.To = ""
        self.UnRead = True

    def Reply(self):
        r = FakeMail(entry_id="", subject="RE: " + self.Subject, imid="")
        r.HTMLBody = "<html><body class='x'><div>signature</div><hr>quoted</body></html>"
        return r

    def Save(self):
        self.saved = True
        if not self.EntryID:
            self.EntryID = "DRAFT-1"

    def Send(self):
        raise AssertionError("Send() must never be called")

    def Delete(self):
        raise AssertionError("Delete() must never be called")

    def Move(self, *_):
        raise AssertionError("Move() must never be called")

    def Display(self, modal=False):
        self.displayed = True


class FakeNonMail:
    Class = 46  # report item
    ReceivedTime = dt.datetime(2026, 11, 16, 8, 0, 0)


class FakeFolder:
    Class = 2

    def __init__(self, name, entry_id, parent=None, items=(), store_id="S1"):
        self.Name = name
        self.EntryID = entry_id
        self.Parent = parent
        self.StoreID = store_id
        self.DefaultItemType = 0
        self._children = []
        self.Items = FakeItems(items)
        if parent is not None and isinstance(parent, FakeFolder):
            parent._children.append(self)

    @property
    def Folders(self):
        return FakeCollection(self._children)


class FakeNamespaceObj:
    Class = 1


class FakeStore:
    def __init__(self, name, store_id, root):
        self.DisplayName = name
        self.StoreID = store_id
        self.ExchangeStoreType = 1
        self.IsDataFileStore = False
        self._root = root

    def GetRootFolder(self):
        return self._root


class FakeNS:
    CurrentProfileName = "Outlook"

    def __init__(self, stores, folders_by_id, items_by_id):
        self.Stores = FakeCollection(stores)
        self.DefaultStore = stores[0]
        self._folders = folders_by_id
        self._items = items_by_id

    def GetFolderFromID(self, entry_id, store_id=None):
        if entry_id not in self._folders:
            raise Exception("not found")
        return self._folders[entry_id]

    def GetItemFromID(self, entry_id, store_id=None):
        if entry_id not in self._items:
            raise Exception("not found")
        return self._items[entry_id]


class FakeApp:
    Version = "16.0"

    def __init__(self, ns):
        self.ns = ns
        self.created = []

    def GetNamespace(self, name):
        assert name == "MAPI"
        return self.ns

    def CreateItem(self, kind):
        m = FakeMail(entry_id="", subject="", imid="")
        m.HTMLBody = "<html><body></body></html>"
        self.created.append(m)
        return m


def build_world():
    nsobj = FakeNamespaceObj()
    root = FakeFolder("Mailbox - Team", "ROOT", parent=nsobj)
    inbox = FakeFolder("Inbox", "INBOX", parent=root)
    mails = [
        FakeMail("E1", "School Subtype Error", "School ID: 123456 missing subtype", conv="C1", imid="<m1@x>"),
        FakeMail("E2", "Webinar", "black screen", received=dt.datetime(2026, 11, 16, 10, 0, 0), conv="C2", imid="<m2@x>"),
    ]
    wen = FakeFolder("Wenjie", "WEN", parent=inbox, items=mails + [FakeNonMail()])
    sub = FakeFolder("School Subtype Needed", "SUB", parent=wen, items=[FakeMail("E3", "Sub", "x", conv="C3", imid="<m3@x>")])
    store = FakeStore("DOH-Schoolimmforms-team", "S1", root)
    ns = FakeNS([store], {"ROOT": root, "INBOX": inbox, "WEN": wen, "SUB": sub}, {m.EntryID: m for m in mails})
    return FakeApp(ns), mails


class FakeComError(Exception):
    def __init__(self, hresult):
        super().__init__("Server execution failed")
        self.hresult = hresult


def install_fake_com(app=None, dispatch_error=False, hresult=None):
    calls = {"init": 0, "uninit": 0}
    pythoncom = types.ModuleType("pythoncom")
    pythoncom.CoInitialize = lambda: calls.__setitem__("init", calls["init"] + 1)
    pythoncom.CoUninitialize = lambda: calls.__setitem__("uninit", calls["uninit"] + 1)
    client = types.ModuleType("win32com.client")

    def dispatch(name):
        if hresult is not None:
            raise FakeComError(hresult)
        if dispatch_error:
            raise Exception("Server execution failed")
        assert name == "Outlook.Application"
        return app

    client.Dispatch = dispatch
    win32com = types.ModuleType("win32com")
    win32com.client = client
    sys.modules["pythoncom"] = pythoncom
    sys.modules["win32com"] = win32com
    sys.modules["win32com.client"] = client
    return calls


class WorkerTests(unittest.TestCase):
    def setUp(self):
        self._saved = {k: sys.modules.get(k) for k in ("pythoncom", "win32com", "win32com.client")}
        self.app, self.mails = build_world()
        self.calls = install_fake_com(self.app)

    def tearDown(self):
        for k, v in self._saved.items():
            if v is None:
                sys.modules.pop(k, None)
            else:
                sys.modules[k] = v

    def run_op(self, op, params=None):
        return w.handle({"op": op, "params": params or {}})

    def test_connection_and_com_lifecycle(self):
        r = self.run_op("test_connection")
        self.assertTrue(r["ok"], r)
        self.assertEqual(r["data"]["outlook_version"], "16.0")
        self.assertEqual(self.calls["init"], 1)
        self.assertEqual(self.calls["uninit"], 1)

    def test_outlook_unavailable(self):
        install_fake_com(dispatch_error=True)
        r = self.run_op("test_connection")
        self.assertFalse(r["ok"])
        self.assertEqual(r["error_code"], "OUTLOOK_UNAVAILABLE")
        self.assertTrue(r["message"])

    def test_outlook_unavailable_elevation_mismatch(self):
        # pywin32 reports CO_E_SERVER_EXEC_FAILURE as a signed int
        install_fake_com(hresult=0x80080005 - 2**32)
        r = self.run_op("test_connection")
        self.assertEqual(r["error_code"], "OUTLOOK_UNAVAILABLE")
        self.assertEqual(r["diagnostics"]["hresult"], "0x80080005")
        self.assertIn("administrator", r["message"])

    def test_unknown_op_and_bad_params(self):
        self.assertEqual(self.run_op("nope")["error_code"], "BAD_REQUEST")
        self.assertEqual(w.handle({"op": "test_connection", "params": "x"})["error_code"], "BAD_REQUEST")

    def test_list_mailboxes_and_folders(self):
        r = self.run_op("list_mailboxes")
        self.assertEqual(r["data"]["mailboxes"][0]["store_id"], "S1")
        r = self.run_op("list_folders", {"store_id": "S1"})
        self.assertTrue(r["ok"], r)
        inbox = r["data"]["root"]["children"][0]
        self.assertEqual(inbox["path"], "Inbox")
        wen = inbox["children"][0]
        self.assertEqual(wen["path"], "Inbox/Wenjie")
        self.assertEqual(wen["children"][0]["path"], "Inbox/Wenjie/School Subtype Needed")
        self.assertEqual(self.run_op("list_folders", {"store_id": "NOPE"})["error_code"], "MAILBOX_NOT_FOUND")

    def test_sync_messages_parent_only_and_subfolders(self):
        req = {"key": "S1|WEN", "store_id": "S1", "entry_id": "WEN", "path": "Inbox/Wenjie", "since_utc": "2026-11-01T00:00:00Z", "include_subfolders": False}
        r = self.run_op("sync_messages", {"folders": [req], "max_items": 50, "body_max_chars": 1000})
        self.assertTrue(r["ok"], r)
        f = r["data"]["folders"][0]
        self.assertEqual(len(f["messages"]), 2, "non-mail items skipped")
        m = f["messages"][0]
        for key in ("store_id", "entry_id", "internet_message_id", "conversation_id", "subject", "sender_name", "sender_email", "received_at", "body", "folder_entry_id", "folder_path", "metadata"):
            self.assertIn(key, m)
        self.assertEqual(m["folder_path"], "Inbox/Wenjie")
        self.assertTrue(m["received_at"].endswith("Z"))
        self.assertIn("datereceived", self.app.ns._folders["WEN"].Items.filters[0])
        req["include_subfolders"] = True
        r = self.run_op("sync_messages", {"folders": [req]})
        self.assertEqual(len(r["data"]["folders"][0]["messages"]), 3)

    def test_sync_truncation_and_body_limit(self):
        req = {"key": "k", "store_id": "S1", "entry_id": "WEN", "path": "Inbox/Wenjie", "since_utc": "2026-11-01T00:00:00Z"}
        r = self.run_op("sync_messages", {"folders": [req], "max_items": 1, "body_max_chars": 5})
        f = r["data"]["folders"][0]
        self.assertTrue(f["truncated"])
        self.assertEqual(len(f["messages"]), 1)
        self.assertLessEqual(len(f["messages"][0]["body"]), 5)
        self.assertTrue(f["messages"][0]["body_truncated"])

    def test_truncation_across_subfolders_returns_oldest_overall(self):
        # The cap must apply to one oldest-first stream over ALL collected folders. Otherwise the
        # parent folder fills the cap, the watermark jumps past older subfolder mail, and it is never imported.
        self.app.ns._folders["SUB"].Items._items[0].ReceivedTime = dt.datetime(2026, 11, 16, 9, 0, 0)
        req = {"key": "k", "store_id": "S1", "entry_id": "WEN", "path": "Inbox/Wenjie",
               "since_utc": "2026-11-01T00:00:00Z", "include_subfolders": True}
        r = self.run_op("sync_messages", {"folders": [req], "max_items": 2})
        f = r["data"]["folders"][0]
        self.assertTrue(f["truncated"])
        self.assertEqual([m["entry_id"] for m in f["messages"]], ["E3", "E1"])
        self.assertEqual(f["messages"][0]["folder_path"], "Inbox/Wenjie/School Subtype Needed")

    def test_folder_recovery_and_missing_folder(self):
        req = {"key": "k", "store_id": "S1", "entry_id": "OLD-ID", "path": "Inbox/Wenjie", "since_utc": "2026-11-01T00:00:00Z"}
        r = self.run_op("sync_messages", {"folders": [req]})
        f = r["data"]["folders"][0]
        self.assertTrue(f["recovered"])
        self.assertEqual(f["resolved_entry_id"], "WEN")
        req2 = dict(req, path="Inbox/Deleted Folder")
        r = self.run_op("sync_messages", {"folders": [req2]})
        self.assertFalse(r["ok"])
        self.assertEqual(r["error_code"], "FOLDER_NOT_FOUND")

    def test_create_reply_draft_never_sends(self):
        with tempfile.TemporaryDirectory() as d:
            img = os.path.join(d, "image3.png")
            with open(img, "wb") as fh:
                fh.write(b"\x89PNG")
            r = self.run_op("create_reply_draft", {
                "mode": "reply", "entry_id": "E1", "store_id": "S1", "subject": "RE: School Subtype Error",
                "html_body": "<p>Hello</p><img src=\"cid:image3.png\">", "inline_images": [{"path": img, "cid": "image3.png"}],
            })
        self.assertTrue(r["ok"], r)
        self.assertFalse(r["data"]["sent"])
        self.assertEqual(r["data"]["entry_id"], "DRAFT-1")
        self.assertEqual(r["data"]["inline_images"], 1)

    def test_reply_html_inserted_before_signature_and_quote(self):
        html = w.insert_html("<html><body class='x'><div>signature</div></body></html>", "<p>NEW</p>")
        self.assertEqual(html, "<html><body class='x'><p>NEW</p><div>signature</div></body></html>")

    def test_create_new_mail_when_no_original(self):
        r = self.run_op("create_reply_draft", {"mode": "new", "to": "x@y.org", "subject": "S", "html_body": "<p>x</p>"})
        self.assertTrue(r["ok"], r)
        m = self.app.created[0]
        self.assertTrue(m.saved)
        self.assertEqual(m.To, "x@y.org")
        self.assertIn("<p>x</p>", m.HTMLBody)

    def test_missing_original_item(self):
        r = self.run_op("create_reply_draft", {"mode": "reply", "entry_id": "GONE", "html_body": "<p>x</p>"})
        self.assertEqual(r["error_code"], "ITEM_NOT_FOUND")

    def test_get_message(self):
        r = self.run_op("get_message", {"entry_id": "E1", "store_id": "S1"})
        self.assertTrue(r["ok"], r)
        self.assertIn("123456", r["data"]["body"])
        self.assertIn("html_body", r["data"])

    def test_diagnostics_do_not_contain_bodies(self):
        req = {"key": "k", "store_id": "S1", "entry_id": "WEN", "path": "Inbox/Wenjie", "since_utc": "2026-11-01T00:00:00Z"}
        r = self.run_op("sync_messages", {"folders": [req]})
        self.assertNotIn("123456", json.dumps(r["diagnostics"]))

    def test_utc_conversion(self):
        s = w.to_utc_iso(dt.datetime(2026, 11, 16, 9, 30))
        self.assertRegex(s, r"^2026-11-1[67]T\d\d:30:00Z$")
        self.assertEqual(w.to_utc_iso(dt.datetime(4501, 1, 1)), "")


class ProcessTests(unittest.TestCase):
    """Runs the real script as a subprocess (stdin/stdout JSON contract)."""

    def call(self, payload):
        env = dict(os.environ, ASR_DESK_MOCK_OUTLOOK="1")
        p = subprocess.run([sys.executable, "-I", os.path.join(WORKER_DIR, "outlook_worker.py")], input=json.dumps(payload).encode(),
                           capture_output=True, env=env, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        return json.loads(p.stdout.decode().strip().splitlines()[-1])

    def test_subprocess_contract(self):
        r = self.call({"op": "list_folders", "params": {}})
        self.assertTrue(r["ok"])
        self.assertEqual(set(r.keys()), {"ok", "data", "error_code", "message", "diagnostics"})

    def test_subprocess_invalid_json(self):
        env = dict(os.environ)
        p = subprocess.run([sys.executable, "-I", os.path.join(WORKER_DIR, "outlook_worker.py")], input=b"{not json",
                           capture_output=True, env=env, timeout=60)
        r = json.loads(p.stdout.decode().strip())
        self.assertFalse(r["ok"])
        self.assertEqual(r["error_code"], "BAD_REQUEST")


if __name__ == "__main__":
    unittest.main()
