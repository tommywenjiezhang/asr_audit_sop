//! Advanced case management in the shared database (P2): links between cases, correcting which case an
//! email belongs to, and the school/field part of combining cases.
//!
//!   relation.link        {other_ticket_id, relation_type = "related", evidence}   (op ticket = case A)
//!   relation.set_status  {relation_id, status: "linked" | "dismissed" | "removed"}
//!   email.move           {email_id, target_ticket_id}  the email leaves the op's case and joins the target
//!
//! Nothing here deletes data: relation and email-association changes are status changes, and every change
//! writes activity on the case(s) involved. Links are never created automatically from a matching sender
//! or school; "possible duplicate" relations are only suggestions for review.

use crate::shared_apply::{self as sa, OpResult, TicketCore, WorkflowTx};
use crate::shared_store::{col, col_i, col_s, SResult, Val};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RelationRec {
    pub relation_id: String,
    pub ticket_a: String,
    pub ticket_b: String,
    /// "related" | "possible_duplicate"
    pub relation_type: String,
    /// "linked" | "open" (suggestion) | "dismissed" | "removed"
    pub status: String,
    pub evidence: Option<String>,
    pub created_at: String,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(String::from).filter(|x| !x.is_empty())
}

/// Follow merge redirects (bounded) to the case that exists now.
pub fn resolve(tx: &mut WorkflowTx<'_, '_>, id: &str) -> SResult<Option<String>> {
    let mut cur = id.to_string();
    for _ in 0..16 {
        let r = tx.query("SELECT merged_into FROM Tickets WHERE ticket_id = ?", &[cur.clone().into()])?;
        match r.first() {
            None => return Ok(None),
            Some(row) => match col(row, 0).filter(|x| !x.is_empty()) {
                Some(next) => cur = next,
                None => return Ok(Some(cur)),
            },
        }
    }
    Ok(None)
}

fn reject(res: &mut OpResult, msg: impl Into<String>) {
    res.result = "rejected".into();
    res.message = msg.into();
}

pub fn apply(tx: &mut WorkflowTx<'_, '_>, op_type: &str, tid: &str, res: &mut OpResult) -> SResult<bool> {
    match op_type {
        "relation.link" => link(tx, tid, res)?,
        "relation.set_status" => set_status(tx, tid, res)?,
        "email.move" => email_move(tx, tid, res)?,
        _ => return Ok(false),
    }
    Ok(true)
}

fn link(tx: &mut WorkflowTx<'_, '_>, a: &str, res: &mut OpResult) -> SResult<()> {
    let p = tx.op().proposed.clone();
    let Some(other) = s(&p, "other_ticket_id") else {
        reject(res, "No case to link.");
        return Ok(());
    };
    let Some(b) = resolve(tx, &other)? else {
        reject(res, "The other case does not exist in the shared database yet.");
        return Ok(());
    };
    if b == a {
        reject(res, "A case cannot be linked to itself.");
        return Ok(());
    }
    let kind = s(&p, "relation_type").unwrap_or_else(|| "related".into());
    let existing = tx.query(
        "SELECT relation_id, relation_status FROM TicketRelations WHERE relation_type = ? AND ((ticket_a = ? AND ticket_b = ?) OR (ticket_a = ? AND ticket_b = ?))",
        &[kind.clone().into(), a.into(), b.clone().into(), b.clone().into(), a.into()],
    )?;
    let now = tx.now();
    let bref = tx.core(&b)?.ticket_ref;
    match existing.first() {
        Some(r) if col_s(r, 1) == "linked" => {
            res.extra = json!({"relation_id": col_s(r, 0), "unchanged": true});
            return Ok(());
        }
        Some(r) => {
            tx.exec("UPDATE TicketRelations SET relation_status = 'linked' WHERE relation_id = ?", &[col_s(r, 0).into()])?;
            res.extra = json!({"relation_id": col_s(r, 0)});
        }
        None => {
            let rid = s(&p, "relation_id").unwrap_or_else(sa::new_id);
            tx.exec(
                "INSERT INTO TicketRelations (relation_id, ticket_a, ticket_b, relation_type, evidence, relation_status, created_at, created_by_device) VALUES (?, ?, ?, ?, ?, 'linked', ?, ?)",
                &[rid.clone().into(), a.into(), b.clone().into(), sa::cut(&kind, 32).into(), s(&p, "evidence").map(Val::Text).unwrap_or(Val::Null), now.into(), tx.op().device_id.clone().into()],
            )?;
            res.extra = json!({"relation_id": rid});
        }
    }
    tx.act(a, "Linked to related case", None, None, Some(bref));
    tx.touch(a);
    tx.touch(&b);
    Ok(())
}

fn set_status(tx: &mut WorkflowTx<'_, '_>, tid: &str, res: &mut OpResult) -> SResult<()> {
    let p = tx.op().proposed.clone();
    let (Some(rid), Some(status)) = (s(&p, "relation_id"), s(&p, "status")) else {
        reject(res, "Relation and status are required.");
        return Ok(());
    };
    if !matches!(status.as_str(), "linked" | "dismissed" | "removed") {
        reject(res, "Unknown relation status.");
        return Ok(());
    }
    let Some(r) = tx.query("SELECT ticket_a, ticket_b, relation_status, relation_type FROM TicketRelations WHERE relation_id = ?", &[rid.clone().into()])?.first().cloned() else {
        reject(res, "The link does not exist.");
        return Ok(());
    };
    if col_s(&r, 2) == status {
        return Ok(());
    }
    tx.exec("UPDATE TicketRelations SET relation_status = ? WHERE relation_id = ?", &[status.clone().into(), rid.into()])?;
    let label = match (col_s(&r, 3).as_str(), status.as_str()) {
        (_, "linked") => "Linked to related case",
        ("possible_duplicate", "dismissed") => "Possible duplicate dismissed",
        _ => "Case link removed",
    };
    let (a, b) = (col_s(&r, 0), col_s(&r, 1));
    let other = if a == tid { b.clone() } else { a.clone() };
    let oref = tx.query("SELECT ticket_ref FROM Tickets WHERE ticket_id = ?", &[other.clone().into()])?.first().map(|r| col_s(r, 0));
    tx.act(tid, label, None, None, oref);
    tx.touch(&a);
    tx.touch(&b);
    Ok(())
}

/// Correct an email association: the email leaves the op's case and becomes part of the target case.
/// The old association stays recorded (inactive). A Done target with a newer substantive email goes to
/// Review through the normal "email attached" rule.
fn email_move(tx: &mut WorkflowTx<'_, '_>, src: &str, res: &mut OpResult) -> SResult<()> {
    let p = tx.op().proposed.clone();
    let (Some(eid), Some(target_in)) = (s(&p, "email_id"), s(&p, "target_ticket_id")) else {
        reject(res, "Email and target case are required.");
        return Ok(());
    };
    let Some(target) = resolve(tx, &target_in)? else {
        reject(res, "The target case does not exist in the shared database yet.");
        return Ok(());
    };
    if target == src {
        return Ok(());
    }
    let link = tx.query("SELECT link_type, is_active FROM TicketEmails WHERE ticket_id = ? AND email_id = ?", &[src.into(), eid.clone().into()])?;
    let Some(l) = link.first().cloned() else {
        reject(res, "This email is not part of the case (it may already have been moved).");
        return Ok(());
    };
    if col_i(&l, 1) == 0 {
        reject(res, "This email was already moved to another case.");
        return Ok(());
    }
    let active_left = tx.query("SELECT email_id FROM TicketEmails WHERE ticket_id = ? AND is_active = 1", &[src.into()])?.len();
    if active_left <= 1 && p.get("allow_empty").and_then(|v| v.as_bool()) != Some(true) {
        reject(res, "This is the only email of the case. Use Add to existing case to combine the cases instead.");
        return Ok(());
    }
    let now = tx.now();
    let dev = tx.op().device_id.clone();
    tx.exec("UPDATE TicketEmails SET is_active = 0 WHERE ticket_id = ? AND email_id = ?", &[src.into(), eid.clone().into()])?;
    let link_type = if col_s(&l, 0) == "original" { "reply".to_string() } else { col_s(&l, 0) };
    if tx.query("SELECT email_id FROM TicketEmails WHERE ticket_id = ? AND email_id = ?", &[target.clone().into(), eid.clone().into()])?.is_empty() {
        tx.exec(
            "INSERT INTO TicketEmails (ticket_id, email_id, link_type, is_active, linked_at, linked_by_device) VALUES (?, ?, ?, 1, ?, ?)",
            &[target.clone().into(), eid.clone().into(), link_type.into(), now.into(), dev.into()],
        )?;
    } else {
        tx.exec("UPDATE TicketEmails SET is_active = 1 WHERE ticket_id = ? AND email_id = ?", &[target.clone().into(), eid.clone().into()])?;
    }
    let tref = tx.core(&target)?.ticket_ref;
    let subj = tx.query("SELECT subject, received_at, sender_name, sender_email, is_internal, is_auto_reply FROM Emails WHERE email_id = ?", &[eid.clone().into()])?.first().cloned();
    tx.act(src, "Email moved to another case", None, subj.as_ref().map(|r| sa::cut(&col_s(r, 0), 200)), Some(tref));
    // the target case reacts like to any attached email (a Done case reopens on a newer reply)
    if let Some(r) = subj {
        let e = sa::EmailRec {
            email_id: eid.clone(), subject: col_s(&r, 0), received_at: col_s(&r, 1), sender_name: col_s(&r, 2), sender_email: col_s(&r, 3),
            is_internal: col_i(&r, 4) != 0, is_auto_reply: col_i(&r, 5) != 0, link_type: "reply".into(), is_active: true, ..Default::default()
        };
        crate::shared_workflow::after_email_attached(tx, &target, &eid, &e)?;
    }
    tx.touch(src);
    tx.touch(&target);
    res.extra = json!({"target_ticket_id": target});
    Ok(())
}

// ------------------------------------------------------------------ merge: schools and fields

fn field_map(tx: &mut WorkflowTx<'_, '_>, tid: &str) -> SResult<Vec<(String, Option<String>, Option<String>, bool, bool)>> {
    Ok(tx
        .query("SELECT field_key, extracted_value, manual_value, has_manual, is_verified FROM TicketFields WHERE ticket_id = ?", &[tid.into()])?
        .iter()
        .map(|r| (col_s(r, 0), col(r, 1), col(r, 2), col_i(r, 3) != 0, col_i(r, 4) != 0))
        .collect())
}

/// (slot, attr) of a school field key: school_name -> (1, name); school3_njiis_id -> (3, njiis_id).
pub fn school_slot_of(key: &str) -> Option<(i64, String)> {
    let rest = key.strip_prefix("school")?;
    if let Some(attr) = rest.strip_prefix('_') {
        return Some((1, attr.to_string()));
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let attr = rest[digits.len()..].strip_prefix('_')?;
    Some((digits.parse().ok()?, attr.to_string()))
}

fn eff(extracted: &Option<String>, manual: &Option<String>, has_manual: bool) -> String {
    if has_manual { manual.clone().unwrap_or_default() } else { extracted.clone().unwrap_or_default() }.trim().to_string()
}

/// Copy the source case's schools (as additional school groups) and any field the target does not have
/// into the target. Target values are never overwritten; differing values are recorded in the activity.
pub fn merge_schools_and_fields(tx: &mut WorkflowTx<'_, '_>, src: &TicketCore, target: &TicketCore) -> SResult<usize> {
    let sf = field_map(tx, &src.ticket_id)?;
    let tf = field_map(tx, &target.ticket_id)?;
    let src_slots: Vec<i64> = {
        let rows = tx.query("SELECT slot_no FROM TicketSchools WHERE ticket_id = ? AND is_removed = 0 ORDER BY slot_no", &[src.ticket_id.clone().into()])?;
        let mut v: Vec<i64> = rows.iter().map(|r| col_i(r, 0)).collect();
        if !v.contains(&1) {
            v.insert(0, 1);
        }
        v
    };
    let tgt_slots_all: Vec<i64> = tx.query("SELECT slot_no FROM TicketSchools WHERE ticket_id = ?", &[target.ticket_id.clone().into()])?.iter().map(|r| col_i(r, 0)).collect();
    let tgt_school = |slot: i64, attr: &str| -> String {
        let key = crate::analyzer::school_key(slot, attr);
        tf.iter().find(|f| f.0 == key).map(|f| eff(&f.1, &f.2, f.3)).unwrap_or_default()
    };
    let tgt_active: Vec<i64> = {
        let rows = tx.query("SELECT slot_no FROM TicketSchools WHERE ticket_id = ? AND is_removed = 0", &[target.ticket_id.clone().into()])?;
        let mut v: Vec<i64> = rows.iter().map(|r| col_i(r, 0)).collect();
        if !v.contains(&1) {
            v.push(1);
        }
        v
    };
    let mut next = tgt_slots_all.iter().copied().max().unwrap_or(1).max(1) + 1;
    let now = tx.now();
    let by = sa::cut(&tx.op().staff_name, 100);
    let mut copied = 0;
    for slot in src_slots {
        let vals: Vec<(String, (Option<String>, Option<String>, bool, bool))> = sf
            .iter()
            .filter_map(|f| school_slot_of(&f.0).filter(|(sl, _)| *sl == slot).map(|(_, attr)| (attr, (f.1.clone(), f.2.clone(), f.3, f.4))))
            .collect();
        if vals.iter().all(|(_, v)| eff(&v.0, &v.1, v.2).is_empty()) {
            continue;
        }
        let get = |attr: &str| vals.iter().find(|(a, _)| a == attr).map(|(_, v)| eff(&v.0, &v.1, v.2)).unwrap_or_default();
        let (sid, sname) = (get("njiis_id"), get("name").to_lowercase());
        // already in the target (same NJIIS ID, or same name when there is no ID)
        let dup = tgt_active.iter().any(|t| {
            let tid = tgt_school(*t, "njiis_id");
            if !sid.is_empty() && !tid.is_empty() {
                tid == sid
            } else {
                !sname.is_empty() && tgt_school(*t, "name").to_lowercase() == sname
            }
        });
        if dup {
            continue;
        }
        // an empty first school group in the target takes the school; otherwise a new group
        let target_slot = if tgt_active.contains(&1) && ["njiis_id", "name"].iter().all(|a| tgt_school(1, a).is_empty()) && copied == 0 { 1 } else { let n = next; next += 1; n };
        if tx.query("SELECT slot_no FROM TicketSchools WHERE ticket_id = ? AND slot_no = ?", &[target.ticket_id.clone().into(), target_slot.into()])?.is_empty() {
            tx.exec("INSERT INTO TicketSchools (ticket_id, slot_no, is_removed) VALUES (?, ?, 0)", &[target.ticket_id.clone().into(), target_slot.into()])?;
        } else {
            tx.exec("UPDATE TicketSchools SET is_removed = 0 WHERE ticket_id = ? AND slot_no = ?", &[target.ticket_id.clone().into(), target_slot.into()])?;
        }
        for (attr, (ex, man, has, ver)) in &vals {
            let key = crate::analyzer::school_key(target_slot, attr);
            tx.exec("DELETE FROM TicketFields WHERE ticket_id = ? AND field_key = ?", &[target.ticket_id.clone().into(), key.clone().into()])?;
            tx.exec(
                "INSERT INTO TicketFields (ticket_id, field_key, extracted_value, manual_value, has_manual, is_verified, row_version, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
                &[target.ticket_id.clone().into(), key.into(), ex.clone().map(Val::Text).unwrap_or(Val::Null), man.clone().map(Val::Text).unwrap_or(Val::Null), (*has).into(), (*ver).into(), now.clone().into(), by.clone().into()],
            )?;
        }
        let label = if !sid.is_empty() { format!("{} ({sid})", get("name")) } else { get("name") };
        tx.act(&target.ticket_id, "School added from combined case", None, Some(src.ticket_ref.clone()), Some(label));
        copied += 1;
    }
    // other (non-school) fields: fill gaps only
    for (key, ex, man, has, ver) in sf.iter().filter(|f| school_slot_of(&f.0).is_none()) {
        let v = eff(ex, man, *has);
        if v.is_empty() {
            continue;
        }
        match tf.iter().find(|f| &f.0 == key) {
            None => {
                tx.exec(
                    "INSERT INTO TicketFields (ticket_id, field_key, extracted_value, manual_value, has_manual, is_verified, row_version, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
                    &[target.ticket_id.clone().into(), key.clone().into(), ex.clone().map(Val::Text).unwrap_or(Val::Null), man.clone().map(Val::Text).unwrap_or(Val::Null), (*has).into(), (*ver).into(), now.clone().into(), by.clone().into()],
                )?;
            }
            Some(t) if eff(&t.1, &t.2, t.3).is_empty() => {
                tx.exec(
                    "UPDATE TicketFields SET extracted_value = ?, manual_value = ?, has_manual = ?, is_verified = ?, row_version = row_version + 1, updated_at = ?, updated_by = ? WHERE ticket_id = ? AND field_key = ?",
                    &[ex.clone().map(Val::Text).unwrap_or(Val::Null), man.clone().map(Val::Text).unwrap_or(Val::Null), (*has).into(), (*ver).into(), now.clone().into(), by.clone().into(), target.ticket_id.clone().into(), key.clone().into()],
                )?;
            }
            Some(t) if eff(&t.1, &t.2, t.3) != v => {
                tx.act(&target.ticket_id, "Combined case had a different value (target value kept)", Some(key), Some(eff(&t.1, &t.2, t.3)), Some(v));
            }
            _ => {}
        }
    }
    Ok(copied)
}

/// Relations involving the given cases (for mirroring).
pub fn relations_for(c: &mut dyn crate::shared_store::SharedConn, ids: &[String]) -> SResult<Vec<RelationRec>> {
    let mut out: Vec<RelationRec> = vec![];
    for chunk in ids.chunks(20) {
        let ph = vec!["?"; chunk.len()].join(", ");
        let mut p: Vec<Val> = chunk.iter().map(|x| x.clone().into()).collect();
        p.extend(chunk.iter().map(|x| Val::from(x.clone())));
        for r in c.query(&format!("SELECT relation_id, ticket_a, ticket_b, relation_type, relation_status, evidence, created_at FROM TicketRelations WHERE ticket_a IN ({ph}) OR ticket_b IN ({ph})"), &p)? {
            let rec = RelationRec { relation_id: col_s(&r, 0), ticket_a: col_s(&r, 1), ticket_b: col_s(&r, 2), relation_type: col_s(&r, 3), status: col_s(&r, 4), evidence: col(&r, 5), created_at: col_s(&r, 6) };
            if !out.iter().any(|x| x.relation_id == rec.relation_id) {
                out.push(rec);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn school_keys_parse() {
        assert_eq!(school_slot_of("school_name"), Some((1, "name".into())));
        assert_eq!(school_slot_of("school3_njiis_id"), Some((3, "njiis_id".into())));
        assert_eq!(school_slot_of("school_type"), Some((1, "type".into())));
        assert_eq!(school_slot_of("schoolyear"), None);
        assert_eq!(school_slot_of("contact_name"), None);
    }
}
