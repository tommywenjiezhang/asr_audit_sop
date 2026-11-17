<script lang="ts">
  import { api, errMsg, fmtDate, localDayStart, type Overview } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";

  const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  const today = new Date();
  let from = $state(iso(new Date(today.getFullYear(), today.getMonth(), today.getDate() - 30)));
  let to = $state(iso(today));
  let o = $state<Overview | null>(null);

  async function load() {
    try { o = await api.overview(from ? localDayStart(from) : "", to ? localDayStart(to, 1) : ""); } catch (e) { toast(errMsg(e), "error"); }
  }
  $effect(() => { void from; void to; void app.ticketsVersion; load(); });
  const total = (k: "unassigned" | "assigned" | "in_progress" | "done") => (o?.rows ?? []).reduce((n, r) => n + r[k], 0);
</script>

<div class="scroll" style="height:100%;padding:16px">
  <div class="row"><h2 style="margin:0">Team Overview</h2><span class="grow"></span><button onclick={load}>Refresh</button></div>
  {#if o}
    <div class="small muted" style="margin:6px 0 10px">
      <b>Reporting scope:</b> {o.scope_summary} · last verified {o.last_reconciled_at ? fmtDate(o.last_reconciled_at) : "never"}
    </div>
    <div class="row small" style="margin-bottom:8px">
      <span class="muted">Done between</span><input type="date" style="width:150px" bind:value={from} /><span class="muted">and</span><input type="date" style="width:150px" bind:value={to} /><span class="muted">(end date included)</span>
    </div>
    <table class="grid panel">
      <thead><tr><th>Staff</th><th>Folder</th><th class="num">Unassigned</th><th class="num">Assigned</th><th class="num">In Progress</th><th class="num">Done (local)</th></tr></thead>
      <tbody>
        {#each o.rows as r (r.staff_id ?? 0)}
          <tr class:muted={!r.active}>
            <td>{r.name}{#if !r.active} <span class="badge grey">inactive</span>{/if}</td>
            <td class="small">{r.folder_path ?? "—"}{#if r.folder_path && !r.folder_in_scope} <span class="badge warn" title="Not in the selected folders: current tickets there are not reconciled on this computer">not reconciled</span>{/if}</td>
            <td class="num">{r.staff_id == null ? r.unassigned : ""}</td>
            <td class="num">{r.staff_id == null ? "" : r.assigned}</td>
            <td class="num">{r.in_progress}</td>
            <td class="num">{r.done}</td>
          </tr>
        {/each}
        <tr><td><b>Total</b></td><td></td><td class="num"><b>{total("unassigned")}</b></td><td class="num"><b>{total("assigned")}</b></td><td class="num"><b>{total("in_progress")}</b></td><td class="num"><b>{total("done")}</b></td></tr>
      </tbody>
    </table>
    <ul class="small muted notes">{#each o.notes as n}<li>{n}</li>{/each}</ul>
  {/if}
</div>

<style>
  .num { text-align: right; font-variant-numeric: tabular-nums; width: 110px; }
  .notes { margin-top: 10px; padding-left: 18px; line-height: 1.6; }
  table { background: #fff; }
</style>
