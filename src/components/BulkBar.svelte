<script lang="ts">
  import { save } from "@tauri-apps/plugin-dialog";
  import { api, errMsg, type BulkOutcome } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";
  import BulkResults from "./BulkResults.svelte";

  /** Actions for several selected cases. Every case is handled and reported separately. */
  let { ids, onclear, onassign, onchanged }: { ids: number[]; onclear: () => void; onassign: (ids: number[]) => void; onchanged: () => void } = $props();

  let busy = $state(false);
  let result = $state<{ title: string; action: string; reason: string | null; outcome: BulkOutcome } | null>(null);

  const LABEL: Record<string, string> = {
    claim: "Claim", takeover: "Take over", release: "Clear owner", start: "Start Work", flag_review: "Flag for Review", resume: "Review handled", done: "Mark Done", archive: "Archive in Outlook", reopen: "Reopen",
  };

  async function run(action: string, list: number[] = ids, reasonIn: string | null = null) {
    let reason = reasonIn;
    if (action === "flag_review" && reason == null) {
      reason = prompt(`Reason for review (${list.length} case(s)):`, "Flagged for review");
      if (reason == null) return;
    }
    if (action === "done" && !confirm(`Mark ${list.length} case(s) Done? No sent-reply check is made. ${app.shared?.mode === "shared" ? "Outlook archiving runs after the shared database confirms each decision." : ""}`)) return;
    if (action === "takeover" && !confirm(`Take over ${list.length} case(s)? The previous owners' work and dates are kept.`)) return;
    if (action === "archive" && !confirm(`Flag and move the covered emails of ${list.length} Done case(s) to the Completed folder?`)) return;
    busy = true;
    try {
      const outcome = await api.bulk(action, list, reason);
      result = { title: `${LABEL[action] ?? action}: ${list.length} case(s)`, action, reason, outcome };
      onchanged();
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      busy = false;
    }
  }

  async function exportSel() {
    const path = await save({ defaultPath: `asr-selected-${new Date().toISOString().slice(0, 10)}.csv`, filters: [{ name: "CSV (Excel)", extensions: ["csv"] }] });
    if (!path) return;
    try {
      const n = await api.exportSelected(path, ids);
      toast(`Exported ${n} case(s).`, "ok");
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
</script>

<div class="bulk row wrap small">
  <b>{ids.length} selected</b>
  <button class="small" disabled={busy} onclick={() => run("claim")}>Claim</button>
  <button class="small" disabled={busy} onclick={() => onassign(ids)} title="Move the emails to a staff folder and send one handoff per case">Assign / Reassign…</button>
  <button class="small" disabled={busy} onclick={() => run("start")}>Start Work</button>
  <button class="small" disabled={busy} onclick={() => run("flag_review")}>Flag for Review…</button>
  <button class="small" disabled={busy} onclick={() => run("done")}>Mark Done</button>
  <button class="small" disabled={busy} onclick={() => run("archive")}>Archive in Outlook</button>
  <details class="more">
    <summary class="small">More</summary>
    <div class="menu">
      <button class="small" disabled={busy} onclick={() => run("takeover")}>Take over…</button>
      <button class="small" disabled={busy} onclick={() => run("release")}>Clear owner</button>
      <button class="small" disabled={busy} onclick={() => run("resume")}>Review handled</button>
      <button class="small" disabled={busy} onclick={() => run("reopen")}>Reopen</button>
      <button class="small" disabled={busy} onclick={exportSel}>Export selected (CSV)</button>
    </div>
  </details>
  <button class="link small" onclick={onclear}>Clear</button>
  {#if busy}<span class="muted">Working…</span>{/if}
</div>

{#if result}
  <BulkResults title={result.title} outcome={result.outcome} onclose={() => (result = null)} onretry={(failed) => { const r = result!; result = null; run(r.action, failed, r.reason); }} />
{/if}

<style>
  .bulk { background: var(--accent-soft); padding: 6px 8px; border-radius: var(--radius); }
  .more { position: relative; }
  .more summary { cursor: pointer; list-style: none; padding: 2px 6px; border: 1px solid var(--border); border-radius: 4px; background: #fff; }
  .menu { position: absolute; z-index: 20; background: #fff; border: 1px solid var(--border); border-radius: 6px; padding: 6px; display: flex; flex-direction: column; gap: 4px; min-width: 170px; box-shadow: 0 4px 12px rgba(0,0,0,.12); }
</style>
