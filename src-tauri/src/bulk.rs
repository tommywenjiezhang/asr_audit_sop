//! Bulk workflow actions (P3): the same validated single-case actions, run case by case, each reported
//! separately. Nothing is skipped silently and one failure never stops the others; a retry simply runs
//! the action again for the failed cases (every action is idempotent for cases already in the target state).
//! Outlook steps (Assign/Reassign moves, archiving) are not here: they run through the coordinated
//! command layer, which also reports per case.

use crate::error::{AppError, AppResult};
use crate::tickets;
use crate::workflow;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BulkResult {
    pub ticket_id: i64,
    pub display_id: String,
    pub ok: bool,
    /// "done" | "unchanged" | "failed"
    pub outcome: String,
    pub message: String,
}

pub const ACTIONS: &[&str] = &["claim", "takeover", "release", "start", "flag_review", "resume", "done", "reopen"];

/// Run one workflow action on one case.
pub fn run_one(conn: &Connection, id: i64, action: &str, reason: &str, actor: &str) -> BulkResult {
    let before = tickets::get(conn, id).ok();
    let disp = tickets::display_id(id);
    let r: AppResult<String> = (|| {
        let t = before.clone().ok_or_else(|| AppError::coded("NOT_FOUND", "Case not found."))?;
        Ok(match action {
            "claim" => workflow::claim(conn, id, actor, false).map(|t| t.status)?,
            "takeover" => workflow::claim(conn, id, actor, true).map(|t| t.status)?,
            "release" => workflow::release(conn, id, actor).map(|t| t.status)?,
            "start" => workflow::start_work(conn, id, actor).map(|t| t.status)?,
            "flag_review" => workflow::flag_review(conn, id, if reason.trim().is_empty() { "Flagged for review" } else { reason }, actor).map(|t| t.status)?,
            "resume" => workflow::resume(conn, id, actor).map(|t| t.status)?,
            "done" => {
                if t.status == tickets::DONE {
                    tickets::DONE.to_string()
                } else {
                    workflow::mark_done(conn, id, actor)?;
                    tickets::DONE.to_string()
                }
            }
            "reopen" => workflow::reopen(conn, id, actor, if reason.trim().is_empty() { "Reopened by staff" } else { reason }).map(|t| t.status)?,
            other => return Err(AppError::coded("INVALID", format!("Unknown bulk action {other}"))),
        })
    })();
    match r {
        Ok(status) => {
            let after = tickets::get(conn, id).ok();
            let changed = match (&before, &after) {
                (Some(b), Some(a)) => b.status != a.status || b.assignee_staff_id != a.assignee_staff_id || b.updated_at != a.updated_at,
                _ => true,
            };
            BulkResult { ticket_id: id, display_id: disp, ok: true, outcome: if changed { "done".into() } else { "unchanged".into() }, message: if changed { format!("Now {status}") } else { "Already in that state".into() } }
        }
        Err(e) => BulkResult { ticket_id: id, display_id: disp, ok: false, outcome: "failed".into(), message: e.to_string() },
    }
}

pub fn run(conn: &Connection, ids: &[i64], action: &str, reason: &str, actor: &str) -> AppResult<Vec<BulkResult>> {
    if !ACTIONS.contains(&action) {
        return Err(AppError::coded("INVALID", format!("Unknown bulk action {action}")));
    }
    if ids.len() > 500 {
        return Err(AppError::coded("INVALID", "Select at most 500 cases at a time."));
    }
    let mut seen = std::collections::BTreeSet::new();
    Ok(ids.iter().filter(|i| seen.insert(**i)).map(|id| run_one(conn, *id, action, reason, actor)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::tests::{conn, msg};

    #[test]
    fn each_case_is_reported_and_failures_do_not_stop_the_rest() {
        let c = conn();
        let me = crate::staff::save(&c, &crate::staff::tests::input("Wenjie", "w@example.org", "F-W")).unwrap();
        crate::staff::set_current_staff_id(&c, Some(me.id)).unwrap();
        crate::tickets::ingest(&c, &[msg(1, "C1", "A", "x", "a@b.org"), msg(2, "C2", "B", "y", "b@b.org")], &crate::settings::Settings::default()).unwrap();
        let r = run(&c, &[1, 2, 999, 1], "claim", "", "Wenjie").unwrap();
        assert_eq!(r.len(), 3, "duplicates are run once");
        assert!(r[0].ok && r[1].ok && !r[2].ok);
        // retry: already claimed cases are reported unchanged, not as errors
        let r = run(&c, &[1, 2], "claim", "", "Wenjie").unwrap();
        assert!(r.iter().all(|x| x.ok && x.outcome == "unchanged"), "{r:?}");
        let r = run(&c, &[1, 2], "start", "", "Wenjie").unwrap();
        assert!(r.iter().all(|x| x.ok));
        let r = run(&c, &[1], "done", "", "Wenjie").unwrap();
        assert_eq!(tickets::get(&c, 1).unwrap().status, tickets::DONE, "{r:?}");
        assert!(run(&c, &[1], "explode", "", "x").is_err());
    }
}
