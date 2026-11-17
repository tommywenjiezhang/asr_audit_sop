//! Application configuration stored as JSON (settings.json in the app data directory).

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderRef {
    pub store_id: String,
    pub entry_id: String,
    /// Display path, e.g. "Inbox/Wenjie". Used for presentation and recovery only.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub mailbox_name: String,
    pub mailbox_store_id: String,
    pub folder: Option<FolderRef>,
    /// "none" | "all" | "selected"
    pub subfolder_mode: String,
    pub selected_subfolders: Vec<FolderRef>,
    pub sync_enabled: bool,
    pub interval_minutes: u32,
    pub check_on_startup: bool,
    pub launch_at_login: bool,
    pub close_to_tray: bool,
    pub initial_lookback_days: u32,
    pub overlap_minutes: u32,
    pub full_rescan_hours: u32,
    pub max_messages_per_sync: u32,
    pub body_max_chars: u32,
    pub python_path: String,
    pub staff_name: String,
    pub default_assigned_staff: String,
    pub asr_deadline: String,
    /// Addresses/domains treated as internal staff (their emails never create tickets).
    pub internal_senders: Vec<String>,
    pub duplicate_window_days: u32,
    pub open_outlook_draft_after_create: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mailbox_name: "DOH-Schoolimmforms-team, Vpdp [DOH]".into(),
            mailbox_store_id: String::new(),
            folder: None,
            subfolder_mode: "none".into(),
            selected_subfolders: vec![],
            sync_enabled: false,
            interval_minutes: 5,
            check_on_startup: true,
            launch_at_login: false,
            close_to_tray: true,
            initial_lookback_days: 14,
            overlap_minutes: 30,
            full_rescan_hours: 24,
            max_messages_per_sync: 500,
            body_max_chars: 100_000,
            python_path: String::new(),
            staff_name: String::new(),
            default_assigned_staff: String::new(),
            asr_deadline: "Dec 1, 2026 at 11:59 PM".into(),
            internal_senders: vec![],
            duplicate_window_days: 14,
            open_outlook_draft_after_create: false,
        }
    }
}

pub const ALLOWED_INTERVALS: &[u32] = &[1, 5, 10, 15, 30];

impl Settings {
    pub fn validate(&self) -> AppResult<()> {
        if !ALLOWED_INTERVALS.contains(&self.interval_minutes) {
            return Err(AppError::coded("INVALID", "Interval must be 1, 5, 10, 15 or 30 minutes"));
        }
        if !["none", "all", "selected"].contains(&self.subfolder_mode.as_str()) {
            return Err(AppError::coded("INVALID", "Invalid subfolder mode"));
        }
        if self.initial_lookback_days == 0 || self.initial_lookback_days > 365 {
            return Err(AppError::coded("INVALID", "Lookback must be between 1 and 365 days"));
        }
        if self.overlap_minutes > 24 * 60 {
            return Err(AppError::coded("INVALID", "Overlap window too large"));
        }
        if !self.python_path.is_empty() && self.python_path.contains(['|', '&', ';', '<', '>', '\n']) {
            return Err(AppError::coded("INVALID", "Python path contains invalid characters"));
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "settings.json unreadable; using defaults");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        self.validate()?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn is_internal_sender(&self, email: &str) -> bool {
        let e = email.trim().to_lowercase();
        if e.is_empty() {
            return false;
        }
        self.internal_senders.iter().any(|s| {
            let s = s.trim().to_lowercase();
            !s.is_empty() && (e == s || (s.starts_with('@') && e.ends_with(&s)))
        })
    }

    /// Folders to scan, including subfolder mode.
    pub fn monitored(&self) -> Vec<(FolderRef, bool)> {
        let Some(f) = &self.folder else { return vec![] };
        match self.subfolder_mode.as_str() {
            "all" => vec![(f.clone(), true)],
            "selected" => {
                let mut v = vec![(f.clone(), false)];
                v.extend(self.selected_subfolders.iter().cloned().map(|s| (s, false)));
                v
            }
            _ => vec![(f.clone(), false)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        let mut s = Settings::default();
        assert_eq!(Settings::load(&p), s);
        s.interval_minutes = 10;
        s.internal_senders = vec!["@doh.nj.gov".into()];
        s.save(&p).unwrap();
        let l = Settings::load(&p);
        assert_eq!(l.interval_minutes, 10);
        assert!(l.is_internal_sender("someone@DOH.nj.gov"));
        assert!(!l.is_internal_sender("x@school.org"));
        s.interval_minutes = 7;
        assert!(s.save(&p).is_err());
    }
}
