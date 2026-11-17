//! Staff members, their notification address and their Outlook folder (Settings › Staff Assignment).
//!
//! A staff folder is identified by StoreID + folder EntryID; the path is for display and recovery.
//! The folder mapping also applies to the folder's descendants unless a more specific staff folder exists.
//! Deactivated staff keep their history but cannot receive new assignments or be inferred as owners.

use crate::db::{get_setting, now, set_setting};
use crate::error::{AppError, AppResult};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._%+'\-]+@[A-Za-z0-9\-]+(\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}$").unwrap());

pub fn valid_email(e: &str) -> bool {
    let e = e.trim();
    e.len() <= 254 && EMAIL_RE.is_match(e)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Staff {
    pub id: i64,
    pub display_name: String,
    pub email: String,
    pub mailbox_name: Option<String>,
    pub store_id: String,
    pub folder_entry_id: String,
    pub folder_path: String,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StaffInput {
    pub id: Option<i64>,
    pub display_name: String,
    pub email: String,
    pub mailbox_name: String,
    pub store_id: String,
    pub folder_entry_id: String,
    pub folder_path: String,
}

const COLS: &str = "id,display_name,email,mailbox_name,store_id,folder_entry_id,folder_path,active,created_at,updated_at";

fn map(r: &rusqlite::Row) -> rusqlite::Result<Staff> {
    Ok(Staff {
        id: r.get(0)?,
        display_name: r.get(1)?,
        email: r.get(2)?,
        mailbox_name: r.get(3)?,
        store_id: r.get(4)?,
        folder_entry_id: r.get(5)?,
        folder_path: r.get(6)?,
        active: r.get::<_, i64>(7)? != 0,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

pub fn list(conn: &Connection, include_inactive: bool) -> AppResult<Vec<Staff>> {
    let sql = format!("SELECT {COLS} FROM staff {} ORDER BY active DESC, display_name COLLATE NOCASE", if include_inactive { "" } else { "WHERE active=1" });
    let mut st = conn.prepare(&sql)?;
    let v = st.query_map([], map)?.collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Staff> {
    conn.query_row(&format!("SELECT {COLS} FROM staff WHERE id=?1"), [id], map)
        .optional()?
        .ok_or_else(|| AppError::coded("NOT_FOUND", "Staff member not found."))
}

pub fn name_of(conn: &Connection, id: Option<i64>) -> AppResult<Option<String>> {
    match id {
        None => Ok(None),
        Some(id) => Ok(conn.query_row("SELECT display_name FROM staff WHERE id=?1", [id], |r| r.get(0)).optional()?),
    }
}

/// Create or update a staff member. Name, a valid email address and an Outlook folder are required.
pub fn save(conn: &Connection, s: &StaffInput) -> AppResult<Staff> {
    let name = s.display_name.trim();
    let email = s.email.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(AppError::coded("INVALID", "Enter the staff member's display name (up to 100 characters)."));
    }
    if !valid_email(email) {
        return Err(AppError::coded("INVALID", format!("\"{email}\" is not a valid email address.")));
    }
    if s.store_id.trim().is_empty() || s.folder_entry_id.trim().is_empty() || s.folder_path.trim().is_empty() {
        return Err(AppError::coded("INVALID", "Choose the staff member's Outlook folder."));
    }
    let me = s.id.unwrap_or(0);
    let dup_folder: Option<String> = conn
        .query_row(
            "SELECT display_name FROM staff WHERE active=1 AND id<>?1 AND folder_entry_id=?2 AND store_id=?3",
            params![me, s.folder_entry_id, s.store_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(other) = dup_folder {
        return Err(AppError::coded("INVALID", format!("{} already uses this folder. Each active staff member needs a separate folder.", other)));
    }
    let ts = now();
    let id = match s.id {
        Some(id) => {
            get(conn, id)?;
            conn.execute(
                "UPDATE staff SET display_name=?2, email=?3, mailbox_name=NULLIF(?4,''), store_id=?5, folder_entry_id=?6, folder_path=?7, updated_at=?8 WHERE id=?1",
                params![id, name, email, s.mailbox_name.trim(), s.store_id, s.folder_entry_id, s.folder_path, ts],
            )?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO staff(display_name,email,mailbox_name,store_id,folder_entry_id,folder_path,active,created_at,updated_at) VALUES(?1,?2,NULLIF(?3,''),?4,?5,?6,1,?7,?7)",
                params![name, email, s.mailbox_name.trim(), s.store_id, s.folder_entry_id, s.folder_path, ts],
            )?;
            conn.last_insert_rowid()
        }
    };
    get(conn, id)
}

/// Deactivate (or reactivate). Tickets and history keep pointing at the staff record.
pub fn set_active(conn: &Connection, id: i64, active: bool) -> AppResult<Staff> {
    let st = get(conn, id)?;
    if active {
        let dup: Option<String> = conn
            .query_row("SELECT display_name FROM staff WHERE active=1 AND id<>?1 AND folder_entry_id=?2 AND store_id=?3", params![id, st.folder_entry_id, st.store_id], |r| r.get(0))
            .optional()?;
        if let Some(other) = dup {
            return Err(AppError::coded("INVALID", format!("{other} now uses this folder; change one of the folders first.")));
        }
    }
    conn.execute("UPDATE staff SET active=?2, updated_at=?3 WHERE id=?1", params![id, active as i64, now()])?;
    get(conn, id)
}

/// Staff member who may receive a new assignment.
pub fn assignable(conn: &Connection, id: i64) -> AppResult<Staff> {
    let st = get(conn, id)?;
    if !st.active {
        return Err(AppError::coded("INVALID", format!("{} is deactivated and cannot receive new assignments.", st.display_name)));
    }
    if !valid_email(&st.email) {
        return Err(AppError::coded("INVALID", format!("{} has no valid email address. Fix it in Settings › Staff Assignment.", st.display_name)));
    }
    if st.folder_entry_id.is_empty() || st.store_id.is_empty() {
        return Err(AppError::coded("INVALID", format!("{} has no Outlook folder. Set it in Settings › Staff Assignment.", st.display_name)));
    }
    Ok(st)
}

/// Owner implied by a message's folder: the most specific active staff folder among the folder and its
/// ancestors (`chain` = [folder, parent, grandparent, ...] EntryIDs).
pub fn owner_for(staff: &[Staff], chain: &[String]) -> Option<i64> {
    for id in chain {
        if let Some(s) = staff.iter().find(|s| s.active && &s.folder_entry_id == id) {
            return Some(s.id);
        }
    }
    None
}

pub fn current_staff_id(conn: &Connection) -> AppResult<Option<i64>> {
    Ok(get_setting(conn, "current_staff_id")?.and_then(|v| v.parse().ok()))
}

pub fn set_current_staff_id(conn: &Connection, id: Option<i64>) -> AppResult<()> {
    if let Some(id) = id {
        get(conn, id)?;
    }
    set_setting(conn, "current_staff_id", &id.map(|i| i.to_string()).unwrap_or_default())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::db::open_in_memory;

    pub fn input(name: &str, email: &str, folder: &str) -> StaffInput {
        StaffInput {
            id: None,
            display_name: name.into(),
            email: email.into(),
            mailbox_name: "Team".into(),
            store_id: "S1".into(),
            folder_entry_id: folder.into(),
            folder_path: format!("Inbox/{name}"),
        }
    }

    #[test]
    fn email_validation() {
        for ok in ["wenjie@example.org", "a.b+c@sub.example.gov", "o'neil@x.us"] {
            assert!(valid_email(ok), "{ok}");
        }
        for bad in ["", "wenjie", "a@b", "a b@x.org", "a@@x.org", "@x.org", "a@x.c"] {
            assert!(!valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn create_edit_deactivate_and_folder_rules() {
        let c = open_in_memory().unwrap();
        let w = save(&c, &input("Wenjie", "wenjie@example.org", "F-WEN")).unwrap();
        let a = save(&c, &input("Aaron", "aaron@example.org", "F-AAR")).unwrap();
        assert!(save(&c, &input("Bad", "not-an-email", "F-X")).is_err());
        assert!(save(&c, &StaffInput { folder_entry_id: "".into(), ..input("NoFolder", "n@example.org", "") }).is_err());
        assert!(save(&c, &input("Copy", "copy@example.org", "F-WEN")).is_err(), "folders are separate per active staff member");
        let mut edit = input("Wenjie Z.", "wz@example.org", "F-WEN");
        edit.id = Some(w.id);
        let w2 = save(&c, &edit).unwrap();
        assert_eq!((w2.display_name.as_str(), w2.email.as_str()), ("Wenjie Z.", "wz@example.org"));
        set_active(&c, a.id, false).unwrap();
        assert!(assignable(&c, a.id).is_err(), "no new assignments to inactive staff");
        assert_eq!(list(&c, false).unwrap().len(), 1);
        assert_eq!(list(&c, true).unwrap().len(), 2, "inactive staff are kept");
        assert_eq!(get(&c, a.id).unwrap().display_name, "Aaron");
        set_current_staff_id(&c, Some(w.id)).unwrap();
        assert_eq!(current_staff_id(&c).unwrap(), Some(w.id));
    }

    #[test]
    fn owner_is_most_specific_active_staff_folder() {
        let c = open_in_memory().unwrap();
        let w = save(&c, &input("Wenjie", "w@example.org", "F-WEN")).unwrap();
        let sub = save(&c, &input("Sub", "s@example.org", "F-WEN-SUB")).unwrap();
        let staff = list(&c, true).unwrap();
        let chain = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(owner_for(&staff, &chain(&["F-LEAF", "F-WEN", "F-INBOX"])), Some(w.id), "descendants inherit the mapping");
        assert_eq!(owner_for(&staff, &chain(&["F-WEN-SUB", "F-WEN", "F-INBOX"])), Some(sub.id), "more specific mapping wins");
        assert_eq!(owner_for(&staff, &chain(&["F-INBOX"])), None);
        set_active(&c, sub.id, false).unwrap();
        let staff = list(&c, true).unwrap();
        assert_eq!(owner_for(&staff, &chain(&["F-WEN-SUB", "F-WEN"])), Some(w.id), "inactive staff are not inferred");
    }
}
