<script lang="ts">
  import { onMount } from "svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { api, errMsg, fmtDate, type AppInfo, type FolderNode, type Settings } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";
  import FolderTree from "../components/FolderTree.svelte";

  let s = $state<Settings | null>(null);
  let info = $state<AppInfo | null>(null);
  let conn = $state<{ ok: boolean; text: string } | null>(null);
  let mailboxes = $state<{ display_name: string; store_id: string }[]>([]);
  let tree = $state<FolderNode | null>(null);
  let sharedName = $state("");
  let busy = $state("");
  let internalText = $state("");

  onMount(async () => {
    try {
      s = await api.settings();
      internalText = s.internal_senders.join("\n");
      sharedName = s.mailbox_name;
      info = await api.appInfo();
    } catch (e) { toast(errMsg(e), "error"); }
  });

  async function step<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
    busy = label;
    try { return await fn(); } catch (e) { toast(errMsg(e), "error"); return undefined; } finally { busy = ""; }
  }

  async function test() {
    const r = await step("test", () => api.outlookTest());
    conn = r ? { ok: true, text: `Connected to Outlook ${r.outlook_version} · profile "${r.profile}" · ${r.store_count} mailbox(es)` } : { ok: false, text: "Outlook is not reachable. See the message shown." };
  }
  async function loadMailboxes() {
    const r = await step("mailboxes", () => api.mailboxes());
    if (r) mailboxes = r.mailboxes;
  }
  async function loadTree(storeId: string | null, shared: string | null) {
    const r = await step("folders", () => api.folders(storeId, shared));
    if (r && s) { tree = r.root; s.mailbox_store_id = r.store_id; }
  }
  function pickMailbox(m: { display_name: string; store_id: string }) {
    if (!s) return;
    s.mailbox_name = m.display_name;
    loadTree(m.store_id, null);
  }

  function pickFolder(n: FolderNode) {
    if (!s) return;
    s.folder = { store_id: n.store_id, entry_id: n.entry_id, path: n.path };
    s.selected_subfolders = [];
  }

  function findNode(n: FolderNode | null, id: string): FolderNode | null {
    if (!n) return null;
    if (n.entry_id === id) return n;
    for (const c of n.children) { const f = findNode(c, id); if (f) return f; }
    return null;
  }
  const selectedNode = $derived(s?.folder ? findNode(tree, s.folder.entry_id) : null);
  function flatten(n: FolderNode): FolderNode[] { return n.children.flatMap((c) => [c, ...flatten(c)]); }
  function toggleSub(n: FolderNode, on: boolean) {
    if (!s) return;
    s.selected_subfolders = s.selected_subfolders.filter((f) => f.entry_id !== n.entry_id);
    if (on) s.selected_subfolders.push({ store_id: n.store_id, entry_id: n.entry_id, path: n.path });
  }

  async function saveSettings() {
    if (!s) return;
    s.internal_senders = internalText.split(/[\n,;]+/).map((x) => x.trim()).filter(Boolean);
    const r = await step("save", () => api.saveSettings($state.snapshot(s) as Settings));
    if (r) {
      s = r;
      toast("Settings saved.", "ok");
      app.sync = await api.syncStatus();
    }
  }

  async function backup() {
    const p = await saveDialog({ defaultPath: `support-desk-backup-${new Date().toISOString().slice(0, 10)}.db`, filters: [{ name: "Database backup", extensions: ["db"] }] });
    if (p && (await step("backup", () => api.backup(p))) !== undefined) toast(`Backup saved to ${p}`, "ok");
  }
  async function restore() {
    const p = await openDialog({ multiple: false, filters: [{ name: "Database backup", extensions: ["db"] }] });
    if (!p || typeof p !== "string") return;
    if (!confirm("Restore this backup? Current data will be replaced. A safety copy of the current database is made first.")) return;
    const safety = await step("restore", () => api.restore(p));
    if (safety) { toast(`Backup restored. Previous data saved to ${safety}`, "ok"); app.ticketsVersion++; }
  }
</script>

{#if s}
  <div class="scroll page">
    <div class="row"><h2 style="margin:0">Settings</h2><span class="grow"></span><button class="primary" disabled={!!busy} onclick={saveSettings}>Save Settings</button></div>

    <section class="panel">
      <h3>Outlook Connection</h3>
      <p class="small muted">Uses your existing classic Outlook for Windows profile. No passwords are stored and no separate sign-in is needed.</p>
      <div class="row"><button onclick={test} disabled={!!busy}>{busy === "test" ? "Testing…" : "Test Outlook Connection"}</button>
        {#if conn}<span class="badge {conn.ok ? 'ok' : 'err'}">{conn.text}</span>{/if}</div>
      <div class="field" style="margin-top:8px"><div class="lbl">Python path (optional)</div>
        <div><input bind:value={s.python_path} placeholder="Auto-detect (py.exe / python.exe)" /><div class="small muted">Only needed if the bundled worker or the Python launcher is not found. Requires the pywin32 package.</div></div></div>
    </section>

    <section class="panel">
      <h3>Monitored Folders</h3>
      <div class="small">Currently monitoring: <b>{s.folder ? `${s.mailbox_name} › ${s.folder.path}` : "nothing selected"}</b>
        · Last successful check: {fmtDate(app.sync?.last_success_at) || "never"}</div>
      <div class="row wrap" style="margin-top:8px">
        <button onclick={loadMailboxes} disabled={!!busy}>{busy === "mailboxes" ? "Loading…" : "Find Mailboxes"}</button>
        <span class="small muted">or open a shared mailbox by name:</span>
        <input style="width:280px" bind:value={sharedName} />
        <button onclick={() => loadTree(null, sharedName)} disabled={!!busy || !sharedName.trim()}>Open</button>
      </div>
      {#if mailboxes.length}
        <div class="row wrap" style="margin-top:6px">
          {#each mailboxes as m}<button class="small" class:primary={m.store_id === s.mailbox_store_id} onclick={() => pickMailbox(m)}>{m.display_name}</button>{/each}
        </div>
      {/if}
      {#if busy === "folders"}<div class="muted small">Reading folder list…</div>{/if}
      {#if tree}
        <div class="treebox panel"><FolderTree node={tree} selectedId={s.folder?.entry_id ?? ""} onselect={pickFolder} /></div>
      {/if}
      {#if s.folder}
        <div class="field" style="margin-top:8px"><div class="lbl">Child folders</div>
          <div class="stack">
            <label class="inline"><input type="radio" bind:group={s.subfolder_mode} value="none" /> Monitor this folder only</label>
            <label class="inline"><input type="radio" bind:group={s.subfolder_mode} value="all" /> Include all child folders</label>
            <label class="inline"><input type="radio" bind:group={s.subfolder_mode} value="selected" /> Include selected child folders</label>
            {#if s.subfolder_mode === "selected"}
              {#if selectedNode}
                {#each flatten(selectedNode) as c}
                  <label class="inline small" style="margin-left:22px"><input type="checkbox" checked={s.selected_subfolders.some((f) => f.entry_id === c.entry_id)} onchange={(e) => toggleSub(c, (e.target as HTMLInputElement).checked)} />{c.path}</label>
                {/each}
              {:else}
                <div class="small muted" style="margin-left:22px">Load the folder list to choose child folders. Selected: {s.selected_subfolders.map((f) => f.path).join(", ") || "none"}</div>
              {/if}
            {/if}
          </div></div>
      {/if}
      <div class="small muted">Folders are remembered by their Outlook ID. If a folder is renamed or moved, the app tries to find it again by its path and tells you.</div>
    </section>

    <section class="panel">
      <h3>Schedule</h3>
      <div class="field"><div class="lbl">Automatic checking</div><label class="inline"><input type="checkbox" bind:checked={s.sync_enabled} /> Check the monitored folder automatically</label></div>
      <div class="field"><div class="lbl">Interval</div><select style="width:160px" bind:value={s.interval_minutes}>{#each [1, 5, 10, 15, 30] as m}<option value={m}>Every {m} minute{m > 1 ? "s" : ""}</option>{/each}</select></div>
      <div class="field"><div class="lbl">On startup</div><label class="inline"><input type="checkbox" bind:checked={s.check_on_startup} /> Check when the app starts</label></div>
      <div class="field"><div class="lbl">Windows login</div><label class="inline"><input type="checkbox" bind:checked={s.launch_at_login} /> Start minimized to the tray when I sign in</label></div>
      <div class="field"><div class="lbl">Closing the window</div><label class="inline"><input type="checkbox" bind:checked={s.close_to_tray} /> Keep running in the system tray</label></div>
      <div class="field"><div class="lbl">First import</div><div class="row"><input type="number" min="1" max="365" style="width:90px" bind:value={s.initial_lookback_days} /> days back</div></div>
      <details class="small"><summary>Advanced</summary>
        <div class="field"><div class="lbl">Overlap window</div><div class="row"><input type="number" min="0" style="width:90px" bind:value={s.overlap_minutes} /> minutes re-checked each time (prevents missed emails)</div></div>
        <div class="field"><div class="lbl">Full re-scan</div><div class="row"><input type="number" min="0" style="width:90px" bind:value={s.full_rescan_hours} /> hours (finds emails moved into the folder later; 0 = off)</div></div>
        <div class="field"><div class="lbl">Max emails per check</div><input type="number" min="10" style="width:90px" bind:value={s.max_messages_per_sync} /></div>
      </details>
    </section>

    <section class="panel">
      <h3>Ticket Preferences</h3>
      <div class="field"><div class="lbl">Your name</div><input bind:value={s.staff_name} placeholder="Recorded in ticket history; available as {'{{staff_name}}'}" /></div>
      <div class="field"><div class="lbl">Default assigned staff</div><input bind:value={s.default_assigned_staff} /></div>
      <div class="field"><div class="lbl">ASR deadline text</div><input bind:value={s.asr_deadline} /></div>
      <div class="field"><div class="lbl">Internal senders</div><div><textarea rows="3" bind:value={internalText} placeholder="one per line, e.g. someone@doh.nj.gov or @doh.nj.gov"></textarea><div class="small muted">Emails from these addresses never create tickets (they are linked to an existing conversation if one exists).</div></div></div>
      <div class="field"><div class="lbl">Duplicate check window</div><div class="row"><input type="number" min="1" style="width:90px" bind:value={s.duplicate_window_days} /> days</div></div>
      <div class="field"><div class="lbl">Outlook drafts</div><label class="inline"><input type="checkbox" bind:checked={s.open_outlook_draft_after_create} /> Open the draft in Outlook after creating it</label></div>
    </section>

    <section class="panel">
      <h3>Database / Backup</h3>
      {#if info}
        <div class="small muted">Version {info.version} · database schema v{info.schema_version} · template catalog v{info.catalog_version} ({info.template_count} templates) · backend startup {info.startup_ms} ms</div>
        <div class="small muted">Data folder: <code>{info.data_dir}</code> · Logs: <code>{info.log_dir}</code></div>
      {/if}
      <div class="row" style="margin-top:8px">
        <button onclick={backup} disabled={!!busy}>Back Up Database…</button>
        <button onclick={restore} disabled={!!busy}>Restore From Backup…</button>
      </div>
    </section>
  </div>
{/if}

<style>
  .page { height: 100%; padding: 16px; }
  section { padding: 12px; margin-top: 12px; }
  .treebox { max-height: 300px; overflow: auto; padding: 6px; margin-top: 8px; }
  details { margin-top: 6px; }
</style>
