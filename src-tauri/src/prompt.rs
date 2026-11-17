//! Copilot prompt: one plain-text block built only from the saved ticket data.
//! The UI shows exactly this string and copies exactly this string.
//! Missing values are shown as "[Not provided]"; nothing is invented and nothing is cut silently.

use crate::analyzer::{school_key, split_current};
use crate::catalog::catalog;
use crate::error::AppResult;
use crate::render;
use crate::settings::Settings;
use crate::templates_store as ts;
use crate::tickets::{self, MessageRow};
use rusqlite::Connection;
use std::collections::HashMap;

pub const NOT_PROVIDED: &str = "[Not provided]";

fn or_np(v: Option<&str>) -> String {
    match v.map(str::trim) {
        Some(s) if !s.is_empty() && s != "[]" => render::display_value(s),
        _ => NOT_PROVIDED.to_string(),
    }
}

/// "2026-11-16T15:30:00Z" -> "Nov 16, 2026 10:30 AM" (local time); unparsable values are shown as stored.
pub fn local_time(iso: Option<&str>) -> String {
    match iso.map(str::trim).filter(|s| !s.is_empty()) {
        None => NOT_PROVIDED.into(),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&chrono::Local).format("%b %-d, %Y %-I:%M %p").to_string())
            .unwrap_or_else(|_| s.to_string()),
    }
}

/// Readable text of a stored body (Outlook normally gives plain text; HTML is converted).
fn readable(body: &str) -> String {
    let b = body.replace("\r\n", "\n").replace('\r', "\n");
    let looks_html = {
        let l = b.trim_start().to_lowercase();
        l.starts_with("<html") || l.starts_with("<!doctype") || (l.starts_with('<') && (l.contains("</p>") || l.contains("</div>")))
    };
    let t = if looks_html { render::html_to_text(&b) } else { b };
    // keep paragraph breaks, drop runs of empty lines
    let mut out = String::new();
    let mut blank = 0;
    for line in t.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

fn truncation_note(m: &MessageRow, chars: usize) -> String {
    if m.body_truncated {
        format!("\n[This email was longer than {chars} characters when imported and the rest was not stored. Use \"Load full email\" on the ticket, then copy the prompt again.]")
    } else {
        String::new()
    }
}

/// Replace any {{placeholder}} still in SOP text with a visible "[Not provided: Label]".
fn mark_missing(text: &str) -> String {
    render::TOKEN_RE
        .replace_all(text, |c: &regex::Captures| {
            let label = catalog().placeholder(&c[1]).map(|p| p.label.clone()).unwrap_or_else(|| c[1].replace('_', " "));
            format!("[Not provided: {label}]")
        })
        .into_owned()
}

fn sop_excerpts(conn: &Connection, ticket_id: i64, issues: &[String], settings: &Settings) -> AppResult<String> {
    let ctx = tickets::build_ctx(conn, ticket_id, settings)?;
    let mut parts: Vec<String> = Vec::new();
    for issue in issues {
        let candidates = ts::selectable(conn, issue)?;
        let chosen = render::select_template(&candidates, &ctx)
            .map(|s| s.id.clone())
            .or_else(|| catalog().issue(issue).map(|i| i.default_template.clone()));
        let Some(t) = chosen.and_then(|id| ts::get(conn, &id).ok()).filter(|t| t.enabled) else { continue };
        if issue == "other" {
            continue; // "manual review" is a placeholder, not approved wording
        }
        let name = catalog().issue(issue).map(|i| i.name.clone()).unwrap_or_else(|| issue.clone());
        let sop = match (t.sop_section.as_deref(), t.sop_page.as_deref()) {
            (Some(s), Some(p)) if !p.is_empty() => format!("; SOP: {s}, p. {p}"),
            (Some(s), _) if !s.is_empty() => format!("; SOP: {s}"),
            _ => String::new(),
        };
        let body = mark_missing(&render::html_to_text(&render::fill(&ts::strip_placeholder_marks(&t.body_html), &ctx)));
        parts.push(format!("[{name} — template \"{}\"{sop}]\n{body}", t.name));
    }
    Ok(if parts.is_empty() { "No matching SOP excerpt available".into() } else { parts.join("\n\n") })
}

pub fn build_prompt(conn: &Connection, ticket_id: i64, settings: &Settings) -> AppResult<String> {
    let t = tickets::get(conn, ticket_id)?;
    let fields: HashMap<String, String> = tickets::fields(conn, ticket_id)?.into_iter().map(|f| (f.key, f.effective)).collect();
    let fv = |k: &str| or_np(fields.get(k).map(String::as_str));
    let mut issue_ids: Vec<String> = tickets::issues(conn, ticket_id)?.into_iter().map(|i| i.issue_type).collect();
    if issue_ids.is_empty() {
        issue_ids.push(t.issue_type.clone());
    }
    let issue_names: Vec<String> = issue_ids.iter().map(|i| catalog().issue(i).map(|d| d.name.clone()).unwrap_or_else(|| i.clone())).collect();

    let mut msgs = tickets::messages(conn, ticket_id)?;
    msgs.sort_by(|a, b| a.received_at.cmp(&b.received_at).then(a.id.cmp(&b.id)));
    let original_idx = msgs
        .iter()
        .position(|m| m.link_type == "original" || m.link_type == "manual")
        .or_else(|| msgs.iter().position(|m| !m.is_internal));

    let mut p = String::new();
    p.push_str(settings.prompt_header());
    p.push_str("\n\nTICKET\n");
    p.push_str(&format!("Ticket ID: {}\n", t.display_id));
    p.push_str(&format!("Issue type(s): {}\n", issue_names.join("; ")));
    p.push_str(&format!("Issue summary: {}\n", fv("issue_description")));
    let errors = fields.get("error_message").map(|e| e.trim().to_string()).unwrap_or_default();
    if errors.contains('\n') {
        p.push_str("Exact error message(s):\n");
        for e in errors.lines().filter(|l| !l.trim().is_empty()) {
            p.push_str(&format!("- \"{}\"\n", e.trim()));
        }
    } else if errors.is_empty() {
        p.push_str(&format!("Exact error message(s): {NOT_PROVIDED}\n"));
    } else {
        p.push_str(&format!("Exact error message(s): \"{errors}\"\n"));
    }
    p.push_str(&format!("Received: {}\n", local_time(t.first_received_at.as_deref().or(Some(&t.created_at)))));
    p.push_str(&format!("Original subject: {}\n", or_np(t.original_subject.as_deref())));

    p.push_str("\nSENDER\n");
    p.push_str(&format!("Name: {}\n", or_np(t.sender_name.as_deref())));
    p.push_str(&format!("Title: {}\n", fv("sender_title")));
    p.push_str(&format!("Email: {}\n", or_np(t.sender_email.as_deref())));
    let phone = fields.get("sender_phone").map(|s| s.trim()).unwrap_or("");
    let ext = fields.get("sender_phone_ext").map(|s| s.trim()).unwrap_or("");
    let phone_txt = match (phone.is_empty(), ext.is_empty()) {
        (true, _) => NOT_PROVIDED.to_string(),
        (false, true) => phone.to_string(),
        (false, false) => format!("{phone} ext. {ext}"),
    };
    p.push_str(&format!("Phone: {phone_txt}\n"));

    let slots = tickets::school_slots(conn, ticket_id)?;
    p.push_str("\nSCHOOL(S)\n");
    for (n, slot) in slots.iter().enumerate() {
        if slots.len() > 1 {
            if n > 0 {
                p.push('\n');
            }
            p.push_str(&format!("School {} of {}\n", n + 1, slots.len()));
        }
        let k = |attr: &str| school_key(*slot, attr);
        let mut njiis = fv(&k("njiis_id"));
        if *slot == 1 {
            // legacy field from older imports: other NJIIS IDs mentioned in the same email
            if let Some(extra) = fields.get("additional_ids").filter(|v| !v.trim().is_empty()) {
                njiis = format!("{njiis} (other NJIIS IDs mentioned in the email: {})", extra.trim());
            }
        }
        p.push_str(&format!("School name: {}\n", fv(&k("name"))));
        p.push_str(&format!("NJIIS identifier: {njiis}\n"));
        p.push_str(&format!("Other school ID(s), with identifier type: {}\n", fv(&k("other_ids"))));
        p.push_str(&format!("School address: {}\n", fv(&k("address"))));
        p.push_str(&format!("Municipality: {}\n", fv(&k("municipality"))));
        p.push_str(&format!("County: {}\n", fv(&k("county"))));
        p.push_str(&format!("ZIP code: {}\n", fv(&k("zip"))));
    }

    p.push_str("\nSTAFF ACTIONS ALREADY TAKEN\n");
    p.push_str(t.staff_actions.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("None recorded"));
    p.push_str("\n\nSTAFF NOTES\n");
    let mut notes: Vec<String> = Vec::new();
    if let Some(n) = t.internal_notes.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        notes.push(n.to_string());
    }
    if let Some(n) = t.resolution_notes.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        notes.push(format!("Earlier resolution notes: {n}"));
    }
    p.push_str(&if notes.is_empty() { "None".to_string() } else { notes.join("\n") });

    p.push_str("\n\nAPPLICABLE APPROVED SOP / TEMPLATE\n");
    p.push_str(&sop_excerpts(conn, ticket_id, &issue_ids, settings)?);

    p.push_str("\n\nORIGINAL EMAIL — REFERENCE MATERIAL\n--- BEGIN ORIGINAL EMAIL ---\n");
    match original_idx.map(|i| &msgs[i]) {
        Some(m) => {
            p.push_str(&format!("From: {} <{}>\n", or_np(m.sender_name.as_deref()), m.sender_email.as_deref().unwrap_or("")));
            p.push_str(&format!("Received: {}\n", local_time(m.received_at.as_deref())));
            p.push_str(&format!("Subject: {}\n\n", or_np(m.subject.as_deref())));
            p.push_str(&readable(m.body_text.as_deref().unwrap_or("")));
            p.push_str(&truncation_note(m, settings.body_max_chars as usize));
        }
        None => p.push_str("[No original email is stored for this ticket.]"),
    }
    p.push_str("\n--- END ORIGINAL EMAIL ---\n\nRELEVANT CONVERSATION CONTEXT\n");
    let others: Vec<&MessageRow> = msgs.iter().enumerate().filter(|(i, _)| Some(*i) != original_idx).map(|(_, m)| m).collect();
    if others.is_empty() {
        p.push_str("None");
    } else {
        for (n, m) in others.iter().enumerate() {
            if n > 0 {
                p.push_str("\n\n");
            }
            let who = match (m.sender_name.as_deref(), m.sender_email.as_deref()) {
                (Some(nm), Some(e)) if !nm.trim().is_empty() => format!("{nm} <{e}>"),
                (_, Some(e)) => e.to_string(),
                _ => NOT_PROVIDED.to_string(),
            };
            let role = if m.is_internal { " (Help Desk staff)" } else { "" };
            p.push_str(&format!("--- {who}{role} | {} | {} ---\n", local_time(m.received_at.as_deref()), or_np(m.subject.as_deref())));
            // only the new part of each message: quoted history repeats earlier messages
            let full = readable(m.body_text.as_deref().unwrap_or(""));
            let (current, quoted) = split_current(&full);
            let cur = current.trim();
            p.push_str(if cur.is_empty() { full.trim() } else { cur });
            if !quoted.trim().is_empty() && !cur.is_empty() {
                p.push_str("\n[Earlier quoted messages omitted]");
            }
            p.push_str(&truncation_note(m, settings.body_max_chars as usize));
        }
    }
    p.push('\n');
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::tests::{conn, msg};
    use crate::tickets::{ingest, set_field, set_issues, update, TicketPatch};

    #[test]
    fn missing_fields_show_not_provided_and_structure_is_complete() {
        let c = conn();
        let s = Settings::default();
        ingest(&c, &[msg(1, "C1", "Question", "Hello, how do I fill out the form?", "")], &s).unwrap();
        let p = build_prompt(&c, 1, &s).unwrap();
        assert!(p.starts_with("Please draft a concise, professional NJIIS Help Desk reply"));
        for section in ["\nTICKET\n", "\nSENDER\n", "\nSCHOOL(S)\n", "\nSTAFF ACTIONS ALREADY TAKEN\nNone recorded", "\nSTAFF NOTES\nNone",
            "\nAPPLICABLE APPROVED SOP / TEMPLATE\n", "--- BEGIN ORIGINAL EMAIL ---", "--- END ORIGINAL EMAIL ---", "\nRELEVANT CONVERSATION CONTEXT\nNone"] {
            assert!(p.contains(section), "missing {section:?}");
        }
        assert!(p.contains("NJIIS identifier: [Not provided]"));
        assert!(p.contains("Title: [Not provided]"));
        assert!(p.contains("Email: [Not provided]"));
        assert!(p.contains("Hello, how do I fill out the form?"));
    }

    #[test]
    fn multiple_issues_schools_sop_and_conversation() {
        let c = conn();
        let s = Settings::default();
        let body = "Hello,\nSchool Name: Maple Leaf Academy\nSchool NJIIS ID: 0012345\n\nSchool Name: Riverside Preschool\nSchool NJIIS ID: 0067890\nThe webinar shows a black screen.\nThanks,\nAnn Lee\nSchool Nurse\nPhone: 609-555-0100 x12";
        ingest(&c, &[msg(1, "C1", "Webinar", body, "ann@school.org")], &s).unwrap();
        set_issues(&c, 1, &["webinar_issue".into(), "school_subtype_missing".into()], "staff").unwrap();
        update(&c, 1, &TicketPatch { staff_actions: Some("Reset the user's training progress.".into()), internal_notes: Some("Called on 11/16".into()), ..Default::default() }, "staff").unwrap();
        ingest(&c, &[msg(2, "C1", "RE: Webinar", "Still black.\n\nFrom: Desk\nSent: Monday\nOld quoted text", "ann@school.org")], &s).unwrap();
        // customized template wording is used in the SOP excerpt
        set_field(&c, 1, "school_name", Some("Maple Leaf Academy".into()), true, "staff").unwrap();
        let ctx = tickets::build_ctx(&c, 1, &s).unwrap();
        let cands = ts::selectable(&c, "webinar_issue").unwrap();
        let tid = render::select_template(&cands, &ctx).unwrap().id.clone();
        ts::save(&c, &tid, "Subject", "<p>APPROVED CUSTOM WORDING for {{school_name}}</p>", None, None).unwrap();
        let p = build_prompt(&c, 1, &s).unwrap();
        assert!(p.contains("Issue type(s): Webinar black screen / training navigation problem; Missing school subtype - request information"), "{p}");
        assert!(p.contains("School 1 of 2\nSchool name: Maple Leaf Academy\nNJIIS identifier: 0012345"));
        assert!(p.contains("School 2 of 2\nSchool name: Riverside Preschool\nNJIIS identifier: 0067890"));
        assert!(p.contains("Phone: 609-555-0100 ext. 12"));
        assert!(p.contains("Title: School Nurse"));
        assert!(p.contains("STAFF ACTIONS ALREADY TAKEN\nReset the user's training progress."));
        assert!(p.contains("STAFF NOTES\nCalled on 11/16"));
        assert!(p.contains("APPROVED CUSTOM WORDING for Maple Leaf Academy"));
        assert!(p.contains("Missing school subtype - request information — template"));
        // the original is not repeated in the conversation context; quoted history is omitted
        let context = &p[p.find("RELEVANT CONVERSATION CONTEXT").unwrap()..];
        assert!(!context.contains("The webinar shows a black screen."));
        assert!(context.contains("Still black.\n[Earlier quoted messages omitted]"));
        assert!(!p.contains("Old quoted text"));
        // signature retained in the original email
        let orig = &p[p.find("--- BEGIN ORIGINAL EMAIL ---").unwrap()..p.find("--- END ORIGINAL EMAIL ---").unwrap()];
        assert!(orig.contains("Ann Lee\nSchool Nurse\nPhone: 609-555-0100 x12"));
    }

    #[test]
    fn truncated_body_is_flagged_not_silently_cut() {
        let c = conn();
        let s = Settings::default();
        let mut m = msg(1, "C1", "Long", "First part of a long email", "a@b.org");
        m.body_truncated = true;
        ingest(&c, &[m], &s).unwrap();
        let p = build_prompt(&c, 1, &s).unwrap();
        assert!(p.contains("longer than 500000 characters when imported"));
    }

    #[test]
    fn edited_instructions_are_used() {
        let c = conn();
        let mut s = Settings::default();
        s.prompt_instructions = "Custom instructions.".into();
        ingest(&c, &[msg(1, "C1", "Q", "help", "a@b.org")], &s).unwrap();
        assert!(build_prompt(&c, 1, &s).unwrap().starts_with("Custom instructions.\n\nTICKET\n"));
        s.prompt_instructions = "  ".into();
        assert!(build_prompt(&c, 1, &s).unwrap().starts_with("Please draft"));
    }
}
