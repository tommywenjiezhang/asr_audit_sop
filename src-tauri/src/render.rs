//! Placeholder engine, conditional rules and template selection.
//!
//! Principles:
//! * Missing values never remove text: the `{{token}}` stays visible.
//! * Only intact, recognized tokens are replaced, so manual edits around them survive.
//! * `verified_only` placeholders only take staff-entered values, never extracted guesses.
//! * Template `asserts` are checked against staff-entered values only; failures produce warnings.

use crate::catalog::{catalog, Condition};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::LazyLock;

pub static TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*([A-Za-z0-9_]+)\s*\}\}").unwrap());

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldVal {
    pub value: String,
    /// true when the value was entered or verified by staff (or comes from Outlook metadata / settings).
    pub trusted: bool,
}

pub type Ctx = HashMap<String, FieldVal>;

pub fn put(ctx: &mut Ctx, key: &str, value: impl Into<String>, trusted: bool) {
    ctx.insert(key.to_string(), FieldVal { value: value.into(), trusted });
}

/// Fields listed in the SOP "create your account" request, in SOP order.
pub const ACCOUNT_DETAILS: &[(&str, &str)] = &[
    ("first_name", "First Name"),
    ("last_name", "Last Name"),
    ("user_email", "Email"),
    ("work_phone", "Work Phone"),
    ("school_name", "Name of the school"),
    ("training_date", "Date of ASR training"),
];

pub fn html_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// Values for multi-select fields are stored as JSON arrays.
pub fn display_value(raw: &str) -> String {
    let t = raw.trim();
    if t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(t) {
            return v.join(", ");
        }
    }
    t.to_string()
}

fn is_blank(raw: &str) -> bool {
    let t = raw.trim();
    t.is_empty() || t == "[]"
}

/// Value used for rules. With `trusted_only`, extracted (unverified) values count as empty.
fn rule_value(ctx: &Ctx, field: &str, trusted_only: bool) -> String {
    match ctx.get(field) {
        Some(v) if !trusted_only || v.trusted => v.value.trim().to_string(),
        _ => String::new(),
    }
}

pub fn eval_cond(c: &Condition, ctx: &Ctx, trusted_only: bool) -> bool {
    let v = rule_value(ctx, &c.field, trusted_only);
    match c.op.as_str() {
        "eq" => v == c.value.clone().unwrap_or_default(),
        "ne" => v != c.value.clone().unwrap_or_default(),
        "in" => c.values.as_ref().map(|vs| vs.iter().any(|x| *x == v)).unwrap_or(false),
        "not_in" => c.values.as_ref().map(|vs| !vs.iter().any(|x| *x == v)).unwrap_or(true),
        "empty" => is_blank(&v),
        "not_empty" => !is_blank(&v),
        "any_empty" => c
            .fields
            .as_ref()
            .map(|fs| fs.iter().any(|f| is_blank(&rule_value(ctx, f, trusted_only))))
            .unwrap_or(false),
        "never" => false,
        "always" => true,
        _ => false,
    }
}

pub fn eval_all(conds: &[Condition], ctx: &Ctx, trusted_only: bool) -> bool {
    conds.iter().all(|c| eval_cond(c, ctx, trusted_only))
}

/// Labels of `asserts` conditions that are not satisfied by staff-entered values.
pub fn assertion_failures(asserts: &[Condition], ctx: &Ctx) -> Vec<String> {
    asserts
        .iter()
        .filter(|c| !eval_cond(c, ctx, true))
        .map(|c| c.label.clone().unwrap_or_else(|| format!("{} {}", c.field, c.op)))
        .collect()
}

/// Minimal template info needed for selection.
#[derive(Debug, Clone)]
pub struct SelectableTemplate {
    pub id: String,
    pub priority: i64,
    pub enabled: bool,
    pub when: Vec<Condition>,
}

/// First enabled template (lowest priority number) whose `when` rules all match.
pub fn select_template<'a>(templates: &'a [SelectableTemplate], ctx: &Ctx) -> Option<&'a SelectableTemplate> {
    let mut sorted: Vec<&SelectableTemplate> = templates.iter().filter(|t| t.enabled).collect();
    sorted.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.id.cmp(&b.id)));
    sorted.into_iter().find(|t| eval_all(&t.when, ctx, false))
}

fn computed_account_details(ctx: &Ctx) -> String {
    ACCOUNT_DETAILS
        .iter()
        .filter(|(k, _)| ctx.get(*k).map(|v| is_blank(&v.value)).unwrap_or(true))
        .map(|(_, label)| format!("<p>{}:</p>", html_escape(label)))
        .collect::<Vec<_>>()
        .join("")
}

/// Resolve one placeholder to HTML, or None to leave the token visible.
pub fn resolve(key: &str, ctx: &Ctx) -> Option<String> {
    if key == "account_details_needed" {
        return Some(computed_account_details(ctx));
    }
    let def = catalog().placeholder(key);
    let fv = ctx.get(key)?;
    if is_blank(&fv.value) {
        return None;
    }
    if let Some(d) = def {
        if d.verified_only && !fv.trusted {
            return None;
        }
        if d.exclude_values.iter().any(|x| x.eq_ignore_ascii_case(fv.value.trim())) {
            return None;
        }
    }
    let shown = display_value(&fv.value);
    // multi-line notes keep their line breaks
    Some(html_escape(&shown).replace('\n', "<br>"))
}

/// Replace intact, resolvable tokens; everything else is left untouched.
pub fn fill(html: &str, ctx: &Ctx) -> String {
    TOKEN_RE
        .replace_all(html, |caps: &regex::Captures| {
            let key = &caps[1];
            resolve(key, ctx).unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

/// Plain-text fill (for subjects): no HTML escaping.
pub fn fill_text(text: &str, ctx: &Ctx) -> String {
    TOKEN_RE
        .replace_all(text, |caps: &regex::Captures| {
            let key = &caps[1];
            match ctx.get(key) {
                Some(v) if !is_blank(&v.value) => display_value(&v.value),
                _ => caps[0].to_string(),
            }
        })
        .into_owned()
}

/// Unique tokens still present in text, in order of appearance.
pub fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in TOKEN_RE.captures_iter(text) {
        let k = c[1].to_string();
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct Unresolved {
    pub key: String,
    pub label: String,
    pub known: bool,
}

pub fn unresolved(subject: &str, body: &str) -> Vec<Unresolved> {
    let mut keys = tokens(subject);
    for k in tokens(body) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys.into_iter()
        .map(|k| {
            let def = catalog().placeholder(&k);
            Unresolved {
                label: def.map(|d| d.label.clone()).unwrap_or_else(|| k.clone()),
                known: def.is_some(),
                key: k,
            }
        })
        .collect()
}

static RE_DROP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<(script|style|head)\b.*?</(script|style|head)\s*>|<!--.*?-->").unwrap());
static RE_BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static RE_BLOCK_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</(p|div|h[1-6]|tr|table|ul|ol|blockquote)\s*>").unwrap());
static RE_LI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<li\b[^>]*>").unwrap());
static RE_LI_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</li\s*>").unwrap());
static RE_IMG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<img\b[^>]*>").unwrap());
static RE_ALT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\balt\s*=\s*"([^"]*)""#).unwrap());
static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static RE_ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[a-zA-Z]{2,8});").unwrap());
static RE_BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Readable plain text from HTML: block elements become line breaks, list items get "- ",
/// images become "[Image: alt]", entities are decoded. Paragraph breaks are kept.
pub fn html_to_text(html: &str) -> String {
    let s = RE_DROP.replace_all(html, "");
    let s = RE_BR.replace_all(&s, "\n");
    let s = RE_LI.replace_all(&s, "\n- ");
    let s = RE_LI_END.replace_all(&s, "");
    let s = RE_BLOCK_END.replace_all(&s, "\n\n");
    let s = RE_IMG.replace_all(&s, |c: &regex::Captures| match RE_ALT.captures(&c[0]).map(|a| a[1].trim().to_string()) {
        Some(alt) if !alt.is_empty() => format!("[Image: {alt}]"),
        _ => "[Image]".into(),
    });
    let s = RE_TAG.replace_all(&s, "");
    let s = RE_ENTITY.replace_all(&s, |c: &regex::Captures| {
        let e = &c[1];
        let ch = if let Some(hex) = e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = e.strip_prefix('#') {
            dec.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            match e {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "ndash" => Some('–'),
                "mdash" => Some('—'),
                "rsquo" => Some('’'),
                "lsquo" => Some('‘'),
                "rdquo" => Some('”'),
                "ldquo" => Some('“'),
                "hellip" => Some('…'),
                "bull" => Some('•'),
                _ => None,
            }
        };
        ch.map(|c| c.to_string()).unwrap_or_else(|| c[0].to_string())
    });
    let lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    let joined = lines.join("\n");
    RE_BLANKS.replace_all(joined.trim(), "\n\n").into_owned()
}

/// Context filled with catalog sample values (template preview without a ticket).
pub fn sample_ctx() -> Ctx {
    let mut ctx = Ctx::new();
    for p in &catalog().placeholders {
        if p.source != "computed" {
            put(&mut ctx, &p.key, p.sample.clone(), true);
        }
    }
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(field: &str, op: &str, value: Option<&str>) -> Condition {
        Condition { field: field.into(), op: op.into(), value: value.map(|s| s.into()), values: None, fields: None, label: None }
    }

    #[test]
    fn missing_values_stay_as_tokens() {
        let ctx = Ctx::new();
        let out = fill("<p>School {{school_name}} ID {{school_njiis_id}}</p>", &ctx);
        assert_eq!(out, "<p>School {{school_name}} ID {{school_njiis_id}}</p>");
    }

    #[test]
    fn available_values_fill_and_escape() {
        let mut ctx = Ctx::new();
        put(&mut ctx, "school_name", "A & B <School>", false);
        let out = fill("<p>{{ school_name }} / {{school_njiis_id}}</p>", &ctx);
        assert_eq!(out, "<p>A &amp; B &lt;School&gt; / {{school_njiis_id}}</p>");
    }

    #[test]
    fn verified_only_requires_trusted() {
        let mut ctx = Ctx::new();
        put(&mut ctx, "submitted_grades", r#"["Kindergarten","Grade 1"]"#, false);
        assert_eq!(fill("{{submitted_grades}}", &ctx), "{{submitted_grades}}");
        put(&mut ctx, "submitted_grades", r#"["Kindergarten","Grade 1"]"#, true);
        assert_eq!(fill("{{submitted_grades}}", &ctx), "Kindergarten, Grade 1");
    }

    #[test]
    fn unknown_subtype_not_rendered() {
        let mut ctx = Ctx::new();
        put(&mut ctx, "school_type", "Unknown", true);
        assert_eq!(fill("{{school_type}}", &ctx), "{{school_type}}");
    }

    #[test]
    fn account_details_lists_only_missing() {
        let mut ctx = Ctx::new();
        put(&mut ctx, "first_name", "Jo", true);
        put(&mut ctx, "user_email", "jo@x.org", true);
        let out = fill("{{account_details_needed}}", &ctx);
        assert!(!out.contains("First Name"));
        assert!(!out.contains("<p>Email:"));
        assert!(out.contains("Last Name:"));
        assert!(out.contains("Work Phone:"));
        assert!(out.contains("Date of ASR training:"));
    }

    #[test]
    fn rules_and_asserts() {
        let mut ctx = Ctx::new();
        assert!(eval_cond(&c("subtype_updated", "ne", Some("Yes")), &ctx, false));
        put(&mut ctx, "subtype_updated", "Yes", false);
        assert!(eval_cond(&c("subtype_updated", "eq", Some("Yes")), &ctx, false));
        // extracted (untrusted) values do not satisfy assertions
        assert_eq!(assertion_failures(&[c("subtype_updated", "eq", Some("Yes"))], &ctx).len(), 1);
        put(&mut ctx, "subtype_updated", "Yes", true);
        assert!(assertion_failures(&[c("subtype_updated", "eq", Some("Yes"))], &ctx).is_empty());
        assert!(eval_cond(&c("g", "empty", None), &ctx, false));
        put(&mut ctx, "g", "[]", true);
        assert!(eval_cond(&c("g", "empty", None), &ctx, false));
        assert!(!eval_cond(&c("x", "never", None), &ctx, false));
    }

    #[test]
    fn selection_by_priority_and_rules() {
        let t = vec![
            SelectableTemplate { id: "fallback".into(), priority: 20, enabled: true, when: vec![] },
            SelectableTemplate { id: "confirm".into(), priority: 5, enabled: true, when: vec![c("s", "eq", Some("Yes"))] },
            SelectableTemplate { id: "disabled".into(), priority: 1, enabled: false, when: vec![] },
        ];
        let mut ctx = Ctx::new();
        assert_eq!(select_template(&t, &ctx).unwrap().id, "fallback");
        put(&mut ctx, "s", "Yes", true);
        assert_eq!(select_template(&t, &ctx).unwrap().id, "confirm");
    }

    #[test]
    fn html_to_text_keeps_structure() {
        let html = "<style>p{}</style><p>Hello&nbsp;Jane,</p><p>Steps:<br>one &amp; two</p><ul><li>Clear cache</li><li>Retry</li></ul><img src=\"cid:x\" alt=\"Screenshot\"><div>Thanks &#8212; Desk</div>";
        assert_eq!(html_to_text(html), "Hello Jane,\n\nSteps:\none & two\n\n- Clear cache\n- Retry\n\n[Image: Screenshot]Thanks — Desk");
    }

    #[test]
    fn subject_fill() {
        let mut ctx = Ctx::new();
        put(&mut ctx, "original_subject", "ASR <help>", true);
        assert_eq!(fill_text("RE: {{original_subject}}", &ctx), "RE: ASR <help>");
    }
}
