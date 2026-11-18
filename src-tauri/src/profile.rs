//! Application profile: where this instance keeps its data, and whether it runs in development test mode.
//!
//! Normal use: Tauri's per-user app data folder (`%APPDATA%\gov.nj.doh.asrsupportdesk`, resolved through the
//! Windows Known-Folder API, so changing the APPDATA variable does NOT move it).
//!
//! Development / multi-instance testing (environment variables, read once at startup):
//!   ASR_APP_DATA_DIR      absolute folder for this instance: local SQLite, settings.json, logs, backups,
//!                         WebView2 data, Outlook mock files. The app never falls back to the normal profile
//!                         when this is set: if the folder cannot be created, startup fails with a message.
//!   ASR_TEST_MODE=1       development test mode: allows several instances at once (no single-instance
//!                         hand-off, no autostart registration), shows the profile name in the window title
//!                         and replaces Outlook with the file-backed mock below unless overridden.
//!   ASR_OUTLOOK=mock|live Outlook side effects. Default: mock in test mode, live otherwise. In mock mode no
//!                         Outlook COM call is made at all (nothing is moved, flagged or sent); every
//!                         side-effecting request is written to `outlook-mock\journal.jsonl`.
//!   ASR_PROFILE_NAME      label for the window title / diagnostics (default: the data folder name).
//!   ASR_TEST_STAFF        test mode only: staff display name to create (if missing) and select as "I am".
//!   ASR_DEVTOOLS_PORT     test mode only: WebView2 remote-debugging port, used by the dual-instance UI driver.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutlookMode {
    Live,
    /// file-backed mock rooted at this folder (inbox.json, faults.json, journal.jsonl)
    Mock(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Profile {
    /// explicit data folder (None = Tauri's default app data folder)
    pub data_dir: Option<PathBuf>,
    pub test_mode: bool,
    pub name: String,
    pub outlook: OutlookMode,
    pub test_staff: Option<String>,
    pub devtools_port: Option<u16>,
}

impl Profile {
    /// Allow more than one running instance (the single-instance plugin is skipped).
    pub fn multi_instance(&self) -> bool {
        self.test_mode || self.data_dir.is_some()
    }
}

fn truthy(v: Option<String>) -> bool {
    matches!(v.as_deref().map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("1") | Some("true") | Some("yes"))
}

/// Build the profile from an environment lookup (injectable for tests). `default_data_dir` is only used to
/// root the Outlook mock when no explicit data folder is given.
pub fn from_lookup(get: impl Fn(&str) -> Option<String>, default_data_dir: Option<&Path>) -> Result<Profile, String> {
    let nonempty = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let data_dir = match nonempty("ASR_APP_DATA_DIR") {
        None => None,
        Some(d) => {
            let p = PathBuf::from(&d);
            if !p.is_absolute() {
                return Err(format!("ASR_APP_DATA_DIR must be an absolute folder path (got \"{d}\")."));
            }
            std::fs::create_dir_all(&p).map_err(|e| format!("ASR_APP_DATA_DIR \"{d}\" could not be created: {e}. The app does not fall back to the normal profile."))?;
            if !p.is_dir() {
                return Err(format!("ASR_APP_DATA_DIR \"{d}\" is not a folder."));
            }
            Some(p)
        }
    };
    let test_mode = truthy(get("ASR_TEST_MODE"));
    let root = data_dir.clone().or_else(|| default_data_dir.map(Path::to_path_buf));
    // ASR_OUTLOOK_MOCK_DIR lets several instances share ONE mock mailbox (like staff sharing one Outlook
    // mailbox); by default each instance has its own under its data folder
    let mock_root = nonempty("ASR_OUTLOOK_MOCK_DIR").map(PathBuf::from).unwrap_or_else(|| root.clone().unwrap_or_default().join("outlook-mock"));
    let outlook = match nonempty("ASR_OUTLOOK").map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("live") => OutlookMode::Live,
        Some("mock") => OutlookMode::Mock(mock_root),
        Some(other) => return Err(format!("ASR_OUTLOOK must be \"mock\" or \"live\" (got \"{other}\").")),
        None if test_mode => OutlookMode::Mock(mock_root),
        None => OutlookMode::Live,
    };
    let name = nonempty("ASR_PROFILE_NAME")
        .or_else(|| data_dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();
    let devtools_port = if test_mode { nonempty("ASR_DEVTOOLS_PORT").and_then(|p| p.parse().ok()) } else { None };
    let test_staff = if test_mode { nonempty("ASR_TEST_STAFF") } else { None };
    Ok(Profile { data_dir, test_mode, name, outlook, test_staff, devtools_port })
}

pub fn from_env(default_data_dir: Option<&Path>) -> Result<Profile, String> {
    from_lookup(|k| std::env::var(k).ok(), default_data_dir)
}

static ACTIVE: OnceLock<Profile> = OnceLock::new();

/// Install the profile of this process (first call wins).
pub fn install(p: Profile) -> &'static Profile {
    let _ = ACTIVE.set(p);
    ACTIVE.get().unwrap()
}

pub fn active() -> Option<&'static Profile> {
    ACTIVE.get()
}

/// The Outlook mode of this process (Live when no profile was installed, e.g. in unit tests).
pub fn outlook_mode() -> OutlookMode {
    active().map(|p| p.outlook.clone()).unwrap_or(OutlookMode::Live)
}

pub fn test_mode() -> bool {
    active().map(|p| p.test_mode).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn default_profile_is_live_and_single_instance() {
        let p = from_lookup(env(&[]), None).unwrap();
        assert!(p.data_dir.is_none());
        assert!(!p.test_mode);
        assert!(!p.multi_instance());
        assert_eq!(p.outlook, OutlookMode::Live);
        assert!(p.devtools_port.is_none());
    }

    #[test]
    fn explicit_data_dir_is_created_and_never_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("WorkstationA");
        let p = from_lookup(env(&[("ASR_APP_DATA_DIR", ws.to_str().unwrap()), ("ASR_TEST_MODE", "1"), ("ASR_TEST_STAFF", "Test A"), ("ASR_DEVTOOLS_PORT", "9301")]), None).unwrap();
        assert!(ws.is_dir());
        assert_eq!(p.data_dir.as_deref(), Some(ws.as_path()));
        assert_eq!(p.name, "WorkstationA");
        assert!(p.multi_instance());
        assert_eq!(p.outlook, OutlookMode::Mock(ws.join("outlook-mock")), "test mode mocks Outlook by default");
        assert_eq!(p.test_staff.as_deref(), Some("Test A"));
        assert_eq!(p.devtools_port, Some(9301));
        // relative paths are refused instead of silently resolving somewhere else
        assert!(from_lookup(env(&[("ASR_APP_DATA_DIR", "relative\\dir")]), None).is_err());
        // a path that cannot be a folder is an error, not a fallback
        let file = dir.path().join("afile");
        std::fs::write(&file, b"x").unwrap();
        assert!(from_lookup(env(&[("ASR_APP_DATA_DIR", file.to_str().unwrap())]), None).is_err());
    }

    #[test]
    fn test_only_switches_are_ignored_outside_test_mode() {
        let p = from_lookup(env(&[("ASR_DEVTOOLS_PORT", "9301"), ("ASR_TEST_STAFF", "X")]), None).unwrap();
        assert!(p.devtools_port.is_none() && p.test_staff.is_none());
        let p = from_lookup(env(&[("ASR_TEST_MODE", "1"), ("ASR_OUTLOOK", "live")]), None).unwrap();
        assert_eq!(p.outlook, OutlookMode::Live, "live Outlook only when asked for explicitly");
        assert!(from_lookup(env(&[("ASR_OUTLOOK", "maybe")]), None).is_err());
    }
}
