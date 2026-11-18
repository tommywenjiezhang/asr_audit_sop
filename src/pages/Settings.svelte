<script lang="ts">
  import { onMount } from "svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { api, errMsg, fmtDate, type AppInfo, type FolderNode, type Settings, type Staff, type StaffInput } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";
  import FolderTree from "../components/FolderTree.svelte";
  import SharedDatabasePanel from "../components/SharedDatabasePanel.svelte";

  let { onopendiagnostics }: { onopendiagnostics?: () => void } = $props();

  let s = $state<Settings | null>(null);
  let info = $state<AppInfo | null>(null);
  let conn = $state<{ ok: boolean; text: string } | null>(null);
  let mailboxes = $state<{ display_name: string; store_id: string }[]>([]);
  let tree = $state<FolderNode | null>(null);
  let sharedName = $state("");
  let busy = $state("");
  let internalText = $state("");

  /** subtree of the monitored folder only (fast), so child folders can be chosen without loading the whole mailbox */
  let sub = $state<FolderNode | null>(null);

  onMount(async () => {
    try {
      s = await api.settings();
      s.selected_with_children ??= [];
      internalText = s.internal_senders.join("\n");
      sharedName = s.mailbox_name;
      info = await api.appInfo();
    } catch (e) { toast(errMsg(e), "error"); }
    if (s?.folder) loadSubtree();
  });

  async function loadSubtree() {
    if (!s?.folder) return;
    const f = s.folder;
    const r = await step("subtree", () => api.folders(f.store_id, null, f.entry_id));
    if (r) sub = r.root;
  }

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
    s.selected_with_children = [];
    sub = n;
  }

  function findNode(n: FolderNode | null, id: string): FolderNode | null {
    if (!n) return null;
    if (n.entry_id === id) return n;
    for (const c of n.children) { const f = findNode(c, id); if (f) return f; }
    return null;
  }
  const selectedNode = $derived(s?.folder ? (findNode(tree, s.folder.entry_id) ?? (sub?.entry_id === s.folder.entry_id ? sub : null)) : null);
  function flatten(n: FolderNode): FolderNode[] { return n.children.flatMap((c) => [c, ...flatten(c)]); }
  function toggleSub(n: FolderNode, on: boolean) {
    if (!s) return;
    s.selected_subfolders = s.selected_subfolders.filter((f) => f.entry_id !== n.entry_id);
    s.selected_with_children = s.selected_with_children.filter((id) => id !== n.entry_id);
    if (on) s.selected_subfolders.push({ store_id: n.store_id, entry_id: n.entry_id, path: n.path });
  }
  function toggleChildren(n: FolderNode, on: boolean) {
    if (!s) return;
    s.selected_with_children = s.selected_with_children.filter((id) => id !== n.entry_id);
    if (on) s.selected_with_children.push(n.entry_id);
  }
  const isSelected = (n: FolderNode) => !!s?.selected_subfolders.some((f) => f.entry_id === n.entry_id);
  /** the selected folder (with "and its subfolders") that already covers this folder, if any */
  function coveredBy(n: FolderNode): string | null {
    if (!s) return null;
    const anc = s.selected_subfolders.find((f) => s!.selected_with_children.includes(f.entry_id) && n.path.startsWith(f.path + "/"));
    return anc ? anc.path : null;
  }
  const scanSummary = $derived.by(() => {
    if (!s?.folder) return "nothing selected";
    if (s.subfolder_mode === "all") return `${s.folder.path} and all its subfolders`;
    const parts = [`${s.folder.path} (this folder only)`];
    if (s.subfolder_mode === "selected") {
      for (const f of s.selected_subfolders) parts.push(s.selected_with_children.includes(f.entry_id) ? `${f.path} and all its subfolders` : `${f.path} (this folder only)`);
    }
    return parts.join(" + ");
  });

  // ---- completion (archive) folder: separate picker so it can live in another (shared) mailbox
  let archTree = $state<FolderNode | null>(null);
  let archShared = $state("");
  let archCheck = $state<{ ok: boolean; text: string } | null>(null);
  async function loadArchTree(storeId: string | null, shared: string | null, name: string) {
    const r = await step("archfolders", () => api.folders(storeId, shared));
    if (r && s) { archTree = r.root; s.archive_mailbox_name = name; }
  }
  function pickArchive(n: FolderNode) {
    if (!s) return;
    s.archive_folder = { store_id: n.store_id, entry_id: n.entry_id, path: n.path };
    archCheck = null;
  }
  async function validateArchive() {
    if (!s?.archive_folder) return;
    const a = s.archive_folder;
    const r = await step("validate", () => api.validateFolder(a.store_id, a.entry_id));
    archCheck = r ? { ok: true, text: `OK: ${r.mailbox ? r.mailbox + " › " : ""}${r.path}` } : { ok: false, text: "Not usable - see the message shown." };
    if (r && s.archive_folder) s.archive_folder.path = r.path;
  }
  // ---- staff assignment (stored in SQLite, saved per person - not with "Save Settings")
  const emptyStaff = (): StaffInput => ({ id: null, display_name: "", email: "", mailbox_name: "", store_id: "", folder_entry_id: "", folder_path: "", windows_account: "" });
  let staffRows = $state<Staff[]>([]);
  let me = $state<number | null>(null);
  let editing = $state<StaffInput | null>(null);
  let staffTree = $state<FolderNode | null>(null);
  let staffMsg = $state<{ ok: boolean; text: string } | null>(null);
  let accounts = $state<{ display_name: string; smtp: string }[]>([]);
  const EMAIL_RE = /^[A-Za-z0-9._%+'-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}$/;

  async function loadStaff() {
    try { staffRows = await api.staffList(true); me = await api.currentStaff(); } catch (e) { toast(errMsg(e), "error"); }
  }
  onMount(loadStaff);
  async function staffFolders() {
    if (!s) return;
    const name = editing?.mailbox_name || s.mailbox_name;
    const r = await step("stafffolders", () => (s!.mailbox_store_id ? api.folders(s!.mailbox_store_id, null) : api.folders(null, name)));
    if (r) { staffTree = r.root; if (editing && !editing.mailbox_name) editing.mailbox_name = s.mailbox_name; }
  }
  function pickStaffFolder(n: FolderNode) {
    if (!editing) return;
    editing.store_id = n.store_id; editing.folder_entry_id = n.entry_id; editing.folder_path = n.path;
  }
  async function saveStaff() {
    if (!editing) return;
    if (!EMAIL_RE.test(editing.email.trim())) { staffMsg = { ok: false, text: "Enter a valid email address." }; return; }
    const r = await step("staffsave", () => api.staffSave($state.snapshot(editing) as StaffInput));
    if (r) { staffMsg = { ok: true, text: `Saved ${r.display_name}.` }; editing = null; staffTree = null; await loadStaff(); }
  }
  async function setActive(st: Staff, active: boolean) {
    if (!active && !confirm(`Deactivate ${st.display_name}? Existing tickets and history keep the name; no new assignments are possible.`)) return;
    if (await step("staffactive", () => api.staffSetActive(st.id, active))) await loadStaff();
  }
  async function setMe(v: string) {
    const id = v ? Number(v) : null;
    if ((await step("me", () => api.setCurrentStaff(id))) !== undefined) me = id;
  }
  async function loadAccounts() {
    const r = await step("accounts", () => api.accounts());
    if (r) accounts = r.accounts;
  }

  async function restorePrompt() {
    if (!s) return;
    s.prompt_instructions = await api.promptDefault();
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

    <SharedDatabasePanel {onopendiagnostics} />

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
          {#each mailboxes as m}<button class="small" class:primary={m.store_id === s.mailbox_store_id} disabled={!!busy} onclick={() => pickMailbox(m)}>{m.display_name}</button>{/each}
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
                {#each flatten(selectedNode) as c (c.entry_id)}
                  {@const cov = coveredBy(c)}
                  <div class="row small" style="margin-left:{22 + 14 * (c.path.split('/').length - selectedNode.path.split('/').length - 1)}px">
                    {#if cov}
                      <label class="inline muted"><input type="checkbox" checked disabled />{c.name} <span class="badge grey">included via {cov}</span></label>
                    {:else}
                      <label class="inline"><input type="checkbox" checked={isSelected(c)} onchange={(e) => toggleSub(c, (e.target as HTMLInputElement).checked)} />{c.name}</label>
                      {#if isSelected(c) && c.children.length}
                        <label class="inline muted"><input type="checkbox" checked={s.selected_with_children.includes(c.entry_id)} onchange={(e) => toggleChildren(c, (e.target as HTMLInputElement).checked)} />and its subfolders ({flatten(c).length})</label>
                      {/if}
                    {/if}
                  </div>
                {:else}
                  <div class="small muted" style="margin-left:22px">This folder has no child folders.</div>
                {/each}
              {:else if busy === "subtree"}
                <div class="small muted" style="margin-left:22px">Reading child folders…</div>
              {:else}
                <div class="small muted" style="margin-left:22px">
                  Child folders could not be read. Selected: {s.selected_subfolders.map((f) => f.path).join(", ") || "none"}
                  <button class="link small" onclick={loadSubtree}>Retry</button>
                </div>
              {/if}
            {/if}
          </div></div>
        <div class="notice small" style="margin-top:6px"><b>Scanning:</b> {scanSummary}{#if s.archive_folder}. <b>Never scanned:</b> {s.archive_folder.path} (completion folder){/if}.
          <div class="muted">Emails moved to other subfolders later are still found for Open Outlook Reply and Mark Done; they do not create new tickets.</div></div>
      {/if}
      <div class="small muted">Folders are remembered by their Outlook ID. If a folder is renamed or moved, the app tries to find it again by its path and tells you.</div>
      <div class="field" style="margin-top:8px"><div class="lbl">Scan</div>
        <div class="stack">
          <label class="inline"><input type="radio" bind:group={s.scan_mode} value="all" /> All items in the folder (recommended)</label>
          <label class="inline"><input type="radio" bind:group={s.scan_mode} value="unread" /> Unread items only</label>
          <div class="small muted">Scanning never marks emails as read, never moves them and never changes their flags. With "Unread items only", an email someone already opened before the scan is not imported.</div>
        </div>
      </div>
    </section>

    <section class="panel">
      <h3>Completion Folder (Mark Done)</h3>
      <p class="small muted">When you click <b>Mark Done</b>, the ticket's incoming emails are flagged complete in Outlook and moved to this folder. Nothing is deleted. Mark Done is not possible until a folder is chosen here.</p>
      <div class="small">Completed emails go to: <b>{s.archive_folder ? `${s.archive_mailbox_name ? s.archive_mailbox_name + " › " : ""}${s.archive_folder.path}` : "not selected"}</b>
        {#if archCheck}<span class="badge {archCheck.ok ? 'ok' : 'err'}">{archCheck.text}</span>{/if}</div>
      <div class="row wrap" style="margin-top:8px">
        <button onclick={loadMailboxes} disabled={!!busy}>{busy === "mailboxes" ? "Loading…" : "Find Mailboxes"}</button>
        <span class="small muted">or shared mailbox:</span>
        <input style="width:260px" bind:value={archShared} placeholder={s.mailbox_name} />
        <button onclick={() => loadArchTree(null, archShared || s!.mailbox_name, archShared || s!.mailbox_name)} disabled={!!busy}>Open</button>
        <button onclick={validateArchive} disabled={!!busy || !s.archive_folder}>{busy === "validate" ? "Checking…" : "Validate"}</button>
      </div>
      {#if mailboxes.length}
        <div class="row wrap" style="margin-top:6px">
          {#each mailboxes as m}<button class="small" disabled={!!busy} onclick={() => loadArchTree(m.store_id, null, m.display_name)}>{m.display_name}</button>{/each}
        </div>
      {/if}
      {#if busy === "archfolders"}<div class="muted small">Reading folder list…</div>{/if}
      {#if archTree}
        <div class="treebox panel"><FolderTree node={archTree} selectedId={s.archive_folder?.entry_id ?? ""} onselect={pickArchive} /></div>
      {/if}
      <div class="small muted">The folder is remembered by mailbox and Outlook folder ID (not only its name) and is checked when you save. It must not be a monitored folder.</div>
    </section>

    <section class="panel">
      <h3>Copilot Prompt</h3>
      <p class="small muted">General instructions at the top of every copied prompt. The ticket details, SOP wording and the original email are added below them automatically.</p>
      <textarea rows="10" bind:value={s.prompt_instructions}></textarea>
      <div class="row" style="margin-top:6px"><button onclick={restorePrompt}>Restore Default</button><span class="small muted">Save Settings to apply.</span></div>
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
      <h3>Staff Assignment</h3>
      <p class="small muted">Each staff member has a name, the email address that receives handoff notifications, and optionally their own Outlook folder (needed only for Assign/Reassign, which moves the emails into it). A folder also covers its subfolders unless another staff member has a more specific folder. Names, email addresses and Windows accounts are shared with the team through the shared database; Outlook folders are remembered on this computer only (they are specific to each Outlook profile). The Windows account only pre-selects "I am" on that person's computer; it is not a security control.</p>
      <table class="grid small">
        <thead><tr><th>Name</th><th>Email</th><th>Folder</th><th></th></tr></thead>
        <tbody>
          {#each staffRows as st (st.id)}
            <tr class:inactive={!st.active}>
              <td>{st.display_name}{#if !st.active} <span class="badge grey">inactive</span>{/if}{#if me === st.id} <span class="badge">me</span>{/if}</td>
              <td>{st.email}</td>
              <td>{#if st.folder_entry_id}{st.mailbox_name ? `${st.mailbox_name} › ` : ""}{st.folder_path}{:else}<span class="muted">no folder on this computer</span>{/if}{#if st.windows_account}<div class="muted">{st.windows_account}</div>{/if}</td>
              <td style="white-space:nowrap">
                <button class="link small" onclick={() => { editing = { id: st.id, display_name: st.display_name, email: st.email, mailbox_name: st.mailbox_name ?? "", store_id: st.store_id, folder_entry_id: st.folder_entry_id, folder_path: st.folder_path, windows_account: st.windows_account ?? "" }; staffMsg = null; }}>Edit</button>
                {#if st.active}<button class="link small" onclick={() => setActive(st, false)}>Deactivate</button>{:else}<button class="link small" onclick={() => setActive(st, true)}>Reactivate</button>{/if}
              </td>
            </tr>
          {:else}
            <tr><td colspan="4" class="muted">No staff members yet.</td></tr>
          {/each}
        </tbody>
      </table>
      {#if editing}
        <div class="panel staffform">
          <div class="field"><div class="lbl">Display name</div><input bind:value={editing.display_name} placeholder="e.g. Wenjie" /></div>
          <div class="field"><div class="lbl">Email address</div><div><input type="email" bind:value={editing.email} placeholder="name@doh.nj.gov" />
            {#if editing.email && !EMAIL_RE.test(editing.email.trim())}<div class="small" style="color:var(--err)">Not a valid email address</div>{/if}</div></div>
          <div class="field"><div class="lbl">Windows account</div><div><input bind:value={editing.windows_account} placeholder="DOMAIN\username (optional)" /><div class="small muted">Pre-selects "I am" when this person opens the app on their computer.</div></div></div>
          <div class="field"><div class="lbl">Outlook folder</div>
            <div>
              <div class="row"><b class="small">{editing.folder_path ? `${editing.mailbox_name ? editing.mailbox_name + " › " : ""}${editing.folder_path}` : "not chosen"}</b>
                <button class="small" onclick={staffFolders} disabled={!!busy}>{busy === "stafffolders" ? "Loading…" : "Choose folder…"}</button></div>
              {#if staffTree}<div class="treebox panel"><FolderTree node={staffTree} selectedId={editing.folder_entry_id} onselect={pickStaffFolder} /></div>{/if}
            </div></div>
          <div class="row"><span class="grow"></span><button onclick={() => { editing = null; staffTree = null; }}>Cancel</button><button class="primary" onclick={saveStaff} disabled={!!busy}>Save staff member</button></div>
        </div>
      {:else}
        <button style="margin-top:8px" onclick={() => { editing = emptyStaff(); staffMsg = null; }}>+ Add staff member</button>
      {/if}
      {#if staffMsg}<div class="small" style="color:{staffMsg.ok ? 'var(--ok)' : 'var(--err)'}">{staffMsg.text}</div>{/if}
      <div class="field" style="margin-top:10px"><div class="lbl">I am</div>
        <div><select style="width:240px" value={me == null ? "" : String(me)} onchange={(e) => setMe((e.target as HTMLSelectElement).value)}>
          <option value="">— not set —</option>
          {#each staffRows.filter((x) => x.active) as st (st.id)}<option value={String(st.id)}>{st.display_name}</option>{/each}
        </select><div class="small muted">Your stable staff identity on this computer: recorded as the author of your notes and changes, and used by My Tickets.</div></div></div>
      <div class="field"><div class="lbl">Send notifications from</div>
        <div><div class="row"><select style="width:300px" bind:value={s.notify_account}>
            <option value="">Outlook's default account</option>
            {#if s.notify_account && !accounts.some((a) => a.smtp === s!.notify_account)}<option value={s.notify_account}>{s.notify_account}</option>{/if}
            {#each accounts as a}<option value={a.smtp}>{a.display_name} ({a.smtp})</option>{/each}
          </select><button class="small" onclick={loadAccounts} disabled={!!busy}>Find accounts</button></div>
          <div class="small muted">An account you are allowed to send from. Shared-mailbox "Send As" is not assumed. Save Settings to apply. Notifications go only to the assigned staff member, never to the school.</div></div></div>
    </section>

    <section class="panel">
      <h3>Ticket Preferences</h3>
      <div class="field"><div class="lbl">Your name</div><input bind:value={s.staff_name} placeholder="Recorded in ticket history; available as {'{{staff_name}}'}" /></div>
      <div class="field"><div class="lbl">Default assigned staff</div><input bind:value={s.default_assigned_staff} /></div>
      <div class="field"><div class="lbl">ASR deadline text</div><input bind:value={s.asr_deadline} /></div>
      <div class="field"><div class="lbl">Internal senders</div><div><textarea rows="3" bind:value={internalText} placeholder="one per line, e.g. someone@doh.nj.gov or @doh.nj.gov"></textarea><div class="small muted">Emails from these addresses never create tickets (they are linked to an existing conversation if one exists).</div></div></div>
      <div class="field"><div class="lbl">Duplicate check window</div><div class="row"><input type="number" min="1" style="width:90px" bind:value={s.duplicate_window_days} /> days</div></div>
    </section>

    <section class="panel">
      <h3>Database / Backup</h3>
      {#if info}
        <div class="small muted">Version {info.version} · database schema v{info.schema_version} · template catalog v{info.catalog_version} ({info.template_count} templates) · backend startup {info.startup_ms} ms</div>
        <div class="small muted">Data folder: <code>{info.data_dir}</code> · Logs: <code>{info.log_dir}</code></div>
        {#if info.worker}<div class="small muted">Outlook worker: <code>{info.worker}</code></div>{/if}
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
  .staffform { padding: 10px; margin-top: 8px; }
  tr.inactive td { color: var(--muted); }
  details { margin-top: 6px; }
</style>
