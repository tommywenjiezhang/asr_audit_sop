<script lang="ts">
  import { onMount } from "svelte";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { api, errMsg, fmtDate, type ImportPreview, type SharedStatus } from "../lib/api";
  import { app, sharedLabel, toast } from "../lib/stores.svelte";

  let { onopendiagnostics }: { onopendiagnostics?: () => void } = $props();

  const DEFAULT_PATH = String.raw`\\epi-qb-5\VPD_Shared\PgmDocs\Assessment and Coverage\Annual Immunization Status Report\ASR Master List Revamp--2023\asr_email_support.accdb`;

  let mode = $state<"shared" | "standalone">("shared");
  let path = $state("");
  let minutes = $state(2);
  let busy = $state("");
  let preview = $state<ImportPreview | null>(null);
  let setup = $state<{ path: string; name: string; environment: "test" | "production" } | null>(null);

  const st = $derived<SharedStatus | null>(app.shared);
  const label = $derived(sharedLabel(st));
  const dirty = $derived(!!st && (mode !== st.mode || path.trim() !== (st.path ?? "").trim()));

  onMount(async () => {
    try {
      const s = await api.settings();
      mode = s.shared_mode;
      path = s.shared_db_path;
      minutes = s.shared_sync_minutes;
      app.shared = await api.sharedStatus();
    } catch (e) { toast(errMsg(e), "error"); }
  });

  async function step<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
    busy = label;
    try { return await fn(); } catch (e) { toast(errMsg(e), "error"); return undefined; } finally { busy = ""; }
  }

  async function saveConfig() {
    if (mode === "standalone" && st?.mode === "shared" && !confirm("Switch to Standalone? Cases stay on this computer and nothing is sent automatically, even when the shared database is reachable. Changes already queued stay queued.")) return;
    const r = await step("save", () => api.sharedSaveConfig(mode, path.trim(), minutes));
    if (r) { toast("Shared database settings saved.", "ok"); app.shared = await api.sharedStatus(); }
  }
  async function test() {
    const r = await step("test", () => api.sharedTest());
    if (r) app.shared = r;
  }
  async function syncNow() {
    const r = await step("sync", () => api.sharedSyncNow());
    if (r) {
      app.shared = r;
      const rep = r.last_report;
      if (rep) toast(`Synchronized: sent ${rep.pushed}, received ${rep.pulled} case(s)${rep.conflicts ? `, ${rep.conflicts} conflict(s)` : ""}.`, rep.conflicts ? "info" : "ok");
      app.ticketsVersion++;
    }
  }
  async function releaseHeld() {
    if (!st?.bound_database_name) return;
    if (!confirm(`Send ${st.queue.held} held change(s) to "${st.bound_database_name}"? They were made before this database was confirmed (or before a backup was restored). Each is still checked against newer shared work; nothing is overwritten silently.`)) return;
    const n = await step("release", () => api.sharedReleaseHeld());
    if (n !== undefined) { toast(`${n} change(s) will be sent.`, "ok"); app.shared = await api.sharedStatus(); }
  }
  async function discardHeld() {
    if (!confirm("Keep the held changes on this computer only? Your local data stays as it is; these changes are not sent. Cases that were never sent can be imported later.")) return;
    const n = await step("discard", () => api.sharedDiscardHeld());
    if (n !== undefined) { toast(`${n} held change(s) kept local.`, "ok"); app.shared = await api.sharedStatus(); }
  }
  async function switchDb() {
    if (!st?.database_id) return;
    if (!confirm(`Use "${st.database_name}" from now on? ${st.queue.pending + st.queue.held} change(s) made for "${st.bound_database_name}" will be isolated and NOT sent to the new database.`)) return;
    const n = await step("switch", () => api.sharedSwitchDatabase(st!.database_id!));
    if (n !== undefined) { toast(`Switched. ${n} change(s) isolated.`, "ok"); await syncNow(); }
  }
  async function loadPreview() {
    preview = (await step("preview", () => api.sharedImportPreview())) ?? null;
  }
  async function runImport() {
    if (!preview) return;
    if (!confirm(`Queue ${preview.local_only} local case(s) for "${st?.bound_database_name ?? "the shared database"}"? Cases already there (same Outlook email) are joined, never duplicated; uncertain matches are kept separate for review.`)) return;
    let total = 0;
    for (;;) {
      const n = await step("import", () => api.sharedImportRun(200));
      if (!n) break;
      total += n;
    }
    toast(`${total} case(s) queued for the shared database.`, "ok");
    await loadPreview();
    app.shared = await api.sharedStatus();
  }
  async function pickNewPath() {
    const p = await saveDialog({ defaultPath: "asr_email_support_TEST.accdb", filters: [{ name: "Access database", extensions: ["accdb"] }] });
    if (p && setup) setup.path = p;
  }
  async function createDb() {
    if (!setup) return;
    if (setup.environment === "production" && !confirm("Create a PRODUCTION shared database? Only one production database should exist. Make sure the share permissions are set first (see the setup guide).")) return;
    const id = await step("create", () => api.sharedCreateDatabase(setup!.path, setup!.name, setup!.environment));
    if (id) {
      toast(`Created "${id.display_name}" (${id.environment}).`, "ok");
      path = setup.path;
      setup = null;
    }
  }

  let initName = $state("ASR Support Desk");
  let initEnv = $state<"test" | "production">("production");
  async function initExisting() {
    if (!st) return;
    if (!confirm(`Set up the empty database at
${st.path}
as the ${initEnv.toUpperCase()} ASR shared database "${initName}"? A verified backup copy of the file is written next to it first. Everyone else must have the file closed.`)) return;
    const r = await step("init", () => api.sharedInitializeExisting(st!.path, initName, initEnv));
    if (r) { toast(`Initialized "${r.identity.display_name}" (${r.tables_created.length} tables). Backup: ${r.backup_path}`, "ok"); await test(); }
  }

  const yesno = (v?: boolean | null) => (v == null ? "—" : v ? "Yes" : "No");
</script>

<section class="panel">
  <div class="row"><h3 style="margin:0">Shared Database</h3><span class="badge {label.kind}">{label.text}</span>{#if st?.running}<span class="badge">Checking…</span>{/if}</div>
  <p class="small muted">Team cases, owners, notes and history are kept in a shared Microsoft Access database on the network share. This computer always saves first in its own local database, so you can keep working when the share, the Access driver or your permissions are unavailable; changes are sent when the database is reachable.</p>

  <div class="field"><div class="lbl">Mode</div>
    <div class="stack">
      <label class="inline"><input type="radio" bind:group={mode} value="shared" /> Shared — save locally and synchronize with the shared database</label>
      <label class="inline"><input type="radio" bind:group={mode} value="standalone" /> Standalone — keep everything on this computer (nothing is uploaded, even when the share is reachable)</label>
    </div></div>
  <div class="field"><div class="lbl">Database path</div>
    <div><input bind:value={path} placeholder={DEFAULT_PATH} disabled={mode === "standalone"} />
      <div class="row small"><button class="link small" onclick={() => (path = DEFAULT_PATH)}>Use the team default</button>
        <span class="muted">Per computer. Never a OneDrive folder. The app never creates or overwrites a database at this path by itself.</span></div></div></div>
  <div class="field"><div class="lbl">Sync every</div><div class="row"><input type="number" min="1" max="60" style="width:70px" bind:value={minutes} /> minutes (and a few seconds after each change)</div></div>
  <div class="row wrap">
    <button class="primary" onclick={saveConfig} disabled={!!busy}>{busy === "save" ? "Saving…" : "Save"}</button>
    <button onclick={test} disabled={!!busy || dirty}>{busy === "test" ? "Testing…" : "Test Connection"}</button>
    <button onclick={syncNow} disabled={!!busy || dirty || mode === "standalone"}>{busy === "sync" ? "Synchronizing…" : "Sync Now"}</button>
    {#if dirty}<span class="small muted">Save first.</span>{/if}
    <span class="grow"></span>
    {#if onopendiagnostics}<button class="link small" onclick={() => onopendiagnostics?.()}>Open Diagnostics</button>{/if}
  </div>

  {#if st}
    <table class="grid small kv" style="margin-top:8px">
      <tbody>
        <tr><th>Connection</th><td>{st.state.replace(/_/g, " ")}{#if st.last_attempt_at} <span class="muted">(checked {fmtDate(st.last_attempt_at)})</span>{/if}</td></tr>
        <tr><th>Reachable / readable / write permission</th><td>{yesno(st.reachable)} / {yesno(st.readable)} / {yesno(st.writable)}</td></tr>
        <tr><th>Database</th><td>{#if st.database_name}<b>{st.database_name}</b> <span class="badge {st.environment === 'production' ? 'ok' : 'warn'}">{st.environment}</span> <span class="muted">id {st.database_id?.slice(0, 8)}…</span>{:else}—{/if}</td></tr>
        <tr><th>This computer is bound to</th><td>{st.bound_database_name ? `${st.bound_database_name} (id ${st.bound_database_id?.slice(0, 8)}…)` : "not yet (first successful connection binds it)"}</td></tr>
        <tr><th>Schema / app compatibility</th><td>{st.schema_version != null ? `database v${st.schema_version}, app v${st.app_schema_version} — ${st.compatibility}` : "—"}</td></tr>
        <tr><th>Last successful sync</th><td>{fmtDate(st.last_success_at) || "never"}{#if st.next_attempt_at && st.mode === "shared"} <span class="muted">· next {fmtDate(st.next_attempt_at)}{st.consecutive_failures ? ` (retrying, attempt ${st.consecutive_failures + 1})` : ""}</span>{/if}</td></tr>
        <tr><th>Changes waiting</th><td>{st.queue.pending} pending · {st.queue.held} held for review · {st.queue.conflicts} conflict(s) · {st.queue.rejected} failed{#if st.queue.isolated} · {st.queue.isolated} isolated (other database){/if}</td></tr>
      </tbody>
    </table>
    {#if st.error_message}
      <div class="notice {st.state === 'unavailable' || st.state === 'locked' ? 'warn' : 'err'} small" style="margin-top:6px">
        {st.error_message}{#if st.provider_detail} <span class="muted">({st.error_code}; {st.provider_detail})</span>{:else if st.error_code} <span class="muted">({st.error_code})</span>{/if}
      </div>
    {/if}
    {#if st.empty_database}
      <div class="notice warn small" style="margin-top:6px">
        This file is an <b>empty</b> Access database. Initialize it as the shared database (administrator, once; a verified backup is made first):
        <div class="row" style="margin-top:4px"><input style="width:220px" bind:value={initName} /><select style="width:130px" bind:value={initEnv}><option value="production">production</option><option value="test">test</option></select>
          <button class="small primary" onclick={initExisting} disabled={!!busy || !initName.trim()}>{busy === "init" ? "Initializing…" : "Initialize this database"}</button></div>
      </div>
    {/if}
    {#if st.state === "identity_changed"}
      <div class="notice err small" style="margin-top:6px">
        The file at this path is <b>{st.database_name ?? "a different database"}</b>, but this computer's changes belong to <b>{st.bound_database_name}</b>. Nothing was sent.
        Point the path back to the original database, or <button class="small" onclick={switchDb} disabled={!!busy || !st.database_id}>Use this database instead…</button> (pending changes for the old database are isolated, not sent).
      </div>
    {/if}
    {#if st.queue.held > 0 && st.bound_database_id}
      <div class="notice warn small" style="margin-top:6px">
        <b>{st.queue.held} change(s) held for review.</b> They were made before this computer confirmed the shared database (or a backup was restored), so they are not sent automatically.
        <div class="row" style="margin-top:4px"><button class="small primary" onclick={releaseHeld} disabled={!!busy}>Send to {st.bound_database_name}</button><button class="small" onclick={discardHeld} disabled={!!busy}>Keep on this computer only</button></div>
      </div>
    {/if}
  {/if}

  <details style="margin-top:8px" ontoggle={(e) => { if ((e.target as HTMLDetailsElement).open && !preview) loadPreview(); }}>
    <summary class="small"><b>Import existing local cases</b> (one-time migration)</summary>
    <div class="small" style="margin-top:6px">
      Cases created on this computer before the shared database (or while in Standalone mode) are not uploaded automatically. Preview first; the import is restartable and matches the same Outlook email across computers so nothing is duplicated. Emails without a Message-ID are never merged automatically (they are flagged as possible duplicates for review).
      {#if preview}
        <ul>
          <li>{preview.local_only} local case(s) not yet in the shared database ({preview.open} open, {preview.done} Done)</li>
          <li>{preview.with_message_id} can be matched by Outlook Message-ID; {preview.without_message_id} cannot (kept separate if similar)</li>
          <li>{preview.already_shared == null ? "Connect to see how many already exist in the shared database." : `${preview.already_shared} already exist in the shared database and will be joined, not duplicated`}</li>
          <li>{preview.staff} staff record(s) will be shared (Outlook folders stay on this computer)</li>
        </ul>
        <div class="row"><button class="small" onclick={loadPreview} disabled={!!busy}>Refresh preview</button>
          <button class="small primary" onclick={runImport} disabled={!!busy || preview.local_only === 0 || !st?.bound_database_id}>{busy === "import" ? "Queuing…" : "Import local cases"}</button>
          {#if !st?.bound_database_id}<span class="muted">Connect first (Test Connection / Sync Now).</span>{/if}</div>
      {:else if busy === "preview"}<div class="muted">Calculating…</div>{/if}
      <div class="muted">Back up first (Database / Backup below). The local database keeps working as before; the import only adds the cases to the shared database.</div>
    </div>
  </details>

  <details style="margin-top:6px">
    <summary class="small"><b>Create a new shared database</b> (setup / test only)</summary>
    <div class="small" style="margin-top:6px">
      Creates a <b>new, empty</b> Access file with the shared tables. It refuses to touch a file that already exists. Use a test database for pilots; create the production database once, on the network share, after permissions are set.
      {#if setup}
        <div class="field"><div class="lbl">New file</div><div class="row"><input bind:value={setup.path} placeholder="\\server\share\folder\asr_email_support_TEST.accdb" /><button class="small" onclick={pickNewPath}>Browse…</button></div></div>
        <div class="field"><div class="lbl">Name</div><input bind:value={setup.name} /></div>
        <div class="field"><div class="lbl">Environment</div><select style="width:160px" bind:value={setup.environment}><option value="test">test</option><option value="production">production</option></select></div>
        <div class="row"><button class="small" onclick={() => (setup = null)}>Cancel</button><button class="small primary" onclick={createDb} disabled={!!busy || !setup.path.trim() || !setup.name.trim()}>{busy === "create" ? "Creating…" : "Create database"}</button></div>
      {:else}
        <div><button class="small" onclick={() => (setup = { path: "", name: "ASR Support Desk (test)", environment: "test" })}>Set up a new database…</button></div>
      {/if}
    </div>
  </details>
</section>

<style>
  section { padding: 12px; margin-top: 12px; }
  .kv th { width: 260px; text-align: left; font-weight: 600; }
  ul { margin: 6px 0; padding-left: 18px; }
</style>
