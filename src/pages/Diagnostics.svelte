<script lang="ts">
  import { tick } from "svelte";
  import { save } from "@tauri-apps/plugin-dialog";
  import { api, copyText, errMsg, fmtDate, type DiagEvent, type DiagOverview, type IntegrityReport, type ProfileInfo } from "../lib/api";
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

  let tool = $state("");
  let toolMsg = $state<{ kind: "ok" | "err"; text: string } | null>(null);
  let integrity = $state<IntegrityReport | null>(null);
  let profile = $state<ProfileInfo | null>(null);
  api.profileInfo().then((p) => (profile = p)).catch(() => {});

  async function runTool(name: string, fn: () => Promise<string>) {
    tool = name;
    toolMsg = null;
    try {
      toolMsg = { kind: "ok", text: await fn() };
    } catch (e) {
      toolMsg = { kind: "err", text: errMsg(e) };
    } finally {
      tool = "";
      load();
    }
  }
  const testDb = () => runTool("db", async () => {
    const s = await api.sharedTest();
    app.shared = s;
    return `Shared database: reachable ${s.reachable ? "yes" : "no"}, readable ${s.readable ? "yes" : "no"}, write permission ${s.writable ? "yes" : "no"}${s.database_name ? ` — ${s.database_name} (${s.environment}, schema v${s.schema_version})` : ""}${s.error_message ? ` — ${s.error_message}` : ""}`;
  });
  const testOutlook = () => runTool("outlook", async () => {
    const r = await api.diagTestOutlook();
    return `Outlook answered: profile ${String(r.profile ?? "?")}, ${String(r.store_count ?? "?")} mailbox(es)${profile?.outlook === "mock" ? " (TEST MODE: mock Outlook)" : ""}`;
  });
  const syncNow = () => runTool("sync", async () => {
    const s = await api.sharedSyncNow();
    app.shared = s;
    const r = s.last_report;
    return r ? `Synchronized: sent ${r.pushed} (accepted ${r.accepted}, conflicts ${r.conflicts}, rejected ${r.rejected}), received ${r.pulled} case(s) in ${r.elapsed_ms} ms` : (s.error_message ?? `State: ${s.state}`);
  });
  const checkIntegrity = () => runTool("integrity", async () => {
    integrity = await api.sharedIntegrity();
    return integrity.ok ? "Shared database integrity: no problems found." : "Shared database integrity: problems found (see the table).";
  });
  const backupShared = () => runTool("backup", async () => {
    const [path] = await api.sharedBackupNow(null);
    return `Verified copy written: ${path}`;
  });
  async function exportLog() {
    const path = await save({ defaultPath: `asr-diagnostics-${new Date().toISOString().slice(0, 10)}.txt`, filters: [{ name: "Text", extensions: ["txt"] }] });
    if (!path) return;
    runTool("export", async () => `${await api.diagExport(path, sev)} line(s) of redacted log saved to ${path}`);
  }
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
        {#if profile}<tr><th>Profile</th><td>{profile.test_mode ? `TEST MODE "${profile.name}" · ` : ""}Outlook {profile.outlook === "mock" ? "mock (no real Outlook changes)" : "live"} · data <code>{profile.data_dir}</code></td></tr>{/if}
      </tbody>
    </table>
  {/if}

  <div class="panel tools">
    <div class="row wrap">
      <b>Tools</b>
      <button class="small" disabled={!!tool} onclick={testDb}>{tool === "db" ? "Testing…" : "Test Database Connection"}</button>
      <button class="small" disabled={!!tool} onclick={testOutlook}>{tool === "outlook" ? "Testing…" : "Test Outlook Connection"}</button>
      <button class="small" disabled={!!tool || app.shared?.mode === "standalone"} onclick={syncNow}>{tool === "sync" ? "Synchronizing…" : "Sync Now / Retry"}</button>
      <button class="small" disabled={!!tool || app.shared?.mode === "standalone"} onclick={checkIntegrity}>{tool === "integrity" ? "Checking…" : "Check Shared Integrity"}</button>
      <button class="small" disabled={!!tool || app.shared?.mode === "standalone"} onclick={backupShared} title="Byte-verified copy next to the shared database file">{tool === "backup" ? "Copying…" : "Back Up Shared Database"}</button>
      <button class="small" disabled={!!tool} onclick={exportLog}>Export Log to File…</button>
    </div>
    {#if toolMsg}<div class="notice small {toolMsg.kind === 'err' ? 'err' : ''}" style="margin-top:6px">{toolMsg.text}</div>{/if}
    {#if integrity}
      <table class="grid small" style="margin-top:6px">
        <thead><tr><th>Check</th><th class="num">Problems</th></tr></thead>
        <tbody>
          {#if integrity.missing_tables.length}<tr><td>Missing tables: {integrity.missing_tables.join(", ")}</td><td class="num bad">{integrity.missing_tables.length}</td></tr>{/if}
          {#each integrity.checks as [c, n]}<tr><td>{c}</td><td class="num" class:bad={n > 0}>{n}</td></tr>{/each}
        </tbody>
      </table>
      <div class="small muted">{integrity.display_name} ({integrity.environment}) schema v{integrity.schema_version} · {integrity.counts.map(([t, n]) => `${t} ${n}`).join(" · ")}</div>
    {/if}
  </div>

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
  .tools { padding: 8px 10px; margin-top: 10px; }
  .num { text-align: right; width: 80px; }
  .bad { color: var(--err); font-weight: 700; }
  .page { height: 100%; padding: 16px; display: flex; flex-direction: column; }
  .kv th { width: 200px; text-align: left; }
  .kv { margin-top: 8px; }
  .list { flex: 1; min-height: 0; }
  tr.hl td { background: var(--accent-soft); }
  .summary { text-align: left; height: auto; white-space: normal; }
  .pre { white-space: pre-wrap; word-break: break-word; }
  textarea { width: 100%; margin-top: 6px; font-family: Consolas, monospace; font-size: 12px; }
</style>
