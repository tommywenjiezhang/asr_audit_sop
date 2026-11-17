<script lang="ts">
  import { api, errMsg } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";

  let { oncreated, onclose }: { oncreated: (id: number) => void; onclose: () => void } = $props();

  let f = $state({ sender_name: "", sender_email: "", subject: "", body: "", issue_type: "", school_njiis_id: "", school_name: "" });
  let busy = $state(false);

  async function create() {
    if (!f.sender_email.trim() && !f.sender_name.trim() && !f.subject.trim()) {
      toast("Enter at least a sender or a subject.", "error");
      return;
    }
    busy = true;
    try {
      oncreated(await api.createTicket(f));
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      busy = false;
    }
  }
</script>

<div class="modal-back">
  <div class="modal stack">
    <h3>New Ticket</h3>
    <div class="small muted">For phone calls or emails outside the monitored folder. Pasted text is analyzed the same way as Outlook emails; everything can be corrected later.</div>
    <div class="field"><div class="lbl">Sender name</div><input bind:value={f.sender_name} /></div>
    <div class="field"><div class="lbl">Sender email</div><input type="email" bind:value={f.sender_email} /></div>
    <div class="field"><div class="lbl">Subject</div><input bind:value={f.subject} /></div>
    <div class="field"><div class="lbl">School NJIIS ID</div><input bind:value={f.school_njiis_id} placeholder="optional" /></div>
    <div class="field"><div class="lbl">School name</div><input bind:value={f.school_name} placeholder="optional" /></div>
    <div class="field"><div class="lbl">Issue category</div>
      <select bind:value={f.issue_type}>
        <option value="">Detect from text</option>
        {#each app.catalog?.issue_types ?? [] as it}<option value={it.id}>{it.name}</option>{/each}
      </select>
    </div>
    <div class="field"><div class="lbl">Message / notes</div><textarea rows="6" bind:value={f.body} placeholder="Paste the email text or describe the request"></textarea></div>
    <div class="row">
      <span class="grow"></span>
      <button onclick={onclose}>Cancel</button>
      <button class="primary" disabled={busy} onclick={create}>Create Ticket</button>
    </div>
  </div>
</div>
