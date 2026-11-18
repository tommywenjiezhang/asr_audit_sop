<script lang="ts">
  import { onMount } from "svelte";
  import { api, debounce, errMsg, fmtDate, outlookPendingLabel, SYNC_LABEL, type Staff, type TicketSummary } from "../lib/api";
  import { app, issueName, toast } from "../lib/stores.svelte";
  import TicketDetail from "../components/TicketDetail.svelte";
  import AssignDialog from "../components/AssignDialog.svelte";
  import BulkBar from "../components/BulkBar.svelte";

  const COLS = ["Unassigned", "Assigned", "In Progress", "Review", "Done"] as const;
  type Col = (typeof COLS)[number];

  let rows = $state<TicketSummary[]>([]);
  let total = $state(0);
  let loading = $state(false);
  let staff = $state<Staff[]>([]);
  let me = $state<number | null>(null);
  let f = $state({ assignee: "", issue: "", school: "", search: "", doneDays: 14, outlook: false });
  let selected = $state<number | null>(null);
  let checked = $state<number[]>([]);
  let assignIds = $state<number[] | null>(null);
  let dragId = $state<number | null>(null);
  let over = $state<Col | null>(null);
  let moving = $state<number | null>(null);

  onMount(async () => {
    try {
      staff = await api.staffList(true);
      me = await api.currentStaff();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  });

  async function load() {
    loading = true;
    try {
      const doneFrom = f.doneDays > 0 ? new Date(Date.now() - f.doneDays * 86400000).toISOString().slice(0, 19) + "Z" : "";
      const base = { view: "team", assignee: f.assignee, issue_type: f.issue, school: f.school, search: f.search, outlook: f.outlook ? "pending" : "", sort: "oldest", limit: 1000 };
      const [open, done] = await Promise.all([
        api.tickets({ ...base, status: "open" }),
        api.tickets({ ...base, status: "Done", sort: "completed", completed_from: doneFrom, limit: 300 }),
      ]);
      rows = [...open.rows, ...done.rows];
      total = open.total + done.total;
      checked = checked.filter((id) => rows.some((r) => r.id === id));
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      loading = false;
    }
  }
  const reloadDebounced = debounce(() => load(), 300);
  $effect(() => {
    void f.assignee; void f.issue; void f.doneDays; void f.outlook; void app.ticketsVersion; void app.shared?.last_success_at;
    load();
  });

  const byCol = $derived(Object.fromEntries(COLS.map((c) => [c, rows.filter((r) => r.status === c)])) as Record<Col, TicketSummary[]>);

  function ageDays(t: TicketSummary): number {
    const d = new Date(t.first_received_at ?? t.updated_at).getTime();
    return Math.max(0, Math.floor((Date.now() - d) / 86400000));
  }

  /** Map a drop onto the validated workflow commands (the backend re-checks every rule). */
  async function moveTo(t: TicketSummary, to: Col) {
    if (t.status === to) return;
    moving = t.id;
    try {
      if (to === "Unassigned") {
        if (!t.assignee_staff_id) throw new Error("This case has no owner.");
        if (!confirm(`Clear the owner of ${t.display_id}? It goes back to Unassigned.`)) return;
        await api.release(t.id);
      } else if (to === "Assigned") {
        if (t.status === "Unassigned") {
          if (me == null) throw new Error("Choose who you are in Settings › Staff Assignment first.");
          await api.claim(t.id, false);
        } else {
          assignIds = [t.id];
          return;
        }
      } else if (to === "In Progress") {
        if (t.status === "Done") await api.reopen(t.id);
        else if (t.status === "Review") await api.resume(t.id);
        else {
          if (!t.assignee_staff_id) await api.claim(t.id, false);
          await api.startWork(t.id);
        }
      } else if (to === "Review") {
        const reason = prompt(`Reason for review of ${t.display_id}:`, "Flagged for review");
        if (reason == null) return;
        await api.flagReview(t.id, reason);
      } else if (to === "Done") {
        if (!confirm(`Mark ${t.display_id} Done? No sent-reply check is made.`)) return;
        const r = await api.complete(t.id);
        toast(r.outlook_message || "Marked Done.", "ok");
      }
      app.ticketsVersion++;
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      moving = null;
      await load();
    }
  }

  function drop(e: DragEvent, col: Col) {
    e.preventDefault();
    over = null;
    const id = Number(e.dataTransfer?.getData("text/plain") || dragId);
    const t = rows.find((r) => r.id === id);
    dragId = null;
    if (t) moveTo(t, col);
  }
  function toggle(id: number, on: boolean) {
    checked = on ? [...new Set([...checked, id])] : checked.filter((x) => x !== id);
  }
</script>

<div class="kb">
  <div class="bar row wrap">
    <h2 style="margin:0">Kanban</h2>
    <select bind:value={f.assignee} title="Owner">
      <option value="">Everyone</option><option value="none">No owner</option>
      {#if me != null}<option value={String(me)}>Me</option>{/if}
      {#each staff as s (s.id)}<option value={String(s.id)}>{s.display_name}{s.active ? "" : " (inactive)"}</option>{/each}
    </select>
    <select bind:value={f.issue} title="Issue type" style="width:200px">
      <option value="">All issue types</option>
      {#each app.catalog?.issue_types ?? [] as it}<option value={it.id}>{it.name}</option>{/each}
    </select>
    <input placeholder="School name / ID" bind:value={f.school} oninput={reloadDebounced} style="width:160px" />
    <input placeholder="Search…" bind:value={f.search} oninput={reloadDebounced} style="width:180px" />
    <label class="row small" style="gap:4px"><input type="checkbox" bind:checked={f.outlook} />Pending Outlook work</label>
    <span class="small muted">Done shown for last</span>
    <select bind:value={f.doneDays} style="width:auto"><option value={7}>7 days</option><option value={14}>14 days</option><option value={30}>30 days</option><option value={0}>all time</option></select>
    <span class="grow"></span>
    <span class="small muted">{rows.length} of {total}{loading ? " · loading…" : ""}</span>
  </div>
  {#if checked.length}
    <div style="padding:0 12px 6px"><BulkBar ids={checked} onclear={() => (checked = [])} onassign={(ids) => (assignIds = ids)} onchanged={() => load()} /></div>
  {/if}
  <div class="body">
    <div class="cols">
      {#each COLS as c (c)}
        <section class="col" class:over={over === c} role="list" aria-label={c}
          ondragover={(e) => { e.preventDefault(); over = c; }} ondragleave={() => (over = over === c ? null : over)} ondrop={(e) => drop(e, c)}>
          <header class="row"><b>{c}</b><span class="badge grey">{byCol[c].length}</span></header>
          <div class="cards">
            {#each byCol[c] as t (t.id)}
              <div class="card" class:sel={selected === t.id} class:moving={moving === t.id} role="listitem" draggable="true"
                ondragstart={(e) => { dragId = t.id; e.dataTransfer?.setData("text/plain", String(t.id)); }}>
                <div class="row small">
                  <input type="checkbox" checked={checked.includes(t.id)} onchange={(e) => toggle(t.id, (e.target as HTMLInputElement).checked)} aria-label="Select {t.display_id}" />
                  <button class="link small" onclick={() => (selected = t.id)}><b>{t.display_id}</b></button>
                  <span class="grow"></span>
                  <span class="muted" title="Age since first received">{ageDays(t)}d</span>
                </div>
                <button class="cardbody" onclick={() => (selected = t.id)}>
                  <div class="school">{t.school_name ?? t.sender_name ?? t.sender_email ?? "(no school)"}</div>
                  <div class="muted small ellipsis">{issueName(t.issue_type)}{(t.issue_count ?? 0) > 1 ? ` +${(t.issue_count ?? 1) - 1}` : ""}</div>
                  <div class="row small wrap">
                    <span>{t.assignee_name ?? "—"}</span>
                    {#if t.new_reply}<span class="badge warn">new reply</span>{/if}
                    {#if t.sync_state === "pending"}<span class="badge grey" title={SYNC_LABEL.pending}>⟳ pending</span>
                    {:else if t.sync_state === "conflict"}<span class="badge err">conflict</span>
                    {:else if t.sync_state === "failed"}<span class="badge err">sync failed</span>{/if}
                    {#if t.outlook_pending}<span class="badge {t.outlook_pending.includes('failed') || t.outlook_pending.includes('uncertain') ? 'err' : 'warn'}" title={outlookPendingLabel(t.outlook_pending)}>Outlook</span>{/if}
                  </div>
                  {#if t.status === "Review" && t.review_reason}<div class="small reason ellipsis">{t.review_reason}</div>{/if}
                  <div class="muted small">activity {fmtDate(t.last_activity_at ?? t.updated_at)}</div>
                </button>
              </div>
            {/each}
          </div>
        </section>
      {/each}
    </div>
    {#if selected}
      <aside class="detail scroll">
        <div class="row"><span class="grow"></span><button class="link small" onclick={() => (selected = null)}>Close ✕</button></div>
        <TicketDetail id={selected} onchanged={() => load()} onassign={(ids) => (assignIds = ids)} onopen={(nid) => (selected = nid)} />
      </aside>
    {/if}
  </div>
  <div class="small muted hint">Drag a card to change its status. Every move uses the same checked action as the buttons (Claim, Start Work, Flag for Review, Mark Done, Clear owner); a refused move leaves the card where it was. Moving to Assigned from another status opens Assign / Reassign.</div>
</div>

{#if assignIds}
  <AssignDialog ticketIds={assignIds} onclose={() => { assignIds = null; load(); app.ticketsVersion++; }} ondone={() => (checked = [])} />
{/if}

<style>
  .kb { display: flex; flex-direction: column; height: 100%; }
  .bar { padding: 10px 12px; }
  .bar select, .bar input { width: auto; }
  .body { flex: 1; min-height: 0; display: flex; }
  .cols { flex: 1; min-width: 0; display: grid; grid-template-columns: repeat(5, minmax(190px, 1fr)); gap: 8px; padding: 0 12px 8px; overflow: auto; }
  .col { background: #eef2f6; border-radius: 8px; display: flex; flex-direction: column; min-height: 0; border: 2px dashed transparent; }
  .col.over { border-color: var(--accent); background: var(--accent-soft); }
  .col header { padding: 8px; position: sticky; top: 0; }
  .cards { padding: 0 6px 8px; display: flex; flex-direction: column; gap: 6px; overflow: auto; }
  .card { background: #fff; border: 1px solid var(--border); border-radius: 6px; padding: 6px 8px; cursor: grab; }
  .card.sel { box-shadow: inset 3px 0 0 var(--accent); }
  .card.moving { opacity: .5; }
  .cardbody { display: block; width: 100%; text-align: left; border: none; background: none; padding: 0; height: auto; }
  .school { font-weight: 500; margin-top: 2px; }
  .reason { color: var(--err); }
  .ellipsis { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .detail { width: 560px; flex: none; border-left: 1px solid var(--border); padding: 8px 12px; background: var(--bg); }
  .hint { padding: 4px 12px 8px; }
</style>
