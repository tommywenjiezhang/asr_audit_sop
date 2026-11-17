<script lang="ts">
  import { api, condMatches, errMsg, fmtDate, type TicketDetail } from "../lib/api";
  import { app, issueName, toast } from "../lib/stores.svelte";
  import FieldInput from "./FieldInput.svelte";

  let { id, onchanged }: { id: number; onchanged: () => void } = $props();

  let d = $state<TicketDetail | null>(null);
  let err = $state("");
  let tab = $state<"email" | "history">("email");
  let openMsg = $state<number | null>(null);

  async function load() {
    try {
      d = await api.ticket(id);
      err = "";
      if (openMsg === null && d.messages.length) openMsg = d.messages[0].id;
    } catch (e) {
      err = errMsg(e);
    }
  }
  $effect(() => { void id; openMsg = null; d = null; load(); });

  const values = $derived(Object.fromEntries((d?.fields ?? []).map((f) => [f.key, f.effective])) as Record<string, string>);
  const issue = $derived(app.catalog?.issue_types.find((i) => i.id === d?.ticket.issue_type));
  const issueFields = $derived((issue?.fields ?? []).filter((f) => !app.catalog?.common_fields.some((c) => c.key === f.key)));
  const visibleIssueFields = $derived(issueFields.filter((f) => condMatches(f.show_if, values)));
  const required = $derived([...(app.catalog?.common_fields ?? []), ...visibleIssueFields].filter((f) => f.required));
  const filledRequired = $derived(required.filter((f) => (values[f.key] ?? "").trim() && values[f.key] !== "[]").length);
  const allFieldsShown = $derived(visibleIssueFields.length + (app.catalog?.common_fields.length ?? 0));
  const filledAll = $derived([...(app.catalog?.common_fields ?? []), ...visibleIssueFields].filter((f) => (values[f.key] ?? "").trim() && values[f.key] !== "[]").length);
  const extraKeys = $derived((d?.fields ?? []).filter((f) => ["additional_ids", "sender_reported_submission"].includes(f.key) && f.effective));

  async function saveField(key: string, value: string | null, verified: boolean) {
    try {
      await api.setField(id, key, value, verified);
      await load();
      onchanged();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }

  async function patch(p: Record<string, unknown>) {
    try {
      await api.updateTicket(id, p);
      await load();
      onchanged();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }

  let notes = $state("");
  let resolution = $state("");
  $effect(() => { notes = d?.ticket.internal_notes ?? ""; resolution = d?.ticket.resolution_notes ?? ""; });

  async function refreshBody(mid: number) {
    try {
      await api.refreshMessage(mid);
      await load();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
</script>

{#if err}
  <div class="notice err">{err}</div>
{:else if !d}
  <div class="muted">Loading…</div>
{:else}
  {@const t = d.ticket}
  <div class="stack">
    <div class="panel pad">
      <div class="row wrap">
        <h2 style="margin:0">{t.display_id}</h2>
        <span class="muted">{t.original_subject ?? ""}</span>
        <span class="grow"></span>
        <span class="small muted">Created {fmtDate(t.created_at)} · Updated {fmtDate(t.updated_at)}</span>
      </div>
      {#if t.possible_duplicate_of}
        <div class="notice warn small" style="margin-top:8px">
          Possible duplicate of T-{String(t.possible_duplicate_of).padStart(6, "0")} (same sender or school and issue). Not merged automatically.
          <button class="link" onclick={() => patch({ clear_duplicate_flag: true })}>Not a duplicate</button>
        </div>
      {/if}
      <div class="grid4">
        <label>Status
          <select value={t.status} onchange={(e) => patch({ status: (e.target as HTMLSelectElement).value })}>
            {#each app.catalog?.statuses ?? [] as s}<option>{s}</option>{/each}
          </select>
        </label>
        <label>Priority
          <select value={t.priority} onchange={(e) => patch({ priority: (e.target as HTMLSelectElement).value })}>
            {#each app.catalog?.priorities ?? [] as p}<option>{p}</option>{/each}
          </select>
        </label>
        <label>Issue Category
          <select value={t.issue_type} onchange={(e) => patch({ issue_type: (e.target as HTMLSelectElement).value })}>
            {#each app.catalog?.issue_types ?? [] as it}<option value={it.id}>{it.name}</option>{/each}
          </select>
          <span class="small muted">{t.issue_type_source === "manual" ? "set by staff" : `auto-classified${t.classification_confidence ? ` (${Math.round(t.classification_confidence * 100)}%)` : ""} — please confirm`}</span>
        </label>
        <label>Assigned Staff
          <input value={t.assigned_staff ?? ""} onchange={(e) => patch({ assigned_staff: (e.target as HTMLInputElement).value })} />
        </label>
        <label>Next Follow-up
          <input type="date" value={t.next_followup_date ?? ""} onchange={(e) => patch({ next_followup_date: (e.target as HTMLInputElement).value })} />
        </label>
        <label>Sender
          <input value={t.sender_name ?? ""} onchange={(e) => patch({ sender_name: (e.target as HTMLInputElement).value })} />
        </label>
        <label>Sender Email
          <input value={t.sender_email ?? ""} onchange={(e) => patch({ sender_email: (e.target as HTMLInputElement).value })} />
        </label>
        <div class="lblbox">School Status
          <div style="padding-top:4px"><span class="badge {t.school_verification === 'Verified' ? 'ok' : 'warn'}">{t.school_verification}</span></div>
        </div>
      </div>
    </div>

    <div class="panel pad">
      <div class="row">
        <button class="tab" class:on={tab === "email"} onclick={() => (tab = "email")}>Original Email ({d.messages.length})</button>
        <button class="tab" class:on={tab === "history"} onclick={() => (tab = "history")}>History ({d.history.length})</button>
      </div>
      {#if tab === "email"}
        {#each d.messages as m (m.id)}
          <div class="msg">
            <button class="msghead" onclick={() => (openMsg = openMsg === m.id ? null : m.id)}>
              <b>{m.sender_name ?? m.sender_email}</b>
              <span class="muted small">&lt;{m.sender_email}&gt;</span>
              {#if m.link_type !== "original"}<span class="badge grey">{m.link_type}</span>{/if}
              {#if m.has_attachments}<span class="badge grey">📎</span>{/if}
              <span class="grow"></span>
              <span class="muted small">{fmtDate(m.received_at)} · {m.folder_path}</span>
            </button>
            {#if openMsg === m.id}
              <div class="small muted">Subject: {m.subject}</div>
              <pre class="body">{m.body_text ?? ""}</pre>
              {#if m.body_truncated}<button class="small" onclick={() => refreshBody(m.id)}>Load full email from Outlook</button>{/if}
            {/if}
          </div>
        {:else}
          <div class="muted small" style="padding:8px 0">Manually created ticket — no Outlook email linked.</div>
        {/each}
      {:else}
        <table class="grid small">
          <thead><tr><th>When</th><th>Who</th><th>Action</th><th>Change</th></tr></thead>
          <tbody>
            {#each d.history as h (h.id)}
              <tr>
                <td style="white-space:nowrap">{fmtDate(h.at)}</td><td>{h.actor}</td><td>{h.action}{h.field ? ` (${h.field})` : ""}</td>
                <td>{#if h.old_value || h.new_value}<span class="muted">{h.old_value || "—"}</span> → {h.new_value || "—"}{/if}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>

    <div class="panel pad">
      <div class="row">
        <h3 style="margin:0">School &amp; Contact</h3>
        <span class="grow"></span>
        <span class="small muted">Completed {filledAll}/{allFieldsShown} fields{#if required.length} · required {filledRequired}/{required.length}{/if}</span>
      </div>
      <div class="progress"><div style="width:{allFieldsShown ? (100 * filledAll) / allFieldsShown : 0}%"></div></div>
      {#each app.catalog?.common_fields ?? [] as f (f.key)}
        <FieldInput def={f} field={d.fields.find((x) => x.key === f.key)} onsave={(v, ver) => saveField(f.key, v, ver)} />
      {/each}
      {#each extraKeys as f}
        <div class="field"><div class="lbl">{f.key === "additional_ids" ? "Other IDs in email" : "Sender reports"}</div><div class="small">{f.effective} <span class="badge warn">Not Verified</span></div></div>
      {/each}
    </div>

    <div class="panel pad">
      <h3>{issueName(t.issue_type)}</h3>
      {#if issue}
        <div class="small muted" style="margin-bottom:8px">{issue.description} · SOP: {issue.sop_section}{issue.sop_page ? ` (p. ${issue.sop_page})` : ""}</div>
      {/if}
      {#each visibleIssueFields as f (f.key)}
        <FieldInput def={f} field={d.fields.find((x) => x.key === f.key)} onsave={(v, ver) => saveField(f.key, v, ver)} />
      {:else}
        <div class="small muted">No additional fields for this issue type.</div>
      {/each}
    </div>

    <div class="panel pad">
      <h3>Notes</h3>
      <div class="field"><div class="lbl">Internal Notes</div><textarea rows="3" bind:value={notes} onblur={() => notes !== (t.internal_notes ?? "") && patch({ internal_notes: notes })}></textarea></div>
      <div class="field"><div class="lbl">Resolution Notes</div><textarea rows="2" bind:value={resolution} onblur={() => resolution !== (t.resolution_notes ?? "") && patch({ resolution_notes: resolution })}></textarea></div>
    </div>
  </div>
{/if}

<style>
  .pad { padding: 12px; }
  .grid4 { display: grid; grid-template-columns: repeat(auto-fill, minmax(190px, 1fr)); gap: 8px 12px; margin-top: 10px; }
  .grid4 label, .grid4 .lblbox { display: flex; flex-direction: column; gap: 3px; color: var(--muted); font-size: 12px; }
  .tab { border: none; border-bottom: 2px solid transparent; border-radius: 0; background: none; font-weight: 600; color: var(--muted); }
  .tab.on { color: var(--accent); border-bottom-color: var(--accent); }
  .msg { border-top: 1px solid var(--border); padding: 6px 0; }
  .msghead { display: flex; gap: 8px; align-items: center; width: 100%; border: none; background: none; padding: 0; height: 26px; text-align: left; }
  .body { white-space: pre-wrap; word-break: break-word; font-family: inherit; background: #f8fafc; border: 1px solid var(--border); border-radius: 6px; padding: 8px; max-height: 360px; overflow: auto; margin: 6px 0; }
  .progress { height: 4px; background: #e8edf3; border-radius: 2px; margin: 6px 0 10px; overflow: hidden; }
  .progress div { height: 100%; background: var(--ok); }
</style>
