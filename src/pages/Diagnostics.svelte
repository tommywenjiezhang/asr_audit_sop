<script lang="ts">
  import { tick } from "svelte";
  import { api, copyText, errMsg, fmtDate, type DiagEvent, type DiagOverview } from "../lib/api";
  import { app, sharedLabel, toast } from "../lib/stores.svelte";

  let sev = $state<"info" | "warning" | "error">("info");
  let events = $state<DiagEvent[]>([]);
  let ov = $state<DiagOverview | null>(null);
  let selected = $state<number[]>([]);
  let open = $state<number | null>(null);
  let fallback = $state("");
  let loading = $state(false);

  async function load() {
    loading = true;
    try {
      [events, ov] = await Promise.all([api.diagRecent(sev, 1000), api.diagOverview()]);
      selected = selected.filter((id) => events.some((e) => e.id === id));
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      loading = false;
    }
  }
  $effect(() => { void sev; void app.shared?.last_attempt_at; load(); });

  async function copy(ids: number[] | null) {
    fallback = "";
    let text = "";
    try {
      text = await api.diagCopyText(ids, sev);
    } catch (e) {
      toast(`Could not build the log: ${errMsg(e)}`, "error");
      return;
    }
    try {
      await copyText(text);
      toast(ids ? `${ids.length} event(s) copied` : "Log copied — paste it into an email or Teams message to the developer", "ok");
    } catch (e) {
      // the clipboard is not essential: show the text for manual copying, and note it (never blocks anything)
      fallback = text;
      api.diagReport("diagnostics", "CLIPBOARD_FAILED", `Clipboard unavailable: ${errMsg(e)}`).catch(() => {});
    }
  }

  async function latestError() {
    if (!ov?.latest_error_id) return;
    if (sev !== "info" && !events.some((e) => e.id === ov!.latest_error_id)) sev = "info";
    open = ov.latest_error_id;
    await tick();
    document.getElementById(`ev-${ov.latest_error_id}`)?.scrollIntoView({ block: "center" });
  }

  function toggle(id: number, on: boolean) {
    selected = on ? [...new Set([...selected, id])] : selected.filter((x) => x !== id);
  }
  const label = $derived(sharedLabel(app.shared));
</script>

<div class="page">
  <div class="row">
    <h2 style="margin:0">Diagnostics</h2>
    <span class="grow"></span>
    <button onclick={load} disabled={loading}>{loading ? "Loading…" : "Refresh"}</button>
  </div>
  <p class="small muted">Technical events for troubleshooting (connections, synchronization, Outlook checks and actions). They are separate from the case history. Email addresses, phone numbers, school and NJIIS IDs, subjects, Windows user names and connection strings are removed before anything is saved or copied; email text and Copilot prompts are never logged.</p>

  {#if ov}
    <table class="grid small kv panel">
      <tbody>
        <tr><th>App version</th><td>{ov.header.app_version} · local database schema v{ov.header.local_schema} · computer {ov.header.device}</td></tr>
        <tr><th>Shared database</th><td><span class="badge {label.kind}">{label.text}</span> {ov.header.shared_database ?? ""}{ov.header.shared_environment ? ` (${ov.header.shared_environment})` : ""}{ov.header.driver ? ` · driver ${ov.header.driver}` : ""}</td></tr>
        <tr><th>Last successful sync</th><td>{fmtDate(ov.header.last_success_at) || "never"}</td></tr>
        <tr><th>Changes waiting</th><td>{ov.header.pending} pending · {ov.header.held} held · {ov.header.conflicts} conflict(s) · {ov.header.failed} failed</td></tr>
        <tr><th>Log files</th><td><code>{ov.log_dir}</code></td></tr>
      </tbody>
    </table>
  {/if}

  <div class="row wrap" style="margin:10px 0 6px">
    <select style="width:200px" bind:value={sev} title="Minimum severity">
      <option value="info">All events</option><option value="warning">Warnings and errors</option><option value="error">Errors only</option>
    </select>
    <button class="primary" onclick={() => copy(null)} disabled={!events.length}>Copy Log</button>
    <button onclick={() => copy(selected)} disabled={!selected.length}>Copy Selected ({selected.length})</button>
    <button onclick={latestError} disabled={!ov?.latest_error_id}>Go to latest error</button>
    <span class="grow"></span>
    <span class="small muted">{events.length} event(s), newest first</span>
  </div>

  {#if fallback}
    <div class="notice warn small">The clipboard is not available. Select the text below (Ctrl+A, Ctrl+C) and copy it manually.
      <textarea rows="10" readonly value={fallback}></textarea></div>
  {/if}

  <div class="scroll list">
    <table class="grid small">
      <thead><tr><th></th><th>When</th><th>Severity</th><th>Area</th><th>Code</th><th>What happened</th></tr></thead>
      <tbody>
        {#each events as e (e.id)}
          <tr id="ev-{e.id}" class:hl={open === e.id} class={e.severity}>
            <td><input type="checkbox" checked={selected.includes(e.id)} onchange={(x) => toggle(e.id, (x.target as HTMLInputElement).checked)} aria-label="Select event" /></td>
            <td style="white-space:nowrap">{fmtDate(e.at)}</td>
            <td><span class="badge {e.severity === 'error' ? 'err' : e.severity === 'warning' ? 'warn' : 'grey'}">{e.severity}</span></td>
            <td>{e.component}</td>
            <td><code>{e.code ?? ""}</code></td>
            <td>
              <button class="link small summary" onclick={() => (open = open === e.id ? null : e.id)}>{e.summary}</button>
              {#if open === e.id && (e.detail || e.correlation)}
                <div class="muted pre">{e.detail ?? ""}{e.correlation ? `\nref ${e.correlation}` : ""}</div>
              {/if}
            </td>
          </tr>
        {:else}
          <tr><td colspan="6" class="muted">{loading ? "Loading…" : "No events."}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>

<style>
  .page { height: 100%; padding: 16px; display: flex; flex-direction: column; }
  .kv th { width: 200px; text-align: left; }
  .kv { margin-top: 8px; }
  .list { flex: 1; min-height: 0; }
  tr.hl td { background: var(--accent-soft); }
  .summary { text-align: left; height: auto; white-space: normal; }
  .pre { white-space: pre-wrap; word-break: break-word; }
  textarea { width: 100%; margin-top: 6px; font-family: Consolas, monospace; font-size: 12px; }
</style>
