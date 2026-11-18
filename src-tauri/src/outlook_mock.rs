//! File-backed stand-in for the Outlook worker, used in development test mode (`ASR_OUTLOOK=mock`).
//!
//! No COM call is made. The "mailbox" is a folder that several app instances can share:
//!   inbox.json     {"messages": [IncomingMessage-like objects]} — what sync_messages / scope_inventory see;
//!                  moves update `folder_entry_id` / `folder_path` / `entry_id` here, like a real move
//!   folders.json   optional folder tree ({"root": FolderNode}); a default tree with Inbox, Completed and
//!                  two staff folders is used otherwise
//!   faults.json    {"<op>": ["fail" | "uncertain" | "item_fail" | "timeout", ...]} — the first entry of an
//!                  op's list is consumed by the next call of that op (fault injection for recovery tests)
//!   journal.jsonl  one line per side-effecting request (moves, flags, sends, opened windows): the evidence
//!                  that a move or handoff happened exactly once
//! A lock file serializes access between processes.

use crate::error::{AppError, AppResult};
use crate::outlook::WorkerResponse;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const STORE: &str = "MOCKSTORE";

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lock(dir: &Path) -> AppResult<Lock> {
    std::fs::create_dir_all(dir)?;
    let p = dir.join(".lock");
    for i in 0..400 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => return Ok(Lock(p)),
            Err(_) => {
                // a stale lock (crashed process) is broken after ~20 s
                if i == 399 {
                    let _ = std::fs::remove_file(&p);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(AppError::coded("OUTLOOK_BUSY", "The mock mailbox is locked."))
}

fn read_json(p: &Path, default: Value) -> Value {
    std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(default)
}

fn write_json(p: &Path, v: &Value) -> AppResult<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(&tmp, p)?;
    Ok(())
}

fn folder(name: &str, id: &str, path: &str, children: Vec<Value>) -> Value {
    json!({"name": name, "entry_id": id, "store_id": STORE, "path": path, "children": children})
}

pub fn default_tree() -> Value {
    let kids = vec![
        folder("Completed", "MOCK-Completed", "Inbox/Completed", vec![]),
        folder("Test Staff A", "MOCK-StaffA", "Inbox/Test Staff A", vec![]),
        folder("Test Staff B", "MOCK-StaffB", "Inbox/Test Staff B", vec![]),
    ];
    json!({"store_id": STORE, "root": folder("Mock Shared Mailbox", "MOCK-Root", "", vec![folder("Inbox", "MOCK-Inbox", "Inbox", kids)])})
}

fn tree(dir: &Path) -> Value {
    read_json(&dir.join("folders.json"), default_tree())
}

fn find_folder<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
    if node.get("entry_id").and_then(|v| v.as_str()) == Some(id) {
        return Some(node);
    }
    node.get("children").and_then(|c| c.as_array()).and_then(|c| c.iter().find_map(|k| find_folder(k, id)))
}

/// Entry ids of the folder's parents (parent first).
fn ancestors(root: &Value, id: &str) -> Vec<String> {
    fn walk(node: &Value, id: &str, trail: &mut Vec<String>) -> bool {
        let me = node.get("entry_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if me == id {
            return true;
        }
        trail.push(me);
        for k in node.get("children").and_then(|c| c.as_array()).cloned().unwrap_or_default() {
            if walk(&k, id, trail) {
                return true;
            }
        }
        trail.pop();
        false
    }
    let mut t = vec![];
    if walk(root, id, &mut t) {
        t.reverse();
        t
    } else {
        vec![]
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn take_fault(dir: &Path, op: &str) -> Option<String> {
    let p = dir.join("faults.json");
    let mut f = read_json(&p, json!({}));
    let list = f.get_mut(op)?.as_array_mut()?;
    if list.is_empty() {
        return None;
    }
    let first = list.remove(0);
    let _ = write_json(&p, &f);
    first.as_str().map(String::from)
}

fn journal(dir: &Path, op: &str, detail: Value) {
    use std::io::Write;
    let who = crate::profile::active().map(|p| p.name.clone()).unwrap_or_default();
    let line = json!({"at": crate::db::now(), "profile": who, "op": op, "detail": detail});
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("journal.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
}

fn ok(data: Value) -> WorkerResponse {
    WorkerResponse { ok: true, data, error_code: None, message: None, diagnostics: json!({"mock": true}) }
}

fn err(code: &str, msg: &str, stage: &str) -> WorkerResponse {
    WorkerResponse { ok: false, data: Value::Null, error_code: Some(code.into()), message: Some(msg.into()), diagnostics: json!({"mock": true, "stage": stage}) }
}

/// Locate a message by entry id first, then by Internet Message-ID (like the real worker's find_item).
fn find_msg(msgs: &[Value], entry: &str, imid: &str) -> Option<usize> {
    msgs.iter().position(|m| !entry.is_empty() && s(m, "entry_id") == entry).or_else(|| {
        let n = crate::shared_apply::normalize_message_id(imid);
        if n.is_empty() {
            None
        } else {
            msgs.iter().position(|m| crate::shared_apply::normalize_message_id(s(m, "internet_message_id")) == n)
        }
    })
}

fn relocate(m: &mut Value, root: &Value, dest_id: &str) {
    let path = find_folder(root, dest_id).map(|f| s(f, "path").to_string()).unwrap_or_default();
    let old = s(m, "entry_id").to_string();
    // a move gives the item a new EntryID, like Exchange does
    let base = old.split("~m").next().unwrap_or(&old).to_string();
    let n = old.matches("~m").count() + 1;
    m["entry_id"] = json!(format!("{base}~m{n}"));
    m["folder_entry_id"] = json!(dest_id);
    m["folder_path"] = json!(path);
    m["folder_ancestors"] = json!(ancestors(root, dest_id));
    m["store_id"] = json!(STORE);
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// The message lies in an excluded folder or below one (like the real worker's walk, which skips excluded subtrees).
fn in_excluded(m: &Value, ex: &[String]) -> bool {
    let anc = m.get("folder_ancestors").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    ex.iter().any(|x| s(m, "folder_entry_id") == x || anc.iter().any(|a| a.as_str() == Some(x.as_str())))
}

/// Answer one worker operation from the mock mailbox in `dir`.
pub fn call(dir: &Path, op: &str, p: &Value) -> AppResult<WorkerResponse> {
    let _l = lock(dir)?;
    match take_fault(dir, op).as_deref() {
        Some("timeout") => return Err(AppError::coded("WORKER_TIMEOUT", "Outlook did not respond (injected fault).")),
        Some("fail") => {
            journal(dir, op, json!({"fault": "fail"}));
            let stage = if op == "send_notification" { "prepare" } else { "" };
            return Ok(err(if op == "send_notification" { "NOTIFY_PREPARE_FAILED" } else { "COM_ERROR" }, "Injected failure (mock Outlook).", stage));
        }
        Some("uncertain") => {
            journal(dir, op, json!({"fault": "uncertain", "notification_id": s(p, "notification_id"), "to": s(p, "to")}));
            return Ok(err("SEND_UNCERTAIN", "Injected uncertain send (mock Outlook).", "send"));
        }
        _ => {}
    }
    let root = tree(dir);
    let inbox_path = dir.join("inbox.json");
    let mut inbox = read_json(&inbox_path, json!({"messages": []}));
    let msgs: Vec<Value> = inbox.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let r = match op {
        "test_connection" => ok(json!({"outlook_version": "mock", "profile": "Mock (test mode)", "default_store": "mock@example.invalid", "store_count": 1})),
        "list_mailboxes" => ok(json!({"mailboxes": [{"display_name": "Mock Shared Mailbox", "store_id": STORE, "exchange_store_type": 1, "is_data_file": false}]})),
        "list_accounts" => ok(json!({"accounts": [{"display_name": "Mock", "smtp": "mock@example.invalid"}]})),
        "list_folders" => {
            let rid = s(p, "root_entry_id");
            if rid.is_empty() {
                ok(root.clone())
            } else {
                match find_folder(&root["root"], rid) {
                    Some(f) => ok(json!({"store_id": STORE, "mailbox": "Mock Shared Mailbox", "root": f})),
                    None => err("FOLDER_NOT_FOUND", "The monitored folder was not found (mock).", ""),
                }
            }
        }
        "validate_folder" => match find_folder(&root["root"], s(p, "entry_id")) {
            Some(f) => ok(json!({"entry_id": s(f, "entry_id"), "store_id": STORE, "path": s(f, "path"), "name": s(f, "name"), "mailbox": "Mock Shared Mailbox"})),
            None => err("FOLDER_NOT_FOUND", "The folder was not found (mock).", ""),
        },
        "sync_messages" => {
            let mut out = vec![];
            for req in p.get("folders").and_then(|f| f.as_array()).cloned().unwrap_or_default() {
                let fid = s(&req, "entry_id").to_string();
                let subs = req.get("include_subfolders").and_then(|v| v.as_bool()).unwrap_or(false);
                let since = s(&req, "since_utc").to_string();
                let ex = strings(&req, "exclude_entry_ids");
                let mut list: Vec<Value> = msgs
                    .iter()
                    .filter(|m| s(m, "folder_entry_id") == fid || (subs && m.get("folder_ancestors").and_then(|a| a.as_array()).map(|a| a.iter().any(|x| x.as_str() == Some(fid.as_str()))).unwrap_or(false)))
                    .filter(|m| !in_excluded(m, &ex))
                    .filter(|m| since.is_empty() || s(m, "received_at") >= since.as_str())
                    .cloned()
                    .collect();
                list.sort_by(|a, b| s(a, "received_at").cmp(s(b, "received_at")));
                out.push(json!({"key": req.get("key"), "ok": true, "recovered": false, "truncated": false, "messages": list}));
            }
            ok(json!({"folders": out, "stats": {"mock": true}}))
        }
        "get_message" => match find_msg(&msgs, s(p, "entry_id"), s(p, "imid")) {
            Some(i) => ok(json!({"body": s(&msgs[i], "body"), "html_body": ""})),
            None => err("ITEM_NOT_FOUND", "The email was not found (mock).", ""),
        },
        "scope_inventory" => {
            let mut items = vec![];
            let mut folders = vec![];
            let ex = strings(p, "exclude");
            for req in p.get("folders").and_then(|f| f.as_array()).cloned().unwrap_or_default() {
                let fid = s(&req, "entry_id").to_string();
                folders.push(json!({"entry_id": fid, "store_id": STORE, "path": s(&req, "path"), "ancestors": ancestors(&root["root"], &fid)}));
                let subs = req.get("include_subfolders").and_then(|v| v.as_bool()).unwrap_or(false);
                for m in &msgs {
                    let inside = s(m, "folder_entry_id") == fid || (subs && m.get("folder_ancestors").and_then(|a| a.as_array()).map(|a| a.iter().any(|x| x.as_str() == Some(fid.as_str()))).unwrap_or(false));
                    if inside && !in_excluded(m, &ex) {
                        items.push(json!({"imid": s(m, "internet_message_id"), "entry_id": s(m, "entry_id"), "folder_entry_id": s(m, "folder_entry_id")}));
                    }
                }
            }
            ok(json!({"scope_key": p.get("scope_key").cloned().unwrap_or(json!("")), "complete": true, "folders": folders, "items": items, "errors": []}))
        }
        "locate_messages" => {
            let items: Vec<Value> = p
                .get("items")
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|it| match find_msg(&msgs, s(it, "entry_id"), s(it, "imid")) {
                    Some(i) => {
                        let m = &msgs[i];
                        json!({"message_id": it.get("message_id"), "status": "found", "entry_id": s(m, "entry_id"), "store_id": STORE, "folder_entry_id": s(m, "folder_entry_id"), "folder_path": s(m, "folder_path"), "ancestors": m.get("folder_ancestors").cloned().unwrap_or(json!([])), "error": null})
                    }
                    None => json!({"message_id": it.get("message_id"), "status": "not_found", "entry_id": "", "store_id": "", "folder_entry_id": "", "folder_path": "", "ancestors": [], "error": null}),
                })
                .collect();
            ok(json!({"items": items}))
        }
        "move_messages" | "complete_messages" => {
            let dest = p.get("dest").cloned().unwrap_or(json!({}));
            let dest_id = s(&dest, "entry_id").to_string();
            if find_folder(&root["root"], &dest_id).is_none() {
                journal(dir, op, json!({"error": "FOLDER_NOT_FOUND", "dest": dest_id}));
                return Ok(err("FOLDER_NOT_FOUND", "The destination folder was not found (mock).", ""));
            }
            let item_fault = take_fault(dir, &format!("{op}.item"));
            let mut msgs = msgs.clone();
            let mut results = vec![];
            for (n, it) in p.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default().iter().enumerate() {
                let mid = it.get("message_id").cloned().unwrap_or(Value::Null);
                if n == 0 && item_fault.as_deref() == Some("item_fail") {
                    results.push(json!({"message_id": mid, "moved": false, "flagged": false, "error_code": "MOVE_FAILED", "message": "Injected item failure (mock)."}));
                    continue;
                }
                match find_msg(&msgs, s(it, "entry_id"), s(it, "imid")) {
                    None => results.push(json!({"message_id": mid, "moved": false, "flagged": false, "error_code": "ITEM_NOT_FOUND", "message": "The email was not found (mock)."})),
                    Some(i) => {
                        let flag = op == "complete_messages" && it.get("need_flag").and_then(|v| v.as_bool()).unwrap_or(true);
                        if flag {
                            msgs[i]["flag"] = json!("complete");
                        }
                        let need_move = op == "move_messages" || it.get("need_move").and_then(|v| v.as_bool()).unwrap_or(true);
                        if need_move && s(&msgs[i], "folder_entry_id") != dest_id {
                            relocate(&mut msgs[i], &root["root"], &dest_id);
                        }
                        let m = &msgs[i];
                        results.push(json!({"message_id": mid, "moved": true, "flagged": flag || op == "complete_messages", "new_entry_id": s(m, "entry_id"), "new_store_id": STORE,
                            "new_folder_entry_id": s(m, "folder_entry_id"), "new_folder_path": s(m, "folder_path"), "ancestors": m.get("folder_ancestors").cloned().unwrap_or(json!([])), "error_code": null, "message": null}));
                    }
                }
            }
            inbox["messages"] = Value::Array(msgs);
            write_json(&inbox_path, &inbox)?;
            journal(dir, op, json!({"dest": dest_id, "items": results.iter().map(|r| json!({"message_id": r["message_id"], "moved": r["moved"], "error": r["error_code"]})).collect::<Vec<_>>()}));
            ok(json!({"items": results}))
        }
        "send_notification" => {
            let to = s(p, "to");
            if to.is_empty() || !to.contains('@') {
                return Ok(err("BAD_REQUEST", "Exactly one staff email address is required.", "prepare"));
            }
            journal(dir, op, json!({"to": to, "subject": s(p, "subject"), "notification_id": s(p, "notification_id")}));
            ok(json!({"submitted": true, "draft_entry_id": format!("MOCK-N-{}", s(p, "notification_id")), "account": p.get("account").cloned().unwrap_or(json!("mock@example.invalid")), "notification_id": s(p, "notification_id")}))
        }
        "find_notification" => {
            let nid = s(p, "notification_id").to_string();
            let sent = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).any(|l| {
                s(&l, "op") == "send_notification" && l["detail"]["notification_id"].as_str() == Some(nid.as_str()) && l["detail"].get("fault").is_none()
            });
            ok(if sent { json!({"found": true, "where": "Sent Items", "entry_id": format!("MOCK-N-{nid}")}) } else { json!({"found": false}) })
        }
        "open_reply" | "open_new_message" | "create_reply_draft" | "open_message" => {
            journal(dir, op, json!({"entry_id": s(p, "entry_id"), "reply_all": p.get("reply_all")}));
            ok(json!({"opened": true, "reply_all": p.get("reply_all").and_then(|v| v.as_bool()).unwrap_or(false), "relocated": false, "source": {}, "entry_id": "MOCK-DRAFT", "mode": "reply", "sent": false}))
        }
        other => err("BAD_REQUEST", &format!("Unknown operation '{other}'."), ""),
    };
    Ok(r)
}

/// Add an incoming message to the mock mailbox (used by tests and the dual-instance driver through files).
pub fn deliver(dir: &Path, msg: Value) -> AppResult<()> {
    let _l = lock(dir)?;
    let p = dir.join("inbox.json");
    let mut inbox = read_json(&p, json!({"messages": []}));
    let mut list = inbox.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    list.push(msg);
    inbox["messages"] = Value::Array(list);
    write_json(&p, &inbox)
}

pub fn journal_lines(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("journal.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(entry: &str, imid: &str, folder: &str) -> Value {
        json!({"store_id": STORE, "entry_id": entry, "internet_message_id": imid, "conversation_id": "C1", "folder_entry_id": folder, "folder_path": "Inbox",
               "subject": "Test", "sender_name": "School", "sender_email": "school@example.invalid", "received_at": "2026-11-18T10:00:00Z", "body": "hello",
               "body_truncated": false, "has_attachments": false, "folder_ancestors": ["MOCK-Root"], "notification_id": "", "metadata": {}})
    }

    #[test]
    fn move_relocates_and_journals_and_faults_are_consumed_once() {
        let d = tempfile::tempdir().unwrap();
        deliver(d.path(), msg("E1", "<a@x>", "MOCK-Inbox")).unwrap();
        let r = call(d.path(), "sync_messages", &json!({"folders": [{"key": "k", "entry_id": "MOCK-Inbox", "since_utc": "2026-01-01T00:00:00Z"}]})).unwrap();
        assert_eq!(r.data["folders"][0]["messages"].as_array().unwrap().len(), 1);
        std::fs::write(d.path().join("faults.json"), r#"{"move_messages": ["fail"]}"#).unwrap();
        let req = json!({"dest": {"entry_id": "MOCK-StaffA", "store_id": STORE}, "items": [{"message_id": 7, "entry_id": "E1", "imid": "<a@x>"}]});
        assert!(!call(d.path(), "move_messages", &req).unwrap().ok, "injected fault");
        let r = call(d.path(), "move_messages", &req).unwrap();
        assert!(r.ok && r.data["items"][0]["moved"] == json!(true));
        assert_eq!(r.data["items"][0]["new_folder_path"], json!("Inbox/Test Staff A"));
        // the moved email is no longer in the Inbox, and is found again by Message-ID with its new EntryID
        let r = call(d.path(), "sync_messages", &json!({"folders": [{"key": "k", "entry_id": "MOCK-Inbox", "since_utc": ""}]})).unwrap();
        assert!(r.data["folders"][0]["messages"].as_array().unwrap().is_empty());
        let r = call(d.path(), "locate_messages", &json!({"items": [{"message_id": 7, "entry_id": "E1", "imid": "a@x"}]})).unwrap();
        assert_eq!(r.data["items"][0]["folder_entry_id"], json!("MOCK-StaffA"));
        assert_eq!(journal_lines(d.path()).iter().filter(|l| l["op"] == "move_messages").count(), 2);
    }

    #[test]
    fn uncertain_send_is_not_found_and_a_real_send_is() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("faults.json"), r#"{"send_notification": ["uncertain"]}"#).unwrap();
        let req = json!({"to": "a@example.invalid", "subject": "s", "notification_id": "N1"});
        let r = call(d.path(), "send_notification", &req).unwrap();
        assert_eq!(r.error_code.as_deref(), Some("SEND_UNCERTAIN"));
        assert_eq!(call(d.path(), "find_notification", &json!({"notification_id": "N1"})).unwrap().data["found"], json!(false));
        assert!(call(d.path(), "send_notification", &req).unwrap().ok);
        assert_eq!(call(d.path(), "find_notification", &json!({"notification_id": "N1"})).unwrap().data["found"], json!(true));
    }
}
