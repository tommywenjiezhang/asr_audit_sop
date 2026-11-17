//! Rule-based email analysis: separates the current message from quoted history,
//! extracts identifiers/values (never invents them) and classifies the issue type.

use crate::catalog::Catalog;
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Extracted {
    pub key: String,
    pub value: String,
    /// high | medium | low
    pub confidence: String,
    /// current | subject | quoted | sender
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub issue_type: String,
    /// Primary issue first, then other clearly present issues (max 3).
    pub issues: Vec<String>,
    pub confidence: f64,
    pub scores: Vec<(String, f64)>,
    pub fields: Vec<Extracted>,
    pub school_ids: Vec<String>,
    pub facility_ids: Vec<String>,
    /// Number of school groups found (field keys via `school_key`).
    pub school_count: i64,
}

/// Attributes stored per school group.
pub const SCHOOL_ATTRS: &[&str] = &["name", "njiis_id", "other_ids", "address", "municipality", "county", "zip"];

/// Field key of a school attribute. School 1 uses the original keys (`school_name`, `school_njiis_id`, ...)
/// so existing tickets and template placeholders keep working; further schools use `school2_name`, ...
pub fn school_key(slot: i64, attr: &str) -> String {
    if slot <= 1 { format!("school_{attr}") } else { format!("school{slot}_{attr}") }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct SchoolEx {
    name: Option<String>,
    njiis_id: Option<String>,
    other_ids: Vec<String>,
    address: Option<String>,
    municipality: Option<String>,
    county: Option<String>,
    zip: Option<String>,
    /// line index where this group was last touched
    line: usize,
}

impl Analysis {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().find(|f| f.key == key).map(|f| f.value.as_str())
    }
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

re!(QUOTE_START, r"(?im)^\s*(?:-{2,}\s*Original Message\s*-{2,}|_{5,}|From:\s.+|On .{3,120}wrote:\s*$|Sent from my )");
re!(SIGNOFF, r"(?im)^\s*(?:thank you|thanks|thank you so much|best regards|regards|sincerely|best|respectfully|warm regards|kind regards)[,.!]?\s*$");
re!(SCHOOL_ID, r"(?i)\b(?:school\s*(?:njiis\s*)?|njiis\s*(?:school\s*)?|org(?:anization)?\s*)(?:id|#|number|no\.?|code)\s*(?:is|:|=|-|#)?\s*#?\s*([0-9]{3,10})\b");
re!(FACILITY_ID, r"(?i)\bfacility\s*(?:id|#|number|no\.?|code)\s*(?:is|:|=|-|#)?\s*#?\s*([0-9]{2,10})\b");
re!(GENERIC_ID, r"(?i)\bID\s*(?:#|:|is|number)?\s*#?\s*([0-9]{4,10})\b");
re!(SCHOOL_NAME_LABEL, r"(?im)^\s*(?:name of (?:the )?school|school name|school)\s*[:\-]\s*(.{3,100}?)\s*$");
re!(SCHOOL_NAME, r"\b((?:[A-Z][A-Za-z0-9'.&\-]*\s+){0,6}(?:School|Academy|Preschool|Pre-School|Learning Center|Day Care|Daycare|Child Care Center|Childcare Center|Early Learning Center|Nursery School|Montessori))\b");
re!(SUBTYPE, r"(?i)\b(?:we\s+are\s+an?|it\s+is\s+an?|it's\s+an?|is\s+an?|(?:subtype|sub-type|type)\s*(?:is|:)\s*(?:an?)?)\s*(charter|public|private)\b");
re!(SUBTYPE2, r"(?i)\b(charter|public|private)\s+school\b");
re!(QUOTED_ERR, r#"(?i)(?:error|message|says|saying|shows|showing|states|getting|get)\s*(?:message)?\s*[:\-]?\s*["“']([^"”']{3,200})["”']"#);
re!(ERR_TEXT, r"(?i)\b(error|access denied|denied|failed|failure|invalid|black screen|not authorized|unauthorized)\b");
re!(TRAIN_DONE, r"(?i)\b(?:i|we|she|he|they|staff|nurse)?\s*(?:have|has|had)?\s*(?:completed|finished|took|taken|done|watched|attended)\s+(?:the\s+|my\s+|our\s+)?(?:asr\s+|audit\s+|on-demand\s+|online\s+)?(?:training|webinar)");
re!(TRAIN_NOT, r"(?i)\b(?:have\s*n[o']t|has\s*n[o']t|did\s*n[o']t|not yet|haven't|hasn't|didn't)\s+(?:yet\s+)?(?:completed|finished|taken|took|done|watched)\s+(?:the\s+)?(?:asr\s+|audit\s+)?training");
re!(DATE, r"(?i)\b(\d{1,2}/\d{1,2}/\d{2,4}|(?:jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\.?\s+\d{1,2}(?:st|nd|rd|th)?,?\s*(?:\d{4})?)\b");
re!(GRADE_NUM, r"(?i)\b(?:grade\s*(\d{1,2})|(\d{1,2})(?:st|nd|rd|th)\s*grade)\b");
re!(LHD, r"\b((?:[A-Z][A-Za-z.&'\-]*\s+){1,5}(?:County\s+)?(?:Health\s+Department|Health\s+Dept\.?|Department\s+of\s+Health|Board\s+of\s+Health|Health\s+Division))\b");
re!(LABELED, r"(?im)^\s*(first\s*name|last\s*name|e-?mail(?:\s*address)?|work\s*phone|phone(?:\s*number)?)\s*:\s*(.{1,120}?)\s*$");
re!(SUBMITTED_CLAIM, r"(?i)\b(?:i|we)\s+(?:have\s+)?(?:submitted|completed and submitted|sent in|entered)\b");
re!(EMAIL, r"(?i)\b[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}\b");
re!(GREETING, r"(?i)^\s*(hi|hello|hey|good (morning|afternoon|evening)|dear|greetings)\b");
re!(LABEL_LINE, r"^\s*[A-Za-z][A-Za-z /#_.-]{1,30}:\s*\S[^.!?]{0,120}$");
// other (non-NJIIS) school identifiers, kept with their type
re!(OTHER_ID, r"(?i)\b(ASR|SHAD|CDS|NCES|county[\s-]*district[\s-]*school)[\s_-]*(?:ID|#|code|number|no\.?)\s*(?:is|:|=|-|#)?\s*#?\s*([A-Z0-9][A-Z0-9-]{1,19})\b");
re!(SCHOOL_ADDR_LABEL, r"(?i)^\s*(?:school\s+address|address\s+of\s+(?:the\s+|our\s+)?school|(?:school\s+)?location|address)\s*[:\-]\s*(.{5,160}?)\s*$");
re!(STREET_LINE, r"(?i)^\s*\d{1,6}\s+[A-Za-z0-9.' -]{2,60}\b(?:street|st|avenue|ave|road|rd|boulevard|blvd|drive|dr|lane|ln|way|place|pl|court|ct|highway|hwy|route|rt|pike|parkway|pkwy|terrace|ter)\b\.?.*$");
re!(CITY_LINE, r"(?i)^\s*([A-Za-z .'-]{2,40}),\s*(?:NJ|N\.J\.|New Jersey)\.?\s+(\d{5}(?:-\d{4})?)\s*$");
re!(CITY_STATE, r"(?i)([A-Za-z .'-]{2,40}),\s*(?:NJ|N\.J\.|New Jersey)\b");
re!(ZIP, r"\b(\d{5}(?:-\d{4})?)\b");
re!(COUNTY, r"\b((?:[A-Z][a-z]+\s){0,2}[A-Z][a-z]+)\s+County\b");
re!(PHONE, r"(?i)(?:^|[^\d])(\(?\d{3}\)?[\s.\-]?\d{3}[\s.\-]?\d{4})(?:\s*(?:,\s*)?(?:x|ext\.?|extension)\s*\.?\s*(\d{1,6}))?");
re!(PHONE_LABEL, r"(?i)^\s*(?:phone|tel|telephone|work\s*phone|cell|office|p)\s*[:.]");
re!(FAX, r"(?i)\bfax\b");
re!(TITLE_WORD, r"(?i)\b(nurse|rn|bsn|lpn|director|principal|administrator|secretary|coordinator|manager|teacher|assistant|registrar|superintendent|specialist|officer|clerk|owner|supervisor|health|aide|counselor|president|vice|head)\b");

const NAME_STOP: &[&str] = &["the", "our", "your", "my", "this", "that", "each", "every", "a", "an", "school", "forms", "annual",
    "new", "public", "private", "charter", "which", "what", "for", "in", "at", "of", "and", "from", "to", "high", "middle", "elementary"];

/// Split body into (current message, quoted history).
pub fn split_current(body: &str) -> (String, String) {
    let normalized = body.replace("\r\n", "\n");
    let mut current = String::new();
    let mut quoted = String::new();
    let mut in_quote = false;
    for line in normalized.lines() {
        if !in_quote && QUOTE_START.is_match(line) && !current.trim().is_empty() {
            in_quote = true;
        }
        if !in_quote && line.trim_start().starts_with('>') {
            quoted.push_str(line.trim_start_matches(['>', ' ']));
            quoted.push('\n');
            continue;
        }
        if in_quote {
            quoted.push_str(line);
            quoted.push('\n');
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    (current, quoted)
}

/// Text before the sign-off line (used for classification only).
fn before_signoff(current: &str) -> &str {
    match SIGNOFF.find(current) {
        Some(m) if m.start() > 20 => &current[..m.start()],
        _ => current,
    }
}

fn push(v: &mut Vec<Extracted>, key: &str, value: &str, conf: &str, src: &str) {
    let value = value.trim();
    if value.is_empty() || v.iter().any(|e| e.key == key) {
        return;
    }
    v.push(Extracted { key: key.into(), value: value.into(), confidence: conf.into(), source: src.into() });
}

fn ids(re: &Regex, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in re.captures_iter(text) {
        let id = c[1].to_string();
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn clean_school_name(raw: &str) -> Option<String> {
    let words: Vec<&str> = raw.split_whitespace().collect();
    // drop leading stop words ("at The Sample School" -> "Sample School")
    let mut start = 0;
    while start < words.len() && NAME_STOP.contains(&words[start].to_lowercase().as_str()) && start + 1 < words.len() {
        start += 1;
    }
    let w = &words[start..];
    if w.len() < 2 {
        return None;
    }
    let first = w[0].to_lowercase();
    if NAME_STOP.contains(&first.as_str()) {
        return None;
    }
    Some(w.join(" "))
}

fn first_school_name(text: &str) -> Option<String> {
    if let Some(c) = SCHOOL_NAME_LABEL.captures(text) {
        let v = c[1].trim().to_string();
        if v.split_whitespace().count() >= 1 && !v.to_lowercase().starts_with("forms") {
            return Some(v);
        }
    }
    SCHOOL_NAME.captures_iter(text).find_map(|c| clean_school_name(&c[1]))
}

/// First name for the greeting, from "Last, First" or "First Last" display names.
pub fn first_name_from_display(display: &str) -> Option<String> {
    let d = display.trim().trim_matches('"');
    if d.is_empty() || d.contains('@') {
        return None;
    }
    let name = if let Some((_, first)) = d.split_once(',') {
        first.split_whitespace().next()?
    } else {
        d.split_whitespace().next()?
    };
    let name = name.trim_matches(|c: char| !c.is_alphabetic() && c != '-' && c != '\'');
    if name.len() < 2 || name.chars().all(|c| c.is_uppercase()) && name.len() > 3 && d.contains('[') {
        return None;
    }
    let mut cs = name.chars();
    let f = cs.next()?;
    Some(f.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase())
}

fn grades(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut g: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        if !g.iter().any(|x| x == s) {
            g.push(s.to_string());
        }
    };
    if ["pre-k", "prek", "preschool", "pre-school", "child care", "childcare", "daycare", "day care"].iter().any(|k| lower.contains(k)) {
        add("Child Care/Pre-K");
    }
    if lower.contains("kindergarten") {
        add("Kindergarten");
    }
    for c in GRADE_NUM.captures_iter(text) {
        let n: u32 = c.get(1).or(c.get(2)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        if (1..=12).contains(&n) {
            add(&format!("Grade {n}"));
        }
    }
    for (word, n) in [("first grade", 1), ("sixth grade", 6), ("seventh grade", 7), ("eleventh grade", 11)] {
        if lower.contains(word) {
            add(&format!("Grade {n}"));
        }
    }
    g
}

fn summary(current: &str) -> String {
    let text: String = before_signoff(current)
        .lines()
        .map(str::trim)
        // "School Name: ...", "NJIIS ID: ..." lines are details, not the description of the problem
        .filter(|l| !l.is_empty() && !GREETING.is_match(l) && !LABEL_LINE.is_match(l))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::new();
    for (i, sentence) in text.split_inclusive(['.', '?', '!']).enumerate() {
        if i >= 2 || out.len() > 300 {
            break;
        }
        out.push_str(sentence);
    }
    let out = out.trim();
    if out.chars().count() > 400 {
        out.chars().take(400).collect::<String>() + "…"
    } else {
        out.to_string()
    }
}

pub fn classify(cat: &Catalog, subject: &str, text: &str) -> (String, f64, Vec<(String, f64)>) {
    let subj = subject.to_lowercase();
    let body = text.to_lowercase();
    let mut scores: Vec<(String, f64)> = cat
        .issue_types
        .iter()
        .map(|it| {
            let s: f64 = it
                .keywords
                .iter()
                .map(|(k, w)| {
                    let k = k.to_lowercase();
                    let mut s = 0.0;
                    if subj.contains(&k) {
                        s += w * 1.5;
                    }
                    if body.contains(&k) {
                        s += w;
                    }
                    s
                })
                .sum();
            (it.id.clone(), s)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    match scores.first() {
        Some((id, top)) if *top >= 3.0 => {
            let second = scores.get(1).map(|s| s.1).unwrap_or(0.0);
            let conf = ((top - second) / top).clamp(0.0, 1.0) * 0.5 + (top / (top + 5.0)) * 0.5;
            (id.clone(), (conf * 100.0).round() / 100.0, scores)
        }
        _ => ("other".into(), 0.0, scores),
    }
}

pub fn analyze(cat: &Catalog, subject: &str, body: &str, sender_name: &str, sender_email: &str) -> Analysis {
    let (current, quoted) = split_current(body);
    let mut f: Vec<Extracted> = Vec::new();

    let (body, signature) = split_signoff(&current);

    // schools: groups of name / NJIIS ID / other IDs / address found together in the message body
    let mut groups = extract_schools(body);
    let facility_ids = {
        let mut v = ids(&FACILITY_ID, &format!("{subject}\n{current}"));
        if v.is_empty() {
            v = ids(&FACILITY_ID, &quoted);
        }
        v
    };
    let mut school_ids: Vec<String> = groups.iter().filter_map(|g| g.njiis_id.clone()).collect();
    // fallback for school 1: identifiers anywhere in the current message > subject > quoted history
    let mut id_conf = "high";
    let mut id_src = "current";
    if school_ids.is_empty() {
        let mut found = ids(&SCHOOL_ID, &current);
        if found.is_empty() {
            found = ids(&SCHOOL_ID, subject);
            id_src = "subject";
        }
        if found.is_empty() {
            found = ids(&SCHOOL_ID, &quoted);
            id_src = "quoted";
            id_conf = "low";
        }
        if found.is_empty() {
            // unlabeled "ID: 12345" -> low confidence, only if it is not a facility ID or another ID type
            let text = format!("{subject}\n{current}");
            let other_vals: Vec<String> = OTHER_ID.captures_iter(&text).map(|c| c[2].to_string()).collect();
            found = ids(&GENERIC_ID, &text).into_iter().filter(|i| !facility_ids.contains(i) && !other_vals.contains(i)).collect();
            id_src = "current";
            id_conf = "low";
        }
        for (k, id) in found.iter().enumerate() {
            if k == 0 {
                if groups.is_empty() {
                    groups.push(SchoolEx::default());
                }
                groups[0].njiis_id = Some(id.clone());
            } else {
                groups.push(SchoolEx { njiis_id: Some(id.clone()), ..Default::default() });
            }
        }
        school_ids = found;
    }
    if groups.first().map(|g| g.name.is_none()).unwrap_or(true) {
        let taken: Vec<String> = groups.iter().filter_map(|g| g.name.clone()).collect();
        let (name, conf) = match first_school_name(&current).or_else(|| first_school_name(subject)) {
            Some(n) => (Some(n), "medium"),
            None => (first_school_name(&quoted), "low"),
        };
        if let Some(n) = name.filter(|n| !taken.contains(n)) {
            if groups.is_empty() {
                groups.push(SchoolEx::default());
            }
            groups[0].name = Some(n.clone());
            if conf == "low" {
                push(&mut f, "school_name", &n, "low", "quoted");
            }
        }
    }
    for (i, g) in groups.iter().enumerate() {
        let slot = i as i64 + 1;
        if let Some(v) = &g.njiis_id {
            push(&mut f, &school_key(slot, "njiis_id"), v, if slot == 1 { id_conf } else { "high" }, if slot == 1 { id_src } else { "current" });
        }
        if let Some(v) = &g.name {
            push(&mut f, &school_key(slot, "name"), v, "medium", "current");
        }
        if !g.other_ids.is_empty() {
            push(&mut f, &school_key(slot, "other_ids"), &g.other_ids.join("; "), "high", "current");
        }
        for (attr, v, conf) in [("address", &g.address, "medium"), ("municipality", &g.municipality, "medium"), ("county", &g.county, "low"), ("zip", &g.zip, "medium")] {
            if let Some(v) = v {
                push(&mut f, &school_key(slot, attr), v, conf, "current");
            }
        }
    }
    if let Some(id) = facility_ids.first() {
        push(&mut f, "facility_id", id, "high", "current");
    }

    // sender details from the signature block (never used as the school address)
    let (title, phone, ext) = signature_details(signature);
    if let Some(t) = title {
        push(&mut f, "sender_title", &t, "medium", "signature");
    }
    if let Some(p) = phone {
        push(&mut f, "sender_phone", &p, "medium", "signature");
    }
    if let Some(x) = ext {
        push(&mut f, "sender_phone_ext", &x, "medium", "signature");
    }

    let cur_subj = format!("{subject}\n{current}");
    if let Some(c) = SUBTYPE.captures(&cur_subj).or_else(|| SUBTYPE2.captures(&current)) {
        let t = c[1].to_lowercase();
        let v = match t.as_str() { "charter" => "Charter", "public" => "Public", _ => "Private" };
        push(&mut f, "school_type", v, "medium", "current");
    }

    // exact error messages: quoted error texts (all of them); otherwise lines describing an error
    let mut errors: Vec<String> = Vec::new();
    for c in QUOTED_ERR.captures_iter(&cur_subj) {
        let e = c[1].trim().to_string();
        if !errors.contains(&e) && errors.len() < 5 {
            errors.push(e);
        }
    }
    if !errors.is_empty() {
        push(&mut f, "error_message", &errors.join("\n"), "high", "current");
    } else {
        // without quotes, only lines that name an actual error ("unable to ..." describes the request, not an error text)
        let lines: Vec<&str> = body.lines().map(str::trim).filter(|l| ERR_TEXT.is_match(l) && l.len() <= 300).take(3).collect();
        if !lines.is_empty() {
            push(&mut f, "error_message", &lines.join("\n"), "medium", "current");
        }
    }

    let lower = cur_subj.to_lowercase();
    let form = if lower.contains("audit") || lower.contains("lhd") || lower.contains("health department") {
        Some("Audit")
    } else if lower.contains("asr") || lower.contains("immunization status report") || lower.contains("annual immunization") {
        Some("ASR")
    } else if lower.contains("training") || lower.contains("webinar") {
        Some("Training")
    } else if lower.contains("password") || lower.contains("login") || lower.contains("log in") || lower.contains("account") {
        Some("NJIIS Access")
    } else {
        None
    };
    if let Some(fm) = form {
        push(&mut f, "form_type", fm, "medium", "current");
    }

    if TRAIN_NOT.is_match(&current) {
        push(&mut f, "asr_training_completed", "No", "medium", "current");
    } else if let Some(m) = TRAIN_DONE.find(&current) {
        push(&mut f, "asr_training_completed", "Yes", "medium", "current");
        // date in the same sentence as the training statement
        let start = current[..m.start()].rfind(['.', '\n']).map(|i| i + 1).unwrap_or(0);
        let end = current[m.end()..].find(['.', '\n']).map(|i| m.end() + i).unwrap_or(current.len());
        if let Some(d) = DATE.captures(&current[start..end]) {
            push(&mut f, "training_date", &d[1], "medium", "current");
        }
    }
    if lower.contains("audit") {
        if let Some(v) = f.iter().find(|e| e.key == "asr_training_completed").map(|e| e.value.clone()) {
            push(&mut f, "audit_training_completed", &v, "low", "current");
        }
    }

    if SUBMITTED_CLAIM.is_match(&current) {
        push(&mut f, "sender_reported_submission", "Sender says the form was submitted (not verified)", "medium", "current");
    }

    let g = grades(&current);
    if !g.is_empty() {
        push(&mut f, "expected_grades", &serde_json::to_string(&g).unwrap_or_default(), "low", "current");
    }

    if let Some(c) = LHD.captures(&current) {
        push(&mut f, "lhd_name", &c[1], "medium", "current");
    }

    for c in LABELED.captures_iter(&current) {
        let label = c[1].to_lowercase().replace(' ', "");
        let val = c[2].trim();
        let key = if label.starts_with("first") {
            "first_name"
        } else if label.starts_with("last") {
            "last_name"
        } else if label.starts_with('e') {
            if !EMAIL.is_match(val) { continue; }
            "user_email"
        } else {
            "work_phone"
        };
        push(&mut f, key, val, "high", "current");
    }

    if let Some(fnm) = first_name_from_display(sender_name) {
        push(&mut f, "contact_first_name", &fnm, "medium", "sender");
    }
    let _ = sender_email;

    let desc = summary(&current);
    if !desc.is_empty() {
        push(&mut f, "issue_description", &desc, "medium", "current");
    }

    let class_text = before_signoff(&current);
    let (issue_type, confidence, scores) = classify(cat, subject, class_text);
    let issues = related_issues(&issue_type, &scores);
    let school_count = groups.len() as i64;

    Analysis { issue_type, issues, confidence, scores, fields: f, school_ids, facility_ids, school_count }
}

/// Primary issue plus other issues that are clearly present (score >= 3 and >= 50% of the top score).
pub fn related_issues(primary: &str, scores: &[(String, f64)]) -> Vec<String> {
    let mut out = vec![primary.to_string()];
    if primary == "other" {
        return out;
    }
    let top = scores.first().map(|s| s.1).unwrap_or(0.0);
    for (id, s) in scores {
        if out.len() >= 3 {
            break;
        }
        if id != primary && *s >= 3.0 && *s >= top * 0.5 {
            out.push(id.clone());
        }
    }
    out
}

/// (message body before the sign-off, signature block from the sign-off on).
fn split_signoff(current: &str) -> (&str, &str) {
    match SIGNOFF.find(current) {
        Some(m) if m.start() > 20 => (&current[..m.start()], &current[m.start()..]),
        _ => (current, ""),
    }
}

fn norm_id_type(raw: &str) -> String {
    let t = raw.trim().to_uppercase();
    if t.starts_with("COUNTY") { "CDS_ID".into() } else { format!("{t}_ID") }
}

fn apply_address(g: &mut SchoolEx, addr: &str) {
    let addr = addr.trim().trim_end_matches(['.', ',']).to_string();
    if let Some(c) = CITY_STATE.captures(&addr) {
        let city = c[1].rsplit(',').next().unwrap_or("").trim();
        // "123 Main St Trenton, NJ" -> keep only the trailing words that are not part of the street line
        let city = city.rsplit(|ch: char| ch.is_ascii_digit()).next().unwrap_or(city).trim();
        let city = STREET_SUFFIX_SPLIT.split(city).last().unwrap_or(city).trim();
        if !city.is_empty() {
            g.municipality = Some(city.to_string());
        }
    }
    if let Some(z) = ZIP.captures_iter(&addr).last() {
        g.zip = Some(z[1].to_string());
    }
    g.address = Some(addr);
}

re!(STREET_SUFFIX_SPLIT, r"(?i)\b(?:street|st|avenue|ave|road|rd|boulevard|blvd|drive|dr|lane|ln|way|place|pl|court|ct|highway|hwy|route|pike|parkway|pkwy|terrace|ter)\.?\s+");

/// Group school details that appear together in the message body (before the sign-off).
/// Conservative: identifiers are paired with a school name on the same or the previous line;
/// addresses are taken only when labelled, or on the line right after a school name/ID.
fn extract_schools(body: &str) -> Vec<SchoolEx> {
    let lines: Vec<&str> = body.lines().collect();
    let mut g: Vec<SchoolEx> = Vec::new();
    // an unlabelled school name seen on a line without an identifier
    let mut pending: Option<(String, usize)> = None;
    let near = |s: &SchoolEx, i: usize, n: usize| i.saturating_sub(s.line) <= n;
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let labeled_name = SCHOOL_NAME_LABEL.captures(line).map(|c| c[1].trim().to_string()).filter(|v| !v.to_lowercase().starts_with("forms"));
        let name_here = labeled_name.clone().or_else(|| SCHOOL_NAME.captures_iter(line).find_map(|c| clean_school_name(&c[1])));
        let ids_here = ids(&SCHOOL_ID, line);
        let others: Vec<String> = OTHER_ID.captures_iter(line).map(|c| format!("{}: {}", norm_id_type(&c[1]), &c[2])).collect();

        if !ids_here.is_empty() {
            for (k, id) in ids_here.iter().enumerate() {
                let reuse = k == 0 && g.last().map(|s| s.njiis_id.is_none() && near(s, i, 2)).unwrap_or(false);
                if reuse {
                    let s = g.last_mut().unwrap();
                    s.njiis_id = Some(id.clone());
                    s.line = i;
                    if s.name.is_none() {
                        s.name = name_here.clone();
                    }
                } else {
                    let name = if k == 0 {
                        name_here.clone().or_else(|| pending.take().filter(|(_, l)| i.saturating_sub(*l) <= 2).map(|(n, _)| n))
                    } else {
                        None
                    };
                    g.push(SchoolEx { njiis_id: Some(id.clone()), name, line: i, ..Default::default() });
                }
            }
            pending = None;
        } else if let Some(n) = labeled_name {
            match g.last_mut().filter(|s| s.name.is_none() && i.saturating_sub(s.line) <= 2) {
                Some(s) => {
                    s.name = Some(n);
                    s.line = i;
                }
                None => g.push(SchoolEx { name: Some(n), line: i, ..Default::default() }),
            }
            pending = None;
        } else if let Some(n) = &name_here {
            if !others.is_empty() {
                g.push(SchoolEx { name: Some(n.clone()), line: i, ..Default::default() });
            } else {
                pending = Some((n.clone(), i));
            }
        }

        if !others.is_empty() {
            if g.is_empty() || !near(g.last().unwrap(), i, 3) {
                let name = pending.take().filter(|(_, l)| i.saturating_sub(*l) <= 2).map(|(n, _)| n);
                g.push(SchoolEx { name, line: i, ..Default::default() });
            }
            let s = g.last_mut().unwrap();
            for o in others {
                if !s.other_ids.contains(&o) {
                    s.other_ids.push(o);
                }
            }
            s.line = i;
        }

        // addresses
        if let Some(c) = SCHOOL_ADDR_LABEL.captures(line) {
            let explicit = line.to_lowercase().contains("school");
            let target = if g.last().map(|s| near(s, i, 3)).unwrap_or(false) {
                g.len().checked_sub(1)
            } else if let Some((n, _)) = pending.take().filter(|(_, l)| i.saturating_sub(*l) <= 2) {
                g.push(SchoolEx { name: Some(n), line: i, ..Default::default() });
                Some(g.len() - 1)
            } else if explicit {
                g.push(SchoolEx { line: i, ..Default::default() });
                Some(g.len() - 1)
            } else {
                None
            };
            if let Some(t) = target {
                if g[t].address.is_none() {
                    let mut addr = c[1].to_string();
                    if let Some(next) = lines.get(i + 1).filter(|l| CITY_LINE.is_match(l)) {
                        addr = format!("{}, {}", addr.trim_end_matches(','), next.trim());
                    }
                    apply_address(&mut g[t], &addr);
                    g[t].line = i;
                }
            }
        } else if STREET_LINE.is_match(line) {
            // unlabelled street line directly below a school name / ID line
            let target = if g.last().map(|s| s.address.is_none() && near(s, i, 1)).unwrap_or(false) {
                g.len().checked_sub(1)
            } else if let Some((n, _)) = pending.take().filter(|(_, l)| i.saturating_sub(*l) <= 1) {
                g.push(SchoolEx { name: Some(n), line: i, ..Default::default() });
                Some(g.len() - 1)
            } else {
                None
            };
            if let Some(t) = target {
                let mut addr = line.trim().to_string();
                if let Some(next) = lines.get(i + 1).filter(|l| CITY_LINE.is_match(l)) {
                    addr = format!("{}, {}", addr.trim_end_matches(','), next.trim());
                }
                apply_address(&mut g[t], &addr);
                g[t].line = i;
            }
        }

        // county (not the county of a health department)
        if let Some(c) = COUNTY.captures(line) {
            let rest = &line[c.get(0).unwrap().end()..].trim_start().to_lowercase();
            let lhd = rest.starts_with("health") || rest.starts_with("department") || rest.starts_with("board") || rest.starts_with("dept");
            if !lhd {
                if let Some(s) = g.last_mut().filter(|s| s.county.is_none() && i.saturating_sub(s.line) <= 3) {
                    s.county = Some(c[1].to_string());
                }
            }
        }
    }
    g
}

/// Sender title, phone and extension from the signature block only. Labelled phone lines in the body
/// (e.g. "Work Phone:" in a new-account request) usually describe another person and are not used.
fn signature_details(signature: &str) -> (Option<String>, Option<String>, Option<String>) {
    let sig_lines: Vec<&str> = signature.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut title = None;
    // line 0 is the sign-off, line 1 usually the name; the title follows
    for l in sig_lines.iter().skip(2).take(3) {
        if l.contains('@') || PHONE.is_match(l) || STREET_LINE.is_match(l) || CITY_LINE.is_match(l) {
            continue;
        }
        if TITLE_WORD.is_match(l) {
            // "School Nurse, Lincoln Elementary School" -> "School Nurse"
            let t = match l.split_once(',') {
                Some((a, b)) if TITLE_WORD.is_match(a) && SCHOOL_NAME.is_match(b) => a.trim(),
                _ => l,
            };
            title = Some(t.to_string());
            break;
        }
    }
    let mut phone = None;
    let mut ext = None;
    for l in sig_lines.iter().copied() {
        if FAX.is_match(l) && !PHONE_LABEL.is_match(l) {
            continue;
        }
        if let Some(c) = PHONE.captures(l) {
            phone = Some(c[1].trim().to_string());
            ext = c.get(2).map(|m| m.as_str().to_string());
            break;
        }
    }
    (title, phone, ext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;

    #[test]
    fn extracts_school_id_and_name_from_current_message_only() {
        let body = "Hello,\nOur school ID is 123456 and we cannot open the ASR.\nThank you,\nJane Doe\nNurse, Lincoln Elementary School\n\nFrom: Someone\nSent: Monday\nSchool ID: 999999 Old Problem";
        let a = analyze(catalog(), "ASR help", body, "Doe, Jane", "jdoe@x.org");
        assert_eq!(a.get("school_njiis_id"), Some("123456"));
        assert_eq!(a.get("school_name"), Some("Lincoln Elementary School"));
        assert_eq!(a.get("contact_first_name"), Some("Jane"));
        assert!(!a.school_ids.contains(&"999999".to_string()));
    }

    #[test]
    fn facility_ids_kept_separate_and_multiple_ids() {
        let body = "Facility ID: 404. School NJIIS ID 11111 and also school id 22222.";
        let a = analyze(catalog(), "help", body, "", "");
        assert_eq!(a.get("facility_id"), Some("404"));
        assert_eq!(a.get("school_njiis_id"), Some("11111"));
        assert_eq!(a.get("school2_njiis_id"), Some("22222"), "each NJIIS ID gets its own school group");
        assert_eq!(a.school_count, 2);
    }

    #[test]
    fn multiple_schools_keep_ids_and_addresses_together() {
        let body = "Hello,\nWe need help for two schools.\n\
            School Name: Maple Leaf Academy\nSchool NJIIS ID: 0012345\nAddress: 12 Oak Street, Trenton, NJ 08608\n\n\
            School Name: Riverside Preschool\nSchool NJIIS ID: 0067890\nASR ID: A-7781\nSHAD ID: 5521\n\
            Riverside Preschool is in Mercer County.\n\
            Thank you,\nJane Doe\nSchool Nurse\n99 District Road, Hamilton, NJ 08610\nPhone: (609) 555-0100 ext. 214\nFax: 609-555-0199\n";
        let a = analyze(catalog(), "ASR access", body, "Doe, Jane", "jdoe@x.org");
        assert_eq!(a.school_count, 2);
        assert_eq!(a.get("school_name"), Some("Maple Leaf Academy"));
        assert_eq!(a.get("school_njiis_id"), Some("0012345"), "leading zeros preserved");
        assert_eq!(a.get("school_address"), Some("12 Oak Street, Trenton, NJ 08608"));
        assert_eq!(a.get("school_municipality"), Some("Trenton"));
        assert_eq!(a.get("school_zip"), Some("08608"));
        assert_eq!(a.get("school2_name"), Some("Riverside Preschool"));
        assert_eq!(a.get("school2_njiis_id"), Some("0067890"));
        assert_eq!(a.get("school2_other_ids"), Some("ASR_ID: A-7781; SHAD_ID: 5521"));
        assert_eq!(a.get("school2_county"), Some("Mercer"));
        assert!(a.get("school2_address").is_none(), "the signature address is not a school address");
        assert!(!a.fields.iter().any(|f| f.value.contains("District Road")), "signature address never extracted");
        assert_eq!(a.get("sender_title"), Some("School Nurse"));
        assert_eq!(a.get("sender_phone"), Some("(609) 555-0100"));
        assert_eq!(a.get("sender_phone_ext"), Some("214"));
    }

    #[test]
    fn summary_skips_label_lines() {
        let body = "Good morning,\nWhen I open the ASR it says \"School subtype is required\".\nSchool Name: Maple Leaf Academy\nSchool NJIIS ID: 0012345\nWe are a private school.\nThanks,\nJo";
        let a = analyze(catalog(), "Subtype", body, "", "");
        assert_eq!(a.get("issue_description"), Some("When I open the ASR it says \"School subtype is required\". We are a private school."));
    }

    #[test]
    fn other_ids_never_become_njiis_ids() {
        let a = analyze(catalog(), "help", "Our ASR ID: 004455 for Lincoln Elementary School. We cannot log in.", "", "");
        assert!(a.get("school_njiis_id").is_none());
        assert_eq!(a.get("school_other_ids"), Some("ASR_ID: 004455"));
        assert_eq!(a.get("school_name"), Some("Lincoln Elementary School"));
    }

    #[test]
    fn multiple_issues_and_errors() {
        let body = "Our school is missing the subtype and the webinar shows a black screen.\n\
            The form says \"Subtype required\" and the video says \"Playback error 224003\".";
        let a = analyze(catalog(), "Subtype and webinar black screen", body, "", "");
        assert!(a.issues.len() >= 2, "{:?} {:?}", a.issues, a.scores);
        assert!(a.issues.contains(&"school_subtype_missing".to_string()));
        assert!(a.issues.contains(&"webinar_issue".to_string()));
        assert_eq!(a.issues[0], a.issue_type);
        assert_eq!(a.get("error_message"), Some("Subtype required\nPlayback error 224003"));
    }

    #[test]
    fn no_invented_id() {
        let a = analyze(catalog(), "Question", "Hello, how do I find the form? Thanks", "Smith, Ann", "a@x.org");
        assert!(a.get("school_njiis_id").is_none());
        assert!(a.get("school_name").is_none());
    }

    #[test]
    fn exact_error_message_preserved() {
        let a = analyze(catalog(), "Audit", "When I open the audit form I get the error \"Access Denied - You do not have permission\" every time.", "", "");
        assert_eq!(a.get("error_message"), Some("Access Denied - You do not have permission"));
        assert_eq!(a.issue_type, "lhd_access_denied");
    }

    #[test]
    fn classification_examples() {
        let cat = catalog();
        let cases = [
            ("School Subtype Error", "It says our school is missing the subtype.", "school_subtype_missing"),
            ("Training video", "The webinar shows a black screen and skips to the end of the webinar.", "webinar_issue"),
            ("Copy of ASR", "How can I save a copy of the form as a PDF?", "save_form_pdf"),
            ("New user", "I need an account. I do not have an account in NJIIS.", "new_user_account"),
            ("Verify receipt", "Can you confirm that you received our ASR? We submitted last week.", "submission_confirm_receipt"),
            ("School forms", "I completed the training but can't see the School Forms tab.", "school_forms_tab"),
            ("Enrollment", "Our school is not enrolled in NJIIS yet.", "school_enrollment"),
            ("hello", "random text without keywords", "other"),
        ];
        for (s, b, want) in cases {
            let a = analyze(cat, s, b, "", "");
            assert_eq!(a.issue_type, want, "subject={s} body={b} scores={:?}", a.scores);
        }
    }

    #[test]
    fn training_status_subtype_grades_and_labels() {
        let body = "Hi,\nI completed the ASR training on 11/10/2026 but still cannot log in.\nWe are a charter school with Kindergarten and 1st grade.\nFirst Name: Maria\nLast Name: Lopez\nEmail: mlopez@school.org\nWork Phone: 609-555-0100\n";
        let a = analyze(catalog(), "access", body, "mlopez@school.org", "mlopez@school.org");
        assert_eq!(a.get("asr_training_completed"), Some("Yes"));
        assert_eq!(a.get("training_date"), Some("11/10/2026"));
        assert_eq!(a.get("school_type"), Some("Charter"));
        assert_eq!(a.get("first_name"), Some("Maria"));
        assert_eq!(a.get("user_email"), Some("mlopez@school.org"));
        assert_eq!(a.get("work_phone"), Some("609-555-0100"));
        let g: Vec<String> = serde_json::from_str(a.get("expected_grades").unwrap()).unwrap();
        assert!(g.contains(&"Kindergarten".to_string()) && g.contains(&"Grade 1".to_string()));
        assert!(a.get("contact_first_name").is_none(), "email address is not a name");
    }

    #[test]
    fn quoted_reply_lines_ignored_for_classification() {
        let body = "Thanks, that worked!\n\n> It seems your school is missing the subtype.\n> Access denied";
        let (cur, q) = split_current(body);
        assert!(!cur.contains("subtype"));
        assert!(q.contains("subtype"));
    }

    #[test]
    fn display_name_parsing() {
        assert_eq!(first_name_from_display("Smith, JANE"), Some("Jane".into()));
        assert_eq!(first_name_from_display("John Q Public"), Some("John".into()));
        assert_eq!(first_name_from_display("x@y.org"), None);
    }
}
