<script lang="ts">
  import { api, debounce, errMsg, fmtDate, type TicketSummary } from "../lib/api";
  import { app, issueName, toast } from "../lib/stores.svelte";
  import TicketDetail from "../components/TicketDetail.svelte";
  import EmailAssistant from "../components/EmailAssistant.svelte";
  import NewTicketDialog from "../components/NewTicketDialog.svelte";

  let { group }: { group: string } = $props();

  const PAGE = 200;
  let rows = $state<TicketSummary[]>([]);
  let total = $state(0);
  let search = $state("");
  let issueFilter = $state("");
  let selected = $state<number | null>(null);
  let loading = $state(false);
  let showNew = $state(false);
  /** bumped when the detail panel changes ticket data (assistant re-checks placeholders) */
  let fieldsVersion = $state(0);

  let leftW = $state(Number(localStorage.getItem("leftW")) || 330);
  let rightW = $state(Number(localStorage.getItem("rightW")) || 520);

  async function load(append = false) {
    loading = true;
    try {
      const r = await api.tickets({ group, search, issue_type: issueFilter, limit: PAGE, offset: append ? rows.length : 0 });
      rows = append ? [...rows, ...r.rows] : r.rows;
      total = r.total;
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      loading = false;
    }
  }
  const reloadDebounced = debounce(() => load(), 250);

  $effect(() => {
    // reload on group/filter change or when sync signals new tickets
    void group; void issueFilter; void app.ticketsVersion;
    load();
  });

  function startDrag(which: "left" | "right", e: PointerEvent) {
    const startX = e.clientX;
    const start = which === "left" ? leftW : rightW;
    const move = (ev: PointerEvent) => {
      const d = ev.clientX - startX;
      if (which === "left") leftW = Math.min(560, Math.max(240, start + d));
      else rightW = Math.min(900, Math.max(360, start - d));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      localStorage.setItem("leftW", String(leftW));
      localStorage.setItem("rightW", String(rightW));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function statusClass(s: string) {
    if (s === "New") return "";
    if (s === "Needs Review") return "warn";
    if (s === "Resolved" || s === "Closed") return "grey";
    if (s.startsWith("Waiting")) return "grey";
    return "ok";
  }

  function onChanged() {
    fieldsVersion++;
    load();
  }

  async function created(id: number) {
    showNew = false;
    await load();
    selected = id;
  }
</script>

<div class="ws">
  <section class="queue" style="width:{leftW}px">
    <div class="qhead stack">
      <div class="row">
        <input class="grow" placeholder="Search school, ID, sender, subject, T-number…" bind:value={search} oninput={reloadDebounced} />
        <button class="primary" onclick={() => (showNew = true)} title="Create a ticket manually">+ New</button>
      </div>
      <select bind:value={issueFilter}>
        <option value="">All issue types</option>
        {#each app.catalog?.issue_types ?? [] as it}<option value={it.id}>{it.name}</option>{/each}
      </select>
      <div class="muted small">{total} ticket(s){loading ? " · loading…" : ""}</div>
    </div>
    <div class="list scroll">
      {#each rows as t (t.id)}
        <button class="item" class:sel={selected === t.id} onclick={() => (selected = t.id)}>
          <div class="row">
            <b>{t.display_id}</b>
            <span class="badge {statusClass(t.status)}">{t.status}</span>
            {#if t.priority === "High" || t.priority === "Urgent"}<span class="badge err">{t.priority}</span>{/if}
            {#if t.possible_duplicate_of}<span class="badge warn" title="Possible duplicate">dup?</span>{/if}
            <span class="grow"></span>
            <span class="muted small">{fmtDate(t.last_email_received_at ?? t.updated_at)}</span>
          </div>
          <div class="school">{t.school_name ?? t.sender_name ?? t.sender_email ?? "(no school)"}
            {#if t.school_verification === "Unverified School"}<span class="muted small"> · Unverified School</span>{/if}
          </div>
          <div class="muted small ellipsis">{issueName(t.issue_type)}</div>
          <div class="muted small ellipsis">{t.original_subject ?? ""}</div>
        </button>
      {:else}
        <div class="muted" style="padding:12px">{loading ? "Loading…" : "No tickets here."}</div>
      {/each}
      {#if rows.length < total}
        <button style="margin:8px" onclick={() => load(true)}>Load more ({total - rows.length})</button>
      {/if}
    </div>
  </section>
  <div class="split" role="separator" aria-orientation="vertical" onpointerdown={(e) => startDrag("left", e)}></div>
  <section class="center scroll">
    {#if selected}
      <TicketDetail id={selected} onchanged={onChanged} />
    {:else}
      <div class="empty muted">Select a ticket, or create one with <b>+ New</b>.<br />Default SOP templates are available on every ticket, even without Outlook.</div>
    {/if}
  </section>
  <div class="split" role="separator" aria-orientation="vertical" onpointerdown={(e) => startDrag("right", e)}></div>
  <section class="assistant scroll" style="width:{rightW}px">
    {#if selected}
      <EmailAssistant ticketId={selected} {fieldsVersion} onchanged={() => load()} />
    {/if}
  </section>
</div>

{#if showNew}
  <NewTicketDialog oncreated={created} onclose={() => (showNew = false)} />
{/if}

<style>
  .ws { display: flex; height: 100%; }
  .queue { flex: none; display: flex; flex-direction: column; background: var(--panel); border-right: 1px solid var(--border); }
  .qhead { padding: 10px; border-bottom: 1px solid var(--border); }
  .list { flex: 1; }
  .item { display: block; width: 100%; height: auto; text-align: left; border: none; border-bottom: 1px solid var(--border); border-radius: 0; padding: 8px 10px; background: #fff; }
  .item:hover { background: #f5f8fc; }
  .item.sel { background: var(--accent-soft); box-shadow: inset 3px 0 0 var(--accent); }
  .school { margin-top: 3px; font-weight: 500; }
  .ellipsis { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .split { width: 5px; flex: none; cursor: col-resize; background: transparent; }
  .split:hover { background: #c6d4e3; }
  .center { flex: 1; min-width: 320px; padding: 12px; }
  .assistant { flex: none; background: var(--panel); border-left: 1px solid var(--border); padding: 12px; }
  .empty { margin-top: 60px; text-align: center; line-height: 1.8; }
</style>
