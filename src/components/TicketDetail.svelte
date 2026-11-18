<script lang="ts">
  import { api, condMatches, copyText, errMsg, fmtDate, reasonLabel, statusBadge, statusText, SYNC_LABEL, type AssignmentOp, type FieldDef, type LocalConflict, type TicketDetail } from "../lib/api";
  import { app, issueName, toast } from "../lib/stores.svelte";
  import FieldInput from "./FieldInput.svelte";

  let { id, onchanged, onassign, onopen }: { id: number; onchanged: () => void; onassign?: (ids: number[]) => void; onopen?: (id: number) => void } = $props();

  type Msg = { kind: "ok" | "err" | "info"; text: string } | null;

  let d = $state<TicketDetail | null>(null);
  let err = $state("");
  let prompt = $state("");
  let promptErr = $state("");
  let busy = $state<"" | "copy" | "reply" | "both" | "done" | "reopen" | "assign" | "start" | "notify" | "claim" | "review" | "archive" | "join">("");
  let me = $state<number | null>(null);
  api.currentStaff().then((v) => (me = v)).catch(() => {});
  /** with a confirmed shared database, Outlook Assign/Reassign waits for P1 (reserved moves) */
  const sharedBound = $derived(app.shared?.mode === "shared" && !!app.shared?.bound_database_id);
  let joinTarget = $state("");
  let showJoin = $state(false);
  let assignMsg = $state<Msg>(null);
  let copyMsg = $state<Msg>(null);
  let outlookMsg = $state<Msg>(null);
  let doneMsg = $state<Msg>(null);
  let failures = $state<{ message_id: number; subject: string; error: string }[]>([]);
  let notFound = $state(false);
  let openMsg = $state<number | null>(null);
  let showHistory = $state(false);
  let addIssue = $state("");

  /** saves that are still running; actions wait for them so the prompt uses the latest data */
  const pending = new Set<Promise<unknown>>();
  function track<T>(p: Promise<T>): Promise<T> {
    pending.add(p);
    p.finally(() => pending.delete(p)).catch(() => {});
    return p;
  }
  async function settle() {
    (document.activeElement as HTMLElement | null)?.blur?.();
    await new Promise((r) => setTimeout(r, 0));
    while (pending.size) await Promise.allSettled([...pending]);
  }

  async function loadPrompt() {
    try {
      prompt = await api.prompt(id);
      promptErr = "";
    } catch (e) {
      promptErr = errMsg(e);
    }
  }
  async function load() {
    try {
      d = await api.ticket(id);
      err = "";
    } catch (e) {
      err = errMsg(e);
    }
    await loadPrompt();
  }
  $effect(() => {
    void id;
    d = null; prompt = ""; copyMsg = null; outlookMsg = null; doneMsg = null; failures = []; notFound = false; openMsg = null; assignMsg = null;
    load();
  });

  const values = $derived(Object.fromEntries((d?.fields ?? []).map((f) => [f.key, f.effective])) as Record<string, string>);
  const field = (key: string) => d?.fields.find((x) => x.key === key);
  const schoolKey = (slot: number, attr: string) => (slot <= 1 ? `school_${attr}` : `school${slot}_${attr}`);
  const def = (key: string, label: string, type = "text"): FieldDef => ({ key, label, type });
  const SCHOOL_ATTRS: [string, string, string][] = [
    ["name", "School name", "text"], ["njiis_id", "NJIIS identifier", "identifier"], ["other_ids", "Other school ID(s)", "text"],
    ["address", "School address", "text"], ["municipality", "Municipality", "text"], ["county", "County", "text"], ["zip", "ZIP code", "identifier"],
  ];
  // catalog fields that are not shown in the main groups (issue-specific details, subtype, LHD, ...)
  const MAIN_KEYS = new Set(["school_njiis_id", "school_name", "error_message", "issue_description"]);
  const issueIds = $derived(d?.issues.length ? d.issues.map((i) => i.issue_type) : d ? [d.ticket.issue_type] : []);
  const moreFields = $derived.by(() => {
    if (!app.catalog) return [] as FieldDef[];
    const seen = new Set<string>();
    const out: FieldDef[] = [];
    for (const f of app.catalog.common_fields) if (!MAIN_KEYS.has(f.key) && !seen.has(f.key)) { seen.add(f.key); out.push(f); }
    for (const id of issueIds) {
      const it = app.catalog.issue_types.find((i) => i.id === id);
      for (const f of it?.fields ?? []) if (!MAIN_KEYS.has(f.key) && !seen.has(f.key) && condMatches(f.show_if, values)) { seen.add(f.key); out.push(f); }
    }
    return out;
  });
  const incoming = $derived((d?.messages ?? []).filter((m) => !m.is_internal && m.entry_id));
  const chronological = $derived([...(d?.messages ?? [])].sort((a, b) => (a.received_at ?? "").localeCompare(b.received_at ?? "") || a.id - b.id));
  const completionOf = (mid: number) => d?.completion.find((c) => c.message_id === mid);
  const isDone = $derived(d?.ticket.status === "Done");

  async function saveField(key: string, value: string | null, verified: boolean) {
    try {
      await track(api.setField(id, key, value, verified));
      await load();
      onchanged();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
  async function patch(p: Record<string, unknown>) {
    try {
      await track(api.updateTicket(id, p));
      await load();
      onchanged();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
  async function saveIssues(list: string[]) {
    try {
      await track(api.setIssues(id, list));
      await load();
      onchanged();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
  async function addSchool() {
    try { await track(api.addSchool(id)); await load(); } catch (e) { toast(errMsg(e), "error"); }
  }
  async function removeSchool(slot: number) {
    if (!confirm(`Remove school ${slot} from this ticket? Its values stay in the ticket history.`)) return;
    try { await track(api.removeSchool(id, slot)); await load(); } catch (e) { toast(errMsg(e), "error"); }
  }

  let actions = $state("");
  $effect(() => { actions = d?.ticket.staff_actions ?? ""; });

  // ---------------------------------------------------------------- notes (one record per note; shared)
  let newNote = $state("");
  let editNote = $state<{ id: string; body: string } | null>(null);
  async function addNote() {
    if (!newNote.trim()) return;
    try { await track(api.notesAdd(id, newNote)); newNote = ""; await load(); onchanged(); } catch (e) { toast(errMsg(e), "error"); }
  }
  async function saveNoteEdit() {
    if (!editNote) return;
    try { await track(api.notesEdit(editNote.id, editNote.body)); editNote = null; await load(); } catch (e) { toast(errMsg(e), "error"); }
  }
  async function deleteNote(noteId: string) {
    if (!confirm("Delete this note? It stays in the case history as deleted.")) return;
    try { await track(api.notesDelete(noteId)); await load(); } catch (e) { toast(errMsg(e), "error"); }
  }

  // ---------------------------------------------------------------- shared-database conflicts
  function conflictLabel(c: LocalConflict): string {
    const f = c.field_name ?? "";
    if (f.startsWith("field:")) return `Field “${f.slice(6)}”`;
    if (f.startsWith("note:")) return "Note";
    if (f === "staff_actions") return "Staff actions already taken";
    if (f === "issues") return "Issue types";
    return f || c.conflict_type;
  }
  async function acceptShared(c: LocalConflict) {
    try { await api.conflictAcceptShared(c.conflict_id); await load(); onchanged(); toast("Kept the shared value. The conflict closes when the shared database confirms.", "ok"); } catch (e) { toast(errMsg(e), "error"); }
  }

  // ---------------------------------------------------------------- actions

  async function copyPrompt(): Promise<boolean> {
    copyMsg = null;
    let text = "";
    try {
      await settle();
      text = await api.prompt(id);
      prompt = text; // the preview shows exactly what is copied
    } catch (e) {
      copyMsg = { kind: "err", text: `Prompt could not be built: ${errMsg(e)}` };
      return false;
    }
    try {
      await copyText(text);
    } catch (e) {
      copyMsg = { kind: "err", text: `Clipboard failed: ${errMsg(e)} You can select the preview text and copy it manually.` };
      toast("Clipboard failed", "error");
      return false;
    }
    copyMsg = { kind: "ok", text: "Prompt copied — paste it into Copilot." };
    toast("Prompt copied", "ok");
    try { await api.promptCopied(id, text); await load(); } catch { /* the copy itself succeeded */ }
    return true;
  }

  async function openReply(replyAll: boolean, messageId: number | null = null): Promise<boolean> {
    outlookMsg = null;
    notFound = false;
    try {
      await api.openReply(id, replyAll, messageId);
      outlookMsg = { kind: "ok", text: `Outlook ${replyAll ? "Reply All" : "reply"} window opened. Paste Copilot's reply, review it, and send it from Outlook.` };
      return true;
    } catch (e) {
      const code = (e as { code?: string })?.code;
      if (code === "ITEM_NOT_FOUND" || code === "NO_SOURCE_EMAIL") notFound = true;
      outlookMsg = { kind: "err", text: `Outlook: ${errMsg(e)}` };
      return false;
    }
  }

  async function run(kind: typeof busy, fn: () => Promise<unknown>) {
    if (busy) return;
    busy = kind;
    try { await fn(); } finally { busy = ""; }
  }

  async function copyAndOpen() {
    // copy first, then open Outlook; each result is reported on its own
    await copyPrompt();
    await openReply(false);
  }

  async function openNew() {
    try {
      await api.openNewMessage(id);
      notFound = false;
      outlookMsg = { kind: "info", text: "A new message to the sender was opened (not threaded with the original email)." };
    } catch (e) {
      outlookMsg = { kind: "err", text: `Outlook: ${errMsg(e)}` };
    }
  }

  async function markDone() {
    doneMsg = null;
    failures = [];
    try {
      await settle();
      const r = await api.complete(id);
      const pending = r.decision.state === "pending" ? " Saved locally — it is sent to the shared database automatically." : "";
      if (r.outlook && !r.outlook.done) failures = r.outlook.failed;
      doneMsg = { kind: r.outlook && !r.outlook.done ? "err" : "ok", text: `Done (${r.decision.covered} incoming email(s) covered).${pending} ${r.outlook_message}` };
      toast("Case marked Done", "ok");
    } catch (e) {
      doneMsg = { kind: "err", text: `Mark Done failed: ${errMsg(e)}` };
    }
    await load();
    onchanged();
  }

  async function archiveOutlook() {
    doneMsg = null;
    failures = [];
    try {
      const o = await api.archiveOutlook(id);
      if (o.done) doneMsg = { kind: "ok", text: o.dest_path ? `Outlook updated: ${o.moved} email(s) flagged complete and moved to ${o.dest_path}.` : "Nothing left to archive in Outlook." };
      else { failures = o.failed; doneMsg = { kind: "err", text: "Some Outlook steps failed. The case stays Done. Fix the problem and retry — finished steps are not repeated." }; }
    } catch (e) {
      doneMsg = { kind: "err", text: `Outlook not updated: ${errMsg(e)} The case stays Done.` };
    }
    await load();
  }

  async function claim(takeover: boolean) {
    if (takeover && !confirm(`Take over this case from ${d?.ticket.assignee_name}? Their notes, history and ownership dates are kept. No email is moved and nobody is notified.`)) return;
    try { await api.claim(id, takeover); await load(); onchanged(); toast(takeover ? "You took over the case." : "Claimed. No email was moved.", "ok"); }
    catch (e) { toast(errMsg(e), "error"); }
  }
  async function releaseOwner() {
    if (!confirm("Clear the owner of this case? It becomes Unassigned (a Review case stays in Review). No email is moved.")) return;
    try { await api.release(id); await load(); onchanged(); } catch (e) { toast(errMsg(e), "error"); }
  }
  async function flagReview() {
    const reason = window.prompt("Reason for review (required):", "");
    if (reason == null) return;
    try { await api.flagReview(id, reason); await load(); onchanged(); } catch (e) { toast(errMsg(e), "error"); }
  }
  async function resumeReview() {
    try { await api.resume(id); await load(); onchanged(); } catch (e) { toast(errMsg(e), "error"); }
  }
  async function joinCase() {
    if (!joinTarget.trim()) return;
    if (!confirm(`Add this case to ${joinTarget.trim()}? Its emails, notes and issue types move to that case; this reference stays searchable and redirects there. No email is moved in Outlook and nobody is notified.`)) return;
    try {
      const t = await api.join(id, joinTarget.trim());
      toast(`Added to ${t.display_id}.`, "ok");
      showJoin = false; joinTarget = "";
      onchanged();
      onopen?.(t.id);
    } catch (e) { toast(errMsg(e), "error"); }
  }
  async function retryConflict(c: LocalConflict) {
    try { await api.conflictRetry(c.conflict_id); await load(); onchanged(); toast("Retried against the latest shared version.", "ok"); } catch (e) { toast(errMsg(e), "error"); }
  }
  const outlookPending = $derived(!!d && d.ticket.status === "Done" && (d.messages ?? []).some((m) => !m.is_internal && m.entry_id && !d!.completion.find((c) => c.message_id === m.id)?.moved_at));
  const contributors = $derived([...new Set([...(d?.notes ?? []).map((n) => n.author_name), ...(d?.ownership ?? []).map((o) => o.staff_name ?? "")].filter(Boolean))]);

  async function reopen() {
    try {
      await api.reopen(id);
      doneMsg = { kind: "info", text: "Reopened. The archived email was not moved back and its flag was not changed." };
      await load();
      onchanged();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }

  // ---------------------------------------------------------------- assignment

  const lastOp = $derived<AssignmentOp | undefined>(d?.assignments.find((o) => o.state !== "superseded"));
  const NOTIFY_LABEL: Record<string, string> = {
    pending: "Notification not sent yet", sending: "Sending notification…", submitted: "Notification submitted to Outlook (delivery not confirmed)",
    failed: "Assigned — notification failed", uncertain: "Notification outcome uncertain", skipped: "No notification (same person)", none: "",
  };

  async function startWork() {
    try { await api.startWork(id); await load(); onchanged(); } catch (e) { toast(errMsg(e), "error"); }
  }
  async function retryAssignment() {
    assignMsg = null;
    try {
      const o = await api.assignmentRetry(id);
      assignMsg = { kind: o.result === "assigned" && (o.notify_state === "submitted" || o.notify_state === "skipped") ? "ok" : "err", text: o.message };
    } catch (e) { assignMsg = { kind: "err", text: errMsg(e) }; }
    await load();
    onchanged();
  }
  async function retryNotification(op: AssignmentOp) {
    assignMsg = null;
    try {
      const [st, e] = await api.notificationRetry(op.id);
      assignMsg = { kind: st === "submitted" ? "ok" : "err", text: `${NOTIFY_LABEL[st] ?? st}${e ? `: ${e}` : ""}` };
    } catch (e) { assignMsg = { kind: "err", text: errMsg(e) }; }
    await load();
  }
  async function checkSent(op: AssignmentOp) {
    try { assignMsg = { kind: "info", text: await api.notificationCheck(op.id) }; } catch (e) { assignMsg = { kind: "err", text: errMsg(e) }; }
    await load();
  }
  async function resend(op: AssignmentOp) {
    if (!confirm("Send the notification again? Only do this if you are sure the first one was not sent (check the Sent Items first).")) return;
    try {
      const [st, e] = await api.notificationResend(op.id);
      assignMsg = { kind: st === "submitted" ? "ok" : "err", text: `${NOTIFY_LABEL[st] ?? st}${e ? `: ${e}` : ""}` };
    } catch (e) { assignMsg = { kind: "err", text: errMsg(e) }; }
    await load();
  }

  async function refreshBody(mid: number) {
    try { await api.refreshMessage(mid); await load(); } catch (e) { toast(errMsg(e), "error"); }
  }
</script>

{#if err}
  <div class="notice err">{err}</div>
{:else if !d}
  <div class="muted">Loading…</div>
{:else}
  {@const t = d.ticket}
  <div class="stack">
    <div class="panel pad actionbar">
      <div class="row wrap">
        <h2 style="margin:0">{t.display_id}</h2>
        <span class="badge {statusBadge(t.status)}">{statusText(t.status, t.assignee_name)}</span>
        {#if t.ownership_conflict}<span class="badge err" title="The ticket's emails are in more than one staff folder. The owner was not changed automatically.">Ownership conflict</span>{/if}
        {#if t.scope_state === "history"}<span class="badge grey" title="Emails no longer in the selected folders">Ticket History: {reasonLabel(t.scope_reason)}</span>{/if}
        {#if t.scope_state === "unverified"}<span class="badge warn">Awaiting verification</span>{/if}
        {#if t.low_confidence}<span class="badge warn" title="The issue type was guessed with low confidence - please check it">Check issue type</span>{/if}
        {#if t.possible_duplicate_of}<span class="badge warn">Possible duplicate of T-{String(t.possible_duplicate_of).padStart(6, "0")}</span>{/if}
        {#if t.needs_review}<span class="badge warn">Needs review</span>{/if}
        {#if t.sync_state !== "synced"}<span class="badge {t.sync_state === 'conflict' || t.sync_state === 'failed' ? 'err' : 'grey'}" title={t.shared_refreshed_at ? `Shared copy refreshed ${fmtDate(t.shared_refreshed_at)}` : ""}>{SYNC_LABEL[t.sync_state]}</span>{/if}
        {#if t.ticket_ref}<span class="small muted" title="Stable reference, the same on every computer">{t.ticket_ref}</span>{/if}
        <span class="grow"></span>
        <span class="small muted">Received {fmtDate(t.first_received_at ?? t.created_at) || "—"}{#if t.completed_at}&nbsp;· Completed {fmtDate(t.completed_at)}{/if}</span>
      </div>
      <div class="subject">{t.original_subject ?? "(no subject)"}</div>
      <div class="row wrap actions">
        <button class="primary" disabled={!!busy} onclick={() => run("copy", copyPrompt)}>{busy === "copy" ? "Copying…" : "1. Copy Prompt"}</button>
        <button class="primary" disabled={!!busy} onclick={() => run("reply", () => openReply(false))}>{busy === "reply" ? "Opening…" : "2. Open Outlook Reply"}</button>
        <button disabled={!!busy} onclick={() => run("reply", () => openReply(true))} title="Reply to everyone on the email">Reply All</button>
        <button disabled={!!busy} onclick={() => run("both", copyAndOpen)}>{busy === "both" ? "Working…" : "Copy Prompt & Open Outlook Reply"}</button>
        <span class="grow"></span>
      </div>
      <div class="row wrap actions">
        {#if !isDone}
          {#if !t.assignee_staff_id}
            <button disabled={!!busy || me == null} title={me == null ? "Choose who you are in Settings › Staff Assignment" : "Become the owner. No email is moved and nobody is notified."} onclick={() => run("claim", () => claim(false))}>Claim</button>
          {:else if t.assignee_staff_id !== me}
            <button disabled={!!busy || me == null} title="Become the owner; the previous owner's work and dates are kept" onclick={() => run("claim", () => claim(true))}>Take over…</button>
          {/if}
          {#if t.assignee_staff_id}<button class="link small" disabled={!!busy} onclick={releaseOwner}>Clear owner</button>{/if}
          {#if !sharedBound}
            <button disabled={!!busy} onclick={() => onassign?.([id])} title="Move the ticket's emails to a staff folder and notify that person">{t.assignee_staff_id ? "Reassign" : "Assign"}</button>
          {/if}
          {#if t.assignee_staff_id && (t.status === "Assigned" || t.status === "Review")}<button disabled={!!busy} onclick={() => run("start", startWork)}>Start Work</button>{/if}
          {#if t.status === "Review"}<button disabled={!!busy} onclick={() => run("review", resumeReview)} title="The review is handled; continue the work">Review handled</button>
          {:else}<button disabled={!!busy} onclick={() => run("review", flagReview)}>Flag for Review…</button>{/if}
        {/if}
        {#if isDone}
          {#if outlookPending}<button disabled={!!busy} onclick={() => run("archive", archiveOutlook)} title="Flag complete and move the emails covered by the Done decision">{busy === "archive" ? "Updating Outlook…" : "Archive in Outlook"}</button>{/if}
          <button disabled={!!busy} onclick={() => run("reopen", reopen)}>Reopen</button>
        {:else}
          <button class="done" disabled={!!busy} onclick={() => run("done", markDone)}>{busy === "done" ? "Saving…" : "3. Mark Done"}</button>
        {/if}
        <button class="link small" onclick={() => (showJoin = !showJoin)}>Add to existing case…</button>
      </div>
      {#if showJoin}
        <div class="row small" style="margin-top:6px">
          <span>Add this case to</span><input style="width:180px" bind:value={joinTarget} placeholder="T-000123 or ASR-1A2B3C4D" />
          <button class="small" disabled={!joinTarget.trim()} onclick={() => run("join", joinCase)}>Add</button>
          <span class="muted">Emails and notes move to that case; this reference keeps working. Nothing changes in Outlook.</span>
        </div>
      {/if}
      {#if t.status === "Review" && t.review_reason}<div class="notice err small" style="margin-top:6px"><b>Review:</b> {t.review_reason}</div>{/if}
      {#if copyMsg}<div class="status {copyMsg.kind}">{copyMsg.text}</div>{/if}
      {#if outlookMsg}<div class="status {outlookMsg.kind}">{outlookMsg.text}</div>{/if}
      {#if notFound}
        <div class="notice warn small">
          The original email could not be found in Outlook. You can open a <b>new</b> message to {t.sender_email || "the sender"} instead — it will not be threaded with the original conversation.
          <button class="small" disabled={!t.sender_email} onclick={openNew}>Open new message to sender</button>
        </div>
      {/if}
      {#if doneMsg}<div class="status {doneMsg.kind}">{doneMsg.text}</div>{/if}
      {#if failures.length}
        <ul class="small fails">{#each failures as f}<li><b>{f.subject || `Email ${f.message_id}`}</b>: {f.error}</li>{/each}</ul>
      {/if}
      {#if !isDone}<div class="small muted">Copying the prompt or opening a reply never marks the case Done. Mark Done records your decision at once (no sent-reply check){sharedBound ? "; Outlook emails are archived separately after the shared database confirms it" : " and then flags the case's incoming emails complete and moves them to the completion folder"}. A new reply later moves the case to Review.</div>{/if}
      {#if isDone && outlookPending}<div class="small muted">Outlook not updated yet for this Done case{sharedBound ? " (Archive in Outlook is available once the Done decision is synced)" : ""}.</div>{/if}
      {#if sharedBound && !isDone}<div class="small muted">Shared database: Claim / Take over change the owner without moving emails. Outlook Assign/Reassign returns with the next phase.</div>{/if}
    </div>

    {#if d.conflicts.length}
      <div class="panel pad conflicts">
        <h3>Conflict — review required</h3>
        <div class="small muted">Someone changed the same thing in the shared database before your change arrived. Your local change was NOT applied there; the shared value is shown in this case. Nothing was lost: both values are kept here until you decide.</div>
        <table class="grid small" style="margin-top:6px">
          <thead><tr><th>What</th><th>You saw</th><th>Your change</th><th>Shared value now</th><th></th></tr></thead>
          <tbody>
            {#each d.conflicts as c (c.conflict_id)}
              <tr>
                <td>{conflictLabel(c)}<div class="muted">{c.reason}</div></td>
                <td class="pre">{c.original_value ?? "—"}</td>
                <td class="pre"><b>{c.proposed_value ?? "—"}</b></td>
                <td class="pre">{c.shared_value ?? "—"}</td>
                <td style="white-space:nowrap">
                  {#if c.status === "resolving"}<span class="badge grey">waiting for shared confirmation</span>
                  {:else}
                    <button class="small" onclick={() => acceptShared(c)}>Keep shared value</button>
                    {#if c.conflict_type !== "duplicate_import"}<button class="small" onclick={() => retryConflict(c)} title={c.conflict_type === "owner" ? "Take over explicitly" : c.conflict_type === "stale_done" ? "Mark Done again, covering the new reply" : "Apply your value again against the latest shared value"}>{c.conflict_type === "owner" ? "Take over…" : c.conflict_type === "stale_done" ? "Mark Done again" : "Retry my change"}</button>{/if}
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        <div class="small muted">To keep your value instead, enter it again above — it is checked against the latest shared value.</div>
      </div>
    {/if}

    {#if d.assignments.length}
      <div class="panel pad">
        <h3>Assignment</h3>
        {#if lastOp}
          {@const op = lastOp}
          <div class="row wrap small">
            <span><b>{op.from_name ?? "Unassigned"}</b> → <b>{op.to_name}</b> · {op.dest_path} · by {op.operator}, {fmtDate(op.finished_at ?? op.created_at)}</span>
          </div>
          {#if op.state === "moving" || op.state === "failed"}
            <div class="notice err small" style="margin-top:6px">
              Not finished: {op.moves.filter((m) => m.moved_at).length} of {op.moves.length} email(s) moved. No notification has been sent.
              {#each op.moves.filter((m) => !m.moved_at && m.last_error) as m}<div>• {m.subject}: {m.last_error}</div>{/each}
              <button class="small" disabled={!!busy} onclick={() => run("assign", retryAssignment)}>Retry assignment</button>
            </div>
          {:else if op.state === "done"}
            <div class="row wrap small" style="margin-top:6px">
              <span class="badge {op.notify_state === 'submitted' || op.notify_state === 'skipped' ? 'ok' : op.notify_state === 'uncertain' ? 'warn' : op.notify_state === 'failed' ? 'err' : 'grey'}">{NOTIFY_LABEL[op.notify_state] ?? op.notify_state}</span>
              {#if op.notify_recipient && op.notify_state !== "skipped"}<span class="muted">to {op.notify_recipient}{op.notify_account ? ` via ${op.notify_account}` : ""}{op.notify_at ? `, ${fmtDate(op.notify_at)}` : ""}</span>{/if}
              {#if op.notify_state === "failed" || op.notify_state === "pending"}<button class="small" disabled={!!busy} onclick={() => run("notify", () => retryNotification(op))}>Retry Notification</button>{/if}
              {#if op.notify_state === "uncertain"}
                <button class="small" disabled={!!busy} onclick={() => run("notify", () => checkSent(op))}>Check Sent Items</button>
                <button class="small danger" disabled={!!busy} onclick={() => run("notify", () => resend(op))}>Send again…</button>
              {/if}
            </div>
            {#if op.notify_error}<div class="small" style="color:var(--err)">{op.notify_error}</div>{/if}
          {/if}
          {#if op.note}<div class="small" style="margin-top:4px"><b>Handoff note:</b> {op.note}</div>{/if}
        {/if}
        {#if assignMsg}<div class="status {assignMsg.kind}">{assignMsg.text}</div>{/if}
        {#if d.assignments.length > 1}
          <details class="small"><summary>Assignment history ({d.assignments.length})</summary>
            <table class="grid small">
              <thead><tr><th>When</th><th>Change</th><th>By</th><th>Folders</th><th>Moves</th><th>Notification</th></tr></thead>
              <tbody>
                {#each d.assignments as o (o.id)}
                  <tr>
                    <td style="white-space:nowrap">{fmtDate(o.created_at)}</td>
                    <td>{o.from_name ?? "Unassigned"} → {o.to_name}{o.note ? ` · “${o.note}”` : ""}</td>
                    <td>{o.operator}</td>
                    <td>{o.prev_folders || "—"} → {o.dest_path}</td>
                    <td>{o.state === "superseded" ? "replaced" : `${o.moves.filter((m) => m.moved_at).length}/${o.moves.length}`}</td>
                    <td>{NOTIFY_LABEL[o.notify_state] ?? o.notify_state}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </details>
        {/if}
      </div>
    {/if}

    <div class="panel pad">
      <h3>Issue</h3>
      <div class="row wrap">
        {#each issueIds as iid, i (iid)}
          <span class="chip" class:primary={i === 0} title={i === 0 ? "Primary issue" : ""}>{issueName(iid)}
            {#if issueIds.length > 1}<button class="link small" title="Remove" onclick={() => saveIssues(issueIds.filter((x) => x !== iid))}>✕</button>{/if}
          </span>
        {/each}
        <select style="width:220px" bind:value={addIssue} onchange={() => { if (addIssue) { saveIssues([...issueIds, addIssue]); addIssue = ""; } }}>
          <option value="">+ Add issue type…</option>
          {#each app.catalog?.issue_types.filter((it) => !issueIds.includes(it.id)) ?? [] as it}<option value={it.id}>{it.name}</option>{/each}
        </select>
      </div>
      <div class="small muted" style="margin:4px 0 8px">{t.issue_type_source === "manual" ? "Set by staff." : `Detected automatically${t.classification_confidence ? ` (${Math.round(t.classification_confidence * 100)}% confidence)` : ""} — please confirm.`}</div>
      <FieldInput def={def("issue_description", "Issue summary", "notes")} field={field("issue_description")} onsave={(v, ver) => saveField("issue_description", v, ver)} />
      <FieldInput def={def("error_message", "Exact error message(s)", "notes")} field={field("error_message")} onsave={(v, ver) => saveField("error_message", v, ver)} />
    </div>

    <div class="panel pad">
      <h3>Sender</h3>
      <div class="field"><div class="lbl">Name</div><input value={t.sender_name ?? ""} placeholder="Not provided" onchange={(e) => patch({ sender_name: (e.target as HTMLInputElement).value })} /></div>
      <FieldInput def={def("sender_title", "Title / job role")} field={field("sender_title")} onsave={(v, ver) => saveField("sender_title", v, ver)} />
      <div class="field"><div class="lbl">Email</div><input type="email" value={t.sender_email ?? ""} placeholder="Not provided" onchange={(e) => patch({ sender_email: (e.target as HTMLInputElement).value })} /></div>
      <FieldInput def={def("sender_phone", "Phone")} field={field("sender_phone")} onsave={(v, ver) => saveField("sender_phone", v, ver)} />
      <FieldInput def={def("sender_phone_ext", "Extension")} field={field("sender_phone_ext")} onsave={(v, ver) => saveField("sender_phone_ext", v, ver)} />
    </div>

    <div class="panel pad">
      <div class="row"><h3 style="margin:0">School{d.schools.length > 1 ? "s" : ""}</h3><span class="grow"></span><button class="small" onclick={addSchool}>+ Add school</button></div>
      {#each d.schools as slot (slot)}
        <div class="school">
          {#if d.schools.length > 1}
            <div class="row"><b>School {slot}</b><span class="grow"></span>{#if slot > 1}<button class="link small" onclick={() => removeSchool(slot)}>Remove</button>{/if}</div>
          {/if}
          {#each SCHOOL_ATTRS as [attr, label, type]}
            <FieldInput def={def(schoolKey(slot, attr), label, type)} field={field(schoolKey(slot, attr))} onsave={(v, ver) => saveField(schoolKey(slot, attr), v, ver)} />
          {/each}
          {#if slot === 1 && values["additional_ids"]}
            <div class="field"><div class="lbl">Other NJIIS IDs in email</div><div class="small">{values["additional_ids"]} <span class="badge warn">from an earlier import — move them to a separate school if needed</span></div></div>
          {/if}
        </div>
      {/each}
      {#if moreFields.length}
        <details>
          <summary class="small">More details for this issue ({moreFields.length})</summary>
          {#each moreFields as f (f.key)}
            <FieldInput def={f} field={field(f.key)} onsave={(v, ver) => saveField(f.key, v, ver)} />
          {/each}
        </details>
      {/if}
    </div>

    <div class="panel pad">
      <h3>Staff</h3>
      <div class="field"><div class="lbl">Staff actions already taken</div>
        <div><textarea rows="3" bind:value={actions} placeholder="e.g. Reset the account on 11/17. Only actions recorded here are described as completed in the reply." onblur={() => actions !== (t.staff_actions ?? "") && patch({ staff_actions: actions })}></textarea></div></div>
      <div class="field"><div class="lbl">Notes</div>
        <div class="stack">
          {#each d.notes as n (n.note_id)}
            <div class="note">
              <div class="row small"><b>{n.author_name}</b><span class="muted">{fmtDate(n.created_at)}{n.updated_at !== n.created_at ? ` · edited ${fmtDate(n.updated_at)}` : ""}</span>
                {#if n.sync_state === "pending"}<span class="badge grey">pending sync</span>{:else if n.sync_state === "conflict"}<span class="badge err">conflict</span>{/if}
                <span class="grow"></span>
                <button class="link small" onclick={() => (editNote = { id: n.note_id, body: n.body })}>Edit</button>
                <button class="link small" onclick={() => deleteNote(n.note_id)}>Delete</button></div>
              {#if editNote?.id === n.note_id}
                <textarea rows="3" bind:value={editNote.body}></textarea>
                <div class="row"><span class="grow"></span><button class="small" onclick={() => (editNote = null)}>Cancel</button><button class="small primary" onclick={saveNoteEdit}>Save note</button></div>
              {:else}<div class="pre small">{n.body}</div>{/if}
            </div>
          {:else}<div class="small muted">No notes yet.</div>{/each}
          <textarea rows="2" bind:value={newNote} placeholder="Add a note for the team (each note keeps its author; notes from different people never overwrite each other)"></textarea>
          <div class="row"><span class="grow"></span><button class="small" disabled={!newNote.trim()} onclick={addNote}>Add note</button></div>
        </div></div>
      {#if t.resolution_notes}
        <div class="field"><div class="lbl">Earlier resolution notes</div><div class="small pre">{t.resolution_notes}</div></div>
      {/if}
      {#if t.possible_duplicate_of || t.needs_review}
        <div class="row small">
          {#if t.possible_duplicate_of}<button class="link small" onclick={() => patch({ clear_duplicate_flag: true })}>Not a duplicate</button>{/if}
          {#if t.needs_review}<button class="link small" onclick={() => patch({ clear_needs_review: true })}>Reviewed — clear flag</button>{/if}
        </div>
      {/if}
    </div>

    <div class="panel pad">
      <div class="row">
        <h3 style="margin:0">Copilot Prompt Preview</h3>
        <span class="grow"></span>
        <span class="small muted">{t.last_prompt_at ? `Last copied ${fmtDate(t.last_prompt_at)}` : "Not copied yet"}</span>
      </div>
      <div class="small muted" style="margin:4px 0">This is exactly what Copy Prompt puts on the clipboard. It updates when you save changes above.</div>
      {#if promptErr}<div class="notice err small">{promptErr}</div>{:else}<pre class="prompt">{prompt}</pre>{/if}
    </div>

    <div class="panel pad">
      <h3>Emails ({d.messages.length})</h3>
      {#each chronological as m (m.id)}
        {@const c = completionOf(m.id)}
        <div class="msg">
          <button class="msghead" onclick={() => (openMsg = openMsg === m.id ? null : m.id)}>
            <b>{m.sender_name ?? m.sender_email}</b>
            <span class="muted small">&lt;{m.sender_email}&gt;</span>
            {#if m.link_type === "original" || m.link_type === "manual"}<span class="badge">original</span>{:else}<span class="badge grey">{m.is_internal ? "staff" : m.link_type}</span>{/if}
            {#if m.has_attachments}<span class="badge grey">📎</span>{/if}
            {#if c?.moved_at}<span class="badge ok" title={c.dest_path ?? ""}>✓ archived</span>{:else if c?.flagged_at}<span class="badge warn">flagged, not moved</span>{/if}
            {#if c?.last_error}<span class="badge err" title={c.last_error}>Outlook error</span>{/if}
            {#if !m.is_internal && m.entry_id}
              {#if m.scope_state === "in"}<span class="badge ok" title="Confirmed in the selected folders{m.owner_name ? ` (${m.owner_name}'s folder)` : ''}">in current folders</span>
              {:else if m.scope_state === "out"}<span class="badge grey" title={m.location_status === "error" ? "Location could not be verified" : ""}>outside: {reasonLabel(m.out_reason) || "elsewhere"}</span>
              {:else}<span class="badge warn">location not verified</span>{/if}
            {/if}
            <span class="grow"></span>
            <span class="muted small">{fmtDate(m.received_at)} · {m.folder_path ?? ""}</span>
          </button>
          {#if openMsg === m.id || (openMsg === null && (m.link_type === "original" || m.link_type === "manual"))}
            <div class="small muted">Subject: {m.subject}</div>
            <pre class="body">{m.body_text ?? ""}</pre>
            <div class="row">
              {#if m.body_truncated}<button class="small" onclick={() => refreshBody(m.id)}>Load full email from Outlook</button>{/if}
              {#if !m.is_internal && m.entry_id && incoming.length > 1}
                <button class="small" disabled={!!busy} onclick={() => run("reply", () => openReply(false, m.id))}>Reply to this email</button>
              {/if}
              {#if c?.last_error}<span class="small" style="color:var(--err)">{c.last_error}</span>{/if}
            </div>
          {/if}
        </div>
      {:else}
        <div class="muted small" style="padding:8px 0">No email is linked to this ticket.</div>
      {/each}
    </div>

    <div class="panel pad">
      <h3>Ownership and completion</h3>
      <div class="small"><b>Owner:</b> {t.assignee_name ?? "nobody"}{#if contributors.length} · <b>Contributors:</b> {contributors.join(", ")}{/if}</div>
      {#if d.ownership.length}
        <table class="grid small" style="margin-top:4px"><thead><tr><th>Owner</th><th>From</th><th>Until</th><th>How</th></tr></thead>
          <tbody>{#each d.ownership as o}<tr><td>{o.staff_name ?? "—"}</td><td>{fmtDate(o.started_at)}</td><td>{o.ended_at ? fmtDate(o.ended_at) : "now"}</td><td>{o.start_reason ?? ""}{o.end_reason ? ` → ${o.end_reason}` : ""}{o.origin === "local" ? " (pending sync)" : ""}</td></tr>{/each}</tbody></table>
      {/if}
      {#if d.completions.length}
        <table class="grid small" style="margin-top:4px"><thead><tr><th>Done decision</th><th>By</th><th>Emails covered</th><th>State</th></tr></thead>
          <tbody>{#each d.completions as c}<tr><td>{fmtDate(c.decided_at)}</td><td>{c.actor_name ?? "—"}</td><td>{c.covered}</td><td>{c.state}{c.state_reason ? ` — ${c.state_reason}` : ""}</td></tr>{/each}</tbody></table>
      {/if}
      {#if d.pending_ops.length}<div class="small muted" style="margin-top:4px">{d.pending_ops.length} change(s) on this computer not yet confirmed by the shared database{d.pending_ops.some((o) => o.state === "held") ? " (some held for review in Settings)" : ""}.</div>{/if}
    </div>

    <div class="panel pad">
      <button class="link" onclick={() => (showHistory = !showHistory)}>{showHistory ? "▾" : "▸"} History ({d.history.length})</button>
      {#if showHistory}
        <table class="grid small">
          <thead><tr><th>When</th><th>Who</th><th>Action</th><th>Change</th></tr></thead>
          <tbody>
            {#each d.history as h (h.id)}
              <tr>
                <td style="white-space:nowrap">{fmtDate(h.at)}</td><td>{h.actor}</td><td>{h.action}{h.field ? ` (${h.field})` : ""}</td>
                <td>{#if h.old_value || h.new_value}<span class="muted">{h.old_value || "—"}</span> → {h.new_value || "—"}{/if}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>
{/if}

<style>
  .pad { padding: 12px; }
  .actionbar { position: sticky; top: 0; z-index: 5; box-shadow: 0 2px 6px rgba(0, 0, 0, .06); }
  .subject { font-weight: 500; margin: 4px 0 8px; }
  .actions button { height: 30px; }
  button.done { background: var(--ok); color: #fff; border-color: var(--ok); }
  .status { margin-top: 6px; padding: 5px 8px; border-radius: var(--radius); font-size: 12px; }
  .status.ok { background: #e3f3e8; color: var(--ok); }
  .status.err { background: var(--err-bg); color: var(--err); }
  .status.info { background: var(--accent-soft); color: var(--accent); }
  .fails { margin: 6px 0 0; padding-left: 18px; color: var(--err); }
  .chip { display: inline-flex; align-items: center; gap: 2px; padding: 2px 8px; border-radius: 12px; background: #eceff3; font-size: 12px; }
  .chip.primary { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
  .school { border-top: 1px solid var(--border); padding-top: 8px; margin-top: 8px; }
  .school:first-of-type { border-top: none; }
  details { margin-top: 8px; }
  .pre { white-space: pre-wrap; }
  .note { border-left: 3px solid #c9d1db; padding: 2px 8px; }
  .conflicts { border: 1px solid var(--err); }
  .prompt { white-space: pre-wrap; word-break: break-word; font-family: Consolas, monospace; font-size: 12px; background: #f8fafc; border: 1px solid var(--border); border-radius: 6px; padding: 8px; max-height: 420px; overflow: auto; margin: 0; user-select: text; }
  .msg { border-top: 1px solid var(--border); padding: 6px 0; }
  .msghead { display: flex; gap: 8px; align-items: center; width: 100%; border: none; background: none; padding: 0; height: 26px; text-align: left; }
  .body { white-space: pre-wrap; word-break: break-word; font-family: inherit; background: #f8fafc; border: 1px solid var(--border); border-radius: 6px; padding: 8px; max-height: 360px; overflow: auto; margin: 6px 0; }
</style>
