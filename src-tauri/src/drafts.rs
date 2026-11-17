//! Email drafts per ticket: default loading, generation (conditional template choice),
//! fill-from-ticket (intact tokens only), save with version history, reset.

use crate::catalog::catalog;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::render::{self, Ctx, Unresolved};
use crate::settings::Settings;
use crate::templates_store::{self as ts, sanitize_html, strip_placeholder_marks, TemplateRow};
use crate::tickets;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize)]
pub struct Draft {
    pub id: i64,
    pub ticket_id: i64,
    pub template_id: Option<String>,
    pub subject: String,
    pub body_html: String,
    pub is_user_edited: bool,
    pub status: String,
    pub outlook_entry_id: Option<String>,
    pub outlook_created_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Editor content proposed by an action (not yet saved).
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub template_id: String,
    pub template_name: String,
    pub subject: String,
    pub body_html: String,
    pub reason: String,
    pub check: DraftCheck,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DraftCheck {
    pub unresolved: Vec<Unresolved>,
    /// Template statements that are not backed by staff-verified ticket values.
    pub unverified_claims: Vec<String>,
    pub sop_status: Option<String>,
    pub sop_reference: Option<String>,
}

fn map_draft(r: &rusqlite::Row) -> rusqlite::Result<Draft> {
    Ok(Draft {
        id: r.get(0)?,
        ticket_id: r.get(1)?,
        template_id: r.get(2)?,
        subject: r.get(3)?,
        body_html: r.get(4)?,
        is_user_edited: r.get::<_, i64>(5)? != 0,
        status: r.get(6)?,
        outlook_entry_id: r.get(7)?,
        outlook_created_at: r.get(8)?,
        version: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
    })
}

const COLS: &str = "id,ticket_id,template_id,subject,body_html,is_user_edited,status,outlook_entry_id,outlook_created_at,version,created_at,updated_at";

pub fn get(conn: &Connection, ticket_id: i64) -> AppResult<Option<Draft>> {
    Ok(conn
        .query_row(&format!("SELECT {COLS} FROM email_drafts WHERE ticket_id=?1"), [ticket_id], map_draft)
        .optional()?)
}

pub fn check(template: Option<&TemplateRow>, subject: &str, body: &str, ctx: &Ctx) -> DraftCheck {
    DraftCheck {
        unresolved: render::unresolved(subject, body),
        unverified_claims: template.map(|t| render::assertion_failures(&t.asserts, ctx)).unwrap_or_default(),
        sop_status: template.map(|t| t.sop_status.clone()),
        sop_reference: template.map(|t| {
            format!("{} (SOP p. {})", t.sop_section.clone().unwrap_or_default(), t.sop_page.clone().unwrap_or_default())
        }),
    }
}

fn template_for(conn: &Connection, ticket_id: i64, template_id: Option<&str>) -> AppResult<TemplateRow> {
    match template_id {
        Some(id) if !id.is_empty() => ts::get(conn, id),
        _ => {
            let t = tickets::get(conn, ticket_id)?;
            let def = catalog()
                .issue(&t.issue_type)
                .map(|i| i.default_template.clone())
                .unwrap_or_else(|| "other.manual_review".into());
            ts::get(conn, &def)
        }
    }
}

/// "Load Default Template": the SOP-based template exactly as stored (placeholders intact).
pub fn load_default(conn: &Connection, ticket_id: i64, template_id: Option<&str>, settings: &Settings) -> AppResult<Proposal> {
    let t = template_for(conn, ticket_id, template_id)?;
    let ctx = tickets::build_ctx(conn, ticket_id, settings)?;
    let subject = render::fill_text(&t.subject_template, &ctx);
    Ok(Proposal {
        check: check(Some(&t), &subject, &t.body_html, &ctx),
        template_id: t.id.clone(),
        template_name: t.name.clone(),
        subject,
        body_html: t.body_html.clone(),
        reason: "Default template loaded".into(),
    })
}

/// "Reset to Default": the template's original system default (ignoring local template customizations).
pub fn reset_default(conn: &Connection, ticket_id: i64, template_id: Option<&str>, settings: &Settings) -> AppResult<Proposal> {
    let t = template_for(conn, ticket_id, template_id)?;
    let ctx = tickets::build_ctx(conn, ticket_id, settings)?;
    let body = t.default_body_html.clone().unwrap_or_else(|| t.body_html.clone());
    let subject = render::fill_text(t.default_subject.as_deref().unwrap_or(&t.subject_template), &ctx);
    Ok(Proposal {
        check: check(Some(&t), &subject, &body, &ctx),
        template_id: t.id.clone(),
        template_name: t.name.clone(),
        subject,
        body_html: body,
        reason: "Reset to system default".into(),
    })
}

/// "Generate Email": choose the template variant from the ticket state and fill it.
pub fn generate(conn: &Connection, ticket_id: i64, settings: &Settings) -> AppResult<Proposal> {
    let ticket = tickets::get(conn, ticket_id)?;
    let ctx = tickets::build_ctx(conn, ticket_id, settings)?;
    let candidates = ts::selectable(conn, &ticket.issue_type)?;
    let chosen = render::select_template(&candidates, &ctx)
        .map(|s| s.id.clone())
        .or_else(|| catalog().issue(&ticket.issue_type).map(|i| i.default_template.clone()))
        .unwrap_or_else(|| "other.manual_review".into());
    let t = ts::get(conn, &chosen)?;
    let subject = render::fill_text(&t.subject_template, &ctx);
    let body = render::fill(&t.body_html, &ctx);
    let reason = if t.rules.is_empty() {
        format!("Selected \"{}\" (default for this issue state)", t.name)
    } else {
        format!(
            "Selected \"{}\" because {}",
            t.name,
            t.rules
                .iter()
                .map(|c| format!("{} {} {}", c.field, c.op, c.value.clone().or_else(|| c.values.as_ref().map(|v| v.join("/"))).unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(" and ")
        )
    };
    Ok(Proposal { check: check(Some(&t), &subject, &body, &ctx), template_id: t.id.clone(), template_name: t.name.clone(), subject, body_html: body, reason })
}

/// "Fill from Ticket": replace only intact recognized tokens in the given (possibly edited) text.
pub fn fill_from_ticket(conn: &Connection, ticket_id: i64, template_id: Option<&str>, subject: &str, body_html: &str, settings: &Settings) -> AppResult<Proposal> {
    let ctx = tickets::build_ctx(conn, ticket_id, settings)?;
    let body = render::fill(&strip_placeholder_marks(body_html), &ctx);
    let subject = render::fill_text(subject, &ctx);
    let t = match template_id {
        Some(id) if !id.is_empty() => ts::get(conn, id).ok(),
        _ => None,
    };
    Ok(Proposal {
        check: check(t.as_ref(), &subject, &body, &ctx),
        template_id: t.as_ref().map(|t| t.id.clone()).unwrap_or_default(),
        template_name: t.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
        subject,
        body_html: body,
        reason: "Filled recognized placeholders from ticket".into(),
    })
}

/// Save the editor content. The previous content is kept in the version history.
pub fn save(conn: &Connection, ticket_id: i64, template_id: Option<&str>, subject: &str, body_html: &str, user_edited: bool, reason: &str, actor: &str) -> AppResult<Draft> {
    if subject.len() > 1000 || body_html.len() > 2_000_000 {
        return Err(AppError::coded("INVALID", "Draft is too large"));
    }
    let body = sanitize_html(&strip_placeholder_marks(body_html));
    let tx = conn.unchecked_transaction()?;
    tickets::get(&tx, ticket_id)?;
    let ts_ = now();
    let draft = match get(&tx, ticket_id)? {
        None => {
            tx.execute(
                "INSERT INTO email_drafts(ticket_id,template_id,subject,body_html,is_user_edited,status,version,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'Draft',1,?6,?6)",
                params![ticket_id, template_id, subject, body, user_edited as i64, ts_],
            )?;
            tickets::history(&tx, ticket_id, actor, "Email draft saved", None, None, template_id)?;
            get(&tx, ticket_id)?.unwrap()
        }
        Some(cur) => {
            if cur.subject == subject && cur.body_html == body && cur.template_id.as_deref() == template_id {
                return Ok(cur);
            }
            tx.execute(
                "INSERT INTO email_draft_versions(draft_id,version,template_id,subject,body_html,reason,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![cur.id, cur.version, cur.template_id, cur.subject, cur.body_html, reason, ts_],
            )?;
            tx.execute(
                "UPDATE email_drafts SET template_id=?2, subject=?3, body_html=?4, is_user_edited=?5, version=version+1,
                   status=CASE WHEN status='Outlook Draft Created' THEN 'Edited After Outlook Draft' ELSE status END, updated_at=?6 WHERE id=?1",
                params![cur.id, template_id, subject, body, user_edited as i64, ts_],
            )?;
            tickets::history(&tx, ticket_id, actor, "Email draft saved", Some("draft_version"), Some(&cur.version.to_string()), Some(&(cur.version + 1).to_string()))?;
            get(&tx, ticket_id)?.unwrap()
        }
    };
    tx.execute("UPDATE tickets SET updated_at=?2 WHERE id=?1", params![ticket_id, ts_])?;
    tx.commit()?;
    Ok(draft)
}

#[derive(Debug, Serialize)]
pub struct DraftVersion {
    pub id: i64,
    pub version: i64,
    pub template_id: Option<String>,
    pub subject: String,
    pub body_html: String,
    pub reason: Option<String>,
    pub created_at: String,
}

pub fn versions(conn: &Connection, ticket_id: i64) -> AppResult<Vec<DraftVersion>> {
    let mut st = conn.prepare(
        "SELECT v.id,v.version,v.template_id,v.subject,v.body_html,v.reason,v.created_at FROM email_draft_versions v
         JOIN email_drafts d ON d.id=v.draft_id WHERE d.ticket_id=?1 ORDER BY v.id DESC LIMIT 200",
    )?;
    let v = st
        .query_map([ticket_id], |r| {
            Ok(DraftVersion { id: r.get(0)?, version: r.get(1)?, template_id: r.get(2)?, subject: r.get(3)?, body_html: r.get(4)?, reason: r.get(5)?, created_at: r.get(6)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn mark_outlook_created(conn: &Connection, ticket_id: i64, entry_id: &str, incomplete: bool, actor: &str) -> AppResult<()> {
    let ts_ = now();
    conn.execute(
        "UPDATE email_drafts SET status='Outlook Draft Created', outlook_entry_id=?2, outlook_created_at=?3 WHERE ticket_id=?1",
        params![ticket_id, entry_id, ts_],
    )?;
    conn.execute("UPDATE tickets SET last_response_at=?2, updated_at=?2 WHERE id=?1", params![ticket_id, ts_])?;
    tickets::history(
        conn,
        ticket_id,
        actor,
        if incomplete { "Outlook draft created (INCOMPLETE - staff confirmed)" } else { "Outlook draft created (not sent)" },
        None,
        None,
        None,
    )?;
    Ok(())
}

static IMG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"src="sop-images/([A-Za-z0-9_\-]+\.png)""#).unwrap());

/// Build the HTML sent to Outlook: SOP images become inline CID attachments;
/// incomplete drafts get visibly highlighted placeholders.
pub fn outlook_html(body: &str, incomplete: bool, images_dir: &std::path::Path) -> (String, Vec<serde_json::Value>) {
    let mut images = Vec::new();
    let mut seen = Vec::new();
    for c in IMG_RE.captures_iter(body) {
        let name = c[1].to_string();
        if !seen.contains(&name) {
            images.push(serde_json::json!({"path": images_dir.join(&name).to_string_lossy(), "cid": name}));
            seen.push(name);
        }
    }
    let mut html = IMG_RE.replace_all(body, r#"src="cid:$1""#).into_owned();
    if incomplete {
        html = render::TOKEN_RE
            .replace_all(&html, r#"<span style="background:#ffeb3b;color:#000">$0</span>"#)
            .into_owned();
    }
    (format!(r#"<div style="font-family:Calibri,Arial,sans-serif;font-size:11pt">{html}</div>"#), images)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::tests::{conn, msg};
    use crate::tickets::{ingest, set_field, update, TicketPatch};

    fn ticket(c: &Connection, issue: &str) -> i64 {
        ingest(c, &[msg(1, "C1", "Question", "Hello, a question.", "a@b.org")], &Settings::default()).unwrap();
        update(c, 1, &TicketPatch { issue_type: Some(issue.into()), ..Default::default() }, "staff").unwrap();
        1
    }

    #[test]
    fn every_issue_default_loads_with_blank_ticket() {
        let c = conn();
        let s = Settings::default();
        let id = ticket(&c, "other");
        for it in &catalog().issue_types {
            update(&c, id, &TicketPatch { issue_type: Some(it.id.clone()), ..Default::default() }, "staff").unwrap();
            let p = load_default(&c, id, None, &s).unwrap();
            assert_eq!(p.template_id, it.default_template);
            assert!(p.body_html.len() > 50);
            let g = generate(&c, id, &s).unwrap();
            assert!(!g.body_html.is_empty(), "generate failed for {}", it.id);
        }
    }

    #[test]
    fn subtype_conditional_branches() {
        let c = conn();
        let s = Settings::default();
        let id = ticket(&c, "school_subtype_missing");
        assert_eq!(generate(&c, id, &s).unwrap().template_id, "school_subtype_missing.request_subtype");
        set_field(&c, id, "school_type", Some("Charter".into()), false, "staff").unwrap();
        assert_eq!(generate(&c, id, &s).unwrap().template_id, "school_subtype_missing.subtype_received_pending");
        set_field(&c, id, "subtype_updated", Some("Yes".into()), true, "staff").unwrap();
        let g = generate(&c, id, &s).unwrap();
        assert_eq!(g.template_id, "school_subtype_missing.confirm_subtype_updated");
        assert!(g.check.unverified_claims.is_empty());
        assert!(g.body_html.contains("Charter"));
    }

    #[test]
    fn submission_verification_never_claims_receipt_without_staff_verification() {
        let c = conn();
        let s = Settings::default();
        let id = ticket(&c, "submission_confirm_receipt");
        let g = generate(&c, id, &s).unwrap();
        assert_eq!(g.template_id, "submission_confirm_receipt.verification_pending");
        assert!(!g.body_html.contains("I can confirm"));
        // loading the confirmation template manually while unverified raises a warning
        let p = load_default(&c, id, Some("submission_confirm_receipt.submission_confirmed"), &s).unwrap();
        assert!(!p.check.unverified_claims.is_empty());
        set_field(&c, id, "submission_verification_status", Some("Verified Submitted".into()), true, "staff").unwrap();
        set_field(&c, id, "submitted_grades", Some(r#"["Kindergarten","Grade 1"]"#.into()), true, "staff").unwrap();
        let g = generate(&c, id, &s).unwrap();
        assert_eq!(g.template_id, "submission_confirm_receipt.submission_confirmed");
        assert!(g.body_html.contains("Kindergarten, Grade 1"));
        assert!(g.check.unverified_claims.is_empty());
        set_field(&c, id, "submission_verification_status", Some("Verified Not Submitted".into()), true, "staff").unwrap();
        assert_eq!(generate(&c, id, &s).unwrap().template_id, "submission_confirm_receipt.submission_not_found");
    }

    #[test]
    fn new_user_requests_only_missing_details_and_lhd_branches() {
        let c = conn();
        let s = Settings::default();
        let id = ticket(&c, "new_user_account");
        set_field(&c, id, "first_name", Some("Maria".into()), false, "staff").unwrap();
        let g = generate(&c, id, &s).unwrap();
        assert_eq!(g.template_id, "new_user_account.request_information_with_training");
        assert!(!g.body_html.contains("First Name:"));
        assert!(g.body_html.contains("Last Name:"));
        set_field(&c, id, "asr_training_completed", Some("Yes".into()), false, "staff").unwrap();
        assert_eq!(generate(&c, id, &s).unwrap().template_id, "new_user_account.request_information");
        set_field(&c, id, "account_created", Some("Yes".into()), true, "staff").unwrap();
        assert_eq!(generate(&c, id, &s).unwrap().template_id, "new_user_account.account_created");

        update(&c, id, &TicketPatch { issue_type: Some("lhd_access_denied".into()), ..Default::default() }, "staff").unwrap();
        assert_eq!(generate(&c, id, &s).unwrap().template_id, "lhd_access_denied.change_current_facility");
        set_field(&c, id, "correct_facility_selected", Some("Yes".into()), false, "staff").unwrap();
        set_field(&c, id, "access_still_denied", Some("Yes".into()), false, "staff").unwrap();
        assert_eq!(generate(&c, id, &s).unwrap().template_id, "lhd_access_denied.access_followup");
    }

    #[test]
    fn missing_values_stay_visible_and_fill_preserves_edits() {
        let c = conn();
        let s = Settings::default();
        let id = ticket(&c, "training_completed_access");
        let p = load_default(&c, id, Some("training_completed_access.access_activated"), &s).unwrap();
        assert!(p.body_html.contains("{{school_name}}"));
        assert!(p.check.unresolved.iter().any(|u| u.key == "school_name"));
        assert!(!p.check.unverified_claims.is_empty(), "activation not verified");

        let edited = p.body_html.replace("If you are still having problems", "MY OWN SENTENCE. If you are still having problems");
        save(&c, id, Some(&p.template_id), &p.subject, &edited, true, "edit", "staff").unwrap();
        set_field(&c, id, "school_name", Some("Oak Elementary School".into()), false, "staff").unwrap();
        let d = get(&c, id).unwrap().unwrap();
        assert!(d.body_html.contains("MY OWN SENTENCE"), "saved draft untouched by field change");
        assert!(d.body_html.contains("{{school_name}}"));
        let f = fill_from_ticket(&c, id, d.template_id.as_deref(), &d.subject, &d.body_html, &s).unwrap();
        assert!(f.body_html.contains("MY OWN SENTENCE"));
        assert!(f.body_html.contains("Oak Elementary School"));
        assert!(!f.body_html.contains("{{school_name}}"));
    }

    #[test]
    fn save_versions_reset_and_persistence_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let s = Settings::default();
        {
            let c = crate::db::open(&path).unwrap();
            ts::seed_embedded(&c).unwrap();
            ingest(&c, &[msg(1, "C1", "Webinar", "black screen", "a@b.org")], &s).unwrap();
            let g = generate(&c, 1, &s).unwrap();
            save(&c, 1, Some(&g.template_id), &g.subject, &g.body_html, false, "generated", "staff").unwrap();
            save(&c, 1, Some(&g.template_id), &g.subject, "<p>Edited text</p>", true, "edit", "staff").unwrap();
            assert_eq!(versions(&c, 1).unwrap().len(), 1);
        }
        let c = crate::db::open(&path).unwrap();
        ts::seed_embedded(&c).unwrap();
        let d = get(&c, 1).unwrap().unwrap();
        assert_eq!(d.body_html, "<p>Edited text</p>");
        assert!(d.is_user_edited);
        assert_eq!(d.version, 2);
        let r = reset_default(&c, 1, d.template_id.as_deref(), &s).unwrap();
        assert!(r.body_html.contains("clear your cache"));
        // ticket data untouched by reset
        assert!(crate::tickets::get(&c, 1).is_ok());
    }
    #[test]
    fn outlook_html_uses_cid_images_and_highlights_incomplete() {
        let dir = std::path::Path::new("C:/x");
        let (h, imgs) = outlook_html(r#"<p>Hi {{school_name}}</p><img src="sop-images/image3.png"><img src="sop-images/image3.png">"#, true, dir);
        assert!(h.contains(r#"src="cid:image3.png""#));
        assert_eq!(imgs.len(), 1);
        assert!(h.contains("background:#ffeb3b"));
        let (h2, _) = outlook_html("<p>Hi {{school_name}}</p>", false, dir);
        assert!(!h2.contains("ffeb3b"));
    }
}
