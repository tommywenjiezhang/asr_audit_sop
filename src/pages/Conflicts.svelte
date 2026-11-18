<script lang="ts">
  import { api, errMsg, fmtDate, type LocalConflict } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";
  import TicketDetail from "../components/TicketDetail.svelte";

  /** Synchronization conflicts of this computer, kept separate from case status. */
  let rows = $state<LocalConflict[]>([]);
  let showResolved = $state(false);
  let selected = $state<number | null>(null);
  let busy = $state("");

  async function load() {
    try {
      rows = await api.conflicts(null, showResolved);
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
  $effect(() => {
    void showResolved; void app.ticketsVersion; void app.shared?.last_success_at;
    load();
  });

  const TYPE: Record<string, string> = {
    field: "Same field changed", text: "Same text changed", owner: "Owner", status: "Status", note: "Note", stale_done: "Done while a reply arrived",
    duplicate_import: "Duplicate import", outlook_busy: "Outlook work elsewhere",
  };
  async function act(c: LocalConflict, kind: "shared" | "retry") {
    busy = c.conflict_id;
    try {
      if (kind === "shared") await api.conflictAcceptShared(c.conflict_id);
      else await api.conflictRetry(c.conflict_id);
      toast(kind === "shared" ? "Keeping the shared value. The conflict closes when the shared database confirms." : "Your change was sent again against the latest shared value.", "ok");
      await load();
      app.ticketsVersion++;
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      busy = "";
    }
  }
  const open = $derived(rows.filter((r) => r.status !== "resolved").length);
</script>

<div class="page">
  <div class="list scroll">
    <div class="row wrap" style="padding:12px 12px 6px">
      <h2 style="margin:0">Sync Conflicts</h2>
      <span class="badge {open ? 'err' : 'ok'}">{open} open</span>
      <span class="grow"></span>
      <label class="row small" style="gap:4px"><input type="checkbox" bind:checked={showResolved} />Show resolved</label>
      <button class="small" onclick={load}>Refresh</button>
    </div>
    <div class="small muted" style="padding:0 12px 8px">A conflict means two people changed the same thing before the shared database saw both changes. Neither value was lost: your value, the value you started from and the shared value are kept until you decide. Conflicts are separate from the case's workflow status.</div>
    <table class="grid small">
      <thead><tr><th>Case</th><th>What</th><th>You saw</th><th>Your change</th><th>Shared value</th><th>When</th><th></th></tr></thead>
      <tbody>
        {#each rows as c (c.conflict_id)}
          <tr class:sel={selected === c.ticket_id}>
            <td>{#if c.ticket_id}<button class="link small" onclick={() => (selected = c.ticket_id ?? null)}>{c.ticket_ref ?? `T-${String(c.ticket_id).padStart(6, "0")}`}</button>{:else}—{/if}</td>
            <td>{TYPE[c.conflict_type] ?? c.conflict_type}{c.field_name ? ` (${c.field_name})` : ""}<div class="muted">{c.reason ?? ""}</div><div class="muted">by {c.staff_name ?? "?"}{c.op_type ? ` · ${c.op_type}` : ""}</div></td>
            <td class="pre">{c.original_value ?? "—"}</td>
            <td class="pre"><b>{c.proposed_value ?? "—"}</b></td>
            <td class="pre">{c.shared_value ?? "—"}</td>
            <td style="white-space:nowrap">{fmtDate(c.created_at)}</td>
            <td style="white-space:nowrap">
              {#if c.status === "resolved"}<span class="badge ok">resolved</span>
              {:else if c.status === "resolving"}<span class="badge grey">waiting for shared confirmation</span>
              {:else}
                <button class="small" disabled={busy === c.conflict_id} onclick={() => act(c, "shared")}>Keep shared value</button>
                {#if c.conflict_type !== "duplicate_import"}<button class="small" disabled={busy === c.conflict_id} onclick={() => act(c, "retry")}>{c.conflict_type === "owner" ? "Take over…" : c.conflict_type === "stale_done" ? "Mark Done again" : "Use my value"}</button>{/if}
              {/if}
            </td>
          </tr>
        {:else}
          <tr><td colspan="7" class="muted">No {showResolved ? "" : "open "}conflicts.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if selected}
    <aside class="detail scroll">
      <div class="row"><span class="grow"></span><button class="link small" onclick={() => (selected = null)}>Close ✕</button></div>
      <TicketDetail id={selected} onchanged={load} onopen={(n) => (selected = n)} />
    </aside>
  {/if}
</div>

<style>
  .page { display: flex; height: 100%; }
  .list { flex: 1; min-width: 0; }
  .pre { white-space: pre-wrap; word-break: break-word; max-width: 220px; }
  tr.sel td { background: var(--accent-soft); }
  .detail { width: 560px; flex: none; border-left: 1px solid var(--border); padding: 8px 12px; background: var(--bg); }
</style>
