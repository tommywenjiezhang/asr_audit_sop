<script lang="ts">
  // Minimal rich-text editor on contenteditable. Placeholder tokens are shown highlighted
  // (<mark class="ph">); marks are removed from the value passed back to the parent.
  // The backend sanitizes all HTML on save.
  import { highlightTokens, stripMarks } from "../lib/api";
  import { app } from "../lib/stores.svelte";

  let { value, onchange, minHeight = 260 }: { value: string; onchange: (html: string) => void; minHeight?: number } = $props();

  let el = $state<HTMLDivElement | null>(null);
  let source = $state(false);
  let lastEmitted = "";
  let srcText = $state("");

  $effect(() => {
    const v = value;
    if (el && !source && v !== lastEmitted) {
      el.innerHTML = highlightTokens(v);
      lastEmitted = v;
    }
  });

  function emit() {
    if (!el) return;
    const html = stripMarks(el.innerHTML);
    lastEmitted = html;
    onchange(html);
  }

  function cmd(name: string, arg?: string) {
    el?.focus();
    document.execCommand(name, false, arg);
    emit();
  }

  function link() {
    const url = prompt("Link address (https://…)");
    if (url && /^(https?:|mailto:)/i.test(url.trim())) cmd("createLink", url.trim());
  }

  function insertPlaceholder(e: Event) {
    const sel = e.target as HTMLSelectElement;
    const key = sel.value;
    sel.value = "";
    if (!key) return;
    el?.focus();
    document.execCommand("insertHTML", false, `<mark class="ph" data-ph="${key}">{{${key}}}</mark>&nbsp;`);
    emit();
  }

  function toggleSource() {
    if (!source) {
      srcText = stripMarks(el?.innerHTML ?? value);
      source = true;
    } else {
      source = false;
      lastEmitted = "";
      onchange(srcText);
    }
  }
</script>

<div class="ed">
  <div class="bar">
    <button class="small" title="Bold" onclick={() => cmd("bold")} disabled={source}><b>B</b></button>
    <button class="small" title="Italic" onclick={() => cmd("italic")} disabled={source}><i>I</i></button>
    <button class="small" title="Underline" onclick={() => cmd("underline")} disabled={source}><u>U</u></button>
    <button class="small" title="Bulleted list" onclick={() => cmd("insertUnorderedList")} disabled={source}>• List</button>
    <button class="small" title="Numbered list" onclick={() => cmd("insertOrderedList")} disabled={source}>1. List</button>
    <button class="small" title="Insert link" onclick={link} disabled={source}>Link</button>
    <button class="small" title="Remove formatting" onclick={() => cmd("removeFormat")} disabled={source}>Clear</button>
    <select class="ph-select" onchange={insertPlaceholder} disabled={source} title="Insert a placeholder that is filled from ticket data">
      <option value="">Insert field…</option>
      {#each app.catalog?.placeholders ?? [] as p}<option value={p.key}>{p.label}</option>{/each}
    </select>
    <span class="grow"></span>
    <button class="small link" onclick={toggleSource}>{source ? "Rich text" : "HTML"}</button>
  </div>
  {#if source}
    <textarea class="src" style="min-height:{minHeight}px" bind:value={srcText} onblur={() => onchange(srcText)}></textarea>
  {:else}
    <div bind:this={el} class="area email-html" style="min-height:{minHeight}px" contenteditable="true" role="textbox" aria-multiline="true" tabindex="0" oninput={emit}></div>
  {/if}
</div>

<style>
  .ed { border: 1px solid var(--border); border-radius: var(--radius); background: #fff; }
  .bar { display: flex; gap: 4px; padding: 4px; border-bottom: 1px solid var(--border); background: #f7f9fb; flex-wrap: wrap; align-items: center; }
  .ph-select { width: auto; height: 22px; padding: 0 4px; font-size: 12px; }
  .grow { flex: 1; }
  .area { padding: 10px; outline: none; overflow-wrap: anywhere; }
  .src { border: none; border-radius: 0; font-family: Consolas, monospace; font-size: 12px; }
</style>
