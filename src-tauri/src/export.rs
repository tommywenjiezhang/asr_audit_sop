//! Reports, CSV export (Excel-compatible), database backup and restore.

use crate::catalog::catalog;
use crate::error::{AppError, AppResult};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Report {
    pub total: i64,
    pub open: i64,
    pub by_status: Vec<(String, i64)>,
    pub by_issue: Vec<(String, String, i64)>,
    pub by_school_verification: Vec<(String, i64)>,
    pub created_last_7_days: i64,
    pub created_last_30_days: i64,
    pub open_older_than_7_days: i64,
    pub followups_due: i64,
    pub outlook_drafts_created: i64,
    pub by_week: Vec<(String, i64)>,
}

/// Read-only task report generated from the same ticket filter used by the Tickets page.
#[derive(Debug, Clone, Serialize)]
pub struct EmailTicketTaskReport {
    pub markdown: String,
    pub ticket_count: usize,
    pub by_status: Vec<(String, i64)>,
    pub by_issue: Vec<(String, i64)>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmailTicketTaskReportFilter {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub received_from: String,
    #[serde(default)]
    pub received_to: String,
}

const NOT_PROVIDED: &str = "[Not provided]";

fn report_value(value: Option<&str>) -> String {
    value.map(str::trim).filter(|v| !v.is_empty() && *v != "[]").unwrap_or(NOT_PROVIDED).to_string()
}

fn report_status(status: &str) -> &str {
    if status == "Review" { "Needs Review" } else { status }
}

fn markdown_code_block(body: &str) -> String {
    let fence = if body.contains("````") { "`````" } else if body.contains("```") { "````" } else { "```" };
    format!("{fence}\n{}\n{fence}", body.replace("\r\n", "\n").replace('\r', "\n"))
}

fn field_map(fields: &[crate::tickets::TicketField]) -> BTreeMap<String, String> {
    fields.iter().map(|f| (f.key.clone(), f.effective.clone())).collect()
}

fn field_value(fields: &BTreeMap<String, String>, key: &str) -> String {
    report_value(fields.get(key).map(String::as_str))
}

fn report_instructions() -> &'static str {
    "## Microsoft Copilot review instructions\n\nFor each ticket, use only the information in this report and preserve the original email wording when quoting it. Do not infer or invent missing facts; call out every `[Not provided]` value and every outstanding issue. Apply the approved ASR/Audit SOP and approved response templates for the issue type. Do not send email, change tickets, or change Outlook.\n\n1. Analyze each ticket and identify urgency, blockers, missing information, and the next safe NJIIS troubleshooting action.\n2. Generate a prioritized daily task list, with the ticket ID, reason for priority, recommended owner, and concrete next step.\n3. Recommend NJIIS troubleshooting actions that are supported by the approved ASR/Audit SOP; distinguish checks already completed from actions still outstanding.\n4. Draft a follow-up email for each ticket using approved ASR/Audit SOP wording. Keep drafts separate from the original messages, mark any missing placeholders, and ask staff to review/redact sensitive information before copying or sending.\n5. Never transmit this report to an AI service automatically. Staff must review and redact it before copying it into Microsoft Copilot."
}

/// Build a complete, reviewable Markdown report. This function only reads the database.
pub fn email_ticket_task_report(conn: &Connection, filter: &EmailTicketTaskReportFilter) -> AppResult<EmailTicketTaskReport> {
    let status = match filter.status.trim() {
        "" | "All" | "all" => "".to_string(),
        "Needs Review" => "Review".to_string(),
        "In Progress" | "Done" => filter.status.trim().to_string(),
        other => return Err(AppError::coded("INVALID", format!("Unknown report status: {other}"))),
    };
    let q = crate::tickets::ListQuery {
        status,
        received_from: filter.received_from.clone(),
        received_to: filter.received_to.clone(),
        limit: 1000,
        ..Default::default()
    };
    let mut ids = Vec::new();
    let mut offset = 0;
    loop {
        let page = crate::tickets::list(conn, &crate::tickets::ListQuery { offset, ..q.clone() })?;
        ids.extend(page.rows.iter().map(|r| r.id));
        if page.rows.is_empty() || ids.len() as i64 >= page.total { break; }
        offset += page.rows.len() as i64;
    }

    let mut status_counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut issue_counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut out = String::new();
    out.push_str("# Email Ticket Task Report\n\n");
    out.push_str(&format!("Generated: {}\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M %Z")));
    out.push_str("## Summary\n\n");
    out.push_str(&format!("- Total tickets: **{}**\n", ids.len()));
    out.push_str(&format!("- Status filter: **{}**\n", if filter.status.trim().is_empty() { "All" } else { filter.status.trim() }));
    out.push_str(&format!("- Received date range: **{}** to **{}**\n\n", report_value(Some(&filter.received_from)), report_value(Some(&filter.received_to))));
    out.push_str("### Counts by status\n\n");
    let mut rendered_tickets = Vec::new();
    for id in ids {
        let d = crate::tickets::detail(conn, id)?;
        let t = &d.ticket;
        *status_counts.entry(report_status(&t.status).to_string()).or_default() += 1;
        let mut issues = d.issues.iter().map(|i| i.issue_type.clone()).collect::<Vec<_>>();
        if issues.is_empty() { issues.push(t.issue_type.clone()); }
        for issue in &issues {
            let name = catalog().issue(issue).map(|x| x.name.clone()).unwrap_or_else(|| issue.clone());
            *issue_counts.entry(name).or_default() += 1;
        }
        rendered_tickets.push((d, issues));
    }
    for (s, n) in &status_counts { out.push_str(&format!("- {s}: {n}\n")); }
    if status_counts.is_empty() { out.push_str("- None\n"); }
    out.push_str("\n### Counts by issue type\n\n");
    for (s, n) in &issue_counts { out.push_str(&format!("- {s}: {n}\n")); }
    if issue_counts.is_empty() { out.push_str("- None\n"); }
    out.push_str("\n");
    out.push_str(report_instructions());
    out.push_str("\n\n---\n\n");

    for (number, (d, issues)) in rendered_tickets.iter().enumerate() {
        let t = &d.ticket;
        let fields = field_map(&d.fields);
        let issue_names = issues.iter().map(|i| catalog().issue(i).map(|x| x.name.clone()).unwrap_or_else(|| i.clone())).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("## {}. {}\n\n", number + 1, t.display_id));
        out.push_str("### Ticket details\n\n");
        out.push_str(&format!("- Ticket ID: {}\n- Status: {}\n- Issue type: {}\n- Subject: {}\n- Received date: {}\n- Assigned staff: {}\n\n", t.display_id, report_status(&t.status), issue_names, report_value(t.original_subject.as_deref()), report_value(t.first_received_at.as_deref().or(t.last_email_received_at.as_deref()).or(Some(&t.created_at))), report_value(t.assignee_name.as_deref().or(t.assigned_staff.as_deref()))));
        out.push_str("### Sender\n\n");
        out.push_str(&format!("- Name: {}\n- Email: {}\n- Phone: {}\n- Title: {}\n\n", report_value(t.sender_name.as_deref()), report_value(t.sender_email.as_deref()), field_value(&fields, "sender_phone"), field_value(&fields, "sender_title")));
        out.push_str("### NJIIS and school information\n\n");
        out.push_str(&format!("- NJIIS username: {}\n- Facility ID: {}\n", field_value(&fields, "njiis_username"), field_value(&fields, "facility_id")));
        for slot in &d.schools {
            let prefix = if *slot == 1 { "school".to_string() } else { format!("school{slot}") };
            out.push_str(&format!("- School name ({}): {}\n- School NJIIS ID ({}): {}\n", slot, field_value(&fields, &format!("{prefix}_name")), slot, field_value(&fields, &format!("{prefix}_njiis_id"))));
        }
        out.push_str("\n### Previously completed actions and outstanding issues\n\n");
        out.push_str(&format!("- Staff actions: {}\n- Resolution notes: {}\n- Internal notes: {}\n- Review reason: {}\n- Next follow-up date: {}\n\n", report_value(t.staff_actions.as_deref()), report_value(t.resolution_notes.as_deref()), report_value(t.internal_notes.as_deref()), report_value(t.review_reason.as_deref()), report_value(t.next_followup_date.as_deref())));
        if !d.notes.is_empty() { out.push_str("**Staff notes:**\n\n"); for n in &d.notes { out.push_str(&format!("- {} ({}): {}\n", n.author_name, n.created_at, n.body)); } out.push('\n'); }
        if !d.history.is_empty() { out.push_str("**Ticket history:**\n\n"); for h in &d.history { out.push_str(&format!("- {} — {} — {}{}\n", h.at, h.actor, h.action, h.new_value.as_deref().map(|v| format!(": {v}")).unwrap_or_default())); } out.push('\n'); }
        out.push_str("### Complete related conversation history\n\n");
        let mut messages = d.messages.clone();
        messages.sort_by(|a, b| a.received_at.cmp(&b.received_at).then(a.id.cmp(&b.id)));
        if messages.is_empty() { out.push_str("[No related messages stored]\n\n"); }
        for (i, m) in messages.iter().enumerate() {
            out.push_str(&format!("#### Message {} — {}\n\n- Link type: {}\n- Sender: {} <{}>\n- Received: {}\n- Attachments: {}\n\n", i + 1, report_value(m.subject.as_deref()), m.link_type, report_value(m.sender_name.as_deref()), report_value(m.sender_email.as_deref()), report_value(m.received_at.as_deref()), if m.has_attachments { "Yes" } else { "No" }));
            match m.body_text.as_deref().filter(|b| !b.is_empty()) {
                Some(body) => out.push_str(&format!("{}\n\n", markdown_code_block(body))),
                None => out.push_str("[Message body not provided]\n\n"),
            }
            if m.body_truncated { out.push_str("[Warning: this stored message is truncated; load the full email before relying on it.]\n\n"); }
        }
        out.push_str("---\n\n");
    }
    Ok(EmailTicketTaskReport { markdown: out, ticket_count: rendered_tickets.len(), by_status: status_counts.into_iter().collect(), by_issue: issue_counts.into_iter().collect() })
}

pub fn export_email_ticket_task_report(conn: &Connection, path: &Path, filter: &EmailTicketTaskReportFilter) -> AppResult<usize> {
    let report = email_ticket_task_report(conn, filter)?;
    std::fs::write(path, &report.markdown)?;
    Ok(report.ticket_count)
}

fn pairs(conn: &Connection, sql: &str) -> AppResult<Vec<(String, i64)>> {
    let mut st = conn.prepare(sql)?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

fn scalar(conn: &Connection, sql: &str) -> AppResult<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

pub fn report(conn: &Connection) -> AppResult<Report> {
    let open_set = "('Unassigned','Assigned','In Progress','Review')";
    let by_issue_raw = pairs(conn, "SELECT issue_type, COUNT(*) FROM tickets GROUP BY issue_type ORDER BY 2 DESC")?;
    let by_issue = by_issue_raw
        .into_iter()
        .map(|(id, n)| {
            let name = catalog().issue(&id).map(|i| i.name.clone()).unwrap_or_else(|| id.clone());
            (id, name, n)
        })
        .collect();
    Ok(Report {
        total: scalar(conn, "SELECT COUNT(*) FROM tickets")?,
        open: scalar(conn, &format!("SELECT COUNT(*) FROM tickets WHERE status IN {open_set}"))?,
        by_status: pairs(conn, "SELECT status, COUNT(*) FROM tickets GROUP BY status ORDER BY 2 DESC")?,
        by_issue,
        by_school_verification: pairs(conn, "SELECT school_verification, COUNT(*) FROM tickets GROUP BY school_verification")?,
        created_last_7_days: scalar(conn, "SELECT COUNT(*) FROM tickets WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days')")?,
        created_last_30_days: scalar(conn, "SELECT COUNT(*) FROM tickets WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-30 days')")?,
        open_older_than_7_days: scalar(conn, &format!("SELECT COUNT(*) FROM tickets WHERE status IN {open_set} AND created_at < strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days')"))?,
        followups_due: scalar(conn, &format!("SELECT COUNT(*) FROM tickets WHERE status IN {open_set} AND next_followup_date IS NOT NULL AND next_followup_date <= date('now','localtime')"))?,
        outlook_drafts_created: scalar(conn, "SELECT COUNT(*) FROM email_drafts WHERE outlook_entry_id IS NOT NULL")?,
        by_week: pairs(conn, "SELECT strftime('%Y-W%W', created_at) w, COUNT(*) FROM tickets WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-84 days') GROUP BY w ORDER BY w")?,
    })
}

fn csv_cell(s: &str) -> String {
    // neutralize spreadsheet formula injection
    let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Export tickets (optionally filtered by status: "" / "all" / "In Progress" / "Done") with all field values to CSV.
pub fn export_csv(conn: &Connection, path: &Path, status: &str) -> AppResult<usize> {
    let status = if status == "all" { "" } else { status };
    let q = crate::tickets::ListQuery { status: status.to_string(), limit: 1000, ..Default::default() };
    let mut ids: Vec<i64> = Vec::new();
    let mut offset = 0;
    loop {
        let res = crate::tickets::list(conn, &crate::tickets::ListQuery { offset, ..q.clone() })?;
        if res.rows.is_empty() {
            break;
        }
        offset += res.rows.len() as i64;
        ids.extend(res.rows.iter().map(|r| r.id));
        if offset >= res.total {
            break;
        }
    }
    write_csv(conn, path, &ids)
}

/// Export exactly these cases (bulk "Export selected").
pub fn export_ids(conn: &Connection, path: &Path, ids: &[i64]) -> AppResult<usize> {
    write_csv(conn, path, ids)
}

fn write_csv(conn: &Connection, path: &Path, ids: &[i64]) -> AppResult<usize> {
    let mut field_keys: BTreeSet<String> = BTreeSet::new();
    let mut st = conn.prepare("SELECT DISTINCT field_key FROM ticket_fields")?;
    for k in st.query_map([], |r| r.get::<_, String>(0))? {
        field_keys.insert(k?);
    }
    let header = [
        "Ticket ID", "Status", "Priority", "Issue Type", "School NJIIS ID", "School Name", "School Verification", "Sender",
        "Sender Email", "Original Subject", "Assigned Staff", "Date Created", "Last Updated", "Last Email Received",
        "Last Response", "Next Follow-up", "Possible Duplicate Of", "Internal Notes", "Resolution Notes",
        "First Received", "Completed", "Staff Actions", "Reference", "Owner", "Review Reason",
    ];
    let mut out = String::from("\u{feff}");
    out.push_str(&header.iter().map(|h| csv_cell(h)).chain(field_keys.iter().map(|k| csv_cell(&format!("Field: {k}")))).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for id in ids {
        let t = crate::tickets::get(conn, *id)?;
        let fs = crate::tickets::fields(conn, *id)?;
        let issue = catalog().issue(&t.issue_type).map(|i| i.name.clone()).unwrap_or(t.issue_type.clone());
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        let mut row = vec![
            t.display_id.clone(), t.status.clone(), t.priority.clone(), issue, o(&t.school_njiis_id), o(&t.school_name),
            t.school_verification.clone(), o(&t.sender_name), o(&t.sender_email), o(&t.original_subject), o(&t.assigned_staff),
            t.created_at.clone(), t.updated_at.clone(), o(&t.last_email_received_at), o(&t.last_response_at), o(&t.next_followup_date),
            t.possible_duplicate_of.map(crate::tickets::display_id).unwrap_or_default(), o(&t.internal_notes), o(&t.resolution_notes),
            o(&t.first_received_at), o(&t.completed_at), o(&t.staff_actions), o(&t.ticket_ref), o(&t.assignee_name), o(&t.review_reason),
        ];
        for k in &field_keys {
            row.push(fs.iter().find(|f| &f.key == k).map(|f| crate::render::display_value(&f.effective)).unwrap_or_default());
        }
        out.push_str(&row.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    std::fs::write(path, out)?;
    Ok(ids.len())
}

/// Consistent online backup (VACUUM INTO).
pub fn backup(conn: &Connection, dest: &Path) -> AppResult<()> {
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().to_string()])?;
    Ok(())
}

/// Validate a backup file before restoring it.
pub fn validate_backup(src: &Path) -> AppResult<()> {
    let c = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| AppError::coded("INVALID_BACKUP", "The selected file is not a readable database."))?;
    let ok: i64 = c
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('tickets','email_templates','ticket_fields')", [], |r| r.get(0))
        .map_err(|_| AppError::coded("INVALID_BACKUP", "The selected file is not an ASR/Audit Support Desk backup."))?;
    if ok != 3 {
        return Err(AppError::coded("INVALID_BACKUP", "The selected file is not an ASR/Audit Support Desk backup."));
    }
    let integrity: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(AppError::coded("INVALID_BACKUP", format!("Backup failed the integrity check: {integrity}")));
    }
    Ok(())
}

/// Restore a backup into the live connection (online backup API), then migrate.
pub fn restore(conn: &mut Connection, src: &Path, safety_copy: &Path) -> AppResult<()> {
    validate_backup(src)?;
    backup(conn, safety_copy)?;
    let source = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    {
        let b = rusqlite::backup::Backup::new(&source, conn)?;
        b.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
    }
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    crate::db::migrate(conn)?;
    crate::db::after_migrate(conn)?;
    crate::templates_store::seed_embedded(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::tickets::tests::{conn, msg};

    #[test]
    fn report_csv_backup_restore() {
        let c = conn();
        crate::tickets::ingest(&c, &[msg(1, "C1", "Subtype", "=cmd School ID: 123456, missing subtype", "a@b.org")], &Settings::default()).unwrap();
        let r = report(&c).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.open, 1);
        let dir = tempfile::tempdir().unwrap();
        let csvp = dir.path().join("t.csv");
        assert_eq!(export_csv(&c, &csvp, "all").unwrap(), 1);
        let text = std::fs::read_to_string(&csvp).unwrap();
        assert!(text.contains("T-000001"));
        assert!(text.contains("123456"));
        assert!(!text.contains("\n=cmd"));

        let bk = dir.path().join("b.db");
        backup(&c, &bk).unwrap();
        validate_backup(&bk).unwrap();
        // change live data then restore
        let mut live = crate::db::open(&dir.path().join("live.db")).unwrap();
        crate::templates_store::seed_embedded(&live).unwrap();
        restore(&mut live, &bk, &dir.path().join("safety.db")).unwrap();
        let n: i64 = live.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert!(dir.path().join("safety.db").exists());
        std::fs::write(dir.path().join("junk.db"), b"not a db").unwrap();
        assert!(validate_backup(&dir.path().join("junk.db")).is_err());
    }

    #[test]
    fn email_ticket_task_report_filters_and_preserves_conversation() {
        let c = conn();
        let s = Settings::default();
        crate::tickets::ingest(&c, &[msg(1, "C1", "Subtype", "Original wording: School subtype is missing.\nPlease help.", "jane@school.org")], &s).unwrap();
        crate::tickets::set_field(&c, 1, "sender_phone", Some("609-555-0100".into()), true, "staff").unwrap();
        crate::tickets::set_field(&c, 1, "sender_title", Some("School Nurse".into()), true, "staff").unwrap();
        let filter = EmailTicketTaskReportFilter { status: "All".into(), received_from: "2026-11-16T00:00:00Z".into(), received_to: "2026-11-17T00:00:00Z".into() };
        let r = email_ticket_task_report(&c, &filter).unwrap();
        assert_eq!(r.ticket_count, 1);
        assert!(r.markdown.contains("Original wording: School subtype is missing."));
        assert!(r.markdown.contains("609-555-0100"));
        assert!(r.markdown.contains("School Nurse"));
        assert!(r.markdown.contains("Microsoft Copilot review instructions"));
        assert!(r.markdown.contains("[Not provided]"));

        let none = email_ticket_task_report(&c, &EmailTicketTaskReportFilter { status: "Done".into(), ..filter }).unwrap();
        assert_eq!(none.ticket_count, 0);
        assert!(none.markdown.contains("Total tickets: **0**"));
    }
}
