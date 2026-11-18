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
        items = self._items
        if w.PR_INTERNET_MESSAGE_ID in flt:
            wanted = flt.split("= '", 1)[1].rstrip("'").replace("''", "'")
            items = [m for m in items if getattr(m, "PropertyAccessor", None) and m.PropertyAccessor.props.get(w.PR_INTERNET_MESSAGE_ID) == wanted]
        if '"urn:schemas:httpmail:read" = 0' in flt:
            items = [m for m in items if getattr(m, "UnRead", True)]
        return FakeItems(items)

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
        self.FlagStatus = 0
        self.Parent = None
        self.replies = []
        self.displayed = False
        # from here on, record every property the worker writes
        self.__dict__["_writes"] = []
        self.__dict__["_ready"] = True

    def __setattr__(self, name, value):
        if self.__dict__.get("_ready"):
            self.__dict__["_writes"].append(name)
        object.__setattr__(self, name, value)

    def _reply(self, prefix, kind):
        r = FakeMail(entry_id="", subject=prefix + self.Subject, imid="")
        r.HTMLBody = "<html><body class='x'><div>signature</div><hr>quoted</body></html>"
        r.__dict__["kind"] = kind
        r.__dict__["_writes"] = []
        self.__dict__["replies"].append(r)
        return r

    def Reply(self):
        return self._reply("RE: ", "reply")

    def ReplyAll(self):
        return self._reply("RE: ", "reply_all")

    def MarkAsTask(self, interval):
        raise AssertionError("MarkAsTask() must not be called on this item")

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


class MovableMail(FakeMail):
    """A mail that Mark Done may flag and move. Moving changes its EntryID (as Exchange does)."""

    def __init__(self, *a, registry=None, move_error=False, **kw):
        super().__init__(*a, **kw)
        self.__dict__.update(registry=registry, move_error=move_error, moves=0, marked=[], saves=0)

    def MarkAsTask(self, interval):
        self.__dict__["marked"].append(interval)
        self.__dict__["FlagStatus"] = w.OL_FLAG_COMPLETE if interval == w.OL_MARK_COMPLETE else 2

    def Save(self):
        self.__dict__["saves"] += 1

    def Move(self, dest):
        if self.move_error:
            raise Exception("access denied")
        src = self.Parent
        if src is not None and self in src.Items._items:
            src.Items._items.remove(self)
        old = self.EntryID
        new = old + "-MOVED"
        if self.registry is not None:
            self.registry.pop(old, None)
            self.registry[new] = self
        self.__dict__.update(EntryID=new, Parent=dest, moves=self.moves + 1)
        dest.Items._items.append(self)
        return self


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

    # ------------------------------------------------------------- scanning only reads

    def test_sync_never_writes_to_items_and_unread_mode(self):
        self.mails[1].__dict__["UnRead"] = False
        req = {"key": "k", "store_id": "S1", "entry_id": "WEN", "path": "Inbox/Wenjie", "since_utc": "2026-11-01T00:00:00Z", "unread_only": True}
        r = self.run_op("sync_messages", {"folders": [req]})
        self.assertTrue(r["ok"], r)
        self.assertEqual([m["entry_id"] for m in r["data"]["folders"][0]["messages"]], ["E1"], "read mail skipped in unread mode")
        self.assertIn("httpmail:read", self.app.ns._folders["WEN"].Items.filters[-1])
        req["unread_only"] = False
        r = self.run_op("sync_messages", {"folders": [req]})
        self.assertEqual(len(r["data"]["folders"][0]["messages"]), 2)
        for m in self.mails:
            self.assertEqual(m._writes, [], "scanning must not change UnRead, flags or anything else")
            self.assertFalse(m.saved)

    # ------------------------------------------------------------- reply window

    def test_open_reply_shows_outlook_reply_without_touching_it(self):
        r = self.run_op("open_reply", {"entry_id": "E1", "store_id": "S1"})
        self.assertTrue(r["ok"], r)
        reply = self.mails[0].replies[0]
        self.assertEqual(reply.kind, "reply")
        self.assertTrue(reply.displayed)
        self.assertFalse(reply.saved, "the reply is not saved by the app")
        self.assertEqual(reply._writes, ["displayed"], "body, recipients and subject are left to Outlook")
        self.assertIn("signature", reply.HTMLBody)
        r = self.run_op("open_reply", {"entry_id": "E1", "reply_all": True})
        self.assertEqual(self.mails[0].replies[1].kind, "reply_all")
        self.assertTrue(r["data"]["reply_all"])

    def test_open_reply_missing_and_relocated_by_message_id(self):
        r = self.run_op("open_reply", {"entry_id": "GONE", "imid": "<nope@x>", "folders": [{"store_id": "S1", "entry_id": "WEN"}]})
        self.assertFalse(r["ok"])
        self.assertEqual(r["error_code"], "ITEM_NOT_FOUND")
        # EntryID changed (item moved) - found again in a candidate folder by Internet Message-ID
        r = self.run_op("open_reply", {"entry_id": "OLD-E2", "imid": "<m2@x>", "folders": [{"store_id": "S1", "entry_id": "SUB"}, {"store_id": "S1", "entry_id": "WEN"}]})
        self.assertTrue(r["ok"], r)
        self.assertTrue(r["data"]["relocated"])
        self.assertTrue(self.mails[1].replies[0].displayed)

    def test_open_message_only_displays_the_original(self):
        r = self.run_op("open_message", {"entry_id": "E1", "store_id": "S1"})
        self.assertTrue(r["ok"], r)
        self.assertTrue(self.mails[0].displayed)
        self.assertFalse(self.mails[0].saved, "nothing is saved")
        self.assertEqual([w for w in self.mails[0]._writes if w != "displayed"], [], "read state, flags and folders are untouched")
        self.assertEqual(self.run_op("open_message", {"entry_id": "GONE"})["error_code"], "ITEM_NOT_FOUND")

    def test_open_new_message_fallback_only_displays(self):
        r = self.run_op("open_new_message", {"to": "a@b.org", "subject": "RE: Help"})
        self.assertTrue(r["ok"], r)
        m = self.app.created[0]
        self.assertTrue(m.displayed)
        self.assertFalse(m.saved)
        self.assertEqual((m.To, m.Subject), ("a@b.org", "RE: Help"))
        self.assertEqual(self.run_op("open_new_message", {"to": ""})["error_code"], "BAD_REQUEST")

    # ------------------------------------------------------------- Mark Done

    def completion_world(self, move_error=False):
        ns = self.app.ns
        wen = ns._folders["WEN"]
        done = FakeFolder("Completed", "DONE", parent=ns._folders["INBOX"])
        ns._folders["DONE"] = done
        a = MovableMail("M1", "Help", "body", conv="C9", imid="<c1@x>", registry=ns._items)
        b = MovableMail("M2", "RE: Help", "body", conv="C9", imid="<c2@x>", registry=ns._items, move_error=move_error)
        for m in (a, b):
            m.__dict__["Parent"] = wen
            wen.Items._items.append(m)
            ns._items[m.EntryID] = m
        return done, a, b

    def item(self, mid, m, **kw):
        d = {"message_id": mid, "entry_id": m.EntryID, "store_id": "S1", "imid": m.PropertyAccessor.props[w.PR_INTERNET_MESSAGE_ID],
             "folders": [{"store_id": "S1", "entry_id": "WEN"}], "need_flag": True, "need_move": True}
        d.update(kw)
        return d

    def test_complete_flags_then_moves_and_reports_new_location(self):
        done, a, b = self.completion_world()
        req = {"dest": {"store_id": "S1", "entry_id": "DONE"}, "items": [self.item(1, a), self.item(2, b)]}
        r = self.run_op("complete_messages", req)
        self.assertTrue(r["ok"], r)
        res = r["data"]["items"]
        for x, m in zip(res, (a, b)):
            self.assertTrue(x["flagged"] and x["moved"], x)
            self.assertEqual(x["new_entry_id"], m.EntryID)
            self.assertTrue(x["new_entry_id"].endswith("-MOVED"))
            self.assertEqual(x["new_folder_path"], "Inbox/Completed")
            self.assertEqual(m.marked, [w.OL_MARK_COMPLETE])
            self.assertIs(m.Parent, done)
        # retry with the old EntryIDs (e.g. the app stopped before saving the results): no second move
        r = self.run_op("complete_messages", {"dest": {"store_id": "S1", "entry_id": "DONE"}, "items": [self.item(1, a, entry_id="M1"), self.item(2, b, entry_id="M2")]})
        self.assertTrue(all(x["flagged"] and x["moved"] for x in r["data"]["items"]), r)
        self.assertEqual((a.moves, b.moves), (1, 1), "an email is never moved twice")
        self.assertEqual(len(a.marked), 1, "an already complete flag is not set again")
        self.assertFalse(any(hasattr(m, "deleted") for m in (a, b)))

    def test_complete_move_failure_keeps_flag_and_reports_error(self):
        done, a, b = self.completion_world(move_error=True)
        req = {"dest": {"store_id": "S1", "entry_id": "DONE"}, "items": [self.item(1, a), self.item(2, b), {"message_id": 3, "entry_id": "GONE", "imid": "<gone@x>"}]}
        r = self.run_op("complete_messages", req)
        self.assertTrue(r["ok"], r)
        ok, failed, missing = r["data"]["items"]
        self.assertTrue(ok["flagged"] and ok["moved"])
        self.assertTrue(failed["flagged"])
        self.assertFalse(failed["moved"])
        self.assertEqual(failed["error_code"], "MOVE_FAILED")
        self.assertEqual(b.FlagStatus, w.OL_FLAG_COMPLETE)
        self.assertIs(b.Parent, self.app.ns._folders["WEN"])
        self.assertEqual(missing["error_code"], "ITEM_NOT_FOUND")
        # retry only the move
        b.__dict__["move_error"] = False
        r = self.run_op("complete_messages", {"dest": {"store_id": "S1", "entry_id": "DONE"}, "items": [self.item(2, b, need_flag=False)]})
        self.assertTrue(r["data"]["items"][0]["moved"], r)
        self.assertEqual(len(b.marked), 1)

    # ------------------------------------------------------------- subfolders

    def test_sync_excludes_completion_folder_subtree(self):
        req = {"key": "k", "store_id": "S1", "entry_id": "WEN", "path": "Inbox/Wenjie", "since_utc": "2026-11-01T00:00:00Z",
               "include_subfolders": True, "exclude_entry_ids": ["SUB"]}
        r = self.run_op("sync_messages", {"folders": [req]})
        self.assertTrue(r["ok"], r)
        self.assertEqual(sorted(m["entry_id"] for m in r["data"]["folders"][0]["messages"]), ["E1", "E2"], "excluded subfolder not scanned")

    def test_list_folders_subtree_only(self):
        r = self.run_op("list_folders", {"store_id": "S1", "root_entry_id": "WEN"})
        self.assertTrue(r["ok"], r)
        root = r["data"]["root"]
        self.assertEqual(root["path"], "Inbox/Wenjie")
        self.assertEqual(root["children"][0]["path"], "Inbox/Wenjie/School Subtype Needed")
        self.assertEqual(self.run_op("list_folders", {"root_entry_id": "NOPE"})["error_code"], "FOLDER_NOT_FOUND")

    def test_moved_email_found_anywhere_under_inbox_only_when_recursive(self):
        # E3 lives two levels below the Inbox (Inbox/Wenjie/School Subtype Needed); its stored EntryID is stale
        flat = [{"store_id": "S1", "entry_id": "INBOX"}]
        r = self.run_op("open_reply", {"entry_id": "OLD-E3", "imid": "<m3@x>", "folders": flat})
        self.assertEqual(r["error_code"], "ITEM_NOT_FOUND", "a non-recursive candidate does not search subfolders")
        r = self.run_op("open_reply", {"entry_id": "OLD-E3", "imid": "<m3@x>", "folders": [dict(flat[0], recursive=True)]})
        self.assertTrue(r["ok"], r)
        self.assertTrue(r["data"]["relocated"])
        e3 =self.app.ns._folders["SUB"].Items._items[0]
        self.assertTrue(e3.replies[0].displayed)

    def test_mark_done_on_email_staff_filed_into_a_subfolder(self):
        done, a, b = self.completion_world()
        sub = self.app.ns._folders["SUB"]
        # staff filed `a` from Inbox/Wenjie into a subfolder: EntryID changed, stored id is stale
        a.Move(sub)
        stale = self.item(1, a, entry_id="M1")
        stale["folders"] = [{"store_id": "S1", "entry_id": "WEN"}, {"store_id": "S1", "entry_id": "INBOX", "recursive": True}]
        r = self.run_op("complete_messages", {"dest": {"store_id": "S1", "entry_id": "DONE"}, "items": [stale]})
        res = r["data"]["items"][0]
        self.assertTrue(res["flagged"] and res["moved"], res)
        self.assertIs(a.Parent, done)
        self.assertEqual(res["new_folder_path"], "Inbox/Completed")

    # ------------------------------------------------------------- scope verification

    def test_inventory_ignores_unread_and_date_filters_and_skips_excluded(self):
        self.mails[1].__dict__["UnRead"] = False
        self.mails[1].__dict__["ReceivedTime"] = dt.datetime(2020, 1, 1)  # very old, read: still inventoried
        r = self.run_op("scope_inventory", {"scope_key": "K", "folders": [{"store_id": "S1", "entry_id": "WEN", "include_subfolders": True}], "exclude": ["SUB"]})
        self.assertTrue(r["ok"], r)
        d = r["data"]
        self.assertTrue(d["complete"])
        self.assertEqual(d["scope_key"], "K")
        self.assertEqual(sorted(i["imid"] for i in d["items"]), ["<m1@x>", "<m2@x>"], "excluded subtree not read; non-mail skipped")
        wen = next(f for f in d["folders"] if f["entry_id"] == "WEN")
        self.assertEqual(wen["ancestors"], ["INBOX", "ROOT"])
        for m in self.mails:
            self.assertEqual(m._writes, [], "verification only reads")

    def test_inventory_uses_table_and_reports_incomplete(self):
        class Row:
            def __init__(self, v): self.v = v
            def GetValues(self): return self.v
        class Table:
            def __init__(self, rows): self.rows = list(rows); self.Columns = types.SimpleNamespace(RemoveAll=lambda: None, Add=lambda c: None)
            @property
            def EndOfTable(self): return not self.rows
            def GetNextRow(self): return Row(self.rows.pop(0))
        sub = self.app.ns._folders["SUB"]
        sub.GetTable = lambda: Table([("T1", "IPM.Note", "<t1@x>"), ("T2", "IPM.Appointment", "<t2@x>")])
        r = self.run_op("scope_inventory", {"scope_key": "K", "folders": [{"store_id": "S1", "entry_id": "SUB"}, {"store_id": "S1", "entry_id": "GONE", "path": "Inbox/Gone"}]})
        d = r["data"]
        self.assertEqual([i["entry_id"] for i in d["items"]], ["T1"], "table rows, mail only")
        self.assertFalse(d["complete"], "a missing scope folder makes the inventory incomplete")
        self.assertTrue(any("Gone" in e for e in d["errors"]))

    def test_inventory_of_a_mailbox_default_inbox(self):
        store = self.app.ns.DefaultStore
        store.GetDefaultFolder = lambda n: self.app.ns._folders["INBOX"] if n == 6 else None
        r = self.run_op("scope_inventory", {"scope_key": "wide", "folders": [{"store_id": "S1", "default_folder": 6, "include_subfolders": True, "path": "Inbox"}]})
        d = r["data"]
        self.assertTrue(d["complete"], d)
        self.assertEqual(sorted(i["imid"] for i in d["items"]), ["<m1@x>", "<m2@x>", "<m3@x>"], "whole Inbox tree incl. subfolders")
        r = self.run_op("scope_inventory", {"scope_key": "wide", "folders": [{"store_id": "NOPE", "default_folder": 6, "path": "Inbox"}]})
        self.assertFalse(r["data"]["complete"])

    def test_locate_found_not_found_and_error(self):
        flat = [{"store_id": "S1", "entry_id": "INBOX", "recursive": True}]
        r = self.run_op("locate_messages", {"items": [
            {"message_id": 1, "entry_id": "OLD", "imid": "<m3@x>", "folders": flat},
            {"message_id": 2, "entry_id": "OLD2", "imid": "<gone@x>", "folders": flat},
            {"message_id": 3, "entry_id": "OLD3", "imid": "<gone@x>", "folders": [{"store_id": "S1", "entry_id": "NO-SUCH-FOLDER"}]},
        ]})
        found, missing, err = r["data"]["items"]
        self.assertEqual(found["status"], "found")
        self.assertEqual(missing["status"], "not_found", "every candidate folder searched without error")
        self.assertEqual(err["status"], "error", "a folder that cannot be opened is not 'not found'")

    def test_notification_marker_detected_in_scan(self):
        self.mails[0].__dict__["Body"] = "Handoff ...\n[ASR-Desk-Notification: ASR-1234ABCD-7-20261117]"
        req = {"key": "k", "store_id": "S1", "entry_id": "WEN", "path": "Inbox/Wenjie", "since_utc": "2026-11-01T00:00:00Z"}
        msgs = self.run_op("sync_messages", {"folders": [req]})["data"]["folders"][0]["messages"]
        by = {m["entry_id"]: m for m in msgs}
        self.assertEqual(by["E1"]["notification_id"], "ASR-1234ABCD-7-20261117")
        self.assertEqual(by["E2"]["notification_id"], "")
        self.assertEqual(by["E1"]["folder_ancestors"], ["INBOX", "ROOT"])

    # ------------------------------------------------------------- assignment moves

    def test_move_messages_to_staff_folder_without_flag_or_read_change(self):
        done, a, b = self.completion_world()
        staff_folder = FakeFolder("Aaron", "AAR", parent=self.app.ns._folders["INBOX"])
        self.app.ns._folders["AAR"] = staff_folder
        b.Move(staff_folder)  # already in the destination
        r = self.run_op("move_messages", {"dest": {"store_id": "S1", "entry_id": "AAR"}, "items": [self.item(1, a), self.item(2, b, entry_id="M2")]})
        self.assertTrue(r["ok"], r)
        ra, rb = r["data"]["items"]
        self.assertTrue(ra["moved"] and rb["moved"])
        self.assertEqual(ra["new_folder_path"], "Inbox/Aaron")
        self.assertEqual(ra["ancestors"], ["INBOX", "ROOT"])
        self.assertEqual((a.moves, b.moves), (1, 1), "an email already in the destination is not moved again")
        self.assertEqual(a.marked, [], "assignment never flags")
        self.assertNotIn("UnRead", a._writes)
        self.assertEqual(self.run_op("move_messages", {"dest": {"store_id": "S1", "entry_id": "NOPE"}, "items": []})["error_code"], "FOLDER_NOT_FOUND")

    # ------------------------------------------------------------- notifications

    def notify_world(self, send_error=False):
        app = self.app

        class Recipients:
            def __init__(self, mail): self.mail = mail
            @property
            def Count(self): return len([x for x in self.mail.To.split(";") if x.strip()])
            def ResolveAll(self): return "@" in self.mail.To

        class NotifyMail(FakeMail):
            def __init__(self):
                super().__init__(entry_id="", subject="", imid="")
                self.__dict__.update(sent=False, send_error=send_error, PropertyAccessor=FakeAccessor())
            @property
            def Recipients(self): return Recipients(self)
            def Send(self):
                if self.send_error:
                    raise Exception("RPC timeout")
                self.__dict__["sent"] = True

        account = types.SimpleNamespace(DisplayName="Sam", SmtpAddress="sam@example.org", DeliveryStore=None)
        app.ns.Accounts = FakeCollection([account])
        app.CreateItem = lambda kind: app.created.append(NotifyMail()) or app.created[-1]
        return account

    def test_send_notification_to_one_staff_address_with_marker(self):
        acct = self.notify_world()
        r = self.run_op("send_notification", {"to": "aaron@example.org", "subject": "ASR/Audit Ticket #T-000001 Assigned to You — Maple", "html_body": "<p>x</p>",
                                              "account": "sam@example.org", "notification_id": "ASR-1-2"})
        self.assertTrue(r["ok"], r)
        m = self.app.created[-1]
        self.assertTrue(m.sent)
        self.assertEqual(m.To, "aaron@example.org")
        self.assertIs(m.SendUsingAccount, acct)
        self.assertEqual(m.PropertyAccessor.props[w.NOTIFY_PROP], "ASR-1-2", "stable notification id header")
        self.assertEqual(r["data"]["account"], "sam@example.org")
        bad = self.run_op("send_notification", {"to": "a@x.org; b@x.org", "notification_id": "n"})
        self.assertEqual((bad["error_code"], bad["diagnostics"]["stage"]), ("BAD_REQUEST", "prepare"))
        nf = self.run_op("send_notification", {"to": "a@x.org", "notification_id": "n", "account": "other@x.org"})
        self.assertEqual((nf["error_code"], nf["diagnostics"]["stage"]), ("ACCOUNT_NOT_FOUND", "prepare"), "nothing sent")

    def test_send_error_is_uncertain_and_find_in_sent_items(self):
        self.notify_world(send_error=True)
        r = self.run_op("send_notification", {"to": "aaron@example.org", "html_body": "<p>x</p>", "notification_id": "ASR-9-9"})
        self.assertEqual((r["error_code"], r["diagnostics"]["stage"]), ("SEND_UNCERTAIN", "send"))
        sent_mail = FakeMail("S1-SENT", "x", "y")
        sent_mail.PropertyAccessor.props[w.NOTIFY_PROP] = "ASR-9-9"
        sent = FakeFolder("Sent Items", "SENT", items=[sent_mail])
        store = self.app.ns.DefaultStore
        store.GetDefaultFolder = lambda fid: sent if fid == w.OL_FOLDER_SENT else FakeFolder("Outbox", "OUT")
        orig_restrict = FakeItems.Restrict

        def restrict(items_self, flt):
            if w.NOTIFY_PROP in flt:
                wanted = flt.split("= '", 1)[1].rstrip("'")
                return FakeItems([m for m in items_self._items if m.PropertyAccessor.props.get(w.NOTIFY_PROP) == wanted])
            return orig_restrict(items_self, flt)
        FakeItems.Restrict = restrict
        try:
            f = self.run_op("find_notification", {"notification_id": "ASR-9-9"})
            self.assertEqual((f["data"]["found"], f["data"]["where"]), (True, "Sent Items"))
            self.assertFalse(self.run_op("find_notification", {"notification_id": "other"})["data"]["found"])
        finally:
            FakeItems.Restrict = orig_restrict

    def test_complete_destination_missing_and_validate_folder(self):
        r = self.run_op("complete_messages", {"dest": {"store_id": "S1", "entry_id": "NOPE"}, "items": []})
        self.assertEqual(r["error_code"], "FOLDER_NOT_FOUND")
        r = self.run_op("validate_folder", {"store_id": "S1", "entry_id": "SUB"})
        self.assertTrue(r["ok"], r)
        self.assertEqual(r["data"]["path"], "Inbox/Wenjie/School Subtype Needed")
        self.assertEqual(self.run_op("validate_folder", {"store_id": "S1", "entry_id": "NOPE"})["error_code"], "FOLDER_NOT_FOUND")


# ----------------------------------------------------------------- shared-mailbox child folder discovery (MAPI)
#
# Reproduces what was observed live on the team (delegate) mailbox: Outlook's Folders collection of the
# Inbox is EMPTY although the Inbox has subfolders; the hierarchy table opened with the entry ID that the
# parent's (root's) hierarchy table hands out lists them. Opening the Inbox by the object model's
# EntryID gives an empty hierarchy table too.

class FakeTags:
    PT_ERROR = 0x000A
    PR_DISPLAY_NAME_W = 0x3001001F
    PR_ENTRYID = 0x0FFF0102

    @staticmethod
    def PROP_TYPE(tag):
        return tag & 0xFFFF

    @staticmethod
    def PROP_ID(tag):
        return tag >> 16


class FakeHierTable:
    def __init__(self, rows):
        self._rows = rows
        self._cols = ()
        self._done = False

    def SetColumns(self, cols, flags):
        self._cols = cols

    def QueryRows(self, n, flags):
        if self._done:
            return []
        self._done = True
        out = []
        for r in self._rows:
            row = []
            for tag in self._cols:
                pid = tag >> 16
                if pid in r:
                    row.append((tag, r[pid]))
                else:
                    row.append(((pid << 16) | FakeTags.PT_ERROR, -2147221233))  # MAPI_E_NOT_FOUND
            out.append(tuple(row))
        return out


class FakeMapiFolder:
    def __init__(self, world, eid):
        self.world, self.eid = world, eid

    def GetHierarchyTable(self, flags):
        self.world.tables_opened.append(self.eid)
        return FakeHierTable(self.world.tables.get(self.eid, []))

    def GetProps(self, tags, flags):
        return 0, ((FakeTags.PR_ENTRYID, self.world.long_term[self.eid]),)


class FakeMsgStore:
    def __init__(self, world):
        self.world = world

    def OpenEntry(self, eid, iid, flags):
        assert flags == 0, "folders must be opened read-only"
        if eid not in self.world.long_term and eid not in self.world.tables:
            raise FakeComError(0x8004010F - 2**32)  # MAPI_E_NOT_FOUND
        return FakeMapiFolder(self.world, eid)


class FakeSession:
    def __init__(self, world):
        self.world = world

    def OpenMsgStore(self, ui, store_eid, iid, flags):
        self.world.stores_opened.append(store_eid)
        return FakeMsgStore(self.world)


class FakeMapiObject:
    def __init__(self, world):
        self.world = world

    def QueryInterface(self, iid):
        return FakeSession(self.world)


class HiddenChildrenFolder(FakeFolder):
    """Outlook's view of the delegate Inbox: the Folders collection is empty."""

    @property
    def Folders(self):
        return FakeCollection([])


class BrokenFolders(FakeFolder):
    @property
    def Folders(self):
        raise FakeComError(0x80040115 - 2**32)  # MAPI_E_NETWORK_ERROR


def install_fake_mapi(world, fail_init=False):
    mapi = types.ModuleType("win32com.mapi.mapi")
    calls = {"init": 0, "uninit": 0}

    def init(_):
        if fail_init:
            raise ImportError("no MAPI")
        calls["init"] += 1
    mapi.MAPIInitialize = init
    mapi.MAPIUninitialize = lambda: calls.__setitem__("uninit", calls["uninit"] + 1)
    mapi.IID_IMAPISession = "IID_IMAPISession"
    mapi.MDB_NO_DIALOG = 1
    mapi.MAPI_BEST_ACCESS = 0x10
    pkg = types.ModuleType("win32com.mapi")
    pkg.mapi = mapi
    pkg.mapitags = FakeTags
    sys.modules["win32com"].mapi = pkg
    sys.modules["win32com.mapi"] = pkg
    sys.modules["win32com.mapi.mapi"] = mapi
    sys.modules["win32com.mapi.mapitags"] = FakeTags
    return calls


def build_shared_world():
    """Team mailbox whose Inbox hides its subfolders from the object model."""
    nsobj = FakeNamespaceObj()
    root = FakeFolder("Team Mailbox", "A0", parent=nsobj, store_id="5A")
    inbox = HiddenChildrenFolder("Inbox", "A1", parent=root, store_id="5A")
    wen = FakeFolder("Wenjie", "A2", parent=inbox, store_id="5A",
                     items=[FakeMail("W1", "Needs training", "x", conv="CW", imid="<w1@x>")])
    sub = FakeFolder("School Subtype Needed", "A3", parent=wen, store_id="5A",
                     items=[FakeMail("S1", "Subtype", "x", conv="CS", imid="<s1@x>")])
    nj = FakeFolder("NJIIS Team", "A4", parent=inbox, store_id="5A",
                    items=[FakeMail("N1", "Access problem", "x", conv="CN", imid="<n1@x>")])
    njdone = FakeFolder("Completed", "A5", parent=nj, store_id="5A",
                        items=[FakeMail("D1", "Done", "x", conv="CD", imid="<d1@x>")])
    cal = FakeFolder("Calendar", "A7", parent=None, store_id="5A")
    cal.Parent = inbox
    cal.DefaultItemType = 1
    inbox._children = []  # the object model shows nothing below the Inbox
    store = FakeStore("Team Mailbox", "5A", root)
    personal_root = FakeFolder("me@example.org", "C0", parent=nsobj, store_id="6B")
    personal = FakeStore("me@example.org", "6B", personal_root)
    ns = FakeNS([personal, store], {"A0": root, "A1": inbox, "A2": wen, "A3": sub, "A4": nj, "A5": njdone, "A7": cal}, {})

    class World:
        pass
    world = World()
    world.tables_opened, world.stores_opened = [], []
    E = bytes.fromhex
    pid = FakeTags.PROP_ID
    hidden = 0x10F4
    def row(name, eid, extra=None):
        return {pid(FakeTags.PR_DISPLAY_NAME_W): name, pid(FakeTags.PR_ENTRYID): eid, **(extra or {})}
    world.tables = {
        E("A0"): [row("Inbox", E("EF01"))],                                   # store root (opened by its own ID)
        E("A1"): [],                                                          # Inbox opened by the OOM EntryID: empty
        E("EF01"): [row("Wenjie", E("EF02")), row("NJIIS Team", E("EF04")), row("Private", E("EF06")),
                    row("Calendar", E("EF07")), row("Search Root", E("EF08"), {hidden: True})],
        E("EF02"): [row("School Subtype Needed", E("EF03"))],
        E("EF04"): [row("Completed", E("EF05"))],
    }
    world.long_term = {E("EF01"): E("B1"), E("EF02"): E("A2"), E("EF03"): E("A3"), E("EF04"): E("A4"),
                       E("EF05"): E("A5"), E("EF06"): E("A6"), E("EF07"): E("A7"), E("EF08"): E("A8")}
    ns.MAPIOBJECT = FakeMapiObject(world)
    ns.DefaultStore = personal
    return FakeApp(ns), world


class SharedMailboxDiscoveryTests(unittest.TestCase):
    def setUp(self):
        self._saved = {k: sys.modules.get(k) for k in ("pythoncom", "win32com", "win32com.client", "win32com.mapi",
                                                       "win32com.mapi.mapi", "win32com.mapi.mapitags")}
        self.app, self.world = build_shared_world()
        self.calls = install_fake_com(self.app)
        self.mapi_calls = install_fake_mapi(self.world)

    def tearDown(self):
        for k, v in self._saved.items():
            if v is None:
                sys.modules.pop(k, None)
            else:
                sys.modules[k] = v

    def run_op(self, op, params=None):
        return w.handle({"op": op, "params": params or {}})

    @staticmethod
    def names(node):
        return [c["name"] for c in node["children"]]

    def test_outlook_really_hides_the_inbox_children(self):
        self.assertEqual(self.app.ns.GetFolderFromID("A1").Folders.Count, 0, "precondition of the regression")

    def test_mailbox_tree_shows_inbox_children_recovered_from_mapi(self):
        r = self.run_op("list_folders", {"store_id": "5A"})
        self.assertTrue(r["ok"], r)
        root = r["data"]["root"]
        inbox = root["children"][0]
        self.assertEqual(inbox["path"], "Inbox")
        self.assertEqual(self.names(inbox), ["Wenjie", "NJIIS Team", "Private"], "Calendar (not mail) and hidden folders are left out")
        self.assertEqual(inbox["child_count"], 3)
        self.assertTrue(inbox["has_children"])
        wen = inbox["children"][0]
        self.assertEqual((wen["path"], wen["entry_id"], wen["store_id"], wen["parent_entry_id"]), ("Inbox/Wenjie", "A2", "5A", "A1"))
        self.assertEqual(self.names(wen), ["School Subtype Needed"], "nested level")
        self.assertEqual(wen["children"][0]["path"], "Inbox/Wenjie/School Subtype Needed")
        self.assertEqual(self.names(inbox["children"][1]), ["Completed"])
        private = inbox["children"][2]
        self.assertFalse(private["accessible"])
        self.assertTrue(private["error"])
        self.assertEqual(private["entry_id"], "", "no fake identifier for a folder that cannot be opened")
        d = r["data"]["discovery"]
        self.assertEqual(d["hierarchy_source"], "outlook+mapi")
        self.assertIn("Inbox", d["repaired"])
        self.assertEqual((d["descendants"], d["problems"]), (6, 1))
        self.assertTrue(any("Inbox/Private" in e for e in d["errors"]), d["errors"])
        self.assertTrue(any("'Inbox': 3 child folder(s)" in line for line in d["log"]), d["log"])
        self.assertEqual(r["data"]["mailbox"], "Team Mailbox")
        self.assertEqual((self.mapi_calls["init"], self.mapi_calls["uninit"]), (1, 1), "MAPI released with the session")
        self.assertEqual(self.calls["uninit"], 1)

    def test_subtree_of_the_inbox_by_entry_id(self):
        r = self.run_op("list_folders", {"store_id": "5A", "root_entry_id": "A1"})
        self.assertTrue(r["ok"], r)
        root = r["data"]["root"]
        self.assertEqual(root["path"], "Inbox")
        self.assertEqual(self.names(root), ["Wenjie", "NJIIS Team", "Private"])
        self.assertEqual(r["data"]["discovery"]["immediate_children"], 3)
        self.assertIn(bytes.fromhex("EF01"), self.world.tables_opened, "the Inbox table was opened with the parent's row entry ID")

    def test_subtree_of_a_nested_folder(self):
        r = self.run_op("list_folders", {"store_id": "5A", "root_entry_id": "A2"})
        self.assertTrue(r["ok"], r)
        self.assertEqual(r["data"]["root"]["path"], "Inbox/Wenjie")
        self.assertEqual(self.names(r["data"]["root"]), ["School Subtype Needed"])

    def test_shared_mailbox_by_name_uses_the_mounted_store_not_the_personal_inbox(self):
        r = self.run_op("list_folders", {"shared_mailbox": "team mailbox"})
        self.assertTrue(r["ok"], r)
        self.assertEqual(r["data"]["store_id"], "5A")
        self.assertEqual(self.names(r["data"]["root"]["children"][0]), ["Wenjie", "NJIIS Team", "Private"])

    def test_name_resolving_to_the_personal_mailbox_is_refused(self):
        personal_inbox = FakeFolder("Inbox", "C1", parent=self.app.ns.DefaultStore.GetRootFolder(), store_id="6B")

        class Rec:
            def Resolve(self):
                return True
        self.app.ns.CreateRecipient = lambda name: Rec()
        self.app.ns.GetSharedDefaultFolder = lambda rec, kind: personal_inbox
        r = self.run_op("list_folders", {"shared_mailbox": "Some Other Box"})
        self.assertFalse(r["ok"])
        self.assertEqual(r["error_code"], "MAILBOX_IS_PERSONAL")

    def test_sync_all_child_folders_reads_hidden_children_and_honours_exclusions(self):
        req = {"key": "5A|A1", "store_id": "5A", "entry_id": "A1", "path": "Inbox", "since_utc": "2026-11-01T00:00:00Z",
               "include_subfolders": True, "exclude_entry_ids": ["A5"]}
        r = self.run_op("sync_messages", {"folders": [req]})
        self.assertTrue(r["ok"], r)
        res = r["data"]["folders"][0]
        self.assertEqual(sorted(m["entry_id"] for m in res["messages"]), ["N1", "S1", "W1"], "excluded NJIIS Team/Completed not scanned")
        self.assertEqual(res["folders_scanned"], 4)
        self.assertFalse(res.get("incomplete", False), "an unopenable folder is a warning, not an incomplete scan")
        self.assertTrue(any("Private" in x for x in res["folder_warnings"]))
        s1 = next(m for m in res["messages"] if m["entry_id"] == "S1")
        self.assertEqual(s1["folder_ancestors"], ["A2", "A1", "A0"])
        self.assertEqual(r["data"]["stats"]["hierarchy_source"], "outlook+mapi")
        # this folder only: no subfolder is read
        r = self.run_op("sync_messages", {"folders": [dict(req, include_subfolders=False)]})
        self.assertEqual(r["data"]["folders"][0]["messages"], [])

    def test_scope_inventory_and_item_search_descend_into_hidden_children(self):
        r = self.run_op("scope_inventory", {"scope_key": "k", "folders": [{"store_id": "5A", "entry_id": "A1", "include_subfolders": True, "path": "Inbox"}], "exclude": []})
        self.assertTrue(r["ok"], r)
        paths = [f["path"] for f in r["data"]["folders"]]
        self.assertIn("Inbox/Wenjie/School Subtype Needed", paths)
        self.assertFalse(r["data"]["complete"], "the unopenable folder makes the inventory incomplete (no false 'left scope')")
        r = self.run_op("open_reply", {"entry_id": "OLD", "imid": "<s1@x>", "folders": [{"store_id": "5A", "entry_id": "A1", "recursive": True}]})
        self.assertTrue(r["ok"], r)
        self.assertTrue(r["data"]["relocated"])

    def test_mapi_unavailable_falls_back_to_outlook_and_says_so(self):
        install_fake_mapi(self.world, fail_init=True)
        r = self.run_op("list_folders", {"store_id": "5A"})
        self.assertTrue(r["ok"], r)
        d = r["data"]["discovery"]
        self.assertEqual(d["hierarchy_source"], "outlook")
        self.assertTrue(d["mapi_error"])
        self.assertTrue(any("Extended MAPI unavailable" in line for line in d["log"]))

    def test_unreadable_subfolders_are_an_error_not_an_empty_folder(self):
        install_fake_mapi(self.world, fail_init=True)
        root = self.app.ns.GetFolderFromID("A0")
        broken = BrokenFolders("Broken", "A9", parent=root, store_id="5A")
        self.app.ns._folders["A9"] = broken
        r = self.run_op("list_folders", {"store_id": "5A", "root_entry_id": "A9"})
        self.assertTrue(r["ok"], r)
        node = r["data"]["root"]
        self.assertEqual(node["error"], "Subfolders could not be read")
        self.assertFalse(node["children_loaded"])
        self.assertTrue(any("0x80040115" in e for e in r["data"]["discovery"]["errors"]))
        req = {"key": "k", "store_id": "5A", "entry_id": "A9", "path": "Broken", "since_utc": "2026-11-01T00:00:00Z", "include_subfolders": True}
        res = self.run_op("sync_messages", {"folders": [req]})["data"]["folders"][0]
        self.assertTrue(res["incomplete"], "a subtree that could not be listed is reported, never silently skipped")

    def test_object_model_failure_alone_is_covered_by_mapi(self):
        # Outlook raises for the root's Folders, MAPI still lists it
        root = self.app.ns.GetFolderFromID("A0")
        broken_root = BrokenFolders("Team Mailbox", "A0", parent=root.Parent, store_id="5A")
        broken_root._children = root._children
        self.app.ns.Stores.Item(2)._root = broken_root
        self.app.ns._folders["A1"].Parent = broken_root
        orig = self.app.ns.GetFolderFromID

        def get(eid, sid=None):
            return self.app.ns._folders["A1"] if eid == "B1" else orig(eid, sid)
        self.app.ns.GetFolderFromID = get
        r = self.run_op("list_folders", {"store_id": "5A"})
        self.assertTrue(r["ok"], r)
        self.assertEqual(self.names(r["data"]["root"]), ["Inbox"])
        self.assertEqual(self.names(r["data"]["root"]["children"][0]), ["Wenjie", "NJIIS Team", "Private"])

    def test_discovery_never_writes(self):
        self.run_op("list_folders", {"store_id": "5A"})
        req = {"key": "k", "store_id": "5A", "entry_id": "A1", "path": "Inbox", "since_utc": "2026-11-01T00:00:00Z", "include_subfolders": True}
        r = self.run_op("sync_messages", {"folders": [req]})
        for f in ("A2", "A3", "A4"):
            for m in self.app.ns._folders[f].Items._items:
                self.assertEqual(m._writes, [], f"no property of {m.EntryID} was changed (read state, flags)")
        self.assertTrue(r["ok"])


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
