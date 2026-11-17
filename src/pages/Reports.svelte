<script lang="ts">
  import { onMount } from "svelte";
  import { save } from "@tauri-apps/plugin-dialog";
  import { api, errMsg, type Report } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";

  let r = $state<Report | null>(null);
  let group = $state("all");

  async function load() {
    try { r = await api.report(); } catch (e) { toast(errMsg(e), "error"); }
  }
  onMount(load);
  $effect(() => { void app.ticketsVersion; load(); });

  async function exportCsv() {
    const path = await save({ defaultPath: `asr-tickets-${new Date().toISOString().slice(0, 10)}.csv`, filters: [{ name: "CSV (Excel)", extensions: ["csv"] }] });
    if (!path) return;
    try {
      const n = await api.exportCsv(path, group);
      toast(`Exported ${n} ticket(s) to ${path}`, "ok");
    } catch (e) { toast(errMsg(e), "error"); }
  }
  const maxIssue = $derived(Math.max(1, ...(r?.by_issue.map((x) => x[2]) ?? [1])));
  const maxWeek = $derived(Math.max(1, ...(r?.by_week.map((x) => x[1]) ?? [1])));
</script>

<div class="scroll" style="height:100%;padding:16px">
  <div class="row"><h2 style="margin:0">Reports</h2><span class="grow"></span><button onclick={load}>Refresh</button></div>
  {#if r}
    <div class="tiles">
      {#each [["Total tickets", r.total], ["Open", r.open], ["Created last 7 days", r.created_last_7_days], ["Created last 30 days", r.created_last_30_days], ["Open > 7 days", r.open_older_than_7_days], ["Follow-ups due", r.followups_due], ["Outlook drafts created", r.outlook_drafts_created]] as [label, n]}
        <div class="panel tile"><div class="n">{n}</div><div class="muted small">{label}</div></div>
      {/each}
    </div>
    <div class="cols">
      <div class="panel pad">
        <h3>Ticket Summary by Status</h3>
        <table class="grid"><tbody>{#each r.by_status as [s, n]}<tr><td>{s}</td><td class="num">{n}</td></tr>{:else}<tr><td class="muted">No tickets yet</td></tr>{/each}</tbody></table>
        <h3 style="margin-top:14px">School Association</h3>
        <table class="grid"><tbody>{#each r.by_school_verification as [s, n]}<tr><td>{s}</td><td class="num">{n}</td></tr>{/each}</tbody></table>
      </div>
      <div class="panel pad">
        <h3>Issue Categories</h3>
        {#each r.by_issue as [, name, n]}
          <div class="bar"><span class="lbl small">{name}</span><span class="track"><span style="width:{(100 * n) / maxIssue}%"></span></span><span class="num small">{n}</span></div>
        {:else}<div class="muted small">No tickets yet</div>{/each}
        <h3 style="margin-top:14px">New Tickets per Week (12 weeks)</h3>
        {#each r.by_week as [w, n]}
          <div class="bar"><span class="lbl small">{w}</span><span class="track"><span style="width:{(100 * n) / maxWeek}%"></span></span><span class="num small">{n}</span></div>
        {:else}<div class="muted small">No data</div>{/each}
      </div>
    </div>
    <div class="panel pad" style="margin-top:12px">
      <h3>Export</h3>
      <div class="row">
        <select style="width:220px" bind:value={group}>
          <option value="all">All tickets</option><option value="open">Open tickets</option><option value="done">Resolved / Closed</option>
          <option value="review">Needs Review</option><option value="waiting">Waiting</option>
        </select>
        <button class="primary" onclick={exportCsv}>Export CSV (Excel-compatible)…</button>
      </div>
      <div class="small muted" style="margin-top:6px">Includes all ticket columns and every ticket field (effective value). Email bodies are not exported.</div>
    </div>
  {/if}
</div>

<style>
  .tiles { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 10px; margin: 12px 0; }
  .tile { padding: 12px; } .tile .n { font-size: 24px; font-weight: 600; color: var(--accent); }
  .cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(380px, 1fr)); gap: 12px; }
  .pad { padding: 12px; }
  .num { text-align: right; font-variant-numeric: tabular-nums; width: 40px; }
  .bar { display: grid; grid-template-columns: 220px 1fr 40px; gap: 8px; align-items: center; margin: 3px 0; }
  .bar .lbl { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .track { background: #eef2f6; height: 12px; border-radius: 3px; overflow: hidden; display: block; }
  .track span { display: block; height: 100%; background: var(--accent); }
</style>
