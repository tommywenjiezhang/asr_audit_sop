<script lang="ts">
  import { onMount } from "svelte";
  import { api, errMsg, type AssignOutcome, type AssignPreview, type Staff } from "../lib/api";

  let { ticketIds, onclose, ondone }: { ticketIds: number[]; onclose: () => void; ondone: () => void } = $props();

  let staff = $state<Staff[]>([]);
  let staffId = $state<number | null>(null);
  let note = $state("");
  let preview = $state<AssignPreview[]>([]);
  let err = $state("");
  let running = $state(false);
  let results = $state<AssignOutcome[] | null>(null);

  onMount(async () => {
    try { staff = await api.staffList(false); } catch (e) { err = errMsg(e); }
  });

  $effect(() => {
    const id = staffId;
    preview = [];
    if (id == null) return;
    api.assignPreview(ticketIds, id).then((p) => { if (staffId === id) preview = p; }).catch((e) => (err = errMsg(e)));
  });

  const target = $derived(staff.find((s) => s.id === staffId));
  const blocked = $derived(preview.filter((p) => p.blocked));
  const runnable = $derived(preview.filter((p) => !p.blocked));
  const emails = $derived(runnable.reduce((n, p) => n + p.emails - p.already_in_folder, 0));
  const notifies = $derived(runnable.filter((p) => p.will_notify).length);

  async function run() {
    if (staffId == null || !runnable.length) return;
    running = true;
    err = "";
    try {
      results = await api.assign(runnable.map((p) => p.ticket_id), staffId, note);
      ondone();
    } catch (e) {
      err = errMsg(e);
    } finally {
      running = false;
    }
  }

  function badge(o: AssignOutcome): [string, string] {
    if (o.result === "assigned") {
      if (o.notify_state === "submitted") return ["ok", "Assigned · notification submitted"];
      if (o.notify_state === "skipped") return ["ok", "Moved · no notification (same person)"];
      if (o.notify_state === "uncertain") return ["warn", "Assigned · notification uncertain"];
      return ["err", "Assigned — notification failed"];
    }
    if (o.result === "no_change") return ["grey", "No change"];
    if (o.result === "moves_failed") return ["err", "Not finished — retry"];
    return ["err", o.result === "busy" ? "Busy" : "Not assigned"];
  }
</script>

<div class="modal-back">
  <div class="modal stack" role="dialog" aria-label="Assign tickets">
    <h3>{ticketIds.length > 1 ? `Assign ${ticketIds.length} tickets` : "Assign / reassign ticket"}</h3>
    {#if !results}
      <div class="field"><div class="lbl">Assign to</div>
        <select bind:value={staffId} disabled={running}>
          <option value={null}>Choose a staff member…</option>
          {#each staff as s (s.id)}<option value={s.id}>{s.display_name}</option>{/each}
        </select>
      </div>
      {#if !staff.length}<div class="notice warn small">No staff members yet. Add them in Settings › Staff Assignment.</div>{/if}
      {#if target}
        <div class="panel pad small confirm">
          <div><b>Staff member:</b> {target.display_name}</div>
          <div><b>Notification email:</b> {target.email}</div>
          <div><b>Destination folder:</b> {target.mailbox_name ? `${target.mailbox_name} › ` : ""}{target.folder_path}</div>
          <div><b>Tickets:</b> {runnable.length}{#if blocked.length} ({blocked.length} cannot be assigned){/if} · <b>emails to move:</b> {emails}</div>
          <div><b>Notifications:</b> {notifies ? `${notifies} email(s) to ${target.email} after all moves succeed` : "none (no ownership change)"}</div>
        </div>
        {#each blocked as b}<div class="notice warn small">{b.display_id}: {b.blocked}</div>{/each}
        <div class="field"><div class="lbl">Handoff note (optional)</div>
          <textarea rows="3" bind:value={note} placeholder="Instructions for the new owner" disabled={running}></textarea></div>
        <div class="small muted">The source emails are moved into the folder above (not flagged, not marked read). The handoff email goes only to {target.display_name}; the school/sender is not notified.</div>
      {/if}
      {#if err}<div class="notice err small">{err}</div>{/if}
      <div class="row">
        <span class="grow"></span>
        <button onclick={onclose} disabled={running}>Cancel</button>
        <button class="primary" disabled={running || !target || !runnable.length} onclick={run}>{running ? "Assigning…" : `Assign ${runnable.length || ""} ticket${runnable.length === 1 ? "" : "s"}`}</button>
      </div>
    {:else}
      <table class="grid small">
        <thead><tr><th>Ticket</th><th>Result</th><th>Details</th></tr></thead>
        <tbody>
          {#each results as o (o.ticket_id)}
            {@const [cls, label] = badge(o)}
            <tr>
              <td>{o.display_id}</td>
              <td><span class="badge {cls}">{label}</span></td>
              <td>{o.message}{#if o.notify_error}<div class="muted">{o.notify_error}</div>{/if}
                {#each o.failures as f}<div style="color:var(--err)">{f.subject}: {f.error}</div>{/each}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="small muted">Failed moves and notifications can be retried from each ticket. Finished moves are never repeated.</div>
      <div class="row"><span class="grow"></span><button class="primary" onclick={onclose}>Close</button></div>
    {/if}
  </div>
</div>

<style>
  .pad { padding: 10px; }
  .confirm div { margin: 2px 0; }
</style>
