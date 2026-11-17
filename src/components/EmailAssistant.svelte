<script lang="ts">
  import { api, errMsg, fmtDate, type DraftCheck, type DraftVersion, type Proposal, type TemplateRow, type Draft } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";
  import HtmlEditor from "./HtmlEditor.svelte";

  let { ticketId, fieldsVersion, onchanged }: { ticketId: number; fieldsVersion: number; onchanged: () => void } = $props();

  let templates = $state<TemplateRow[]>([]);
  let issueType = $state("");
  let templateId = $state<string>("");
  let subject = $state("");
  let body = $state("");
  let saved = $state<Draft | null>(null);
  let dirty = $state(false);
  let userEdited = $state(false);
  let check = $state<DraftCheck | null>(null);
  let reason = $state("");
  let busy = $state(false);
  let ticketChanged = $state(false);
  let versions = $state<DraftVersion[] | null>(null);
  let confirmOutlook = $state<DraftCheck | null>(null);
  let internalSteps = $state<string | null>(null);

  async function init() {
    busy = true;
    versions = null;
    ticketChanged = false;
    try {
      const det = await api.ticket(ticketId);
      issueType = det.ticket.issue_type;
      templates = await api.templates(issueType);
      internalSteps = app.catalog?.issue_types.find((i) => i.id === issueType)?.internal_steps ?? null;
      const st = await api.draft(ticketId);
      if (st.draft) {
        saved = st.draft;
        templateId = st.draft.template_id ?? "";
        subject = st.draft.subject;
        body = st.draft.body_html;
        userEdited = st.draft.is_user_edited;
        check = st.check;
        reason = `Saved draft v${st.draft.version} (${fmtDate(st.draft.updated_at)})`;
        dirty = false;
      } else {
        saved = null;
        apply(await api.generate(ticketId), false);
        reason = "Not saved yet — " + reason;
      }
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      busy = false;
    }
  }

  let lastTicket = 0;
  $effect(() => {
    if (ticketId !== lastTicket) { lastTicket = ticketId; init(); }
  });

  // ticket data or issue type changed in the center panel
  let lastFv = -1;
  $effect(() => {
    const fv = fieldsVersion;
    if (lastFv === -1) { lastFv = fv; return; }
    if (fv === lastFv) return;
    lastFv = fv;
    (async () => {
      try {
        const det = await api.ticket(ticketId);
        if (det.ticket.issue_type !== issueType) {
          issueType = det.ticket.issue_type;
          templates = await api.templates(issueType);
          internalSteps = app.catalog?.issue_types.find((i) => i.id === issueType)?.internal_steps ?? null;
          if (!userEdited && !saved) { apply(await api.generate(ticketId), false); return; }
          toast("Issue type changed. Use Generate Email to switch to the matching template.", "info");
        }
        ticketChanged = true;
        check = await api.draftCheck(ticketId, templateId || null, subject, body);
      } catch { /* ignore */ }
    })();
  });

  function apply(p: Proposal, markDirty = true) {
    templateId = p.template_id || templateId;
    subject = p.subject;
    body = p.body_html;
    check = p.check;
    reason = p.reason;
    dirty = markDirty;
    userEdited = false;
    ticketChanged = false;
  }

  async function protectEdits(action: string): Promise<boolean> {
    if (!(userEdited && dirty)) return true;
    if (!confirm(`${action} will replace your edited text in the editor.\n\nYour current text will be saved as a previous version first. Continue?`)) return false;
    await api.saveDraft(ticketId, templateId || null, subject, body, true, `Auto-saved before: ${action}`);
    return true;
  }

  async function run(fn: () => Promise<void>) {
    if (busy) return;
    busy = true;
    try { await fn(); } catch (e) { toast(errMsg(e), "error"); } finally { busy = false; }
  }

  const loadDefault = () => run(async () => { if (await protectEdits("Load Default Template")) apply(await api.loadDefault(ticketId, templateId || null)); });
  const resetDefault = () => run(async () => { if (await protectEdits("Reset to Default")) apply(await api.resetDefault(ticketId, templateId || null)); });
  const generate = () => run(async () => { if (await protectEdits("Generate Email")) apply(await api.generate(ticketId)); });
  const fill = () => run(async () => {
    const p = await api.fill(ticketId, templateId || null, subject, body);
    const keepEdited = userEdited;
    apply(p);
    userEdited = keepEdited; // filling tokens does not discard edits
  });
  const chooseTemplate = (id: string) => run(async () => {
    if (!(await protectEdits("Switching template"))) return;
    templateId = id;
    apply(await api.loadDefault(ticketId, id));
  });

  const save = () => run(async () => {
    saved = await api.saveDraft(ticketId, templateId || null, subject, body, userEdited, userEdited ? "Edited by staff" : reason || "Saved");
    dirty = false;
    reason = `Saved draft v${saved.version}`;
    check = await api.draftCheck(ticketId, templateId || null, subject, body);
    versions = null;
    toast("Draft saved.", "ok");
    onchanged();
  });

  async function createOutlook(confirmIncomplete: boolean) {
    await run(async () => {
      if (dirty || !saved) {
        saved = await api.saveDraft(ticketId, templateId || null, subject, body, userEdited, "Saved before creating Outlook draft");
        dirty = false;
      }
      const r = await api.createOutlookDraft(ticketId, confirmIncomplete);
      if (r.needs_confirmation) { confirmOutlook = r.check; return; }
      confirmOutlook = null;
      toast(`Outlook ${r.mode === "reply" ? "reply" : "email"} draft created in your Drafts folder. It has NOT been sent.`, "ok");
      saved = (await api.draft(ticketId)).draft;
      onchanged();
    });
  }

  async function showVersions() {
    versions = versions ? null : await api.draftVersions(ticketId);
  }

  function restoreVersion(v: DraftVersion) {
    if (dirty && !confirm("Replace the editor text with this version? (Unsaved changes will be lost unless you save first.)")) return;
    templateId = v.template_id ?? templateId;
    subject = v.subject;
    body = v.body_html;
    dirty = true;
    userEdited = true;
    reason = `Restored version ${v.version} — click Save Draft to keep it`;
    versions = null;
  }

  const tpl = $derived(templates.find((t) => t.id === templateId));
  const sopBadge = $derived(check?.sop_status === "review_required" ? "warn" : check?.sop_status === "adapted" ? "" : "ok");
</script>

<div class="stack">
  <div class="row">
    <h3 style="margin:0">Email Assistant</h3>
    <span class="grow"></span>
    {#if saved?.outlook_entry_id}<span class="badge ok" title={fmtDate(saved.outlook_created_at)}>Outlook draft created</span>{/if}
    {#if dirty}<span class="badge warn">Unsaved</span>{:else if saved}<span class="badge grey">Saved v{saved.version}</span>{/if}
  </div>

  <div class="row">
    <select class="grow" value={templateId} onchange={(e) => chooseTemplate((e.target as HTMLSelectElement).value)} disabled={busy}>
      {#each templates as t}
        <option value={t.id} disabled={!t.enabled}>{t.name}{t.sop_status === "review_required" ? " (Review Required)" : ""}{t.enabled ? "" : " (disabled)"}</option>
      {/each}
    </select>
  </div>

  {#if check?.sop_reference}
    <div class="small"><span class="badge {sopBadge}">{check.sop_status === "review_required" ? "Review Required" : check.sop_status === "adapted" ? "SOP-adapted" : "SOP wording"}</span> <span class="muted">{check.sop_reference}</span></div>
  {/if}
  {#if tpl?.description}<div class="small muted">{tpl.description}</div>{/if}
  {#if internalSteps}
    <details class="small"><summary>Staff verification steps (internal, not sent)</summary><div class="notice" style="margin-top:4px">{internalSteps}</div></details>
  {/if}

  <div class="row wrap actions">
    <button onclick={loadDefault} disabled={busy} title="Load the original SOP-based template for the selected variant">Load Default</button>
    <button onclick={fill} disabled={busy} title="Replace remaining {'{{placeholders}}'} with ticket values; your edits are kept">Fill from Ticket</button>
    <button onclick={generate} disabled={busy} title="Pick the template variant for the current ticket state and fill it">Generate Email</button>
    <button onclick={resetDefault} disabled={busy} title="Restore the system default text (ticket data is not changed)">Reset to Default</button>
  </div>

  {#if reason}<div class="small muted">{reason}</div>{/if}
  {#if ticketChanged}<div class="notice small">Ticket data changed. Click <b>Fill from Ticket</b> to update remaining placeholders (your edits are kept).</div>{/if}

  {#if check && check.unverified_claims.length}
    <div class="notice err small"><b>Not verified:</b> this template states facts the ticket has not confirmed — {check.unverified_claims.join("; ")}. Verify the fields or choose another template.</div>
  {/if}
  {#if check && check.unresolved.length}
    <div class="notice warn small"><b>Missing information ({check.unresolved.length}):</b>
      {#each check.unresolved as u}<mark class="ph">{u.label}</mark> {/each}
      <div class="muted">Fill these in the ticket form or type over them in the email.</div>
    </div>
  {:else if check}
    <div class="small" style="color:var(--ok)">✓ All placeholders filled</div>
  {/if}

  <label class="small muted">Subject
    <input value={subject} oninput={(e) => { subject = (e.target as HTMLInputElement).value; dirty = true; userEdited = true; }} />
  </label>
  <HtmlEditor value={body} onchange={(h) => { body = h; dirty = true; userEdited = true; }} />

  <div class="row wrap">
    <button class="primary" onclick={save} disabled={busy}>Save Draft</button>
    <button onclick={() => createOutlook(false)} disabled={busy} title="Creates a reply draft in Outlook. Nothing is sent.">Create Outlook Draft</button>
    <span class="grow"></span>
    <button class="link small" onclick={showVersions}>{versions ? "Hide" : "Version history"}</button>
  </div>
  <div class="small muted">Emails are never sent automatically. Creating an Outlook draft does not resolve the ticket.</div>

  {#if versions}
    <div class="panel" style="padding:8px">
      {#each versions as v}
        <div class="row small vrow">
          <span>v{v.version}</span><span class="muted">{fmtDate(v.created_at)}</span><span class="grow muted">{v.reason}</span>
          <button class="small" onclick={() => restoreVersion(v)}>Restore</button>
        </div>
      {:else}
        <div class="small muted">No previous versions.</div>
      {/each}
    </div>
  {/if}
</div>

{#if confirmOutlook}
  <div class="modal-back">
    <div class="modal stack">
      <h3>Create an incomplete Outlook draft?</h3>
      {#if confirmOutlook.unresolved.length}
        <div>These placeholders are still unfilled and will be highlighted in yellow in the Outlook draft:</div>
        <div>{#each confirmOutlook.unresolved as u}<mark class="ph">{u.label}</mark> {/each}</div>
      {/if}
      {#if confirmOutlook.unverified_claims.length}
        <div class="notice err small">The text confirms facts not verified on the ticket: {confirmOutlook.unverified_claims.join("; ")}</div>
      {/if}
      <div class="small muted">The subject will start with “[INCOMPLETE DRAFT]”. The draft is saved to Outlook only — it will not be sent.</div>
      <div class="row">
        <span class="grow"></span>
        <button onclick={() => (confirmOutlook = null)}>Cancel</button>
        <button class="primary" onclick={() => createOutlook(true)}>Create incomplete draft</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .actions button { flex: 1; }
  label { display: flex; flex-direction: column; gap: 3px; }
  .vrow { padding: 3px 0; border-bottom: 1px solid var(--border); }
</style>
