<script lang="ts">
  import { api, errMsg, type MergePreview, type MergeResult } from "../lib/api";

  /** Combine `sources` into `target`. Nothing changes until "Combine" is pressed. */
  let { target, sources: initial = [], onclose, ondone }: { target: number; sources?: number[]; onclose: () => void; ondone: () => void } = $props();

  let sources = $state<number[]>([...initial]);
  let add = $state("");
  let preview = $state<MergePreview | null>(null);
  let err = $state("");
  let running = $state(false);
  let results = $state<MergeResult[] | null>(null);

  async function refresh() {
    err = "";
    try {
      preview = await api.mergePreview(sources, target);
    } catch (e) {
      err = errMsg(e);
      preview = null;
    }
  }
  $effect(() => {
    void sources.length;
    refresh();
  });

  function parseId(v: string): number | null {
    const d = v.trim().replace(/^t-?/i, "");
    return /^\d+$/.test(d) ? Number(d) : null;
  }
  async function addCase() {
    const id = parseId(add);
    if (id == null) {
      err = "Enter a ticket number such as T-000123 (references can be used in Add to existing case).";
      return;
    }
    if (id !== target && !sources.includes(id)) sources = [...sources, id];
    add = "";
  }

  async function run() {
    if (!preview?.can_run) return;
    if (!confirm(`Combine ${preview.sources.length} case(s) into ${preview.target_display}? Emails, notes, schools and history are kept; the combined cases' references keep working.`)) return;
    running = true;
    try {
      results = await api.mergeRun(sources, target);
      ondone();
    } catch (e) {
      err = errMsg(e);
    } finally {
      running = false;
    }
  }
</script>

<div class="modal-back">
  <div class="modal stack wide" role="dialog" aria-label="Combine cases">
    <h3>Combine cases into {preview?.target_display ?? `T-${String(target).padStart(6, "0")}`}</h3>
    {#if !results}
      <div class="small muted">Use this when several tickets are really one support case. The remaining case keeps every email, note, school, issue type, owner period and history entry; the combined tickets stay as aliases (their references keep working). Nothing is moved or sent in Outlook.</div>
      <div class="row">
        <input style="width:200px" bind:value={add} placeholder="T-000123" onkeydown={(e) => e.key === "Enter" && addCase()} />
        <button class="small" disabled={!add.trim()} onclick={addCase}>Add case</button>
      </div>
      {#if preview}
        <table class="grid small">
          <thead><tr><th>Case</th><th>Status / owner</th><th>Emails</th><th>Notes</th><th>Schools</th><th>Issues</th><th></th></tr></thead>
          <tbody>
            <tr class="target"><td><b>{preview.target_display}</b> (remains)<div class="muted">{preview.target_ref ?? ""}</div></td><td>{preview.target_status}{preview.target_owner ? ` — ${preview.target_owner}` : ""}</td><td colspan="5"></td></tr>
            {#each preview.sources as s (s.ticket_id)}
              <tr>
                <td>{s.display_id}<div class="muted">{s.ticket_ref ?? ""}</div></td>
                <td>{s.status}{s.owner ? ` — ${s.owner}` : ""}</td>
                <td>{s.emails}</td><td>{s.notes}</td>
                <td>{s.schools.join("; ") || "—"}</td>
                <td>{s.issues.join("; ")}</td>
                <td><button class="link small" onclick={() => (sources = sources.filter((x) => x !== s.ticket_id))}>Remove</button></td>
              </tr>
              {#if s.blocked}<tr><td colspan="7"><span class="badge err">Cannot combine</span> {s.blocked}</td></tr>{/if}
              {#each s.warnings as w}<tr><td colspan="7" class="muted">⚠ {w}</td></tr>{/each}
            {/each}
          </tbody>
        </table>
        {#if preview.sources.length}
          <div class="panel pad small">
            <b>After combining:</b> {preview.emails_after} email(s), {preview.notes_after} note(s), status <b>{preview.status_after}</b><br />
            <b>Schools:</b> {preview.schools_after.join("; ") || "—"}<br />
            <b>Issue types:</b> {preview.issues_after.join("; ")}
          </div>
        {:else}
          <div class="notice small">Add the tickets that belong to this case.</div>
        {/if}
      {/if}
      {#if err}<div class="notice err small">{err}</div>{/if}
      <div class="row">
        <span class="grow"></span>
        <button onclick={onclose} disabled={running}>Cancel</button>
        <button class="primary" disabled={running || !preview?.can_run} onclick={run}>{running ? "Combining…" : "Combine"}</button>
      </div>
    {:else}
      <table class="grid small">
        <thead><tr><th>Case</th><th>Result</th></tr></thead>
        <tbody>{#each results as r (r.ticket_id)}<tr><td>{r.display_id}</td><td><span class="badge {r.ok ? 'ok' : 'err'}">{r.ok ? "Combined" : "Not combined"}</span> {r.message}</td></tr>{/each}</tbody>
      </table>
      <div class="small muted">The shared database re-checks every combination when it synchronizes; a refused one appears as a conflict on the case.</div>
      <div class="row"><span class="grow"></span><button class="primary" onclick={onclose}>Close</button></div>
    {/if}
  </div>
</div>

<style>
  .wide { width: min(900px, 94vw); }
  .pad { padding: 8px 10px; }
  tr.target td { background: var(--accent-soft); }
</style>
