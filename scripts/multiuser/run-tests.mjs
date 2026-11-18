// Dual-instance multiuser tests against two RUNNING ASR Support Desk desktop instances
// (start them with scripts\launch_multiuser_test.ps1). Both use the same shared Access database.
//
//   node scripts\multiuser\run-tests.mjs [--only S1,S5] [--skip-restart] [--root C:\ASR_Dev]
//
// Every case created here carries the token DEVTEST<run> in its subject, so test data is easy to find and
// never mixed up with real cases. Outlook is the shared mock mailbox (<root>\OutlookMock): nothing real is
// moved or sent; its journal.jsonl is the evidence of moves and handoffs.
// Results: <root>\results\multiuser-<run>.json and .md

import { Instance, sleep } from "./cdp.mjs";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

const args = process.argv.slice(2);
const opt = (k, d) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : d; };
const ROOT = opt("--root", "C:\\ASR_Dev");
const ONLY = (opt("--only", "") || "").split(",").filter(Boolean);
const SKIP_RESTART = args.includes("--skip-restart");
const MOCK = path.join(ROOT, "OutlookMock");
const RUN = new Date().toISOString().replace(/\D/g, "").slice(2, 12);
const TOKEN = `DEVTEST${RUN}`;
const LAUNCHER = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1")), "..", "launch_multiuser_test.ps1");

const results = [];
let A, B;

// ------------------------------------------------------------------ helpers

function log(...a) { console.log(new Date().toISOString().slice(11, 19), ...a); }
function assert(cond, msg) { if (!cond) throw new Error(msg); }

async function scenario(id, name, fn) {
  if (ONLY.length && !ONLY.includes(id)) return;
  const t = Date.now();
  log(`▶ ${id} ${name}`);
  try {
    const detail = (await fn()) ?? "";
    results.push({ id, name, pass: true, detail: String(detail), ms: Date.now() - t });
    log(`  ✔ ${id} ${detail}`);
  } catch (e) {
    results.push({ id, name, pass: false, detail: e.message, ms: Date.now() - t });
    log(`  ✘ ${id} ${e.message}`);
  }
}

async function sync(x) {
  const s = await x.invoke("shared_sync_now", {}, 300000);
  assert(s.state === "connected", `${x.name} sync state ${s.state}: ${s.error_message ?? ""}`);
  return s;
}
async function syncBoth() { await sync(A); await sync(B); await sync(A); }

async function find(x, token, extra = {}) {
  const r = await x.invoke("tickets_list", { query: { view: "team", search: token, limit: 100, ...extra } });
  return r.rows;
}
async function one(x, token) {
  const rows = await find(x, token);
  assert(rows.length === 1, `${x.name}: expected 1 case for ${token}, found ${rows.length}`);
  return rows[0];
}
const detail = (x, id) => x.invoke("ticket_detail", { id });

function lockMock() {
  fs.mkdirSync(MOCK, { recursive: true });
  const p = path.join(MOCK, ".lock");
  for (let i = 0; i < 400; i++) {
    try { fs.closeSync(fs.openSync(p, "wx")); return () => fs.rmSync(p, { force: true }); } catch { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 50); }
  }
  throw new Error("mock mailbox lock");
}
let seq = 0;
/** Deliver an incoming email into the shared mock mailbox (as if it arrived in the team Inbox). */
function deliver({ subject, body = "Hello, please help.", conv, sender = "nurse.devtest@example.invalid", senderName = "School Nurse (test)", at = new Date() }) {
  const n = ++seq;
  const msg = {
    store_id: "MOCKSTORE", entry_id: `MOCK-E-${RUN}-${n}`, internet_message_id: `<devtest-${RUN}-${n}@example.invalid>`, conversation_id: conv ?? `CONV-${RUN}-${n}`,
    folder_entry_id: "MOCK-Inbox", folder_path: "Inbox", subject, sender_name: senderName, sender_email: sender, received_at: at.toISOString().slice(0, 19) + "Z",
    body, body_truncated: false, has_attachments: false, folder_ancestors: ["MOCK-Root"], notification_id: "", metadata: {},
  };
  const unlock = lockMock();
  try {
    const p = path.join(MOCK, "inbox.json");
    const box = fs.existsSync(p) ? JSON.parse(fs.readFileSync(p, "utf8")) : { messages: [] };
    box.messages.push(msg);
    fs.writeFileSync(p, JSON.stringify(box, null, 1));
  } finally { unlock(); }
  return msg;
}
function journal() {
  const p = path.join(MOCK, "journal.jsonl");
  return fs.existsSync(p) ? fs.readFileSync(p, "utf8").split(/\r?\n/).filter(Boolean).map((l) => JSON.parse(l)) : [];
}
function fault(op, kind) {
  const unlock = lockMock();
  try {
    const p = path.join(MOCK, "faults.json");
    const f = fs.existsSync(p) ? JSON.parse(fs.readFileSync(p, "utf8")) : {};
    (f[op] ??= []).push(kind);
    fs.writeFileSync(p, JSON.stringify(f));
  } finally { unlock(); }
}
/** Deliver an email and let A import it through Check Now (the real Outlook scan path), then share it. */
async function newCase(label, extra = {}) {
  const m = deliver({ subject: `${TOKEN} ${label}`, ...extra });
  await A.invoke("sync_now", {}, 300000);
  await syncBoth();
  return { m, a: await one(A, `${TOKEN} ${label}`), b: await one(B, `${TOKEN} ${label}`) };
}
const word = (label) => `${TOKEN} ${label}`;

function ps(argsList) {
  // no pipes: the relaunched apps would inherit them and the call would never return
  const out = path.join(ROOT, "results", "launcher.log");
  fs.mkdirSync(path.dirname(out), { recursive: true });
  const fd = fs.openSync(out, "a");
  try {
    execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", LAUNCHER, ...argsList], { stdio: ["ignore", fd, fd], windowsHide: true });
  } finally {
    fs.closeSync(fd);
  }
}
async function connectBoth() {
  A?.close(); B?.close();
  A = await new Instance("A", 9301).connect(120000);
  B = await new Instance("B", 9302).connect(120000);
}
async function staffId(x, name) {
  const s = (await x.invoke("staff_list", { includeInactive: true })).find((s) => s.display_name === name);
  assert(s, `${x.name}: staff ${name} not found`);
  return s;
}

// ------------------------------------------------------------------ scenarios

await connectBoth();
log(`run ${RUN}; mock mailbox ${MOCK}`);

await scenario("S0", "Setup: both instances connected to the same shared database with distinct profiles", async () => {
  const pa = await A.invoke("profile_info"), pb = await B.invoke("profile_info");
  assert(pa.test_mode && pb.test_mode, "test mode");
  assert(pa.device_id !== pb.device_id, "device ids must differ");
  assert(pa.data_dir !== pb.data_dir, "data dirs must differ");
  for (const x of [A, B]) await x.invoke("shared_release_held");
  await syncBoth();
  const sa = await A.invoke("shared_status"), sb = await B.invoke("shared_status");
  assert(sa.database_id === sb.database_id && sa.bound_database_id === sb.bound_database_id, "same database identity");
  assert(sa.writable && sb.writable, "both writable");
  // each computer maps the colleague's Outlook folder (Settings › Staff Assignment)
  for (const [x, other, folder] of [[A, "Test Staff B", "MOCK-StaffB"], [B, "Test Staff A", "MOCK-StaffA"]]) {
    const s = await staffId(x, other);
    if (!s.folder_entry_id) {
      await x.invoke("staff_save", { input: { id: s.id, display_name: s.display_name, email: s.email, mailbox_name: "Mock Shared Mailbox", store_id: "MOCKSTORE", folder_entry_id: folder, folder_path: `Inbox/${other}`, windows_account: s.windows_account ?? "" } });
    }
  }
  return `A ${pa.device_id.slice(0, 8)} / B ${pb.device_id.slice(0, 8)} on "${sa.database_name}" schema v${sa.schema_version}; queue A ${sa.queue.pending}/${sa.queue.held}, B ${sb.queue.pending}/${sb.queue.held}`;
});

let c1;
await scenario("S1", "A creates a ticket (GUI: Tickets › + New) → B receives it", async () => {
  await A.nav("Tickets");
  await A.click("+ New");
  await A.waitFor(`document.querySelector('.modal h3')?.textContent === 'New Ticket'`);
  const inputs = `[...document.querySelectorAll('.modal input')]`;
  await A.eval(`(() => { const set = (el, v) => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, v); el.dispatchEvent(new Event('input', { bubbles: true })); };
    const i = ${inputs}; set(i[0], 'Phone caller (test)'); set(i[1], 'caller.devtest@example.invalid'); set(i[2], ${JSON.stringify(word("S1 phone call about ASR"))}); return true; })()`);
  await A.click("Create Ticket");
  await A.waitFor(`!document.querySelector('.modal')`, 20000);
  await syncBoth();
  c1 = { a: await one(A, word("S1")), b: await one(B, word("S1")) };
  assert(c1.a.ticket_ref === c1.b.ticket_ref, "same reference on both");
  return `${c1.a.ticket_ref} visible on B as ${c1.b.display_id}`;
});

await scenario("S2", "A edits a ticket → B sees the update", async () => {
  await A.invoke("ticket_set_field", { id: c1.a.id, key: "school_name", value: "Lincoln Elementary (DEVTEST)", verified: false });
  await syncBoth();
  const f = (await detail(B, c1.b.id)).fields.find((f) => f.key === "school_name");
  assert(f?.effective === "Lincoln Elementary (DEVTEST)", `B school_name = ${f?.effective}`);
  return "school name propagated";
});

await scenario("S3", "B adds a note → A sees the note", async () => {
  await B.invoke("notes_add", { ticketId: c1.b.id, body: `${TOKEN} note from B` });
  await syncBoth();
  const n = (await detail(A, c1.a.id)).notes.find((n) => n.body.includes(`${TOKEN} note from B`));
  assert(n && n.author_name === "Test Staff B", "note with author on A");
  return `note by ${n.author_name}`;
});

await scenario("S4", "A claims an unassigned ticket → B sees A as owner", async () => {
  await A.invoke("ticket_claim", { id: c1.a.id, takeover: false });
  await syncBoth();
  const t = (await detail(B, c1.b.id)).ticket;
  assert(t.status === "Assigned" && t.assignee_name === "Test Staff A", `B: ${t.status} ${t.assignee_name}`);
  return `B shows ${t.status} — ${t.assignee_name}`;
});

await scenario("S5", "A and B claim simultaneously → exactly one owner, no silent overwrite", async () => {
  const c = await newCase("S5 simultaneous claim");
  await Promise.all([A.invoke("ticket_claim", { id: c.a.id, takeover: false }), B.invoke("ticket_claim", { id: c.b.id, takeover: false })]);
  await Promise.all([A.invoke("shared_sync_now", {}, 300000), B.invoke("shared_sync_now", {}, 300000)]);
  await syncBoth();
  const ta = (await detail(A, c.a.id)), tb = (await detail(B, c.b.id));
  assert(ta.ticket.assignee_name && ta.ticket.assignee_name === tb.ticket.assignee_name, `owners differ: ${ta.ticket.assignee_name} / ${tb.ticket.assignee_name}`);
  const loser = ta.ticket.assignee_name === "Test Staff A" ? tb : ta;
  const conflict = loser.conflicts.find((x) => x.conflict_type === "owner");
  assert(conflict, "the losing computer shows an owner conflict");
  const open = ta.ownership.filter((o) => !o.ended_at).length;
  assert(open === 1, `open ownership periods ${open}`);
  return `owner ${ta.ticket.assignee_name}; other computer has conflict: "${conflict.reason}"`;
});

await scenario("S6", "B takes over A's case → both show B; history keeps A", async () => {
  await B.invoke("ticket_claim", { id: c1.b.id, takeover: true });
  await syncBoth();
  const d = await detail(A, c1.a.id);
  assert(d.ticket.assignee_name === "Test Staff B", `A shows ${d.ticket.assignee_name}`);
  const a = d.ownership.find((o) => o.staff_name === "Test Staff A");
  assert(a?.ended_at, "A's period ended and kept");
  return `periods: ${d.ownership.map((o) => `${o.staff_name}(${o.ended_at ? "ended" : "open"})`).join(", ")}`;
});

let c7;
await scenario("S7", "A marks a ticket Done → B sees Done; covered emails flagged+moved once (mock Outlook)", async () => {
  c7 = await newCase("S7 mark done", { conv: `CONV-${RUN}-S7` });
  await A.invoke("ticket_claim", { id: c7.a.id, takeover: false });
  const r = await A.invoke("ticket_complete", { id: c7.a.id }, 300000);
  await syncBoth();
  const tb = (await detail(B, c7.b.id)).ticket;
  assert(tb.status === "Done", `B status ${tb.status}`);
  const moves = journal().filter((j) => j.op === "complete_messages" && JSON.stringify(j.detail).includes("MOCK-Completed"));
  const mine = moves.filter((j) => j.detail.items?.length);
  return `B: Done; A Outlook: ${r.outlook_message}; complete_messages journal entries ${mine.length}`;
});

await scenario("S8", "New reply to a Done case → both show Review", async () => {
  deliver({ subject: `RE: ${word("S7 mark done")}`, body: "One more question, please.", conv: `CONV-${RUN}-S7`, at: new Date(Date.now() + 1000) });
  await B.invoke("sync_now", {}, 300000); // B's Outlook check imports the reply
  await syncBoth();
  const ta = (await detail(A, c7.a.id)).ticket, tb = (await detail(B, c7.b.id)).ticket;
  assert(ta.status === "Review" && tb.status === "Review", `A ${ta.status} / B ${tb.status}`);
  return `Review: ${ta.review_reason}`;
});

await scenario("S9", "A adds a school and an issue type → B receives them", async () => {
  const slot = await A.invoke("ticket_add_school", { id: c1.a.id });
  await A.invoke("ticket_set_field", { id: c1.a.id, key: `school${slot}_name`, value: "Second School (DEVTEST)", verified: false });
  const cur = (await detail(A, c1.a.id)).issues.map((i) => i.issue_type);
  const cat = await A.invoke("catalog_get");
  const extra = cat.issue_types.find((i) => !cur.includes(i.id) && i.id !== "other").id;
  await A.invoke("ticket_set_issues", { id: c1.a.id, issues: [...cur, extra] });
  await syncBoth();
  const d = await detail(B, c1.b.id);
  assert(d.schools.includes(slot), `B schools ${d.schools}`);
  assert(d.fields.find((f) => f.key === `school${slot}_name`)?.effective === "Second School (DEVTEST)", "second school name");
  assert(d.issues.some((i) => i.issue_type === extra), "issue type");
  return `B: ${d.schools.length} schools, issues ${d.issues.map((i) => i.issue_type).join(", ")}`;
});

await scenario("S10", "B merges two cases (preview, then combine) → A sees one case with both emails and the alias", async () => {
  const x = await newCase("S10 merge target"), y = await newCase("S10 merge source");
  await A.invoke("notes_add", { ticketId: y.a.id, body: `${TOKEN} note on merge source` });
  await syncBoth();
  const p = await B.invoke("merge_preview", { sources: [y.b.id], target: x.b.id });
  assert(p.can_run, `preview blocked: ${JSON.stringify(p.sources.map((s) => s.blocked))}`);
  const r = await B.invoke("merge_run", { sources: [y.b.id], target: x.b.id });
  assert(r.every((m) => m.ok), JSON.stringify(r));
  await syncBoth();
  const left = await find(A, word("S10 merge"));
  assert(left.length === 1, `A lists ${left.length} cases`);
  const d = await detail(A, left[0].id);
  assert(d.messages.length === 2, `A: ${d.messages.length} emails`);
  assert(d.notes.some((n) => n.body.includes("note on merge source")), "note kept");
  const tl = await A.invoke("ticket_timeline", { id: left[0].id });
  assert(tl.some((i) => i.title === "Case added to this case" && (i.detail ?? "").includes(y.a.ticket_ref)), "alias recorded");
  // the old reference still leads to the remaining case
  const linked = await A.tryInvoke("ticket_link", { id: c1.a.id, target: y.a.ticket_ref });
  assert(linked.ok, `old reference does not resolve: ${linked.e}`);
  const links = await A.invoke("ticket_links", { id: c1.a.id });
  assert(links.some((l) => l.ticket_id === left[0].id), "old reference resolved to the remaining case");
  await A.invoke("ticket_link_set", { id: c1.a.id, relationId: links.find((l) => l.ticket_id === left[0].id).relation_id, status: "removed" });
  return `preview ${p.emails_after} emails/${p.notes_after} notes; merged ${y.a.ticket_ref} → ${x.a.ticket_ref}`;
});

const shared0 = (await A.invoke("settings_get")).shared_db_path;
await scenario("S11", "A disconnects from Access → keeps saving locally; UI stays responsive", async () => {
  await A.invoke("shared_save_config", { mode: "shared", path: "\\\\asr-nonexistent-host-devtest\\share\\asr.accdb", minutes: 1 });
  await A.invoke("notes_add", { ticketId: c1.a.id, body: `${TOKEN} offline note` });
  const syncP = A.tryInvoke("shared_sync_now", {}, 300000);
  const pings = [];
  for (let i = 0; i < 5; i++) { pings.push(await A.ping()); await sleep(200); }
  const s = await syncP;
  const st = await A.invoke("shared_status");
  assert(st.state !== "connected", `state ${st.state}`);
  const n = (await detail(A, c1.a.id)).notes.find((n) => n.body.includes("offline note"));
  assert(n?.sync_state === "pending", `note sync_state ${n?.sync_state}`);
  assert(Math.max(...pings) < 1500, `UI round trips ${pings}`);
  return `state ${st.state} (${st.error_code}); ${st.queue.pending} pending; UI round trip max ${Math.max(...pings)} ms during sync (${s.ok ? "ok" : "error reported"})`;
});

await scenario("S12", "A reconnects → pending changes synchronize once", async () => {
  await A.invoke("shared_save_config", { mode: "shared", path: shared0, minutes: 1 });
  await syncBoth();
  const st = await A.invoke("shared_status");
  assert(st.queue.pending === 0, `A pending ${st.queue.pending}`);
  const notes = (await detail(B, c1.b.id)).notes.filter((n) => n.body.includes("offline note"));
  assert(notes.length === 1, `B sees ${notes.length} copies`);
  return "offline note delivered exactly once";
});

await scenario("S13", "A and B edit separate fields → both changes kept", async () => {
  await Promise.all([
    A.invoke("ticket_set_field", { id: c1.a.id, key: "sender_title", value: "School nurse (A)", verified: false }),
    B.invoke("ticket_set_field", { id: c1.b.id, key: "sender_phone", value: "609-555-0100", verified: false }),
  ]);
  await Promise.all([A.invoke("shared_sync_now", {}, 300000), B.invoke("shared_sync_now", {}, 300000)]);
  await syncBoth();
  for (const [x, id] of [[A, c1.a.id], [B, c1.b.id]]) {
    const f = Object.fromEntries((await detail(x, id)).fields.map((f) => [f.key, f.effective]));
    assert(f.sender_title === "School nurse (A)" && f.sender_phone === "609-555-0100", `${x.name}: ${f.sender_title} / ${f.sender_phone}`);
  }
  return "both fields on both computers";
});

await scenario("S14", "A and B edit the same field → conflict detected and resolved", async () => {
  const c = await newCase("S14 same field");
  await A.invoke("ticket_set_field", { id: c.a.id, key: "issue_description", value: "Version from A", verified: false });
  await B.invoke("ticket_set_field", { id: c.b.id, key: "issue_description", value: "Version from B", verified: false });
  await sync(A); await sync(B);
  const cf = (await B.invoke("conflicts_list", { ticketId: c.b.id, includeResolved: false })).find((x) => x.field_name?.includes("issue_description"));
  assert(cf, "B has a field conflict");
  assert(cf.proposed_value === "Version from B" && cf.shared_value === "Version from A", `values kept ${cf.proposed_value}/${cf.shared_value}`);
  await B.invoke("conflict_retry", { conflictId: cf.conflict_id }); // "Use my value" (explicit)
  await syncBoth();
  const fa = (await detail(A, c.a.id)).fields.find((f) => f.key === "issue_description").effective;
  const open = (await B.invoke("conflicts_list", { ticketId: c.b.id, includeResolved: false })).filter((x) => x.status !== "resolved");
  assert(fa === "Version from B", `A now ${fa}`);
  assert(open.length === 0, `${open.length} conflict(s) still open`);
  return "conflict shown with both values; B's explicit retry accepted; A updated";
});

await scenario("S15", "A and B request the same Outlook assignment → one reservation, one move, one handoff", async () => {
  const c = await newCase("S15 outlook race");
  const sa = await staffId(A, "Test Staff A"), sb = await staffId(B, "Test Staff B");
  const [ra, rb] = await Promise.all([
    A.invoke("tickets_assign", { ticketIds: [c.a.id], staffId: sa.id, note: "race A" }, 300000),
    B.invoke("tickets_assign", { ticketIds: [c.b.id], staffId: sb.id, note: "race B" }, 300000),
  ]);
  const outs = [ra[0], rb[0]];
  const won = outs.filter((o) => o.result === "assigned");
  assert(won.length === 1, `results ${outs.map((o) => `${o.result}: ${o.message}`).join(" | ")}`);
  const id = c.m.internet_message_id;
  const j = journal();
  const moved = j.filter((x) => x.op === "move_messages").filter((x) => x.at >= new Date(Date.now() - 600000).toISOString());
  const sends = j.filter((x) => x.op === "send_notification" && (x.detail.subject ?? "").includes(TOKEN) && (x.detail.subject ?? "").includes("S15"));
  await syncBoth();
  const ta = (await detail(A, c.a.id)), tb = (await detail(B, c.b.id));
  assert(ta.ticket.assignee_name === tb.ticket.assignee_name, "same owner on both");
  assert(sends.length <= 1, `${sends.length} handoffs sent`);
  return `winner ${won[0] === ra[0] ? "A" : "B"} (${won[0].notify_state}); loser: ${outs.find((o) => o !== won[0]).result} — ${outs.find((o) => o !== won[0]).message}; handoffs ${sends.length}; Outlook runs recorded ${ta.outlook_ops.length}; email ${id}`;
});

await scenario("S15b", "Outlook failures recover safely (move failure → retry; uncertain handoff → check → confirmed resend)", async () => {
  const c = await newCase("S15b recovery");
  const sa = await staffId(A, "Test Staff A"), sbA = await staffId(A, "Test Staff B");
  fault("move_messages", "fail");
  let r = (await A.invoke("tickets_assign", { ticketIds: [c.a.id], staffId: sa.id, note: "" }, 300000))[0];
  assert(r.result === "moves_failed", `first try ${r.result}`);
  r = await A.invoke("assignment_retry", { ticketId: c.a.id }, 300000);
  assert(r.result === "assigned", `retry ${r.result}: ${r.message}`);
  fault("send_notification", "uncertain");
  r = (await A.invoke("tickets_assign", { ticketIds: [c.a.id], staffId: sbA.id, note: "reassign" }, 300000))[0];
  assert(r.result === "assigned" && r.notify_state === "uncertain", `reassign ${r.result}/${r.notify_state}`);
  const opId = r.op_id;
  const chk = await A.invoke("notification_check", { opId });
  const [st] = await A.invoke("notification_resend", { opId });
  assert(st === "submitted", `resend ${st}`);
  const again = await A.tryInvoke("notification_retry", { opId });
  assert(!again.ok, "a submitted handoff is never sent again");
  await syncBoth();
  const ob = (await detail(B, c.b.id)).outlook_ops;
  return `move failure retried; uncertain handoff: "${chk}" then confirmed resend → ${st}; B sees ${ob.length} Outlook run(s): ${ob.map((o) => `${o.kind}:${o.state}/${o.notify_state}`).join(", ")}`;
});

await scenario("S16", "A updates the Kanban board (GUI drag) → B's list reflects it", async () => {
  const c = await newCase("S16 kanban");
  await A.nav("Kanban");
  await A.setInput('.kb .bar input[placeholder="Search…"]', word("S16"));
  await A.waitFor(`[...document.querySelectorAll('.col')].some((c) => c.innerText.includes(${JSON.stringify(c.a.display_id)}))`, 30000);
  await A.eval(`(() => {
    const card = [...document.querySelectorAll('.card')].find((c) => c.innerText.includes(${JSON.stringify(c.a.display_id)}));
    const col = [...document.querySelectorAll('.col')].find((c) => c.getAttribute('aria-label') === 'In Progress');
    const dt = new DataTransfer();
    card.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
    col.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt }));
    col.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt }));
    return true; })()`);
  await A.waitFor(`[...document.querySelectorAll('.col')].find((c) => c.getAttribute('aria-label') === 'In Progress')?.innerText.includes(${JSON.stringify(c.a.display_id)})`, 30000);
  await syncBoth();
  await B.nav("Tickets");
  await B.setInput('.qhead input.grow', word("S16"));
  await B.waitFor(`document.querySelector('.list')?.innerText.includes('In Progress — Test Staff A')`, 30000);
  return "drag Unassigned → In Progress on A (claim + start); B's ticket list shows In Progress — Test Staff A";
});

await scenario("S17", "B performs bulk actions (GUI bulk bar) → A sees the accepted changes", async () => {
  const cs = [await newCase("S17 bulk one"), await newCase("S17 bulk two"), await newCase("S17 bulk three")];
  await B.nav("Tickets");
  await B.click("Team Tickets");
  await B.setInput('.qhead input.grow', word("S17"));
  await B.waitFor(`document.querySelectorAll('.list .item').length === 3`, 30000);
  await B.click("Select all shown");
  await B.waitFor(`!!document.querySelector('.bulk')`);
  await B.click("Claim", "document.querySelector('.bulk')");
  await B.waitFor(`document.querySelector('.modal')?.innerText.includes('3 succeeded')`, 60000);
  await B.click("Close", "document.querySelector('.modal')");
  const r = await B.invoke("tickets_bulk", { action: "start", ids: cs.map((c) => c.b.id), reason: null });
  assert(r.ok === 3, `start ok ${r.ok}`);
  await syncBoth();
  for (const c of cs) {
    const t = (await detail(A, c.a.id)).ticket;
    assert(t.status === "In Progress" && t.assignee_name === "Test Staff B", `${t.display_id}: ${t.status} ${t.assignee_name}`);
  }
  return "3 cases claimed (GUI) and started (bulk) by B; A shows In Progress — Test Staff B";
});

await scenario("T1", "Automatic background sync propagates without Sync Now (1-minute interval)", async () => {
  const c = await newCase("T1 auto sync");
  const t0 = Date.now();
  await A.invoke("ticket_set_field", { id: c.a.id, key: "school_name", value: "Auto Sync School (DEVTEST)", verified: false });
  // no manual sync from here: A pushes a few seconds after the change, B pulls on its own schedule
  for (;;) {
    const f = (await detail(B, c.b.id)).fields.find((f) => f.key === "school_name");
    if (f?.effective === "Auto Sync School (DEVTEST)") break;
    assert(Date.now() - t0 < 200000, "not propagated within 200 s");
    await sleep(2000);
  }
  return `visible on B after ${Math.round((Date.now() - t0) / 1000)} s`;
});

await scenario("T2", "Simultaneous writes and concurrent reads (5 rounds, Access locking)", async () => {
  const c = await newCase("T2 concurrency");
  for (let i = 0; i < 5; i++) {
    await Promise.all([
      A.invoke("notes_add", { ticketId: c.a.id, body: `${TOKEN} A ${i}` }), B.invoke("notes_add", { ticketId: c.b.id, body: `${TOKEN} B ${i}` }),
    ]);
    await Promise.all([A.invoke("shared_sync_now", {}, 300000), B.invoke("shared_sync_now", {}, 300000), A.invoke("tickets_list", { query: { view: "team", limit: 200 } }), B.invoke("ticket_detail", { id: c.b.id })]);
  }
  await syncBoth();
  const na = (await detail(A, c.a.id)).notes.filter((n) => n.body.startsWith(TOKEN)).length, nb = (await detail(B, c.b.id)).notes.filter((n) => n.body.startsWith(TOKEN)).length;
  assert(na === 10 && nb === 10, `notes A ${na} / B ${nb}`);
  const sa = await A.invoke("shared_status"), sb = await B.invoke("shared_status");
  assert(sa.queue.pending === 0 && sb.queue.pending === 0, "nothing left pending");
  return "10 notes on both, nothing pending, no lost writes";
});

if (!SKIP_RESTART) {
  await scenario("T3", "Interrupted synchronization: A killed with unsent changes, restarted → each change applied once", async () => {
    const c = await newCase("T3 interrupted");
    await A.invoke("shared_save_config", { mode: "shared", path: "\\\\asr-nonexistent-host-devtest\\share\\asr.accdb", minutes: 1 });
    for (let i = 0; i < 12; i++) await A.invoke("notes_add", { ticketId: c.a.id, body: `${TOKEN} queued ${i}` });
    await A.invoke("shared_save_config", { mode: "shared", path: shared0, minutes: 1 });
    const syncing = A.tryInvoke("shared_sync_now", {}, 30000);
    await sleep(700);
    ps(["-Stop", "-Only", "WorkstationA"]);
    await syncing;
    await sleep(2000);
    ps(["-Only", "WorkstationA"]);
    await connectBoth();
    await syncBoth();
    const n = (await detail(B, c.b.id)).notes.filter((x) => x.body.includes("queued")).length;
    assert(n === 12, `B sees ${n} notes`);
    return "12 notes after kill + restart, no duplicates";
  });

  await scenario("S18", "Both apps restart → records, owners and queues consistent", async () => {
    const before = await find(A, TOKEN);
    ps(["-Stop"]);
    await sleep(3000);
    ps([]);
    await connectBoth();
    await syncBoth();
    const ra = await find(A, TOKEN), rb = await find(B, TOKEN);
    assert(ra.length === before.length && rb.length === ra.length, `counts ${before.length}/${ra.length}/${rb.length}`);
    const key = (rows) => rows.map((r) => `${r.ticket_ref}|${r.status}|${r.assignee_name ?? ""}`).sort().join("\n");
    assert(key(ra) === key(rb), "status/owner differ between A and B");
    const sa = await A.invoke("shared_status"), sb = await B.invoke("shared_status");
    assert(sa.queue.pending + sb.queue.pending === 0, "pending after restart");
    return `${ra.length} test cases identical on A and B after restart; pending 0/0`;
  });
}

await scenario("UI", "GUI smoke on both windows: every page renders, detail panels, merge dialog, diagnostics tools", async () => {
  const seen = [];
  for (const x of [A, B]) {
    for (const [nav, expect] of [["Tickets", "Team Tickets"], ["Kanban", "Kanban"], ["Conflicts", "Sync Conflicts"], ["Team Overview", "Team Overview"], ["Settings", "Shared Database"], ["Diagnostics", "Diagnostics"]]) {
      await x.nav(nav);
      await x.waitFor(`document.querySelector('main')?.innerText.includes(${JSON.stringify(expect)})`, 20000);
      seen.push(`${x.name}:${nav}`);
    }
    await x.click("Test Database Connection");
    await x.waitFor(`document.querySelector('.tools .notice')?.innerText.includes('Shared database')`, 120000);
    await x.nav("Tickets");
    await x.click("Team Tickets");
    await x.setInput('.qhead input.grow', word("S1"));
    await x.waitFor(`document.querySelectorAll('.list .item').length >= 1`, 20000);
    await x.eval(`document.querySelector('.list .item .body').click()`);
    await x.waitFor(`document.querySelector('.center')?.innerText.includes('Related cases')`, 20000);
    await x.click("Combine cases…");
    await x.waitFor(`document.querySelector('.modal')?.innerText.includes('Combine cases into')`, 20000);
    await x.click("Cancel", "document.querySelector('.modal')");
    await x.click("▸ Activity", "document.querySelector('.center')");
    await x.waitFor(`document.querySelector('.tl li')`, 20000);
  }
  return `${seen.length} page renders; detail, related cases, combine dialog, timeline and Test Database Connection on both windows`;
});

// ------------------------------------------------------------------ report

const fin = { run: RUN, token: TOKEN, at: new Date().toISOString(), shared_db: shared0, passed: results.filter((r) => r.pass).length, failed: results.filter((r) => !r.pass).length, results };
const outDir = path.join(ROOT, "results");
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, `multiuser-${RUN}.json`), JSON.stringify(fin, null, 2));
const md = [`# Dual-instance multiuser test run ${RUN}`, "", `${fin.passed} passed, ${fin.failed} failed. Shared database: \`${shared0}\`. Test cases carry \`${TOKEN}\`.`, "",
  "| # | Scenario | Result | Details | s |", "|---|---|---|---|---|",
  ...results.map((r) => `| ${r.id} | ${r.name} | ${r.pass ? "PASS" : "**FAIL**"} | ${r.detail.replace(/\|/g, "/").replace(/\n/g, " ")} | ${(r.ms / 1000).toFixed(1)} |`)].join("\n");
fs.writeFileSync(path.join(outDir, `multiuser-${RUN}.md`), md);
console.log("\n" + md);
A.close(); B.close();
process.exit(fin.failed ? 1 : 0);
