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
    pub confidence: f64,
    pub scores: Vec<(String, f64)>,
    pub fields: Vec<Extracted>,
    pub school_ids: Vec<String>,
    pub facility_ids: Vec<String>,
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
re!(ERR_LINE, r"(?i)\b(error|access denied|denied|unable to|cannot|can't|failed|failure|not working|invalid|black screen|not authorized|unauthorized)\b");
re!(TRAIN_DONE, r"(?i)\b(?:i|we|she|he|they|staff|nurse)?\s*(?:have|has|had)?\s*(?:completed|finished|took|taken|done|watched|attended)\s+(?:the\s+|my\s+|our\s+)?(?:asr\s+|audit\s+|on-demand\s+|online\s+)?(?:training|webinar)");
re!(TRAIN_NOT, r"(?i)\b(?:have\s*n[o']t|has\s*n[o']t|did\s*n[o']t|not yet|haven't|hasn't|didn't)\s+(?:yet\s+)?(?:completed|finished|taken|took|done|watched)\s+(?:the\s+)?(?:asr\s+|audit\s+)?training");
re!(DATE, r"(?i)\b(\d{1,2}/\d{1,2}/\d{2,4}|(?:jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\.?\s+\d{1,2}(?:st|nd|rd|th)?,?\s*(?:\d{4})?)\b");
re!(GRADE_NUM, r"(?i)\b(?:grade\s*(\d{1,2})|(\d{1,2})(?:st|nd|rd|th)\s*grade)\b");
re!(LHD, r"\b((?:[A-Z][A-Za-z.&'\-]*\s+){1,5}(?:County\s+)?(?:Health\s+Department|Health\s+Dept\.?|Department\s+of\s+Health|Board\s+of\s+Health|Health\s+Division))\b");
re!(LABELED, r"(?im)^\s*(first\s*name|last\s*name|e-?mail(?:\s*address)?|work\s*phone|phone(?:\s*number)?)\s*:\s*(.{1,120}?)\s*$");
re!(SUBMITTED_CLAIM, r"(?i)\b(?:i|we)\s+(?:have\s+)?(?:submitted|completed and submitted|sent in|entered)\b");
re!(EMAIL, r"(?i)\b[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}\b");
re!(GREETING, r"(?i)^\s*(hi|hello|hey|good (morning|afternoon|evening)|dear|greetings)\b");

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
        .filter(|l| !l.is_empty() && !GREETING.is_match(l))
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

    // identifiers: current message > subject > quoted history
    let mut school_ids = ids(&SCHOOL_ID, &current);
    let mut id_src = "current";
    if school_ids.is_empty() {
        school_ids = ids(&SCHOOL_ID, subject);
        id_src = "subject";
    }
    if school_ids.is_empty() {
        school_ids = ids(&SCHOOL_ID, &quoted);
        id_src = "quoted";
    }
    let facility_ids = {
        let mut v = ids(&FACILITY_ID, &format!("{subject}\n{current}"));
        if v.is_empty() {
            v = ids(&FACILITY_ID, &quoted);
        }
        v
    };
    if let Some(id) = school_ids.first() {
        push(&mut f, "school_njiis_id", id, if id_src == "quoted" { "low" } else { "high" }, id_src);
    } else {
        // unlabeled "ID: 12345" -> low confidence, only if it is not a facility ID
        let generic: Vec<String> = ids(&GENERIC_ID, &format!("{subject}\n{current}"))
            .into_iter()
            .filter(|i| !facility_ids.contains(i))
            .collect();
        if let Some(id) = generic.first() {
            push(&mut f, "school_njiis_id", id, "low", "current");
            school_ids = generic;
        }
    }
    if school_ids.len() > 1 {
        push(&mut f, "additional_ids", &school_ids[1..].join(", "), "medium", "current");
    }
    if let Some(id) = facility_ids.first() {
        push(&mut f, "facility_id", id, "high", "current");
    }

    if let Some(n) = first_school_name(&current).or_else(|| first_school_name(subject)) {
        push(&mut f, "school_name", &n, "medium", "current");
    } else if let Some(n) = first_school_name(&quoted) {
        push(&mut f, "school_name", &n, "low", "quoted");
    }

    let cur_subj = format!("{subject}\n{current}");
    if let Some(c) = SUBTYPE.captures(&cur_subj).or_else(|| SUBTYPE2.captures(&current)) {
        let t = c[1].to_lowercase();
        let v = match t.as_str() { "charter" => "Charter", "public" => "Public", _ => "Private" };
        push(&mut f, "school_type", v, "medium", "current");
    }

    // exact error message: quoted text first, then the first line describing an error
    if let Some(c) = QUOTED_ERR.captures(&cur_subj) {
        push(&mut f, "error_message", &c[1], "high", "current");
    } else if let Some(line) = before_signoff(&current).lines().map(str::trim).find(|l| ERR_LINE.is_match(l) && l.len() <= 400) {
        push(&mut f, "error_message", line, "medium", "current");
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

    Analysis { issue_type, confidence, scores, fields: f, school_ids, facility_ids }
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
        assert_eq!(a.get("additional_ids"), Some("22222"));
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
