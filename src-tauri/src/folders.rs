//! Outlook folder discovery: request building, single-flight guard and diagnostics for `list_folders`.
//!
//! The worker reads child folders from both the Outlook object model and Extended MAPI hierarchy tables
//! (some shared/delegate mailboxes report an empty `Folders` collection for a folder that has
//! subfolders). This module turns its `discovery` block into one diagnostics event per discovery, so
//! staff can copy the log for the developer. Folder names are not mail content; no item data is involved.

use crate::diag::Severity;
use serde_json::Value;
use std::sync::Mutex;

/// Default depth of a discovery. Deep enough for any real mailbox; the worker marks nodes whose
/// children were not loaded (`children_loaded: false`) instead of claiming they have none.
pub const DEFAULT_MAX_DEPTH: u32 = 12;

/// What one `list_folders` request asked for (for messages and the single-flight key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRequest {
    pub store_id: String,
    pub shared_mailbox: String,
    pub root_entry_id: String,
}

impl DiscoveryRequest {
    pub fn params(&self) -> Value {
        serde_json::json!({"store_id": self.store_id, "shared_mailbox": self.shared_mailbox,
            "root_entry_id": self.root_entry_id, "max_depth": DEFAULT_MAX_DEPTH})
    }
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.store_id, self.shared_mailbox.to_lowercase(), self.root_entry_id)
    }
    pub fn describe(&self) -> String {
        if !self.root_entry_id.is_empty() {
            "the monitored folder's subfolders".into()
        } else if !self.shared_mailbox.is_empty() {
            format!("mailbox '{}'", self.shared_mailbox)
        } else if !self.store_id.is_empty() {
            "the selected mailbox".into()
        } else {
            "the default mailbox".into()
        }
    }
}

/// Rejects a second identical discovery while one is running (the first one's result is used).
#[derive(Default)]
pub struct DiscoveryGuard {
    running: Mutex<Vec<String>>,
}

pub struct DiscoveryTicket<'a> {
    guard: &'a DiscoveryGuard,
    key: String,
}

impl DiscoveryGuard {
    pub fn try_start(&self, key: &str) -> Option<DiscoveryTicket<'_>> {
        let mut r = self.running.lock().unwrap();
        if r.iter().any(|k| k == key) {
            return None;
        }
        r.push(key.to_string());
        Some(DiscoveryTicket { guard: self, key: key.to_string() })
    }
}

impl Drop for DiscoveryTicket<'_> {
    fn drop(&mut self) {
        self.guard.running.lock().unwrap().retain(|k| k != &self.key);
    }
}

/// One diagnostics event for a finished discovery: (severity, code, summary, detail).
pub fn discovery_event(req: &DiscoveryRequest, data: &Value) -> (Severity, &'static str, String, String) {
    let d = data.get("discovery").cloned().unwrap_or(Value::Null);
    let n = |k: &str| d.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let mailbox = data.get("mailbox").and_then(|v| v.as_str()).unwrap_or("");
    let store = data.get("store_id").and_then(|v| v.as_str()).unwrap_or("");
    let root = data.get("root").cloned().unwrap_or(Value::Null);
    let root_path = root.get("path").and_then(|v| v.as_str()).filter(|p| !p.is_empty()).unwrap_or("(mailbox root)");
    let source = d.get("hierarchy_source").and_then(|v| v.as_str()).unwrap_or("outlook");
    let repaired: Vec<String> = d.get("repaired").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let problems = n("problems");
    let summary = format!(
        "Folder discovery for {}: mailbox '{}' (store …{}), '{}' has {} immediate child folder(s), {} folder(s) in total, {} with problems, {} ms, source {}{}",
        req.describe(),
        mailbox,
        tail(store, 12),
        root_path,
        n("immediate_children"),
        n("descendants"),
        problems,
        n("elapsed_ms"),
        source,
        if repaired.is_empty() { String::new() } else { format!("; subfolders recovered through MAPI under {}", repaired.join(", ")) }
    );
    let mut detail: Vec<String> = Vec::new();
    if let Some(e) = d.get("mapi_error").and_then(|v| v.as_str()) {
        detail.push(format!("Extended MAPI unavailable: {e}"));
    }
    if let Some(errs) = d.get("errors").and_then(|v| v.as_array()) {
        detail.extend(errs.iter().filter_map(|x| x.as_str()).map(|x| format!("[ERROR] {x}")));
    }
    if let Some(log) = d.get("log").and_then(|v| v.as_array()) {
        detail.extend(log.iter().filter_map(|x| x.as_str()).map(String::from));
    }
    let (sev, code) = if problems > 0 || d.get("errors").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty()) {
        (Severity::Warning, "FOLDERS_PARTIAL")
    } else if !repaired.is_empty() {
        (Severity::Info, "FOLDERS_RECOVERED")
    } else {
        (Severity::Info, "FOLDERS_OK")
    };
    (sev, code, summary, detail.join("\n"))
}

/// Diagnostics event for a failed discovery.
pub fn discovery_failure(req: &DiscoveryRequest, code: &str, message: &str) -> (Severity, String) {
    (Severity::Error, format!("Folder discovery for {} failed ({code}): {message}", req.describe()))
}

fn tail(s: &str, n: usize) -> &str {
    let start = s.len().saturating_sub(n);
    s.get(start..).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> DiscoveryRequest {
        DiscoveryRequest { store_id: "00ABCDEF0123456789".into(), shared_mailbox: String::new(), root_entry_id: String::new() }
    }

    #[test]
    fn guard_rejects_duplicate_concurrent_discovery() {
        let g = DiscoveryGuard::default();
        let t = g.try_start("a").expect("first");
        assert!(g.try_start("a").is_none(), "same request while running");
        assert!(g.try_start("b").is_some(), "a different mailbox may load meanwhile");
        drop(t);
        assert!(g.try_start("a").is_some(), "released when finished");
    }

    #[test]
    fn event_summarizes_counts_recovery_and_log() {
        let data = serde_json::json!({"store_id": "00ABCDEF0123456789", "mailbox": "Team Mailbox",
            "root": {"path": "", "name": "Team Mailbox"},
            "discovery": {"hierarchy_source": "outlook+mapi", "immediate_children": 17, "descendants": 62, "problems": 0,
                "repaired": ["Inbox"], "errors": [], "elapsed_ms": 2100, "log": ["[INFO] 'Inbox': 6 child folder(s)"]}});
        let (sev, code, summary, detail) = discovery_event(&req(), &data);
        assert_eq!(sev, Severity::Info);
        assert_eq!(code, "FOLDERS_RECOVERED");
        assert!(summary.contains("17 immediate child folder(s)") && summary.contains("62 folder(s) in total"), "{summary}");
        assert!(summary.contains("recovered through MAPI under Inbox"), "{summary}");
        assert!(summary.contains("store …EF0123456789"), "{summary}");
        assert!(detail.contains("'Inbox': 6 child folder(s)"));
        let partial = serde_json::json!({"discovery": {"problems": 1, "errors": ["'Inbox/Private': com_error 0x80070005"]}});
        let (sev, code, _, detail) = discovery_event(&req(), &partial);
        assert_eq!((sev, code), (Severity::Warning, "FOLDERS_PARTIAL"));
        assert!(detail.contains("0x80070005"));
    }

    #[test]
    fn request_params_and_description() {
        let r = DiscoveryRequest { store_id: String::new(), shared_mailbox: "Team".into(), root_entry_id: String::new() };
        assert_eq!(r.params()["max_depth"], DEFAULT_MAX_DEPTH);
        assert_eq!(r.describe(), "mailbox 'Team'");
        assert_ne!(r.key(), req().key());
        let (sev, msg) = discovery_failure(&r, "ACCESS_DENIED", "no access");
        assert_eq!(sev, Severity::Error);
        assert!(msg.contains("ACCESS_DENIED"));
    }
}
