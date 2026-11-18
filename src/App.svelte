<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api, errMsg, fmtDate, type SharedStatus, type SyncStatus } from "./lib/api";
  import { app, loadCatalog, sharedLabel, toast } from "./lib/stores.svelte";
  import Tickets from "./pages/Tickets.svelte";
  import Templates from "./pages/Templates.svelte";
  import Reports from "./pages/Reports.svelte";
  import Overview from "./pages/Overview.svelte";
  import SettingsPage from "./pages/Settings.svelte";
  import Diagnostics from "./pages/Diagnostics.svelte";

  type Page = "tickets" | "overview" | "templates" | "reports" | "settings" | "diagnostics";
  let page = $state<Page>("tickets");
  let ready = $state(false);
  let loadError = $state("");
  const sl = $derived(sharedLabel(app.shared));

  onMount(() => {
    const unsubs: (() => void)[] = [];
    (async () => {
      try {
        await loadCatalog();
        app.sync = await api.syncStatus();
        app.shared = await api.sharedStatus();
        ready = true;
      } catch (e) {
        loadError = errMsg(e);
      }
      unsubs.push(await listen<SyncStatus>("sync-status", (ev) => (app.sync = { ...app.sync, ...ev.payload })));
      unsubs.push(await listen("tickets-changed", () => app.ticketsVersion++));
      unsubs.push(await listen<SharedStatus>("shared-status", (ev) => (app.shared = ev.payload)));
    })();
    return () => unsubs.forEach((u) => u());
  });

  async function checkNow() {
    try {
      const r = await api.syncNow();
      const rep = r.report;
      toast(`Checked Outlook: ${rep.new_messages} new email(s), ${rep.created_tickets} new ticket(s), ${rep.updated_tickets} updated.`, "ok");
      for (const w of r.warnings) toast(w, "info");
      app.ticketsVersion++;
    } catch (e) {
      toast(errMsg(e), "error");
    }
    app.sync = await api.syncStatus();
  }
</script>

<div class="shell">
  <nav class="side">
    <div class="brand">ASR/Audit<br /><span>Support Desk</span></div>
    <button class="nav" class:active={page === "tickets"} onclick={() => (page = "tickets")}>Tickets</button>
    <button class="nav" class:active={page === "templates"} onclick={() => (page = "templates")} title="Approved SOP wording used in the Copilot prompt">SOP Templates</button>
    <button class="nav" class:active={page === "overview"} onclick={() => (page = "overview")}>Team Overview</button>
    <button class="nav" class:active={page === "reports"} onclick={() => (page = "reports")}>Reports</button>
    <button class="nav" class:active={page === "settings"} onclick={() => (page = "settings")}>Settings</button>
    <button class="nav" class:active={page === "diagnostics"} onclick={() => (page = "diagnostics")} title="Technical events and copyable log for troubleshooting">Diagnostics</button>
    <div class="grow"></div>
    <div class="syncbox small">
      <div><b>Folder:</b> {app.sync?.folder_display ?? "not selected"}</div>
      <div><b>Last check:</b> {fmtDate(app.sync?.last_success_at) || "never"}</div>
      {#if app.sync?.next_run_at}<div><b>Next:</b> {fmtDate(app.sync.next_run_at)}</div>{/if}
      {#if app.sync?.last_error}<div class="errline" title={app.sync.last_error}>⚠ {app.sync.last_error}</div>{/if}
      <button class="sharedline {sl.kind}" title={app.shared?.error_message ?? "Shared database status — open Settings"} onclick={() => (page = "settings")}>{sl.text}</button>
      <button class="primary" style="width:100%;margin-top:6px" disabled={app.sync?.running} onclick={checkNow}>
        {app.sync?.running ? "Checking…" : "Check Now"}
      </button>
    </div>
  </nav>

  <main>
    {#if loadError}
      <div class="notice err" style="margin:16px">Could not start: {loadError}</div>
    {:else if !ready}
      <div class="muted" style="padding:16px">Loading…</div>
    {:else if page === "tickets"}
      <Tickets />
    {:else if page === "templates"}
      <Templates />
    {:else if page === "overview"}
      <Overview />
    {:else if page === "reports"}
      <Reports />
    {:else if page === "diagnostics"}
      <Diagnostics />
    {:else}
      <SettingsPage onopendiagnostics={() => (page = "diagnostics")} />
    {/if}
  </main>

  {#if app.toast}
    <div class="toast {app.toast.kind}" role="status">{app.toast.text}<button class="link" onclick={() => (app.toast = null)}>✕</button></div>
  {/if}
</div>

<style>
  .shell { display: flex; height: 100%; }
  .side { width: 190px; flex: none; background: #17324d; color: #dce6f0; display: flex; flex-direction: column; padding: 10px 8px; gap: 2px; }
  .brand { font-weight: 700; font-size: 15px; padding: 4px 8px 12px; color: #fff; line-height: 1.2; }
  .brand span { font-weight: 400; font-size: 13px; color: #a9c1d9; }
  .side button { background: none; border: none; color: inherit; text-align: left; height: 30px; border-radius: 5px; }
  .side button.nav { font-weight: 600; }
  .side button.active { background: #2b5279; color: #fff; }
  .side button:hover { background: #24476b; }
  .side button.primary { background: #3a77b5; color: #fff; text-align: center; }
  .grow { flex: 1; }
  .syncbox { background: #0f2539; border-radius: 6px; padding: 8px; line-height: 1.5; word-break: break-word; }
  .errline { color: #ffb4ab; max-height: 60px; overflow: hidden; }
  .side button.sharedline { height: auto; padding: 3px 4px; margin-top: 4px; font-size: 11px; line-height: 1.3; width: 100%; white-space: normal; }
  .sharedline.ok { color: #9fe0b5; } .sharedline.warn { color: #ffd38a; } .sharedline.err { color: #ffb4ab; } .sharedline.grey { color: #a9c1d9; }
  main { flex: 1; min-width: 0; height: 100%; overflow: hidden; }
  .toast { position: fixed; right: 16px; bottom: 16px; max-width: 460px; padding: 10px 12px; border-radius: 6px; background: #17324d; color: #fff; box-shadow: 0 4px 14px rgba(0,0,0,.25); z-index: 60; display: flex; gap: 8px; align-items: start; }
  .toast.ok { background: #245e3c; } .toast.error { background: #8c1d18; }
  .toast button { color: #fff; }
</style>
