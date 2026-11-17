<script lang="ts">
  import { onMount } from "svelte";
  import { api, errMsg, fmtDate, highlightTokens, type Condition, type DraftCheck, type TemplateRow, type TemplateVersion } from "../lib/api";
  import { app, issueName, toast } from "../lib/stores.svelte";
  import HtmlEditor from "../components/HtmlEditor.svelte";

  let view = $state<"templates" | "issues">("templates");
  let all = $state<TemplateRow[]>([]);
  let filter = $state("");
  let sel = $state<TemplateRow | null>(null);
  let subject = $state("");
  let body = $state("");
  let name = $state("");
  let dirty = $state(false);
  let previewMode = $state<"edit" | "sample" | "ticket">("edit");
  let previewTicket = $state("");
  let preview = $state<{ subject: string; body_html: string; check: DraftCheck } | null>(null);
  let versions = $state<TemplateVersion[] | null>(null);

  async function loadAll(keep?: string) {
    all = await api.templates();
    const id = keep ?? sel?.id;
    if (id) select(all.find((t) => t.id === id) ?? null, true);
  }
  onMount(() => { loadAll().catch((e) => toast(errMsg(e), "error")); });

  function select(t: TemplateRow | null, force = false) {
    if (!force && dirty && !confirm("Discard unsaved template changes?")) return;
    sel = t;
    subject = t?.subject_template ?? "";
    body = t?.body_html ?? "";
    name = t?.name ?? "";
    dirty = false;
    versions = null;
    previewMode = "edit";
    preview = null;
  }

  const grouped = $derived.by(() => {
    const q = filter.toLowerCase();
    const list = all.filter((t) => !q || t.name.toLowerCase().includes(q) || issueName(t.issue_type).toLowerCase().includes(q) || t.body_html.toLowerCase().includes(q));
    return (app.catalog?.issue_types ?? []).map((it) => ({ it, ts: list.filter((t) => t.issue_type === it.id) })).filter((g) => g.ts.length);
  });

  async function act(fn: () => Promise<TemplateRow | void>, msg: string) {
    try {
      const r = await fn();
      await loadAll(r ? r.id : undefined);
      toast(msg, "ok");
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }

  const save = () => act(() => api.saveTemplate(sel!.id, subject, body, name !== sel!.name ? name : undefined, "Edited in Template Editor"), "Template saved (previous version kept in history).");
  const dup = () => {
    const n = prompt("Name for the copy", `${sel!.name} (copy)`);
    if (n) act(() => api.duplicateTemplate(sel!.id, n), "Template duplicated. Copies are chosen manually in the Email Assistant.");
  };
  const toggle = () => act(() => api.setTemplateEnabled(sel!.id, !sel!.enabled), sel!.enabled ? "Template disabled." : "Template enabled.");
  const restore = () => { if (confirm("Restore the original SOP-based system text? Your current text is kept in version history.")) act(() => api.restoreTemplate(sel!.id), "System default restored."); };

  async function runPreview(mode: "edit" | "sample" | "ticket") {
    previewMode = mode;
    if (mode === "edit") return;
    try {
      const tid = mode === "ticket" ? Number(previewTicket.replace(/\D/g, "")) : null;
      if (mode === "ticket" && !tid) { toast("Enter a ticket number to preview with.", "error"); previewMode = "edit"; return; }
      preview = await api.preview(sel?.id ?? null, subject, body, tid);
    } catch (e) {
      toast(errMsg(e), "error");
      previewMode = "edit";
    }
  }

  function describe(c: Condition): string {
    if (c.op === "never") return "only when chosen manually";
    const label = app.catalog?.issue_types.flatMap((i) => i.fields).concat(app.catalog?.common_fields ?? []).find((f) => f.key === c.field)?.label ?? c.field;
    const ops: Record<string, string> = { eq: "is", ne: "is not", in: "is one of", not_in: "is not one of", empty: "is empty", not_empty: "is filled" };
    const val = c.value ?? (c.values ?? []).map((v) => v || "(blank)").join(" / ");
    return `${label} ${ops[c.op] ?? c.op} ${val}`.trim();
  }
  function statusBadge(s: string) { return s === "review_required" ? ["warn", "Review Required"] : s === "adapted" ? ["", "SOP-adapted"] : ["ok", "SOP wording"]; }
</script>

<div class="page">
  <aside class="tlist">
    <div class="row" style="padding:10px 10px 0">
      <button class="tab" class:on={view === "templates"} onclick={() => (view = "templates")}>Default Templates</button>
      <button class="tab" class:on={view === "issues"} onclick={() => (view = "issues")}>Issue Types</button>
    </div>
    {#if view === "templates"}
      <div style="padding:8px 10px"><input placeholder="Search templates…" bind:value={filter} /></div>
      <div class="scroll grow">
        {#each grouped as g}
          <div class="ghead">{g.it.name}</div>
          {#each g.ts as t (t.id)}
            <button class="titem" class:sel={sel?.id === t.id} class:off={!t.enabled} onclick={() => select(t)}>
              {t.name}
              {#if t.id === g.it.default_template}<span class="badge">default</span>{/if}
              {#if t.sop_status === "review_required"}<span class="badge warn">review</span>{/if}
              {#if t.is_customized}<span class="badge grey">edited</span>{/if}
              {#if !t.is_system}<span class="badge grey">custom</span>{/if}
              {#if t.system_update_available}<span class="badge err" title="A newer system default exists">update</span>{/if}
            </button>
          {/each}
        {/each}
      </div>
    {/if}
  </aside>

  <section class="tmain scroll">
    {#if view === "issues"}
      <h2>Issue Types ({app.catalog?.issue_types.length})</h2>
      <p class="muted small">Every issue type ships with SOP-based default templates. Source: {app.catalog?.sop_document}.</p>
      <table class="grid panel">
        <thead><tr><th>Issue type</th><th>SOP section</th><th>Page</th><th>Issue-specific fields</th><th>Templates</th></tr></thead>
        <tbody>
          {#each app.catalog?.issue_types ?? [] as it}
            <tr>
              <td><b>{it.name}</b><div class="small muted">{it.description}</div></td>
              <td class="small">{it.sop_section}</td>
              <td class="small">{it.sop_page}</td>
              <td class="small">{it.fields.map((f) => f.label).join(", ") || "—"}</td>
              <td class="small">{#each all.filter((t) => t.issue_type === it.id) as t}<div><button class="link" onclick={() => { view = "templates"; select(t); }}>{t.name}</button></div>{/each}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else if !sel}
      <div class="muted" style="margin-top:40px;text-align:center">Select a template to view, edit or preview it.</div>
    {:else}
      {@const [bc, bl] = statusBadge(sel.sop_status)}
      <div class="stack">
        <div class="row wrap">
          <input class="title" bind:value={name} oninput={() => (dirty = true)} />
          <span class="badge {bc}">{bl}</span>
          {#if !sel.enabled}<span class="badge grey">Disabled</span>{/if}
          <span class="grow"></span>
          <span class="small muted">v{sel.version} · modified {fmtDate(sel.last_modified)}</span>
        </div>
        <div class="panel meta small">
          <div><b>Issue type:</b> {issueName(sel.issue_type)}</div>
          <div><b>SOP reference:</b> {sel.sop_section || "—"} {sel.sop_page ? `(p. ${sel.sop_page})` : ""}</div>
          <div><b>Description:</b> {sel.description}</div>
          <div><b>Selected automatically when:</b> {sel.rules.length ? sel.rules.map(describe).join(" AND ") : "no other variant matches (fallback)"} · priority {sel.priority}</div>
          {#if sel.asserts.length}<div><b>Requires verified:</b> {sel.asserts.map((a) => a.label ?? describe(a)).join("; ")}</div>{/if}
          <div><b>Required fields:</b> {sel.required_fields.join(", ") || "none"} · <b>Optional:</b> {sel.optional_fields.join(", ") || "none"}</div>
          <div><b>Template ID:</b> <code>{sel.id}</code> · system version {sel.system_version ?? "—"}</div>
          {#if sel.system_update_available}<div class="notice warn">A newer system default is available. Your edits were preserved; use “Restore System Default” to adopt it.</div>{/if}
        </div>

        <div class="row wrap">
          <button class:primary={previewMode === "edit"} onclick={() => runPreview("edit")}>Edit</button>
          <button class:primary={previewMode === "sample"} onclick={() => runPreview("sample")}>Preview: sample data</button>
          <input style="width:120px" placeholder="Ticket # (e.g. 12)" bind:value={previewTicket} />
          <button class:primary={previewMode === "ticket"} onclick={() => runPreview("ticket")}>Preview: ticket</button>
        </div>

        {#if previewMode === "edit"}
          <label class="small muted">Subject template<input bind:value={subject} oninput={() => (dirty = true)} /></label>
          <HtmlEditor value={body} minHeight={340} onchange={(h) => { body = h; dirty = true; }} />
        {:else if preview}
          <div class="panel" style="padding:12px">
            <div class="small muted">Subject: {preview.subject}</div>
            <hr />
            <div class="email-html">{@html highlightTokens(preview.body_html)}</div>
          </div>
          {#if preview.check.unresolved.length}<div class="small notice warn">Unfilled: {preview.check.unresolved.map((u) => u.label).join(", ")}</div>{/if}
          {#if preview.check.unverified_claims.length}<div class="small notice err">With this ticket, the template would state unverified facts: {preview.check.unverified_claims.join("; ")}</div>{/if}
        {/if}

        <div class="row wrap">
          <button class="primary" disabled={!dirty} onclick={save}>Save Template</button>
          <button onclick={dup}>Duplicate</button>
          <button onclick={toggle}>{sel.enabled ? "Disable" : "Enable"}</button>
          {#if sel.is_system}<button onclick={restore}>Restore System Default</button>{/if}
          <span class="grow"></span>
          <button class="link" onclick={async () => (versions = versions ? null : await api.templateVersions(sel!.id))}>{versions ? "Hide history" : "Version history"}</button>
        </div>
        {#if versions}
          <div class="panel" style="padding:8px">
            {#each versions as v}
              <div class="row small" style="border-bottom:1px solid var(--border);padding:3px 0">
                <span>v{v.version}</span><span class="muted">{fmtDate(v.saved_at)}</span><span class="grow muted">{v.note}</span>
                <button class="small" onclick={() => { subject = v.subject_template; body = v.body_html; dirty = true; previewMode = "edit"; }}>Load into editor</button>
              </div>
            {:else}<div class="small muted">No previous versions.</div>{/each}
          </div>
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .page { display: flex; height: 100%; }
  .tlist { width: 320px; flex: none; background: var(--panel); border-right: 1px solid var(--border); display: flex; flex-direction: column; }
  .grow { flex: 1; }
  .ghead { padding: 8px 10px 4px; font-size: 11px; text-transform: uppercase; letter-spacing: .03em; color: var(--muted); font-weight: 600; }
  .titem { display: block; width: 100%; height: auto; text-align: left; border: none; border-radius: 0; padding: 5px 10px 5px 16px; background: none; }
  .titem:hover { background: #f5f8fc; }
  .titem.sel { background: var(--accent-soft); box-shadow: inset 3px 0 0 var(--accent); }
  .titem.off { color: var(--muted); text-decoration: line-through; }
  .tmain { flex: 1; padding: 14px; min-width: 0; }
  .tab { border: none; border-bottom: 2px solid transparent; border-radius: 0; background: none; font-weight: 600; color: var(--muted); }
  .tab.on { color: var(--accent); border-bottom-color: var(--accent); }
  .title { font-size: 15px; font-weight: 600; width: auto; flex: 1; min-width: 200px; }
  .meta { padding: 10px; line-height: 1.6; }
  label { display: flex; flex-direction: column; gap: 3px; }
  hr { border: none; border-top: 1px solid var(--border); }
</style>
