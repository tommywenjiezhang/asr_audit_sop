<script lang="ts">
  import { onMount } from "svelte";
  import { save } from "@tauri-apps/plugin-dialog";
  import { api, copyText, errMsg, localDayStart, type EmailTicketTaskReport, type Report } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";

  let r = $state<EmailTicketTaskReport | null>(null);
  let legacy = $state<Report | null>(null);
  let status = $state("All");
  let from = $state("");
  let to = $state("");
  let loading = $state(false);
  let csvStatus = $state("");

  function filter() {
    return { status, received_from: from ? localDayStart(from) : "", received_to: to ? localDayStart(to, 1) : "" };
  }
  async function load() {
    loading = true;
    try { [r, legacy] = await Promise.all([api.emailTicketTaskReport(filter()), api.report()]); } catch (e) { toast(errMsg(e), "error"); }
    finally { loading = false; }
  }
  onMount(load);
  $effect(() => { void app.ticketsVersion; void load(); });

  async function copyReport() {
    if (!r) await load();
    if (!r) return;
    try { await copyText(r.markdown); toast("Report copied. Review and redact it before pasting into Copilot.", "ok"); }
    catch (e) { toast(errMsg(e), "error"); }
  }
  async function exportReport(extension: "md" | "txt") {
    const path = await save({ defaultPath: `asr-email-ticket-report-${new Date().toISOString().slice(0, 10)}.${extension}`, filters: [{ name: extension === "md" ? "Markdown" : "Text", extensions: [extension] }] });
    if (!path) return;
    try { const n = await api.exportEmailTicketTaskReport(path, filter()); toast(`Exported ${n} ticket(s) to ${path}`, "ok"); }
    catch (e) { toast(errMsg(e), "error"); }
  }
  async function exportCsv() {
    const path = await save({ defaultPath: `asr-tickets-${new Date().toISOString().slice(0, 10)}.csv`, filters: [{ name: "CSV (Excel)", extensions: ["csv"] }] });
    if (!path) return;
    try { const n = await api.exportCsv(path, csvStatus); toast(`Exported ${n} ticket(s) to ${path}`, "ok"); }
    catch (e) { toast(errMsg(e), "error"); }
  }
</script>

<div class="scroll" style="height:100%;padding:16px">
  <div class="row"><h2 style="margin:0">Reports</h2><span class="grow"></span><button onclick={load} disabled={loading}>Refresh</button></div>
  <div class="panel pad" style="margin:12px 0">
    <h3>Email Ticket Task Report</h3>
    <div class="row wrap">
      <label>Status <select bind:value={status} onchange={load}><option>All</option><option>In Progress</option><option>Needs Review</option><option>Done</option></select></label>
      <label>Received from <input type="date" bind:value={from} onchange={load} /></label>
      <label>Received through <input type="date" bind:value={to} onchange={load} /></label>
      <button class="primary" onclick={load} disabled={loading}>Preview</button>
    </div>
    <div class="small muted" style="margin-top:8px">Read-only report. It includes every linked message and preserves original wording. No ticket, Outlook email, or AI service is changed. Review and redact sensitive information before copying.</div>
  </div>

  {#if r}
    <div class="tiles">
      <div class="panel tile"><div class="n">{r.ticket_count}</div><div class="muted small">Matching tickets</div></div>
      {#each r.by_status as [s, n]}<div class="panel tile"><div class="n">{n}</div><div class="muted small">{s}</div></div>{/each}
    </div>
    <div class="cols">
      <div class="panel pad"><h3>Counts by status</h3><table class="grid"><tbody>{#each r.by_status as [s, n]}<tr><td>{s}</td><td class="num">{n}</td></tr>{:else}<tr><td class="muted">No matching tickets</td></tr>{/each}</tbody></table></div>
      <div class="panel pad"><h3>Counts by issue type</h3><table class="grid"><tbody>{#each r.by_issue as [s, n]}<tr><td>{s}</td><td class="num">{n}</td></tr>{:else}<tr><td class="muted">No matching tickets</td></tr>{/each}</tbody></table></div>
    </div>
    <div class="panel pad" style="margin-top:12px">
      <h3>Report actions</h3>
      <div class="row wrap"><button onclick={copyReport}>Copy Report</button><button class="primary" onclick={() => exportReport("md")}>Export Markdown (.md)</button><button onclick={() => exportReport("txt")}>Export Text (.txt)</button></div>
      <div class="small muted" style="margin-top:8px">The report contains Microsoft Copilot instructions for analysis, prioritized daily tasks, NJIIS troubleshooting recommendations, and approved ASR/Audit SOP follow-up drafts. Copying is manual and staff-controlled.</div>
    </div>
    <div class="panel pad" style="margin-top:12px"><h3>Preview</h3><textarea class="report-preview" readonly value={r.markdown}></textarea></div>
  {/if}
  {#if legacy}
    <div class="panel pad" style="margin-top:12px">
      <h3>Existing ticket overview and CSV export</h3>
      <div class="tiles compact">
        {#each [["Total tickets", legacy.total], ["In Progress", legacy.open], ["Created last 7 days", legacy.created_last_7_days], ["Follow-ups due", legacy.followups_due]] as [label, n]}<div class="tile"><div class="n">{n}</div><div class="muted small">{label}</div></div>{/each}
      </div>
      <div class="row wrap"><select bind:value={csvStatus} style="width:220px"><option value="">All tickets</option><option value="In Progress">In Progress</option><option value="Done">Done</option></select><button onclick={exportCsv}>Export CSV (Excel-compatible)…</button></div>
      <div class="small muted" style="margin-top:6px">The existing CSV export remains available for ticket field data. Email bodies are not included in CSV.</div>
    </div>
  {/if}
</div>

<style>
  label { display:inline-flex; align-items:center; gap:6px; color:var(--muted); }
  label select, label input { width:auto; min-width:150px; color:var(--text); }
  .tiles { display:grid; grid-template-columns:repeat(auto-fill,minmax(145px,1fr)); gap:10px; margin:12px 0; }
  .tiles.compact { margin:8px 0; }
  .tile { padding:12px; } .tile .n { font-size:24px; font-weight:600; color:var(--accent); }
  .cols { display:grid; grid-template-columns:repeat(auto-fit,minmax(320px,1fr)); gap:12px; }
  .pad { padding:12px; }
  .num { text-align:right; font-variant-numeric:tabular-nums; width:50px; }
  .report-preview { min-height:420px; width:100%; font-family:Consolas,monospace; white-space:pre; }
</style>
