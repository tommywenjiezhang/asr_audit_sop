<script lang="ts">
  import { api, errMsg, fmtDate, type TimelineItem } from "../lib/api";

  /** Complete activity timeline of a case (emails, notes, owners, Done decisions, Outlook runs, history). */
  let { id, version = 0 }: { id: number; version?: number } = $props();

  let items = $state<TimelineItem[]>([]);
  let err = $state("");
  let kinds = $state<Record<string, boolean>>({ email: true, note: true, owner: true, done: true, outlook: true, history: true });

  $effect(() => {
    void id;
    void version;
    api.timeline(id).then((v) => { items = v; err = ""; }).catch((e) => (err = errMsg(e)));
  });
  const ICON: Record<string, string> = { email: "✉", note: "✎", owner: "👤", done: "✔", outlook: "⇄", history: "•" };
  const LABEL: Record<string, string> = { email: "Emails", note: "Notes", owner: "Owners", done: "Done", outlook: "Outlook", history: "Changes" };
  const shown = $derived(items.filter((i) => kinds[i.kind]));
</script>

<div class="stack">
  <div class="row small wrap">
    {#each Object.keys(LABEL) as k}
      <label class="row" style="gap:3px"><input type="checkbox" bind:checked={kinds[k]} />{LABEL[k]}</label>
    {/each}
    <span class="grow"></span><span class="muted">{shown.length} of {items.length}</span>
  </div>
  {#if err}<div class="notice err small">{err}</div>{/if}
  <ol class="tl">
    {#each shown as i, n (n)}
      <li class="k-{i.kind}">
        <span class="ic" aria-hidden="true">{ICON[i.kind] ?? "•"}</span>
        <div>
          <div class="row small"><b>{i.title}</b>{#if i.actor}<span class="muted">· {i.actor}</span>{/if}<span class="grow"></span><span class="muted">{fmtDate(i.at)}</span></div>
          {#if i.detail}<div class="small detail">{i.detail}</div>{/if}
        </div>
      </li>
    {:else}<li class="muted small">Nothing yet.</li>{/each}
  </ol>
</div>

<style>
  .tl { list-style: none; margin: 0; padding: 0; max-height: 520px; overflow: auto; }
  .tl li { display: grid; grid-template-columns: 22px 1fr; gap: 6px; padding: 5px 0; border-bottom: 1px solid var(--border); }
  .ic { text-align: center; color: var(--muted); }
  .k-email .ic { color: var(--accent); } .k-done .ic { color: var(--ok); } .k-outlook .ic { color: var(--warn); }
  .detail { white-space: pre-wrap; word-break: break-word; color: var(--text); }
</style>
