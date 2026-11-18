<script lang="ts">
  import { onMount } from "svelte";
  import { api, debounce, errMsg, fmtDate, HISTORY_REASONS, localDayStart, reasonLabel, statusBadge, statusText, SYNC_LABEL, type ListQuery, type ScopeStatus, type Staff, type TicketSummary } from "../lib/api";
  import { app, issueName, sharedLabel, toast } from "../lib/stores.svelte";
  import TicketDetail from "../components/TicketDetail.svelte";
  import NewTicketDialog from "../components/NewTicketDialog.svelte";
  import AssignDialog from "../components/AssignDialog.svelte";

  /** team views use the shared case records (any folder); the folder views use this computer's Outlook scope */
  type View = "team" | "mine" | "unassigned" | "current" | "unverified" | "history";
  const PAGE = 100;
  const blank = { search: "", status: "", assignee: "", issue: "", recFrom: "", recTo: "", compFrom: "", compTo: "", asgFrom: "", asgTo: "", age: 0, reason: "", school: "", sender: "", sort: "received", sync: "" };
  let view = $state<View>("team");
  let f = $state({ ...blank });
  let rows = $state<TicketSummary[]>([]);
  let total = $state(0);
  let selected = $state<number | null>(null);
  let checked = $state<number[]>([]);
  let loading = $state(false);
  let showNew = $state(false);
  let assignIds = $state<number[] | null>(null);
  let more = $state(false);
  let scope = $state<ScopeStatus | null>(null);
  let staff = $state<Staff[]>([]);
  let me = $state<number | null>(null);
  let leftW = $state(Number(localStorage.getItem("leftW")) || 400);

  onMount(async () => {
    try {
      staff = await api.staffList(true);
      me = await api.currentStaff();
    } catch (e) { toast(errMsg(e), "error"); }
  });

  function query(offset: number): ListQuery {
    const team = view === "team" || view === "mine" || view === "unassigned";
    return {
      view: team ? "team" : view,
      status: view === "mine" ? (f.status || "open") : view === "unassigned" ? "Unassigned" : f.status,
      assignee: view === "mine" ? String(me ?? -1) : f.assignee, search: f.search, issue_type: f.issue, sort: f.sort, limit: PAGE, offset, sync: f.sync,
      reason: view === "history" ? f.reason : "", school: f.school, sender: f.sender, min_age_days: Number(f.age) || 0,
      // local midnight of the start date .. local midnight after the end date: end dates are inclusive
      received_from: f.recFrom ? localDayStart(f.recFrom) : "", received_to: f.recTo ? localDayStart(f.recTo, 1) : "",
      completed_from: f.compFrom ? localDayStart(f.compFrom) : "", completed_to: f.compTo ? localDayStart(f.compTo, 1) : "",
      assigned_from: f.asgFrom ? localDayStart(f.asgFrom) : "", assigned_to: f.asgTo ? localDayStart(f.asgTo, 1) : "",
    };
  }

  async function load(append = false) {
    loading = true;
    try {
      const [r, s] = await Promise.all([api.tickets(query(append ? rows.length : 0)), api.scopeStatus()]);
      rows = append ? [...rows, ...r.rows] : r.rows;
      total = r.total;
      scope = s;
      if (!append) checked = checked.filter((id) => rows.some((t) => t.id === id));
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      loading = false;
    }
  }
  const reloadDebounced = debounce(() => load(), 250);

  $effect(() => {
    void view; void f.status; void f.assignee; void f.issue; void f.recFrom; void f.recTo; void f.compFrom; void f.compTo;
    void f.asgFrom; void f.asgTo; void f.age; void f.reason; void f.sort; void f.sync; void app.ticketsVersion; void app.sync?.running;
    load();
  });

  const filtered = $derived(JSON.stringify(f) !== JSON.stringify(blank));
  function clearFilters() { f = { ...blank }; }
  function shortcut(kind: "mine" | "unassigned") {
    f = { ...blank };
    view = kind;
  }
  function toggle(id: number, on: boolean) {
    checked = on ? [...new Set([...checked, id])] : checked.filter((x) => x !== id);
  }

  function startDrag(e: PointerEvent) {
    const startX = e.clientX;
    const start = leftW;
    const move = (ev: PointerEvent) => { leftW = Math.min(640, Math.max(300, start + ev.clientX - startX)); };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      localStorage.setItem("leftW", String(leftW));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  async function created(id: number) {
    showNew = false;
    await load();
    selected = id;
  }

  const views: [View, string][] = [["team", "Team Tickets"], ["mine", "My Tickets"], ["unassigned", "Unassigned"], ["current", "Current Tickets"], ["unverified", "Awaiting Location Check"], ["history", "Ticket History"]];
  const countOf = (v: View) => (scope ? (v === "current" ? scope.current : v === "history" ? scope.history : v === "unverified" ? scope.unverified : -1) : 0);
  const sharedTag = $derived(sharedLabel(app.shared));
</script>

<div class="ws">
  <section class="queue" style="width:{leftW}px">
    <div class="tabs">
      {#each views as [v, label]}
        {#if v !== "unverified" || countOf(v) > 0 || view === v}
          <button class="tab" class:on={view === v} disabled={v === "mine" && me == null} title={v === "mine" && me == null ? "Choose who you are in Settings › Staff Assignment" : ""} onclick={() => { view = v; checked = []; }}>{label}{#if countOf(v) >= 0} <span class="muted">({countOf(v)})</span>{/if}</button>
        {/if}
      {/each}
    </div>
    <div class="scopebar small">
      {#if view === "team" || view === "mine" || view === "unassigned"}
        <div><b>Shared team cases</b> (any Outlook folder) · <span class="badge {sharedTag.kind}">{sharedTag.text}</span></div>
        <div class="muted">Shared data refreshed: {app.shared?.last_success_at ? fmtDate(app.shared.last_success_at) : "not yet"}{app.shared?.mode === "standalone" ? " · Standalone: only this computer's cases" : ""}. Your folder scan is shown under Current Tickets and is not complete team coverage.</div>
      {:else if scope}
        <div><b>{scope.mailbox}</b> · {scope.summary}</div>
        <div>
          {#if scope.running}<span class="badge">Checking Outlook…</span>
          {:else if scope.last_error}<span class="badge err" title={scope.last_error}>Last check incomplete</span>
          {:else if scope.stale}<span class="badge warn">Needs refresh</span>{/if}
          Last verified: {scope.last_ok_at ? fmtDate(scope.last_ok_at) : "never"}
        </div>
        {#if view === "unverified"}<div class="notice warn">These tickets have not been confirmed in the selected folders yet. Click <b>Check Now</b> to verify them. They are not claimed to be current.</div>{/if}
        {#if view === "history"}<div class="muted">Earlier tickets whose emails are no longer in the selected folders. Nothing is deleted.</div>{/if}
      {/if}
    </div>
    <div class="qhead stack">
      <div class="row">
        <input class="grow" placeholder="Search ticket, school, ID, sender, phone, subject, email text, notes…" bind:value={f.search} oninput={reloadDebounced} />
        <button class="primary" onclick={() => (showNew = true)} title="Create a ticket manually">+ New</button>
      </div>
      <div class="row small">
        <span class="grow"></span>
        <button class="link small" onclick={() => (more = !more)}>{more ? "Fewer filters" : "More filters"}</button>
        <button class="link small" disabled={!filtered} onclick={clearFilters}>Clear Filters</button>
      </div>
      <div class="row">
        <select bind:value={f.status} title="Workflow status">
          <option value="">All statuses</option><option value="open">Open (not Done)</option>
          <option>Unassigned</option><option>Assigned</option><option>In Progress</option><option>Review</option><option>Done</option>
        </select>
        <select bind:value={f.assignee} title="Assignee">
          <option value="">Anyone</option><option value="none">No assignee</option>
          {#each staff as s (s.id)}<option value={String(s.id)}>{s.display_name}{s.active ? "" : " (inactive)"}</option>{/each}
        </select>
      </div>
      {#if more}
        <select bind:value={f.sync} title="Shared database state">
          <option value="">Any sync state</option><option value="attention">Needs attention (pending, conflict, failed)</option>
          <option value="pending">Pending sync</option><option value="conflict">Conflict — review required</option><option value="failed">Failed — retry available</option>
        </select>
        <select bind:value={f.issue} title="Issue type">
          <option value="">All issue types</option>
          {#each app.catalog?.issue_types ?? [] as it}<option value={it.id}>{it.name}</option>{/each}
        </select>
        {#if view === "history"}
          <select bind:value={f.reason} title="Reason for leaving Current Tickets">
            <option value="">Any reason</option>
            {#each HISTORY_REASONS as [k, label]}<option value={k}>{label}</option>{/each}
          </select>
        {/if}
        <div class="row">
          <input placeholder="School name / ID" bind:value={f.school} oninput={reloadDebounced} />
          <input placeholder="Sender name / email" bind:value={f.sender} oninput={reloadDebounced} />
        </div>
        <div class="dates small">
          <span class="muted dl">Received (email date), end date included</span>
          <input type="date" bind:value={f.recFrom} /><span class="muted">to</span><input type="date" bind:value={f.recTo} />
          <span class="muted dl">Assigned (assignment date), end date included</span>
          <input type="date" bind:value={f.asgFrom} /><span class="muted">to</span><input type="date" bind:value={f.asgTo} />
          <span class="muted dl">Completed (Mark Done date), end date included</span>
          <input type="date" bind:value={f.compFrom} /><span class="muted">to</span><input type="date" bind:value={f.compTo} />
        </div>
        <div class="row small">
          <span class="muted">Older than</span><input type="number" min="0" style="width:70px" bind:value={f.age} /><span class="muted">days</span>
          <span class="grow"></span>
          <span class="muted">Sort</span>
          <select style="width:auto" bind:value={f.sort}>
            <option value="received">Received</option><option value="updated">Updated</option><option value="assigned">Assigned</option><option value="completed">Completed</option>
          </select>
        </div>
      {/if}
      <div class="row small">
        <span class="muted"><b>{total}</b> {view === "current" ? "current" : view === "history" ? "historical" : view === "unverified" ? "unverified" : view === "mine" ? "open (mine)" : view === "unassigned" ? "unassigned" : "team"} case{total === 1 ? "" : "s"}{loading ? " · loading…" : ""}</span>
        <span class="grow"></span>
        {#if checked.length && !(app.shared?.mode === "shared" && app.shared?.bound_database_id)}
          <button class="small primary" onclick={() => (assignIds = [...checked])}>Assign selected ({checked.length})</button>
          <button class="link small" onclick={() => (checked = [])}>Clear</button>
        {/if}
      </div>
    </div>
    <div class="list scroll">
      {#each rows as t (t.id)}
        <div class="item" class:sel={selected === t.id}>
          <input type="checkbox" class="pick" checked={checked.includes(t.id)} onchange={(e) => toggle(t.id, (e.target as HTMLInputElement).checked)} aria-label="Select {t.display_id}" />
          <button class="body" onclick={() => (selected = t.id)}>
            <div class="row">
              <b>{t.display_id}</b>
              <span class="badge {statusBadge(t.status)}">{statusText(t.status, t.assignee_name)}</span>
              {#if t.new_reply}<span class="badge warn" title="A new incoming email arrived after the last staff action">new reply</span>{/if}
              {#if t.ownership_conflict}<span class="badge err" title="Emails are in more than one staff folder">owner conflict</span>{/if}
              {#if t.low_confidence}<span class="badge warn" title="Issue type guessed with low confidence">check type</span>{/if}
              {#if t.possible_duplicate_of}<span class="badge warn" title="Possible duplicate">dup?</span>{/if}
              {#if t.needs_review}<span class="badge warn" title="Needs review">review</span>{/if}
              {#if t.sync_state === "pending"}<span class="badge grey" title={SYNC_LABEL.pending}>⟳ pending</span>
              {:else if t.sync_state === "conflict"}<span class="badge err" title={SYNC_LABEL.conflict}>conflict</span>
              {:else if t.sync_state === "failed"}<span class="badge err" title={SYNC_LABEL.failed}>sync failed</span>{/if}
              <span class="grow"></span>
              <span class="muted small">{fmtDate(t.first_received_at)}</span>
            </div>
            <div class="school">{t.school_name ?? t.sender_name ?? t.sender_email ?? "(no school)"}{#if t.school_njiis_id}<span class="muted small">&nbsp;· {t.school_njiis_id}</span>{/if}</div>
            <div class="muted small ellipsis">{issueName(t.issue_type)}{(t.issue_count ?? 0) > 1 ? ` +${(t.issue_count ?? 1) - 1} more` : ""}</div>
            <div class="muted small ellipsis">{t.original_subject ?? ""}</div>
            {#if view === "history" && t.scope_reason}<div class="small reason">{reasonLabel(t.scope_reason)}</div>{/if}
            {#if t.status === "Review" && t.review_reason}<div class="small reason">Review: {t.review_reason}</div>{/if}
            <div class="muted small">{t.ticket_ref ?? ""} · updated {fmtDate(t.updated_at)}</div>
          </button>
        </div>
      {:else}
        <div class="muted" style="padding:12px">{loading ? "Loading…" : view === "current" ? "No tickets in the selected folders." : "No tickets here."}</div>
      {/each}
      {#if rows.length < total}
        <button style="margin:8px" onclick={() => load(true)}>Load more ({total - rows.length})</button>
      {/if}
    </div>
  </section>
  <div class="split" role="separator" aria-orientation="vertical" onpointerdown={startDrag}></div>
  <section class="center scroll">
    {#if selected}
      <TicketDetail id={selected} onchanged={() => load()} onassign={(ids) => (assignIds = ids)} onopen={(nid) => (selected = nid)} />
    {:else}
      <div class="empty muted">
        Select a ticket.<br />
        <b>Copy Prompt</b> → paste into Copilot → <b>Open Outlook Reply</b> → paste Copilot's reply and send it from Outlook → <b>Mark Done</b>.
      </div>
    {/if}
  </section>
</div>

{#if showNew}
  <NewTicketDialog oncreated={created} onclose={() => (showNew = false)} />
{/if}
{#if assignIds}
  <AssignDialog ticketIds={assignIds} onclose={() => { assignIds = null; load(); app.ticketsVersion++; }} ondone={() => { checked = []; }} />
{/if}

<style>
  .ws { display: flex; height: 100%; }
  .queue { flex: none; display: flex; flex-direction: column; background: var(--panel); border-right: 1px solid var(--border); }
  .tabs { display: flex; border-bottom: 1px solid var(--border); padding: 0 6px; flex-wrap: wrap; }
  .tab { border: none; border-bottom: 2px solid transparent; border-radius: 0; background: none; font-weight: 600; color: var(--muted); padding: 0 8px; height: 34px; }
  .tab.on { color: var(--accent); border-bottom-color: var(--accent); }
  .scopebar { padding: 6px 10px; background: #f6f8fb; border-bottom: 1px solid var(--border); line-height: 1.5; }
  .scopebar .notice { margin-top: 4px; }
  .qhead { padding: 10px; border-bottom: 1px solid var(--border); }
  .dates { display: grid; grid-template-columns: 1fr auto 1fr; gap: 4px 6px; align-items: center; }
  .dates .dl { grid-column: 1 / -1; }
  .dates input { padding: 2px 4px; }
  .list { flex: 1; }
  .item { display: flex; align-items: flex-start; gap: 6px; border-bottom: 1px solid var(--border); background: #fff; padding-left: 8px; }
  .item:hover { background: #f5f8fc; }
  .item.sel { background: var(--accent-soft); box-shadow: inset 3px 0 0 var(--accent); }
  .pick { margin-top: 11px; }
  .body { display: block; flex: 1; min-width: 0; height: auto; text-align: left; border: none; border-radius: 0; padding: 8px 10px 8px 2px; background: transparent; }
  .school { margin-top: 3px; font-weight: 500; }
  .reason { color: var(--accent); margin-top: 2px; }
  .ellipsis { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .split { width: 5px; flex: none; cursor: col-resize; background: transparent; }
  .split:hover { background: #c6d4e3; }
  .center { flex: 1; min-width: 420px; padding: 12px; }
  .empty { margin-top: 60px; text-align: center; line-height: 1.8; }
</style>
