"""
ASR/Audit Support Desk - Outlook COM worker.

Short-lived process: reads ONE JSON request from stdin, performs ONE Outlook COM
operation on its own (COM-initialized) main thread, writes ONE JSON response to
stdout and exits. No live COM object ever leaves this process.

Request:  {"op": "<operation>", "params": {...}}
Response: {"ok": bool, "data": {...}, "error_code": str|None,
           "message": str|None, "diagnostics": {...}}

Operations: test_connection, list_mailboxes, list_folders, sync_messages,
            get_message, create_reply_draft

Safety rules enforced here:
  * never sends, deletes, moves, marks read or archives any item
  * never logs message bodies (diagnostics contain counts/codes only)
"""

from __future__ import annotations

import datetime as _dt
import heapq
import json
import logging
import os
import re
import sys
import time
import traceback

LOG = logging.getLogger("outlook_worker")

OL_MAIL_ITEM_CLASS = 43       # olMail
OL_FOLDER_INBOX = 6           # olFolderInbox
OL_BY_VALUE = 1               # olByValue
PR_INTERNET_MESSAGE_ID = "http://schemas.microsoft.com/mapi/proptag/0x1035001F"
PR_ATTACH_CONTENT_ID = "http://schemas.microsoft.com/mapi/proptag/0x3712001F"
PR_ATTACHMENT_HIDDEN = "http://schemas.microsoft.com/mapi/proptag/0x7FFE000B"
PR_SMTP_ADDRESS = "http://schemas.microsoft.com/mapi/proptag/0x39FE001F"
CO_E_SERVER_EXEC_FAILURE = 0x80080005  # COM call across elevation levels (elevated app vs. normal Outlook)


class WorkerError(Exception):
    def __init__(self, code: str, message: str, **diag):
        super().__init__(message)
        self.code = code
        self.message = message
        self.diag = diag


# --------------------------------------------------------------------------- helpers

def safe(obj, attr, default=None):
    """Defensive COM property access: not every item exposes every property."""
    try:
        value = getattr(obj, attr)
        return default if value is None else value
    except Exception:  # noqa: BLE001 - COM raises many exception types
        return default


def to_utc_iso(value) -> str:
    """Outlook returns local wall-clock times (pywin32 may tag them as UTC). Treat as local."""
    if value is None:
        return ""
    try:
        naive = _dt.datetime(value.year, value.month, value.day, value.hour, value.minute, value.second)
        if naive.year < 1990 or naive.year > 4000:
            return ""
        return naive.astimezone().astimezone(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    except Exception:  # noqa: BLE001
        return ""


def parse_utc(s: str) -> _dt.datetime:
    return _dt.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=_dt.timezone.utc)


def sender_smtp(item) -> str:
    """SMTP address of the sender, resolving Exchange (EX) addresses when possible."""
    addr_type = safe(item, "SenderEmailType", "")
    addr = safe(item, "SenderEmailAddress", "") or ""
    if addr_type == "EX" or (addr and "@" not in addr):
        try:
            sender = item.Sender
            ex_user = sender.GetExchangeUser()
            if ex_user is not None and ex_user.PrimarySmtpAddress:
                return ex_user.PrimarySmtpAddress
        except Exception:  # noqa: BLE001
            pass
        try:
            return item.Sender.PropertyAccessor.GetProperty(PR_SMTP_ADDRESS)
        except Exception:  # noqa: BLE001
            pass
    return addr


def internet_message_id(item) -> str:
    try:
        return item.PropertyAccessor.GetProperty(PR_INTERNET_MESSAGE_ID) or ""
    except Exception:  # noqa: BLE001
        return ""


def folder_path(folder) -> str:
    """Path below the store root, e.g. 'Inbox/Wenjie'."""
    parts = []
    f = folder
    for _ in range(20):
        parent = safe(f, "Parent")
        # the store root's parent is the Namespace (Class 0) - stop there
        if parent is None or safe(parent, "Class", 0) != 2:  # 2 = olFolder
            break
        parts.append(safe(f, "Name", "?"))
        f = parent
    return "/".join(reversed(parts))


def message_dict(item, folder, store_id: str, body_max: int) -> dict:
    body = safe(item, "Body", "") or ""
    truncated = len(body) > body_max
    if truncated:
        body = body[:body_max]
    attachments = 0
    try:
        attachments = int(item.Attachments.Count)
    except Exception:  # noqa: BLE001
        pass
    return {
        "store_id": store_id,
        "entry_id": safe(item, "EntryID", ""),
        "internet_message_id": internet_message_id(item),
        "conversation_id": safe(item, "ConversationID", "") or "",
        "folder_entry_id": safe(folder, "EntryID", ""),
        "folder_path": folder_path(folder),
        "subject": safe(item, "Subject", "") or "",
        "sender_name": safe(item, "SenderName", "") or "",
        "sender_email": sender_smtp(item) or "",
        "received_at": to_utc_iso(safe(item, "ReceivedTime")),
        "body": body,
        "body_truncated": truncated,
        "has_attachments": attachments > 0,
        "metadata": {
            "importance": safe(item, "Importance", 1),
            "categories": safe(item, "Categories", "") or "",
            "unread": bool(safe(item, "UnRead", False)),
            "attachment_count": attachments,
            "conversation_topic": safe(item, "ConversationTopic", "") or "",
            "to": safe(item, "To", "") or "",
            "cc": safe(item, "CC", "") or "",
        },
    }


# --------------------------------------------------------------------------- Outlook session

class OutlookSession:
    """Context manager that initializes COM on this thread and releases it afterwards."""

    def __init__(self):
        self.app = None
        self.ns = None
        self._pythoncom = None

    def __enter__(self):
        try:
            import pythoncom  # type: ignore
            import win32com.client  # type: ignore
        except ImportError as exc:
            raise WorkerError("PYWIN32_MISSING", "The pywin32 package is not installed for this Python.", detail=str(exc))
        self._pythoncom = pythoncom
        pythoncom.CoInitialize()
        try:
            self.app = win32com.client.Dispatch("Outlook.Application")
            self.ns = self.app.GetNamespace("MAPI")
        except Exception as exc:  # noqa: BLE001
            pythoncom.CoUninitialize()
            hresult = (getattr(exc, "hresult", 0) or 0) & 0xFFFFFFFF
            if hresult == CO_E_SERVER_EXEC_FAILURE:
                # Typical cause: this app runs elevated ("Run as administrator") while Outlook does not.
                # Windows does not allow COM between different elevation levels.
                message = ("Outlook could not be reached because this app and Outlook are running at different "
                           "permission levels. Do not start the Support Desk with 'Run as administrator', "
                           "or close Outlook and start it again normally.")
            else:
                message = ("Classic Outlook for Windows could not be reached. "
                           "Make sure Outlook (classic) is installed and your profile is set up.")
            raise WorkerError("OUTLOOK_UNAVAILABLE", message, detail=type(exc).__name__,
                              hresult=f"0x{hresult:08X}" if hresult else None)
        return self

    def __exit__(self, *exc):
        self.ns = None
        self.app = None
        if self._pythoncom is not None:
            try:
                self._pythoncom.CoUninitialize()
            except Exception:  # noqa: BLE001
                pass
        return False


def store_by_id(ns, store_id: str):
    for i in range(1, int(ns.Stores.Count) + 1):
        st = ns.Stores.Item(i)
        if safe(st, "StoreID") == store_id:
            return st
    return None


def find_by_path(root, path: str):
    f = root
    for name in [p for p in path.split("/") if p]:
        nxt = None
        for i in range(1, int(f.Folders.Count) + 1):
            sub = f.Folders.Item(i)
            if (safe(sub, "Name", "") or "").lower() == name.lower():
                nxt = sub
                break
        if nxt is None:
            return None
        f = nxt
    return f


def folder_tree(folder, store_id: str, depth: int, max_depth: int, base: str = "") -> dict:
    name = safe(folder, "Name", "?")
    path = f"{base}/{name}" if base else name
    node = {"name": name, "entry_id": safe(folder, "EntryID", ""), "store_id": store_id, "path": path, "children": []}
    if depth < max_depth:
        try:
            for i in range(1, int(folder.Folders.Count) + 1):
                sub = folder.Folders.Item(i)
                # only mail folders (DefaultItemType 0 = olMailItem)
                if safe(sub, "DefaultItemType", 0) == 0:
                    node["children"].append(folder_tree(sub, store_id, depth + 1, max_depth, path))
        except Exception:  # noqa: BLE001
            node["error"] = "subfolders unavailable"
    return node


# --------------------------------------------------------------------------- operations (real Outlook)

def op_test_connection(s: OutlookSession, p: dict) -> dict:
    ns = s.ns
    default_store = ""
    try:
        default_store = ns.DefaultStore.DisplayName
    except Exception:  # noqa: BLE001
        pass
    return {
        "outlook_version": safe(s.app, "Version", ""),
        "profile": safe(ns, "CurrentProfileName", ""),
        "default_store": default_store,
        "store_count": int(safe(ns.Stores, "Count", 0)),
    }


def op_list_mailboxes(s: OutlookSession, p: dict) -> dict:
    out = []
    for i in range(1, int(s.ns.Stores.Count) + 1):
        st = s.ns.Stores.Item(i)
        out.append({
            "display_name": safe(st, "DisplayName", ""),
            "store_id": safe(st, "StoreID", ""),
            "exchange_store_type": safe(st, "ExchangeStoreType", -1),
            "is_data_file": bool(safe(st, "IsDataFileStore", False)),
        })
    return {"mailboxes": out}


def op_list_folders(s: OutlookSession, p: dict) -> dict:
    max_depth = int(p.get("max_depth", 6))
    shared = (p.get("shared_mailbox") or "").strip()
    if shared:
        # mailbox not added as a store: open its Inbox via the recipient
        rec = s.ns.CreateRecipient(shared)
        if not rec.Resolve():
            raise WorkerError("MAILBOX_NOT_FOUND", f"The mailbox '{shared}' could not be resolved in the address book.")
        try:
            inbox = s.ns.GetSharedDefaultFolder(rec, OL_FOLDER_INBOX)
        except Exception as exc:  # noqa: BLE001
            raise WorkerError("ACCESS_DENIED", "You do not have access to that mailbox's Inbox.", detail=type(exc).__name__)
        store_id = safe(inbox, "StoreID", "")
        return {"store_id": store_id, "root": folder_tree(inbox, store_id, 0, max_depth)}
    store_id = p.get("store_id") or ""
    st = store_by_id(s.ns, store_id) if store_id else s.ns.DefaultStore
    if st is None:
        raise WorkerError("MAILBOX_NOT_FOUND", "The selected mailbox is no longer available in this Outlook profile.")
    root = st.GetRootFolder()
    sid = safe(st, "StoreID", store_id)
    tree = folder_tree(root, sid, 0, max_depth)
    # present paths relative to the store root ("Inbox/Wenjie")
    def strip(node, prefix):
        node["path"] = node["path"][len(prefix) + 1:] if node["path"].startswith(prefix + "/") else ""
        for c in node["children"]:
            strip(c, prefix)
    strip(tree, tree["name"])
    return {"store_id": sid, "root": tree}


def resolve_folder(s: OutlookSession, req: dict):
    """Find a folder by EntryID; if moved/renamed/deleted, try its display path. Returns (folder, recovered)."""
    try:
        return s.ns.GetFolderFromID(req["entry_id"], req.get("store_id") or None), False
    except Exception:  # noqa: BLE001
        pass
    st = store_by_id(s.ns, req.get("store_id", ""))
    if st is not None and req.get("path"):
        f = find_by_path(st.GetRootFolder(), req["path"])
        if f is not None:
            return f, True
    raise WorkerError("FOLDER_NOT_FOUND", f"Folder '{req.get('path', '')}' was not found (renamed, moved, deleted or no access).")


def iter_mail(folder, since_utc: str):
    items = folder.Items
    since = parse_utc(since_utc)
    restricted = None
    try:
        # DASL datetime comparisons are in UTC
        restricted = items.Restrict(
            '@SQL="urn:schemas:httpmail:datereceived" >= \'' + since.strftime("%Y-%m-%d %H:%M") + "'"
        )
    except Exception:  # noqa: BLE001
        try:
            local = since.astimezone().strftime("%m/%d/%Y %I:%M %p")
            restricted = items.Restrict(f"[ReceivedTime] >= '{local}'")
        except Exception:  # noqa: BLE001
            restricted = items
    try:
        restricted.Sort("[ReceivedTime]", False)  # oldest first
    except Exception:  # noqa: BLE001
        pass
    item = restricted.GetFirst()
    while item is not None:
        yield item
        item = restricted.GetNext()


def collect_folders(folder, include_sub: bool):
    yield folder
    if include_sub:
        try:
            for i in range(1, int(folder.Folders.Count) + 1):
                sub = folder.Folders.Item(i)
                if safe(sub, "DefaultItemType", 0) == 0:
                    yield from collect_folders(sub, True)
        except Exception:  # noqa: BLE001
            return


def op_sync_messages(s: OutlookSession, p: dict) -> dict:
    max_items = int(p.get("max_items", 500))
    body_max = int(p.get("body_max_chars", 100000))
    results = []
    stats = {"folders": 0, "items_scanned": 0, "skipped_non_mail": 0}
    for req in p.get("folders", []):
        res = {"key": req.get("key"), "ok": True, "recovered": False, "truncated": False, "messages": []}
        try:
            folder, recovered = resolve_folder(s, req)
            if recovered:
                res["recovered"] = True
                res["resolved_entry_id"] = safe(folder, "EntryID", "")
                res["resolved_path"] = folder_path(folder)
            store_id = safe(folder, "StoreID", req.get("store_id", ""))

            def mail_in(f):
                stats["folders"] += 1
                for item in iter_mail(f, req["since_utc"]):
                    stats["items_scanned"] += 1
                    if safe(item, "Class", 0) != OL_MAIL_ITEM_CLASS:
                        stats["skipped_non_mail"] += 1
                        continue
                    yield to_utc_iso(safe(item, "ReceivedTime")), item, f

            # One oldest-first stream over the folder and its subfolders, so that when the cap is hit
            # every message older than the newest one returned has been returned (the caller's watermark
            # relies on this). Capping folder by folder would skip older mail in later subfolders.
            streams = [mail_in(f) for f in collect_folders(folder, bool(req.get("include_subfolders")))]
            for _, item, f in heapq.merge(*streams, key=lambda t: t[0]):
                if len(res["messages"]) >= max_items:
                    res["truncated"] = True
                    break
                res["messages"].append(message_dict(item, f, store_id, body_max))
        except WorkerError as exc:
            res.update({"ok": False, "error_code": exc.code, "message": exc.message})
        except Exception as exc:  # noqa: BLE001
            res.update({"ok": False, "error_code": "COM_ERROR", "message": f"Outlook error while reading '{req.get('path', '')}'.", "detail": type(exc).__name__})
        results.append(res)
    if results and all(not r["ok"] for r in results):
        first = results[0]
        raise WorkerError(first.get("error_code", "COM_ERROR"), first.get("message", "Folder unavailable"), **stats)
    return {"folders": results, "stats": stats}


def op_get_message(s: OutlookSession, p: dict) -> dict:
    try:
        item = s.ns.GetItemFromID(p["entry_id"], p.get("store_id") or None)
    except Exception:  # noqa: BLE001
        raise WorkerError("ITEM_NOT_FOUND", "The email could not be found in Outlook (it may have been moved or deleted).")
    folder = safe(item, "Parent")
    d = message_dict(item, folder, p.get("store_id", ""), 10_000_000)
    d["html_body"] = safe(item, "HTMLBody", "") or ""
    return d


BODY_TAG = re.compile(r"<body[^>]*>", re.IGNORECASE)


def insert_html(existing: str, new_html: str) -> str:
    m = BODY_TAG.search(existing or "")
    if not m:
        return new_html + (existing or "")
    return existing[: m.end()] + new_html + existing[m.end():]


def add_inline_images(mail, images: list) -> int:
    added = 0
    for img in images or []:
        path, cid = img.get("path"), img.get("cid")
        if not path or not cid or not os.path.isfile(path):
            continue
        att = mail.Attachments.Add(os.path.abspath(path), OL_BY_VALUE, 0)
        pa = att.PropertyAccessor
        pa.SetProperty(PR_ATTACH_CONTENT_ID, cid)
        try:
            pa.SetProperty(PR_ATTACHMENT_HIDDEN, True)
        except Exception:  # noqa: BLE001
            pass
        added += 1
    return added


def op_create_reply_draft(s: OutlookSession, p: dict) -> dict:
    """Create (and SAVE) a draft. Never calls .Send()."""
    html = p.get("html_body") or ""
    if not html.strip():
        raise WorkerError("BAD_REQUEST", "The draft body is empty.")
    mode = p.get("mode", "reply")
    if mode == "reply":
        try:
            original = s.ns.GetItemFromID(p["entry_id"], p.get("store_id") or None)
        except Exception:  # noqa: BLE001
            raise WorkerError("ITEM_NOT_FOUND", "The original email could not be found in Outlook (moved or deleted). Try again after the next sync.")
        mail = original.Reply()
    else:
        mail = s.app.CreateItem(0)  # olMailItem
        if p.get("to"):
            mail.To = p["to"]
    images = add_inline_images(mail, p.get("inline_images"))
    if p.get("subject"):
        mail.Subject = p["subject"]
    mail.HTMLBody = insert_html(safe(mail, "HTMLBody", "") or "", html)
    mail.Save()
    entry_id = safe(mail, "EntryID", "")
    if p.get("display"):
        try:
            mail.Display(False)
        except Exception:  # noqa: BLE001
            pass
    return {"entry_id": entry_id, "mode": mode, "inline_images": images, "sent": False}


OPS = {
    "test_connection": op_test_connection,
    "list_mailboxes": op_list_mailboxes,
    "list_folders": op_list_folders,
    "sync_messages": op_sync_messages,
    "get_message": op_get_message,
    "create_reply_draft": op_create_reply_draft,
}


# --------------------------------------------------------------------------- mock mode (tests / demo without Outlook)

def mock_dispatch(op: str, p: dict) -> dict:
    if op == "test_connection":
        return {"outlook_version": "mock", "profile": "Mock", "default_store": "mock@example.org", "store_count": 1}
    if op == "list_mailboxes":
        return {"mailboxes": [{"display_name": "DOH-Schoolimmforms-team, Vpdp [DOH]", "store_id": "MOCKSTORE", "exchange_store_type": 1, "is_data_file": False}]}
    if op == "list_folders":
        kids = [{"name": n, "entry_id": f"MOCK-{n}", "store_id": "MOCKSTORE", "path": f"Inbox/{n}", "children": []} for n in ["Aaron", "Jeni", "Mersi", "NJIIS Team"]]
        wen = {"name": "Wenjie", "entry_id": "MOCK-Wenjie", "store_id": "MOCKSTORE", "path": "Inbox/Wenjie", "children": [
            {"name": "School Subtype Needed", "entry_id": "MOCK-Sub", "store_id": "MOCKSTORE", "path": "Inbox/Wenjie/School Subtype Needed", "children": []},
            {"name": "Verify Receipt of ASR/Audit", "entry_id": "MOCK-Ver", "store_id": "MOCKSTORE", "path": "Inbox/Wenjie/Verify Receipt of ASR/Audit", "children": []}]}
        inbox = {"name": "Inbox", "entry_id": "MOCK-Inbox", "store_id": "MOCKSTORE", "path": "Inbox", "children": kids + [wen]}
        return {"store_id": "MOCKSTORE", "root": {"name": "Mailbox", "entry_id": "MOCK-Root", "store_id": "MOCKSTORE", "path": "", "children": [inbox]}}
    if op == "sync_messages":
        return {"folders": [{"key": f.get("key"), "ok": True, "recovered": False, "truncated": False, "messages": []} for f in p.get("folders", [])], "stats": {}}
    if op == "create_reply_draft":
        if not (p.get("html_body") or "").strip():
            raise WorkerError("BAD_REQUEST", "The draft body is empty.")
        return {"entry_id": "MOCK-DRAFT", "mode": p.get("mode", "reply"), "inline_images": 0, "sent": False}
    if op == "get_message":
        return {"body": "mock body", "html_body": ""}
    raise WorkerError("BAD_REQUEST", f"Unknown operation '{op}'.")


# --------------------------------------------------------------------------- entry point

def handle(request: dict) -> dict:
    started = time.perf_counter()
    op = request.get("op", "")
    params = request.get("params") or {}
    diag = {"op": op}
    try:
        if not isinstance(params, dict):
            raise WorkerError("BAD_REQUEST", "params must be an object")
        if params.get("mock") or os.environ.get("ASR_DESK_MOCK_OUTLOOK") == "1":
            data = mock_dispatch(op, params)
        else:
            fn = OPS.get(op)
            if fn is None:
                raise WorkerError("BAD_REQUEST", f"Unknown operation '{op}'.")
            with OutlookSession() as session:
                data = fn(session, params)
        diag["elapsed_ms"] = round((time.perf_counter() - started) * 1000)
        return {"ok": True, "data": data, "error_code": None, "message": None, "diagnostics": diag}
    except WorkerError as exc:
        diag.update(exc.diag)
        diag["elapsed_ms"] = round((time.perf_counter() - started) * 1000)
        return {"ok": False, "data": None, "error_code": exc.code, "message": exc.message, "diagnostics": diag}
    except Exception as exc:  # noqa: BLE001
        diag["exception"] = type(exc).__name__
        diag["trace"] = traceback.format_exc(limit=3)[-1500:]
        diag["elapsed_ms"] = round((time.perf_counter() - started) * 1000)
        code = "COM_ERROR" if "com_error" in type(exc).__name__.lower() else "INTERNAL"
        return {"ok": False, "data": None, "error_code": code, "message": "Outlook operation failed.", "diagnostics": diag}


def main() -> int:
    logging.basicConfig(level=logging.WARNING, stream=sys.stderr, format="%(levelname)s %(message)s")
    raw = sys.stdin.buffer.read().decode("utf-8", errors="replace")
    try:
        request = json.loads(raw) if raw.strip() else {}
    except json.JSONDecodeError:
        request = {"op": "", "params": "invalid"}
    response = handle(request)
    sys.stdout.buffer.write((json.dumps(response, ensure_ascii=True, default=str) + "\n").encode("ascii"))
    sys.stdout.buffer.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
