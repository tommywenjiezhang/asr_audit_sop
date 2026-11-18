//! Demo data for the user-guide screenshots (test-only).
//!
//!   cargo test --no-default-features -- --ignored write_docs_fixtures
//!
//! Runs the REAL backend logic (extraction, classification, badges, prompt) on FICTIONAL emails in an
//! in-memory database and writes docs/guide/demo/fixtures.json for the mocked UI. No real data is read.

use crate::completion::{self, ItemResult};
use crate::settings::{FolderRef, Settings};
use crate::tickets::{self, IncomingMessage, ListQuery, TicketPatch};
use serde_json::{json, Value};

const STORE: &str = "DEMO-STORE";
const STAFF: &str = "Sam Staff";

fn folder(id: &str, path: &str) -> FolderRef {
    FolderRef { store_id: STORE.into(), entry_id: id.into(), path: path.into() }
}

#[allow(clippy::too_many_arguments)]
fn mail(n: u32, conv: &str, folder_id: &str, folder_path: &str, when: &str, from: (&str, &str), subject: &str, body: &str) -> IncomingMessage {
    IncomingMessage {
        store_id: STORE.into(),
        entry_id: format!("DEMO-ENTRY-{n}"),
        internet_message_id: format!("<demo-{n}@example.org>"),
        conversation_id: conv.into(),
        folder_entry_id: folder_id.into(),
        folder_path: folder_path.into(),
        subject: subject.into(),
        sender_name: from.0.into(),
        sender_email: from.1.into(),
        received_at: when.into(),
        body: body.replace("\n", "\r\n"),
        folder_ancestors: ancestors_of(folder_id),
        ..Default::default()
    }
}

/// Parent chain of the demo folders (Inbox > My Folder > Needs Subtype / Verify Receipt).
fn ancestors_of(folder_id: &str) -> Vec<String> {
    let v: &[&str] = match folder_id {
        "F-SUB" | "F-VER" => &["F-MY", "F-INBOX", "F-ROOT"],
        "F-MY" | "F-TA" | "F-TB" | "F-DONE" => &["F-INBOX", "F-ROOT"],
        _ => &["F-ROOT"],
    };
    v.iter().map(|s| s.to_string()).collect()
}

fn node(name: &str, id: &str, path: &str, children: Vec<Value>) -> Value {
    json!({"name": name, "entry_id": id, "store_id": STORE, "path": path, "children": children})
}

#[test]
#[ignore]
fn write_docs_fixtures() {
    let c = crate::db::open_in_memory().unwrap();
    crate::templates_store::seed_embedded(&c).unwrap();

    let inbox = folder("F-INBOX", "Inbox");
    let my = folder("F-MY", "Inbox/My Folder");
    let done_folder = folder("F-DONE", "Inbox/Completed");
    let mut s = Settings::default();
    s.mailbox_name = "Help Desk Shared Mailbox (demo)".into();
    s.mailbox_store_id = STORE.into();
    s.folder = Some(inbox.clone());
    s.subfolder_mode = "selected".into();
    s.selected_subfolders = vec![my.clone()];
    s.selected_with_children = vec![my.entry_id.clone()];
    s.archive_folder = Some(done_folder.clone());
    s.archive_mailbox_name = s.mailbox_name.clone();
    s.staff_name = STAFF.into();
    s.sync_enabled = true;
    s.internal_senders = vec!["@helpdesk.example.gov".into()];

    s.notify_account = "sam.staff@helpdesk.example.gov".into();
    // staff and their folders (owner of an email = staff folder it is in, including subfolders)
    let staff_input = |name: &str, email: &str, id: &str, path: &str| crate::staff::StaffInput {
        id: None, display_name: name.into(), email: email.into(), mailbox_name: "Help Desk Shared Mailbox (demo)".into(),
        store_id: STORE.into(), folder_entry_id: id.into(), folder_path: path.into(), windows_account: String::new(),
    };
    let sam = crate::staff::save(&c, &staff_input("Sam Staff", "sam.staff@helpdesk.example.gov", "F-MY", "Inbox/My Folder")).unwrap();
    let ta = crate::staff::save(&c, &staff_input("Teammate A", "teammate.a@helpdesk.example.gov", "F-TA", "Inbox/Teammate A")).unwrap();
    crate::staff::save(&c, &staff_input("Teammate B", "teammate.b@helpdesk.example.gov", "F-TB", "Inbox/Teammate B")).unwrap();
    crate::staff::set_current_staff_id(&c, Some(sam.id)).unwrap();

    let sub = "Inbox/My Folder/Needs Subtype";
    let ver = "Inbox/My Folder/Verify Receipt";
    let msgs = vec![
        mail(1, "C-1", "F-SUB", sub, "2026-11-10T14:05:00Z", ("Rivera, Jordan", "jrivera@mapleleaf.example.org"),
            "School Subtype Error on the ASR",
            "Good morning,\n\nWhen I open the ASR it says \"School subtype is required\" and I cannot continue.\n\
             School Name: Maple Leaf Academy\nSchool NJIIS ID: 0012345\nAddress: 12 Oak Street, Trenton, NJ 08608\n\n\
             We are a private school with Kindergarten through Grade 5.\n\n\
             Thank you,\nJordan Rivera\nSchool Nurse\nPhone: (609) 555-0142 ext. 214\n"),
        mail(2, "C-2", "F-INBOX", "Inbox", "2026-11-12T15:30:00Z", ("Priya Shah", "pshah@brookside.example.org"),
            "Webinar black screen and subtype question",
            "Hello,\n\nThe ASR training webinar shows a black screen and skips to the end. Our school is also missing the subtype.\n\
             School NJIIS ID: 0045678 for Brookside Elementary School.\n\nThanks,\nPriya Shah\nHealth Office Secretary\n555-0188\n"),
        mail(3, "C-3", "F-MY", "Inbox/My Folder", "2026-11-13T13:10:00Z", ("Marcus Lee", "mlee@hillcrest.example.org"),
            "New user account",
            "Hi,\n\nI am the new school nurse and I need an account in NJIIS. I do not have an account yet.\n\
             First Name: Marcus\nLast Name: Lee\nEmail: mlee@hillcrest.example.org\n\nSchool Name: Hillcrest Middle School\n\n\
             Best regards,\nMarcus Lee\nSchool Nurse\n(856) 555-0107\n"),
        mail(4, "C-4", "F-MY", "Inbox/My Folder", "2026-11-14T16:45:00Z", ("Elena Gomez", "egomez@district.example.org"),
            "ASR access for two of our schools",
            "Hello Help Desk,\n\nI cannot open the ASR for two of our schools. I get \"Access denied\".\n\n\
             School Name: Riverside Preschool\nSchool NJIIS ID: 0067890\nASR ID: R-2041\n\n\
             School Name: Oak Hill Academy\nSchool NJIIS ID: 0078901\n\n\
             Oak Hill Academy is in Mercer County.\n\nRegards,\nElena Gomez\nDistrict Nurse Coordinator\nPhone: 609-555-0123 x31\n"),
        mail(5, "C-5", "F-VER", ver, "2026-11-11T19:20:00Z", ("Tom Baker", "tbaker@pinegrove.example.org"),
            "Did you receive our ASR?",
            "Hi,\n\nCan you confirm that you received our ASR? We submitted it last week.\nSchool Name: Pine Grove School\nSchool NJIIS ID: 0034567\n\nThanks,\nTom Baker\n"),
        mail(6, "C-5", "F-VER", ver, "2026-11-15T13:02:00Z", ("Tom Baker", "tbaker@pinegrove.example.org"),
            "RE: Did you receive our ASR?",
            "Just following up - we also added Grade 6 this year.\n\nFrom: Tom Baker\nSent: Tuesday\nCan you confirm that you received our ASR?\n"),
        mail(7, "C-7", "F-MY", "Inbox/My Folder", "2026-11-09T12:00:00Z", ("Ava Chen", "achen@lakeside.example.org"),
            "User guide",
            "Hello,\n\nWhere can I find the user guide for the ASR? Thank you!\n\nAva Chen\n"),
        mail(8, "C-8", "F-INBOX", "Inbox", "2026-11-16T17:40:00Z", ("Noah Patel", "npatel@sunrise.example.org"),
            "Question",
            "Hi, I have a question about our report from last year. Who should I contact?\n\nNoah Patel\n"),
        mail(9, "C-9", "F-MY", "Inbox/My Folder", "2026-11-17T14:15:00Z", ("Grace Kim", "gkim@meadow.example.org"),
            "Save ASR as PDF",
            "Good afternoon,\n\nHow can I save a copy of the ASR form as a PDF for our records?\nSchool Name: Meadow View School\n\nThank you,\nGrace Kim\nSchool Nurse\n"),
    ];
    tickets::ingest(&c, &msgs, &s).unwrap();

    // staff work on ticket 1 (Maple Leaf Academy)
    tickets::set_field(&c, 1, "sender_title", Some("School Nurse".into()), true, STAFF).unwrap();
    tickets::set_field(&c, 1, "school_njiis_id", Some("0012345".into()), true, STAFF).unwrap();
    tickets::update(&c, 1, &TicketPatch {
        staff_actions: Some("Checked NJIIS: the subtype field is empty for this school.".into()),
        internal_notes: Some("School confirmed by phone that they are a private school.".into()),
        ..Default::default()
    }, STAFF).unwrap();
    tickets::record_prompt(&c, 1, &crate::prompt::build_prompt(&c, 1, &s).unwrap(), STAFF).unwrap();
    // the webinar email also mentions a missing subtype: staff adds the second issue type
    tickets::set_issues(&c, 2, &["webinar_issue".into(), "school_subtype_missing".into()], STAFF).unwrap();

    // ticket for "Ava Chen" was already finished with Mark Done
    let done_id: i64 = c.query_row("SELECT id FROM tickets WHERE sender_email='achen@lakeside.example.org'", [], |r| r.get(0)).unwrap();
    let p = completion::pending(&c, done_id).unwrap();
    completion::begin_attempt(&c, done_id, &p).unwrap();
    let res: Vec<ItemResult> = p
        .iter()
        .map(|i| ItemResult { message_id: i.message_id, flagged: true, moved: true, new_entry_id: Some(format!("{}-DONE", i.entry_id)),
            new_store_id: Some(STORE.into()), new_folder_entry_id: Some("F-DONE".into()), new_folder_path: Some("Inbox/Completed".into()), ..Default::default() })
        .collect();
    completion::apply_results(&c, done_id, &done_folder, &p, &res, STAFF).unwrap();

    // assignments (simulated Outlook results - nothing real is moved or sent)
    use crate::assignment::{self as asg, Begin, MoveResult, NotifyOutcome};
    let sc = crate::scope::cfg(&s);
    let assign = |tid: i64, to: i64, folder: &str, path: &str, note: &str, notify: Option<NotifyOutcome>| {
        let Begin::Op(op) = asg::begin(&c, tid, to, STAFF, note).unwrap() else { return };
        let pend = asg::pending_moves(&c, op).unwrap();
        let res: Vec<MoveResult> = pend.iter().map(|m| MoveResult { message_id: m.message_id, moved: true, new_entry_id: Some(format!("{}-A", m.entry_id)),
            new_store_id: Some(STORE.into()), new_folder_entry_id: Some(folder.into()), new_folder_path: Some(path.into()), ancestors: ancestors_of(folder), ..Default::default() }).collect();
        asg::apply_moves(&c, op, &sc, &pend, &res).unwrap();
        if let Some(n) = notify {
            asg::claim_notification(&c, op).unwrap();
            asg::record_notification(&c, op, n).unwrap();
        }
    };
    // the webinar ticket in the Inbox goes to Sam: moved into My Folder (stays current); the notification failed
    assign(2, sam.id, "F-MY", "Inbox/My Folder", "Please check the webinar settings with the school.", Some(NotifyOutcome::Failed("Outlook is not running.".into())));
    // the new-user ticket is handed to Teammate A: moved outside the selected folders -> Ticket History
    let marcus: i64 = c.query_row("SELECT id FROM tickets WHERE sender_email='mlee@hillcrest.example.org'", [], |r| r.get(0)).unwrap();
    assign(marcus, ta.id, "F-TA", "Inbox/Teammate A", "Teammate A handles new accounts this week.",
        Some(NotifyOutcome::Submitted { entry_id: Some("DEMO-NOTE".into()), account: "sam.staff@helpdesk.example.gov".into() }));
    // Sam started work on the Maple Leaf ticket
    let _ = crate::tickets::start_work(&c, 1, STAFF);

    let all = tickets::list(&c, &ListQuery { limit: 1000, ..Default::default() }).unwrap();
    let mut details = serde_json::Map::new();
    let mut prompts = serde_json::Map::new();
    for t in &all.rows {
        details.insert(t.id.to_string(), serde_json::to_value(tickets::detail(&c, t.id).unwrap()).unwrap());
        prompts.insert(t.id.to_string(), Value::String(crate::prompt::build_prompt(&c, t.id, &s).unwrap()));
    }
    let templates = crate::templates_store::list(&c, None).unwrap();
    let catalog = {
        let mut v = serde_json::to_value(crate::catalog::catalog()).unwrap();
        v["statuses"] = json!(tickets::STATUSES);
        v["priorities"] = json!(tickets::PRIORITIES);
        v
    };
    let inbox_tree = node("Inbox", "F-INBOX", "Inbox", vec![
        node("My Folder", "F-MY", "Inbox/My Folder", vec![
            node("Needs Subtype", "F-SUB", sub, vec![]),
            node("Verify Receipt", "F-VER", ver, vec![]),
        ]),
        node("Teammate A", "F-TA", "Inbox/Teammate A", vec![node("Training", "F-TA1", "Inbox/Teammate A/Training", vec![])]),
        node("Teammate B", "F-TB", "Inbox/Teammate B", vec![]),
        node("Completed", "F-DONE", "Inbox/Completed", vec![]),
    ]);
    let mailbox_tree = node("Help Desk Shared Mailbox (demo)", "F-ROOT", "", vec![
        inbox_tree.clone(),
        node("Drafts", "F-DRAFTS", "Drafts", vec![]),
        node("Sent Items", "F-SENT", "Sent Items", vec![]),
        node("Deleted Items", "F-DEL", "Deleted Items", vec![]),
    ]);
    let out = json!({
        "_note": "FICTIONAL demo data for user-guide screenshots. Generated by src-tauri/src/demo.rs.",
        "catalog": catalog,
        "settings": s,
        "default_prompt": crate::settings::DEFAULT_PROMPT_INSTRUCTIONS,
        "scan_summary": s.scan_summary(),
        "tickets": all.rows,
        "details": details,
        "prompts": prompts,
        "templates": templates,
        "report": crate::export::report(&c).unwrap(),
        "staff": crate::staff::list(&c, true).unwrap(),
        "current_staff_id": sam.id,
        "overview": crate::overview::overview(&c, &s, "2026-10-18T04:00:00Z", "2026-11-18T05:00:00Z").unwrap(),
        "scope_counts": crate::scope::counts(&c).unwrap(),
        "accounts": [{"display_name": "Sam Staff", "smtp": "sam.staff@helpdesk.example.gov"}],
        "mailboxes": [
            {"display_name": "sam.staff@helpdesk.example.gov", "store_id": "DEMO-OWN"},
            {"display_name": "Help Desk Shared Mailbox (demo)", "store_id": STORE},
        ],
        "mailbox_tree": mailbox_tree,
        "inbox_tree": inbox_tree,
    });
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/guide/demo/fixtures.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    println!("wrote {} ({} tickets)", path.display(), all.rows.len());
}
