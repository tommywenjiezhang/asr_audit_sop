"""
ASR/Audit Support Desk - Outlook COM worker.

Short-lived process: reads ONE JSON request from stdin, performs ONE Outlook COM
operation on its own (COM-initialized) main thread, writes ONE JSON response to
stdout and exits. No live COM object ever leaves this process.

Request:  {"op": "<operation>", "params": {...}}
Response: {"ok": bool, "data": {...}, "error_code": str|None,
           "message": str|None, "diagnostics": {...}}

Operations: test_connection, list_mailboxes, list_folders, validate_folder,
            sync_messages, get_message, scope_inventory, locate_messages,
            open_reply, open_new_message, complete_messages, move_messages,
            list_accounts, send_notification, find_notification,
            create_reply_draft (legacy, unused by the UI)

Safety rules enforced here:
  * never deletes any item and never changes the read state
  * scanning and verification (sync_messages, get_message, scope_inventory,
    locate_messages, find_notification) only read
  * open_reply / open_new_message only display a window; staff send replies to
    schools manually
  * moves happen ONLY in complete_messages (Mark Done: flag complete + move to
    the completion folder) and move_messages (assignment: move to the staff
    member's folder, no flag change)
  * the ONLY automatic send is send_notification: one assignment handoff email
    to one staff address, marked with an X-ASR-Desk-Notification header so it is
    never imported as a ticket
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
OL_MARK_COMPLETE = 5          # OlMarkInterval.olMarkComplete
OL_FLAG_COMPLETE = 1          # OlFlagStatus.olFlagComplete
OL_FOLDER_OUTBOX = 4
OL_FOLDER_SENT = 5
PR_TRANSPORT_HEADERS = "http://schemas.microsoft.com/mapi/proptag/0x007D001F"
# Internet header on assignment notifications sent by this app (named property in the PS_INTERNET_HEADERS set,
# so it is transmitted as "X-ASR-Desk-Notification: <id>"). Recognized notifications are never imported.
NOTIFY_HEADER = "X-ASR-Desk-Notification"
NOTIFY_PROP = "http://schemas.microsoft.com/mapi/string/{00020386-0000-0000-C000-000000000046}/" + NOTIFY_HEADER
NOTIFY_BODY_RE = re.compile(r"\[ASR-Desk-Notification:\s*([^\]\s]+)\]")
SEND_USING_ACCOUNT_DISPID = 64209  # MailItem.SendUsingAccount (pywin32 needs Invoke to set it)


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


def folder_ancestors(folder) -> list:
    """EntryIDs of the folder's parents (parent first, up to the store root)."""
    ids = []
    f = folder
    for _ in range(25):
        parent = safe(f, "Parent")
        if parent is None or safe(parent, "Class", 0) != 2:  # 2 = olFolder
            break
        ids.append(safe(parent, "EntryID", ""))
        f = parent
    return ids


def notification_id(item, body: str) -> str:
    """Id of an assignment notification sent by this app ('' for normal mail): header, then body marker."""
    try:
        v = item.PropertyAccessor.GetProperty(NOTIFY_PROP)
        if v:
            return str(v)
    except Exception:  # noqa: BLE001
        pass
    try:
        headers = item.PropertyAccessor.GetProperty(PR_TRANSPORT_HEADERS) or ""
        m = re.search(r"^" + NOTIFY_HEADER + r":\s*(\S+)", headers, re.IGNORECASE | re.MULTILINE)
        if m:
            return m.group(1)
    except Exception:  # noqa: BLE001
        pass
    m = NOTIFY_BODY_RE.search(body or "")
    return m.group(1) if m else ""


def message_dict(item, folder, store_id: str, body_max: int, ancestors: list | None = None) -> dict:
    body = safe(item, "Body", "") or ""
    nid = notification_id(item, body)
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
        "folder_ancestors": ancestors if ancestors is not None else folder_ancestors(folder),
        "notification_id": nid,
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
    root_id = (p.get("root_entry_id") or "").strip()
    if root_id:
        # only the subtree of one folder (fast: skips unrelated folders of the mailbox)
        try:
            folder = s.ns.GetFolderFromID(root_id, p.get("store_id") or None)
        except Exception:  # noqa: BLE001
            raise WorkerError("FOLDER_NOT_FOUND", "The monitored folder was not found (renamed, moved, deleted or no access).")
        store_id = safe(folder, "StoreID", p.get("store_id", ""))
        base = folder_path(folder)
        parent = base.rsplit("/", 1)[0] if "/" in base else ""
        return {"store_id": store_id, "root": folder_tree(folder, store_id, 0, min(max_depth, 4), parent)}
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


def iter_mail(folder, since_utc: str, unread_only: bool = False):
    items = folder.Items
    since = parse_utc(since_utc)
    restricted = None
    try:
        # DASL datetime comparisons are in UTC. Reading UnRead never changes it.
        flt = '"urn:schemas:httpmail:datereceived" >= \'' + since.strftime("%Y-%m-%d %H:%M") + "'"
        if unread_only:
            flt = "(" + flt + ' AND "urn:schemas:httpmail:read" = 0)'
        restricted = items.Restrict("@SQL=" + flt)
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


def collect_folders(folder, include_sub: bool, exclude=()):
    """The folder and (optionally) all its mail subfolders, skipping excluded folders and their subtrees."""
    if safe(folder, "EntryID", "") in exclude:
        return
    yield folder
    if include_sub:
        try:
            for i in range(1, int(folder.Folders.Count) + 1):
                sub = folder.Folders.Item(i)
                if safe(sub, "DefaultItemType", 0) == 0:
                    yield from collect_folders(sub, True, exclude)
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

            unread_only = bool(req.get("unread_only"))

            def mail_in(f):
                stats["folders"] += 1
                for item in iter_mail(f, req["since_utc"], unread_only):
                    stats["items_scanned"] += 1
                    if safe(item, "Class", 0) != OL_MAIL_ITEM_CLASS:
                        stats["skipped_non_mail"] += 1
                        continue
                    # also checked here in case the provider ignored the read filter
                    if unread_only and not safe(item, "UnRead", False):
                        stats["skipped_read"] = stats.get("skipped_read", 0) + 1
                        continue
                    yield to_utc_iso(safe(item, "ReceivedTime")), item, f

            # One oldest-first stream over the folder and its subfolders, so that when the cap is hit
            # every message older than the newest one returned has been returned (the caller's watermark
            # relies on this). Capping folder by folder would skip older mail in later subfolders.
            exclude = set(req.get("exclude_entry_ids") or [])
            folders = list(collect_folders(folder, bool(req.get("include_subfolders")), exclude))
            ancestors = {safe(f, "EntryID", ""): folder_ancestors(f) for f in folders}
            streams = [mail_in(f) for f in folders]
            for _, item, f in heapq.merge(*streams, key=lambda t: t[0]):
                if len(res["messages"]) >= max_items:
                    res["truncated"] = True
                    break
                res["messages"].append(message_dict(item, f, store_id, body_max, ancestors.get(safe(f, "EntryID", ""))))
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


# --------------------------------------------------------------------------- reply window / completion

def find_in_folder(folder, imid: str, errors: list | None = None):
    """Find a mail item in one folder by its Internet Message-ID (stable across moves)."""
    if not imid:
        return None
    flt = '@SQL="' + PR_INTERNET_MESSAGE_ID + '" = \'' + imid.replace("'", "''") + "'"
    try:
        found = folder.Items.Restrict(flt).GetFirst()
        if found is not None and internet_message_id(found) == imid:
            return found
    except Exception as exc:  # noqa: BLE001
        if errors is not None:
            errors.append(f"search failed in '{folder_path(folder)}' ({type(exc).__name__})")
    return None


def walk_folders(folder, include_sub: bool, exclude=(), errors: list | None = None):
    """Like collect_folders, but records folders whose subfolders could not be read."""
    if safe(folder, "EntryID", "") in exclude:
        return
    yield folder
    if include_sub:
        try:
            subs = [folder.Folders.Item(i) for i in range(1, int(folder.Folders.Count) + 1)]
        except Exception as exc:  # noqa: BLE001
            if errors is not None:
                errors.append(f"subfolders of '{folder_path(folder)}' could not be read ({type(exc).__name__})")
            return
        for sub in subs:
            if safe(sub, "DefaultItemType", 0) == 0:
                yield from walk_folders(sub, True, exclude, errors)


def find_item(s: OutlookSession, entry_id: str, store_id: str, imid: str, folders: list, errors: list | None = None):
    """Resolve a message by EntryID; if that fails (EntryIDs change when an item moves between
    folders), search the candidate folders by Internet Message-ID. A candidate with "recursive": true
    is searched together with all its mail subfolders. Returns (item, relocated).
    Folder problems are appended to `errors`, so "not found" is only claimed after a complete search."""
    if entry_id:
        try:
            item = s.ns.GetItemFromID(entry_id, store_id or None)
            if item is not None:
                return item, False
        except Exception:  # noqa: BLE001
            pass
    if not imid:
        return None, False
    searched = set()
    for f in folders or []:
        fid, sid = f.get("entry_id", ""), f.get("store_id", "")
        if not fid:
            continue
        try:
            top = s.ns.GetFolderFromID(fid, sid or None)
        except Exception as exc:  # noqa: BLE001
            if errors is not None:
                errors.append(f"folder '{f.get('path') or fid[-8:]}' could not be opened ({type(exc).__name__})")
            continue
        for folder in walk_folders(top, bool(f.get("recursive")), (), errors):
            key = safe(folder, "EntryID", "")
            if key in searched:
                continue
            searched.add(key)
            item = find_in_folder(folder, imid, errors)
            if item is not None:
                return item, True
    return None, False


def location(item) -> dict:
    parent = safe(item, "Parent")
    return {
        "entry_id": safe(item, "EntryID", ""),
        "store_id": safe(parent, "StoreID", "") if parent is not None else "",
        "folder_entry_id": safe(parent, "EntryID", "") if parent is not None else "",
        "folder_path": folder_path(parent) if parent is not None else "",
    }


def op_open_reply(s: OutlookSession, p: dict) -> dict:
    """Open Outlook's own Reply (or Reply All) window for the source email. The body is not touched,
    so Outlook adds the signature and the quoted conversation as usual. Nothing is saved or sent."""
    item, relocated = find_item(s, p.get("entry_id", ""), p.get("store_id", ""), p.get("imid", ""), p.get("folders", []))
    if item is None:
        raise WorkerError("ITEM_NOT_FOUND", "The original email could not be found in Outlook (moved, deleted or no access).")
    reply_all = bool(p.get("reply_all"))
    reply = item.ReplyAll() if reply_all else item.Reply()
    try:
        reply.Display(False)
    except Exception as exc:  # noqa: BLE001
        raise WorkerError("DISPLAY_FAILED", "Outlook could not show the reply window. Close any open Outlook dialog and try again.", detail=type(exc).__name__)
    return {"opened": True, "reply_all": reply_all, "relocated": relocated, "source": location(item)}


def op_open_new_message(s: OutlookSession, p: dict) -> dict:
    """Fallback when the source email is gone: a new (unthreaded) message to the sender, displayed only."""
    to = (p.get("to") or "").strip()
    if not to:
        raise WorkerError("BAD_REQUEST", "No recipient address is stored for this ticket.")
    mail = s.app.CreateItem(0)  # olMailItem
    mail.To = to
    if p.get("subject"):
        mail.Subject = p["subject"]
    try:
        mail.Display(False)
    except Exception as exc:  # noqa: BLE001
        raise WorkerError("DISPLAY_FAILED", "Outlook could not show the new message window.", detail=type(exc).__name__)
    return {"opened": True}


def op_validate_folder(s: OutlookSession, p: dict) -> dict:
    try:
        folder = s.ns.GetFolderFromID(p["entry_id"], p.get("store_id") or None)
    except Exception:  # noqa: BLE001
        raise WorkerError("FOLDER_NOT_FOUND", "The selected folder was not found (renamed, moved, deleted or no access).")
    if safe(folder, "DefaultItemType", 0) != 0:
        raise WorkerError("NOT_MAIL_FOLDER", "The selected folder is not a mail folder.")
    store = safe(folder, "Store")
    return {
        "entry_id": safe(folder, "EntryID", ""),
        "store_id": safe(folder, "StoreID", p.get("store_id", "")),
        "path": folder_path(folder),
        "name": safe(folder, "Name", ""),
        "mailbox": safe(store, "DisplayName", "") if store is not None else "",
    }


def same_folder(s: OutlookSession, a, b) -> bool:
    ida, idb = safe(a, "EntryID", ""), safe(b, "EntryID", "")
    if not ida or not idb:
        return False
    if ida == idb:
        return True
    try:
        return bool(s.ns.CompareEntryIDs(ida, idb))
    except Exception:  # noqa: BLE001
        return False


def is_flag_complete(item) -> bool:
    return safe(item, "FlagStatus", 0) == OL_FLAG_COMPLETE


def op_complete_messages(s: OutlookSession, p: dict) -> dict:
    """Mark Done: flag each incoming email complete, then move it to the completion folder.
    Each email gets its own result; one failure does not stop the others. Already-finished steps
    are skipped (an email already in the destination counts as moved). Nothing is deleted or sent."""
    dest_ref = p.get("dest") or {}
    try:
        dest = s.ns.GetFolderFromID(dest_ref["entry_id"], dest_ref.get("store_id") or None)
    except Exception:  # noqa: BLE001
        raise WorkerError("FOLDER_NOT_FOUND", "The completion folder was not found (renamed, moved, deleted or no access). Choose it again in Settings.")
    results = []
    for it in p.get("items", []):
        r = {"message_id": it.get("message_id"), "flagged": False, "moved": False, "new_entry_id": None,
             "new_store_id": None, "new_folder_entry_id": None, "new_folder_path": None, "error_code": None, "message": None}
        results.append(r)
        try:
            folders = list(it.get("folders") or []) + [dest_ref]
            item, _ = find_item(s, it.get("entry_id", ""), it.get("store_id", ""), it.get("imid", ""), folders)
        except Exception:  # noqa: BLE001
            item = None
        if item is None:
            r.update(error_code="ITEM_NOT_FOUND", message="The email was not found in Outlook (moved, deleted or no access).")
            continue
        # 1. flag complete
        if not it.get("need_flag", True) or is_flag_complete(item):
            r["flagged"] = True
        else:
            try:
                try:
                    item.MarkAsTask(OL_MARK_COMPLETE)
                except Exception:  # noqa: BLE001
                    item.FlagStatus = OL_FLAG_COMPLETE
                item.Save()
                r["flagged"] = True
            except Exception as exc:  # noqa: BLE001
                r.update(error_code="FLAG_FAILED", message=f"Could not flag the email complete ({type(exc).__name__}).")
                continue  # do not move an email whose flag could not be set
        # 2. move (never twice)
        target = item
        if same_folder(s, safe(item, "Parent"), dest) or not it.get("need_move", True):
            r["moved"] = True
        else:
            try:
                target = item.Move(dest)
                r["moved"] = True
            except Exception as exc:  # noqa: BLE001
                r.update(error_code="MOVE_FAILED", message=f"Could not move the email to the completion folder ({type(exc).__name__}). Check that you can write to that folder.")
        loc = location(target)
        r.update(new_entry_id=loc["entry_id"], new_store_id=loc["store_id"], new_folder_entry_id=loc["folder_entry_id"], new_folder_path=loc["folder_path"])
    return {"items": results}


# --------------------------------------------------------------------------- scope verification / assignment

def list_mail_ids(folder) -> list:
    """(EntryID, Internet Message-ID) of every mail item in one folder. Uses the fast Table API; falls back
    to Items. Raises on errors so the caller can mark the inventory incomplete."""
    try:
        table = folder.GetTable()
    except Exception:  # noqa: BLE001 - provider without tables: slower item loop
        table = None
    rows = []
    if table is not None:
        cols = table.Columns
        cols.RemoveAll()
        for c in ("EntryID", "MessageClass", PR_INTERNET_MESSAGE_ID):
            cols.Add(c)
        while not table.EndOfTable:
            v = table.GetNextRow().GetValues()
            if str(v[1] or "").startswith("IPM.Note"):
                rows.append((str(v[0] or ""), str(v[2] or "")))
        return rows
    items = folder.Items  # one collection object: each .Items access returns a new one in Outlook
    item = items.GetFirst()
    while item is not None:
        if safe(item, "Class", 0) == OL_MAIL_ITEM_CLASS:
            rows.append((safe(item, "EntryID", ""), internet_message_id(item)))
        item = items.GetNext()
    return rows


def op_scope_inventory(s: OutlookSession, p: dict) -> dict:
    """Every mail item currently in the scope folders (no date window, no unread filter) - identifiers only.
    `complete` is true only if every folder and subfolder was read without error; the app applies
    absence (emails that left the scope) only for complete inventories."""
    exclude = set(p.get("exclude") or [])
    out = {"scope_key": p.get("scope_key", ""), "complete": True, "folders": [], "items": [], "errors": []}
    seen = set()
    for req in p.get("folders", []):
        try:
            if req.get("default_folder"):
                # e.g. 6 = the Inbox of that mailbox (used to find where emails went)
                st = store_by_id(s.ns, req.get("store_id", ""))
                top = st.GetDefaultFolder(int(req["default_folder"]))
            else:
                top = s.ns.GetFolderFromID(req["entry_id"], req.get("store_id") or None)
        except Exception:  # noqa: BLE001
            out["complete"] = False
            out["errors"].append(f"Folder '{req.get('path', '')}' was not found or is not accessible.")
            continue
        for f in walk_folders(top, bool(req.get("include_subfolders")), exclude, out["errors"]):
            fid = safe(f, "EntryID", "")
            if fid in seen:
                continue
            seen.add(fid)
            out["folders"].append({"entry_id": fid, "store_id": safe(f, "StoreID", req.get("store_id", "")), "path": folder_path(f), "ancestors": folder_ancestors(f)})
            try:
                for entry, imid in list_mail_ids(f):
                    out["items"].append({"imid": imid, "entry_id": entry, "folder_entry_id": fid})
            except Exception as exc:  # noqa: BLE001
                out["errors"].append(f"'{folder_path(f)}' could not be read ({type(exc).__name__}).")
    if out["errors"]:
        out["complete"] = False
    return out


def op_locate_messages(s: OutlookSession, p: dict) -> dict:
    """Lookup only: where is each email now? status found | not_found (all candidate folders searched
    without error) | error (location unknown). Nothing is changed."""
    results = []
    for it in p.get("items", []):
        r = {"message_id": it.get("message_id"), "status": "error", "entry_id": "", "store_id": "", "folder_entry_id": "", "folder_path": "", "ancestors": [], "error": None}
        errors: list = []
        try:
            item, _ = find_item(s, it.get("entry_id", ""), it.get("store_id", ""), it.get("imid", ""), it.get("folders", []), errors)
            if item is not None:
                parent = safe(item, "Parent")
                r.update(status="found", entry_id=safe(item, "EntryID", ""), store_id=safe(parent, "StoreID", "") if parent is not None else "",
                         folder_entry_id=safe(parent, "EntryID", "") if parent is not None else "", folder_path=folder_path(parent) if parent is not None else "",
                         ancestors=folder_ancestors(parent) if parent is not None else [])
            elif errors:
                r["error"] = "; ".join(errors[:3])
            else:
                r["status"] = "not_found"
        except Exception as exc:  # noqa: BLE001
            r["error"] = type(exc).__name__
        results.append(r)
    return {"items": results}


def op_move_messages(s: OutlookSession, p: dict) -> dict:
    """Assignment: move each email into the staff member's folder. Read state and flags are not changed;
    an email already in the destination counts as moved; one failure does not stop the others."""
    dest_ref = p.get("dest") or {}
    try:
        dest = s.ns.GetFolderFromID(dest_ref["entry_id"], dest_ref.get("store_id") or None)
    except Exception:  # noqa: BLE001
        raise WorkerError("FOLDER_NOT_FOUND", "The staff member's folder was not found (renamed, moved, deleted or no access). Update it in Settings › Staff Assignment.")
    dest_ancestors = folder_ancestors(dest)
    results = []
    for it in p.get("items", []):
        r = {"message_id": it.get("message_id"), "moved": False, "new_entry_id": None, "new_store_id": None, "new_folder_entry_id": None,
             "new_folder_path": None, "ancestors": [], "error_code": None, "message": None}
        results.append(r)
        try:
            item, _ = find_item(s, it.get("entry_id", ""), it.get("store_id", ""), it.get("imid", ""), list(it.get("folders") or []) + [dest_ref])
        except Exception:  # noqa: BLE001
            item = None
        if item is None:
            r.update(error_code="ITEM_NOT_FOUND", message="The email was not found in Outlook (moved, deleted or no access).")
            continue
        target = item
        if not same_folder(s, safe(item, "Parent"), dest):
            try:
                target = item.Move(dest)
            except Exception as exc:  # noqa: BLE001
                r.update(error_code="MOVE_FAILED", message=f"Could not move the email to the staff folder ({type(exc).__name__}). Check that you can write to that folder.")
                continue
        loc = location(target)
        r.update(moved=True, new_entry_id=loc["entry_id"], new_store_id=loc["store_id"] or safe(dest, "StoreID", ""), new_folder_entry_id=loc["folder_entry_id"] or safe(dest, "EntryID", ""),
                 new_folder_path=loc["folder_path"] or folder_path(dest), ancestors=folder_ancestors(safe(target, "Parent")) if safe(target, "Parent") is not None else dest_ancestors)
    return {"items": results}


def op_list_accounts(s: OutlookSession, p: dict) -> dict:
    out = []
    try:
        accounts = s.ns.Accounts
        for i in range(1, int(accounts.Count) + 1):
            a = accounts.Item(i)
            out.append({"display_name": safe(a, "DisplayName", ""), "smtp": safe(a, "SmtpAddress", "")})
    except Exception:  # noqa: BLE001
        pass
    return {"accounts": out}


def find_account(s: OutlookSession, smtp: str):
    accounts = s.ns.Accounts
    for i in range(1, int(accounts.Count) + 1):
        a = accounts.Item(i)
        if (safe(a, "SmtpAddress", "") or "").strip().lower() == smtp.strip().lower():
            return a
    return None


def op_send_notification(s: OutlookSession, p: dict) -> dict:
    """Send the assignment handoff email to ONE staff address. Two stages:
    prepare (errors here mean nothing was sent) and send (errors here are reported as uncertain)."""
    to = (p.get("to") or "").strip()
    nid = (p.get("notification_id") or "").strip()
    if not to or "@" not in to or ";" in to or "," in to:
        raise WorkerError("BAD_REQUEST", "Exactly one staff email address is required.", stage="prepare")
    if not nid:
        raise WorkerError("BAD_REQUEST", "A notification id is required.", stage="prepare")
    acct_smtp = (p.get("account") or "").strip()
    try:
        account = find_account(s, acct_smtp) if acct_smtp else None
        if acct_smtp and account is None:
            raise WorkerError("ACCOUNT_NOT_FOUND", f"The sending account '{acct_smtp}' is not in this Outlook profile. Choose another in Settings › Staff Assignment.", stage="prepare")
        mail = s.app.CreateItem(0)  # olMailItem
        mail.To = to
        mail.Subject = p.get("subject") or "ASR/Audit ticket assigned to you"
        mail.HTMLBody = p.get("html_body") or ""
        try:
            mail.PropertyAccessor.SetProperty(NOTIFY_PROP, nid)
        except Exception:  # noqa: BLE001 - the body marker still identifies the notification
            pass
        if account is not None:
            try:
                mail.SendUsingAccount = account
            except Exception:  # noqa: BLE001
                mail._oleobj_.Invoke(*(SEND_USING_ACCOUNT_DISPID, 0, 8, 0, account))
        recips = mail.Recipients
        if int(recips.Count) != 1:
            raise WorkerError("BAD_REQUEST", "The notification must have exactly one recipient.", stage="prepare")
        if not recips.ResolveAll():
            raise WorkerError("RECIPIENT_UNRESOLVED", f"Outlook could not resolve the address '{to}'.", stage="prepare")
        mail.Save()
        draft_id = safe(mail, "EntryID", "")
    except WorkerError:
        raise
    except Exception as exc:  # noqa: BLE001
        raise WorkerError("NOTIFY_PREPARE_FAILED", f"The notification could not be prepared ({type(exc).__name__}); nothing was sent.", stage="prepare")
    try:
        mail.Send()
    except Exception as exc:  # noqa: BLE001
        raise WorkerError("SEND_UNCERTAIN", f"Outlook reported an error during Send ({type(exc).__name__}). The email may or may not have been sent.", stage="send")
    used = acct_smtp
    if not used:
        try:
            used = s.ns.Accounts.Item(1).SmtpAddress
        except Exception:  # noqa: BLE001
            used = ""
    return {"submitted": True, "draft_entry_id": draft_id, "account": used, "notification_id": nid}


def op_find_notification(s: OutlookSession, p: dict) -> dict:
    """Look for a notification (by its id) in Sent Items and Outbox of the sending account."""
    nid = (p.get("notification_id") or "").strip()
    acct_smtp = (p.get("account") or "").strip()
    store = None
    if acct_smtp:
        a = find_account(s, acct_smtp)
        store = safe(a, "DeliveryStore") if a is not None else None
    if store is None:
        store = s.ns.DefaultStore
    flt = '@SQL="' + NOTIFY_PROP + '" = \'' + nid.replace("'", "''") + "'"
    for fid, name in ((OL_FOLDER_SENT, "Sent Items"), (OL_FOLDER_OUTBOX, "Outbox")):
        try:
            folder = store.GetDefaultFolder(fid)
            item = folder.Items.Restrict(flt).GetFirst()
        except Exception as exc:  # noqa: BLE001
            raise WorkerError("SEARCH_FAILED", f"{name} could not be searched ({type(exc).__name__}).")
        if item is not None:
            return {"found": True, "where": name, "entry_id": safe(item, "EntryID", "")}
    return {"found": False}


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
    "open_reply": op_open_reply,
    "open_new_message": op_open_new_message,
    "validate_folder": op_validate_folder,
    "complete_messages": op_complete_messages,
    "scope_inventory": op_scope_inventory,
    "locate_messages": op_locate_messages,
    "move_messages": op_move_messages,
    "list_accounts": op_list_accounts,
    "send_notification": op_send_notification,
    "find_notification": op_find_notification,
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
    if op == "open_reply":
        return {"opened": True, "reply_all": bool(p.get("reply_all")), "relocated": False, "source": {}}
    if op == "open_new_message":
        return {"opened": True}
    if op == "validate_folder":
        return {"entry_id": p.get("entry_id", ""), "store_id": p.get("store_id", ""), "path": "Inbox/Completed", "name": "Completed", "mailbox": "Mock"}
    if op == "scope_inventory":
        return {"scope_key": p.get("scope_key", ""), "complete": True, "folders": [], "items": [], "errors": []}
    if op == "locate_messages":
        return {"items": [{"message_id": i.get("message_id"), "status": "not_found", "ancestors": []} for i in p.get("items", [])]}
    if op == "move_messages":
        return {"items": [{"message_id": i.get("message_id"), "moved": True, "new_entry_id": f"MOCK-M-{i.get('message_id')}", "new_store_id": "MOCKSTORE",
                           "new_folder_entry_id": (p.get("dest") or {}).get("entry_id", ""), "new_folder_path": "Inbox/Staff", "ancestors": ["MOCK-Inbox"]} for i in p.get("items", [])]}
    if op == "list_accounts":
        return {"accounts": [{"display_name": "Mock", "smtp": "mock@example.org"}]}
    if op == "send_notification":
        return {"submitted": True, "draft_entry_id": "MOCK-N", "account": p.get("account") or "mock@example.org", "notification_id": p.get("notification_id", "")}
    if op == "find_notification":
        return {"found": False}
    if op == "complete_messages":
        return {"items": [{"message_id": i.get("message_id"), "flagged": True, "moved": True, "new_entry_id": f"MOCK-{i.get('message_id')}",
                           "new_store_id": "MOCKSTORE", "new_folder_entry_id": "MOCK-DONE", "new_folder_path": "Inbox/Completed",
                           "error_code": None, "message": None} for i in p.get("items", [])]}
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
