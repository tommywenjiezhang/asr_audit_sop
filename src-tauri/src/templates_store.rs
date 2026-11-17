//! Email template persistence: first-run seeding, upgrade-safe system updates,
//! editing with version history, duplicate/enable/restore.

use crate::catalog::{Catalog, Condition};
use crate::db::{get_setting, now, set_setting};
use crate::error::{AppError, AppResult};
use crate::render::SelectableTemplate;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Sanitize user/template HTML. Keeps formatting, links, images and placeholder tokens.
pub fn sanitize_html(html: &str) -> String {
    let mut b = ammonia::Builder::default();
    b.add_tags(["mark", "font", "center"])
        .add_tag_attributes("ol", ["type", "start"])
        .add_tag_attributes("mark", ["class", "data-ph"])
        .add_tag_attributes("span", ["class"])
        .add_tag_attributes("p", ["align"])
        .add_tag_attributes("td", ["colspan", "rowspan", "align", "valign"])
        .add_url_schemes(["cid"])
        .link_rel(Some("noopener noreferrer"));
    b.clean(html).to_string()
}

/// Remove editor-only highlight wrappers `<mark class="ph" ...>{{x}}</mark>` -> `{{x}}`.
pub fn strip_placeholder_marks(html: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"<mark class="ph"[^>]*>(\{\{\s*[A-Za-z0-9_]+\s*\}\})</mark>"#).unwrap()
    });
    RE.replace_all(html, "$1").into_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateRow {
    pub id: String,
    pub issue_type: String,
    pub name: String,
    pub description: Option<String>,
    pub sop_section: Option<String>,
    pub sop_page: Option<String>,
    pub sop_status: String,
    pub version: i64,
    pub system_version: Option<i64>,
    pub priority: i64,
    pub required_fields: Vec<String>,
    pub optional_fields: Vec<String>,
    pub rules: Vec<Condition>,
    pub asserts: Vec<Condition>,
    pub subject_template: String,
    pub body_html: String,
    pub default_subject: Option<String>,
    pub default_body_html: Option<String>,
    pub is_system: bool,
    pub is_customized: bool,
    pub system_update_available: bool,
    pub enabled: bool,
    pub created_at: String,
    pub last_modified: String,
}

const COLS: &str = "id,issue_type,name,description,sop_section,sop_page,sop_status,version,system_version,priority,required_fields,optional_fields,rules_json,asserts_json,subject_template,body_html,default_subject,default_body_html,is_system,is_customized,system_update_available,enabled,created_at,last_modified";

fn json_vec<T: serde::de::DeserializeOwned>(s: String) -> Vec<T> {
    serde_json::from_str(&s).unwrap_or_default()
}

fn map_row(r: &Row) -> rusqlite::Result<TemplateRow> {
    Ok(TemplateRow {
        id: r.get(0)?,
        issue_type: r.get(1)?,
        name: r.get(2)?,
        description: r.get(3)?,
        sop_section: r.get(4)?,
        sop_page: r.get(5)?,
        sop_status: r.get(6)?,
        version: r.get(7)?,
        system_version: r.get(8)?,
        priority: r.get(9)?,
        required_fields: json_vec(r.get(10)?),
        optional_fields: json_vec(r.get(11)?),
        rules: json_vec(r.get(12)?),
        asserts: json_vec(r.get(13)?),
        subject_template: r.get(14)?,
        body_html: r.get(15)?,
        default_subject: r.get(16)?,
        default_body_html: r.get(17)?,
        is_system: r.get::<_, i64>(18)? != 0,
        is_customized: r.get::<_, i64>(19)? != 0,
        system_update_available: r.get::<_, i64>(20)? != 0,
        enabled: r.get::<_, i64>(21)? != 0,
        created_at: r.get(22)?,
        last_modified: r.get(23)?,
    })
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct SeedReport {
    pub issue_types: usize,
    pub inserted: usize,
    pub updated: usize,
    pub preserved_customizations: usize,
}

/// Seed / upgrade issue definitions and system templates from the catalog.
/// * New templates are inserted.
/// * Newer system versions replace uncustomized templates (old body kept in version history).
/// * Customized templates are never overwritten; only their stored system default is refreshed
///   and `system_update_available` is flagged.
pub fn seed(conn: &Connection, cat: &Catalog, body_of: &dyn Fn(&str) -> Option<String>) -> AppResult<SeedReport> {
    let tx = conn.unchecked_transaction()?;
    let ts = now();
    let mut rep = SeedReport::default();
    for it in &cat.issue_types {
        tx.execute(
            "INSERT INTO issue_definitions(id,name,description,sop_section,sop_page,fields_json,keywords_json,internal_steps,default_template_id,sort_order,enabled,catalog_version)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, description=excluded.description, sop_section=excluded.sop_section,
               sop_page=excluded.sop_page, fields_json=excluded.fields_json, keywords_json=excluded.keywords_json,
               internal_steps=excluded.internal_steps, default_template_id=excluded.default_template_id,
               sort_order=excluded.sort_order, catalog_version=excluded.catalog_version",
            params![
                it.id, it.name, it.description, it.sop_section, it.sop_page,
                serde_json::to_string(&it.fields)?, serde_json::to_string(&it.keywords)?,
                it.internal_steps, it.default_template, it.sort, cat.catalog_version
            ],
        )?;
        rep.issue_types += 1;
    }
    for t in &cat.templates {
        let body = body_of(&t.file)
            .ok_or_else(|| AppError::msg(format!("template file missing: {}", t.file)))?;
        let body = sanitize_html(body.trim());
        let existing: Option<(i64, i64, i64, String, String)> = tx
            .query_row(
                "SELECT COALESCE(system_version,0), is_customized, version, subject_template, body_html FROM email_templates WHERE id=?1",
                [&t.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let rules = serde_json::to_string(&t.when)?;
        let asserts = serde_json::to_string(&t.asserts)?;
        let req = serde_json::to_string(&t.required_fields)?;
        let opt = serde_json::to_string(&t.optional_fields)?;
        match existing {
            None => {
                tx.execute(
                    &format!("INSERT INTO email_templates({COLS}) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9,?10,?11,?12,?13,?14,?15,?14,?15,1,0,0,1,?16,?16)"),
                    params![t.id, t.issue_type, t.name, t.description, t.sop_section, t.sop_page, t.sop_status,
                        t.version, t.priority, req, opt, rules, asserts, t.subject, body, ts],
                )?;
                rep.inserted += 1;
            }
            Some((sys_ver, customized, ver, old_subj, old_body)) if sys_ver < t.version => {
                if customized == 0 {
                    tx.execute(
                        "INSERT INTO email_template_versions(template_id,version,subject_template,body_html,note,saved_at) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![t.id, ver, old_subj, old_body, format!("Before system update to v{}", t.version), ts],
                    )?;
                    tx.execute(
                        "UPDATE email_templates SET name=?2, description=?3, sop_section=?4, sop_page=?5, sop_status=?6, system_version=?7,
                           priority=?8, required_fields=?9, optional_fields=?10, rules_json=?11, asserts_json=?12,
                           subject_template=?13, body_html=?14, default_subject=?13, default_body_html=?14,
                           version=version+1, system_update_available=0, last_modified=?15 WHERE id=?1",
                        params![t.id, t.name, t.description, t.sop_section, t.sop_page, t.sop_status, t.version,
                            t.priority, req, opt, rules, asserts, t.subject, body, ts],
                    )?;
                    rep.updated += 1;
                } else {
                    tx.execute(
                        "UPDATE email_templates SET sop_section=?2, sop_page=?3, sop_status=?4, system_version=?5,
                           default_subject=?6, default_body_html=?7, system_update_available=1 WHERE id=?1",
                        params![t.id, t.sop_section, t.sop_page, t.sop_status, t.version, t.subject, body],
                    )?;
                    rep.preserved_customizations += 1;
                }
            }
            Some(_) => {}
        }
    }
    set_setting(&tx, "catalog_version", &cat.catalog_version.to_string())?;
    tx.commit()?;
    Ok(rep)
}

pub fn seed_embedded(conn: &Connection) -> AppResult<SeedReport> {
    let cat = crate::catalog::catalog();
    let stored: i64 = get_setting(conn, "catalog_version")?.and_then(|s| s.parse().ok()).unwrap_or(0);
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM email_templates WHERE is_system=1", [], |r| r.get(0))?;
    if stored >= cat.catalog_version && count as usize >= cat.templates.len() {
        return Ok(SeedReport::default());
    }
    let rep = seed(conn, cat, &|f| crate::catalog::template_body(f).map(|s| s.to_string()))?;
    tracing::info!(?rep, "template catalog seeded");
    Ok(rep)
}

pub fn list(conn: &Connection, issue_type: Option<&str>) -> AppResult<Vec<TemplateRow>> {
    let sql = format!(
        "SELECT {COLS} FROM email_templates t WHERE (?1 IS NULL OR issue_type=?1)
         ORDER BY (SELECT sort_order FROM issue_definitions d WHERE d.id=t.issue_type), priority, name"
    );
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map([issue_type], map_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: &str) -> AppResult<TemplateRow> {
    conn.query_row(&format!("SELECT {COLS} FROM email_templates WHERE id=?1"), [id], map_row)
        .optional()?
        .ok_or_else(|| AppError::coded("NOT_FOUND", format!("Template {id} not found")))
}

pub fn selectable(conn: &Connection, issue_type: &str) -> AppResult<Vec<SelectableTemplate>> {
    Ok(list(conn, Some(issue_type))?
        .into_iter()
        .map(|t| SelectableTemplate { id: t.id, priority: t.priority, enabled: t.enabled, when: t.rules })
        .collect())
}

fn push_version(conn: &Connection, t: &TemplateRow, note: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO email_template_versions(template_id,version,subject_template,body_html,note,saved_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![t.id, t.version, t.subject_template, t.body_html, note, now()],
    )?;
    Ok(())
}

pub fn save(conn: &Connection, id: &str, subject: &str, body_html: &str, name: Option<&str>, note: Option<&str>) -> AppResult<TemplateRow> {
    let tx = conn.unchecked_transaction()?;
    let cur = get(&tx, id)?;
    let body = sanitize_html(&strip_placeholder_marks(body_html));
    if body.trim().is_empty() {
        return Err(AppError::coded("INVALID", "Template body cannot be empty"));
    }
    if body == cur.body_html && subject == cur.subject_template && name.map(|n| n == cur.name).unwrap_or(true) {
        return Ok(cur);
    }
    push_version(&tx, &cur, note.unwrap_or("Edited"))?;
    let customized = cur.is_system
        && (Some(body.as_str()) != cur.default_body_html.as_deref().map(str::trim)
            || Some(subject) != cur.default_subject.as_deref());
    tx.execute(
        "UPDATE email_templates SET subject_template=?2, body_html=?3, name=COALESCE(?4,name), version=version+1,
           is_customized=?5, last_modified=?6 WHERE id=?1",
        params![id, subject, body, name, customized as i64, now()],
    )?;
    tx.commit()?;
    get(conn, id)
}

pub fn duplicate(conn: &Connection, id: &str, new_name: &str) -> AppResult<TemplateRow> {
    let src = get(conn, id)?;
    let ts = now();
    let suffix = chrono::Utc::now().timestamp_millis();
    let new_id = format!("custom.{}.{}", src.issue_type, suffix);
    let never = serde_json::to_string(&vec![Condition {
        field: String::new(), op: "never".into(), value: None, values: None, fields: None, label: None,
    }])?;
    conn.execute(
        &format!("INSERT INTO email_templates({COLS}) VALUES(?1,?2,?3,?4,?5,?6,?7,1,NULL,?8,?9,?10,?11,?12,?13,?14,NULL,NULL,0,0,0,1,?15,?15)"),
        params![new_id, src.issue_type, new_name, src.description, src.sop_section, src.sop_page,
            if src.sop_status == "verbatim" { "adapted".to_string() } else { src.sop_status.clone() },
            src.priority + 50, serde_json::to_string(&src.required_fields)?, serde_json::to_string(&src.optional_fields)?,
            never, serde_json::to_string(&src.asserts)?, src.subject_template, src.body_html, ts],
    )?;
    get(conn, &new_id)
}

pub fn set_enabled(conn: &Connection, id: &str, enabled: bool) -> AppResult<TemplateRow> {
    let n = conn.execute(
        "UPDATE email_templates SET enabled=?2, last_modified=?3 WHERE id=?1",
        params![id, enabled as i64, now()],
    )?;
    if n == 0 {
        return Err(AppError::coded("NOT_FOUND", "Template not found"));
    }
    get(conn, id)
}

pub fn restore_default(conn: &Connection, id: &str) -> AppResult<TemplateRow> {
    let tx = conn.unchecked_transaction()?;
    let cur = get(&tx, id)?;
    let (Some(ds), Some(db)) = (cur.default_subject.clone(), cur.default_body_html.clone()) else {
        return Err(AppError::coded("NO_DEFAULT", "This template has no system default (it is a custom copy)."));
    };
    push_version(&tx, &cur, "Before restore to system default")?;
    tx.execute(
        "UPDATE email_templates SET subject_template=?2, body_html=?3, version=version+1, is_customized=0,
           system_update_available=0, last_modified=?4 WHERE id=?1",
        params![id, ds, db, now()],
    )?;
    tx.commit()?;
    get(conn, id)
}

#[derive(Debug, Serialize)]
pub struct TemplateVersion {
    pub id: i64,
    pub version: i64,
    pub subject_template: String,
    pub body_html: String,
    pub note: Option<String>,
    pub saved_at: String,
}

pub fn versions(conn: &Connection, id: &str) -> AppResult<Vec<TemplateVersion>> {
    let mut st = conn.prepare(
        "SELECT id,version,subject_template,body_html,note,saved_at FROM email_template_versions WHERE template_id=?1 ORDER BY id DESC",
    )?;
    let v = st
        .query_map([id], |r| {
            Ok(TemplateVersion { id: r.get(0)?, version: r.get(1)?, subject_template: r.get(2)?, body_html: r.get(3)?, note: r.get(4)?, saved_at: r.get(5)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;
    use crate::db::open_in_memory;

    fn seeded() -> Connection {
        let c = open_in_memory().unwrap();
        seed_embedded(&c).unwrap();
        c
    }

    #[test]
    fn every_issue_type_has_enabled_default_template() {
        let c = seeded();
        let cat = catalog();
        for it in &cat.issue_types {
            let ts = list(&c, Some(&it.id)).unwrap();
            assert!(!ts.is_empty(), "no templates for {}", it.id);
            let def = ts.iter().find(|t| t.id == it.default_template)
                .unwrap_or_else(|| panic!("default template {} missing for {}", it.default_template, it.id));
            assert!(def.enabled);
            assert!(def.body_html.len() > 80, "template {} too short", def.id);
        }
    }

    #[test]
    fn required_issue_types_present() {
        let cat = catalog();
        assert!(cat.issue_types.len() >= 18);
        for id in ["school_enrollment", "school_subtype_missing", "school_subtype_updated", "asr_training", "lhd_training",
            "new_user_account", "new_user_created", "training_completed_access", "school_forms_tab", "lhd_access_denied",
            "webinar_issue", "save_form_pdf", "general_asr_process", "submission_confirm_receipt", "submission_not_submitted",
            "grade_submission_problem", "user_guide", "other"] {
            assert!(cat.issue(id).is_some(), "missing issue type {id}");
        }
    }

    #[test]
    fn all_template_files_exist_with_content_and_sop_reference() {
        let cat = catalog();
        for t in &cat.templates {
            let body = crate::catalog::template_body(&t.file).unwrap_or_else(|| panic!("missing file {}", t.file));
            assert!(body.contains("<p>"), "{} has no paragraphs", t.file);
            assert!(body.split_whitespace().count() > 15, "{} is not a usable email", t.file);
            assert!(cat.issue(&t.issue_type).is_some(), "{} has unknown issue type", t.id);
            assert!(["verbatim", "adapted", "review_required"].contains(&t.sop_status.as_str()));
            if t.sop_status != "review_required" {
                assert!(!t.sop_page.is_empty() && !t.sop_section.is_empty(), "{} lacks SOP reference", t.id);
            }
            for tok in crate::render::tokens(body) {
                assert!(cat.placeholder(&tok).is_some(), "{} uses undocumented placeholder {tok}", t.id);
            }
            for img in regex::Regex::new(r#"src="sop-images/([^"]+)""#).unwrap().captures_iter(body) {
                assert!(crate::catalog::sop_images().iter().any(|(n, _)| *n == &img[1]), "missing image {}", &img[1]);
            }
        }
    }

    #[test]
    fn multi_state_issue_types_have_multiple_templates() {
        let c = seeded();
        for id in ["school_subtype_missing", "asr_training", "new_user_account", "lhd_access_denied",
            "submission_confirm_receipt", "webinar_issue"] {
            assert!(list(&c, Some(id)).unwrap().len() >= 2, "{id} needs multiple templates");
        }
    }

    #[test]
    fn edit_versioning_restore_and_upgrade_preserves_customization() {
        let c = seeded();
        let id = "user_guide.user_guide";
        let orig = get(&c, id).unwrap();
        let edited = save(&c, id, "Custom subject", "<p>My custom text {{school_name}}</p>", None, Some("test")).unwrap();
        assert!(edited.is_customized);
        assert_eq!(edited.version, orig.version + 1);
        assert_eq!(versions(&c, id).unwrap().len(), 1);

        // simulate an application upgrade with a newer system version of every template
        let mut cat = catalog().clone();
        cat.catalog_version += 1;
        for t in cat.templates.iter_mut() { t.version += 1; }
        let rep = seed(&c, &cat, &|f| crate::catalog::template_body(f).map(|s| format!("{s}<p>v2 addition</p>"))).unwrap();
        assert!(rep.preserved_customizations >= 1);
        let after = get(&c, id).unwrap();
        assert_eq!(after.body_html, "<p>My custom text {{school_name}}</p>");
        assert!(after.system_update_available);
        assert!(after.default_body_html.unwrap().contains("v2 addition"));
        // uncustomized template was upgraded
        let other = get(&c, "save_form_pdf.save_as_pdf").unwrap();
        assert!(other.body_html.contains("v2 addition"));

        let restored = restore_default(&c, id).unwrap();
        assert!(!restored.is_customized);
        assert!(restored.body_html.contains("v2 addition"));
    }

    #[test]
    fn duplicate_and_disable() {
        let c = seeded();
        let d = duplicate(&c, "webinar_issue.clear_cache", "My copy").unwrap();
        assert!(!d.is_system);
        assert_eq!(d.rules[0].op, "never");
        let off = set_enabled(&c, &d.id, false).unwrap();
        assert!(!off.enabled);
        assert!(restore_default(&c, &d.id).is_err());
    }

    #[test]
    fn sanitizer_strips_scripts_keeps_tokens_and_images() {
        let s = sanitize_html(r#"<p onclick="x()">Hi {{school_name}}<script>alert(1)</script><img src="sop-images/image3.png" onerror="y"></p><a href="javascript:alert(1)">x</a>"#);
        assert!(!s.contains("script"));
        assert!(!s.contains("onclick"));
        assert!(!s.contains("onerror"));
        assert!(!s.contains("javascript"));
        assert!(s.contains("{{school_name}}"));
        assert!(s.contains(r#"src="sop-images/image3.png""#));
        assert_eq!(strip_placeholder_marks(r#"a <mark class="ph" data-ph="x">{{x}}</mark> b"#), "a {{x}} b");
    }
}
