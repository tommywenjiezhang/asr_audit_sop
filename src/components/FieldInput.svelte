<script lang="ts">
  import type { FieldDef, TicketField } from "../lib/api";
  import { app } from "../lib/stores.svelte";

  let { def, field, onsave }: {
    def: FieldDef;
    field: TicketField | undefined;
    onsave: (value: string | null, verified: boolean) => void;
  } = $props();

  const effective = $derived(field?.effective ?? "");
  let draft = $state("");
  $effect(() => { draft = effective; });

  // dates are stored as MM/DD/YYYY (US format used in replies); <input type=date> needs YYYY-MM-DD
  function toIso(v: string): string {
    const m = v.match(/^(\d{1,2})\/(\d{1,2})\/(\d{4})$/);
    return m ? `${m[3]}-${m[1].padStart(2, "0")}-${m[2].padStart(2, "0")}` : /^\d{4}-\d{2}-\d{2}$/.test(v) ? v : "";
  }
  function fromIso(v: string): string {
    const m = v.match(/^(\d{4})-(\d{2})-(\d{2})$/);
    return m ? `${m[2]}/${m[3]}/${m[1]}` : v;
  }

  function commit(v: string) {
    if (v === effective) return;
    onsave(v, false);
  }

  function grades(): string[] {
    try { const g = JSON.parse(effective || "[]"); return Array.isArray(g) ? g : []; } catch { return []; }
  }
  function toggleGrade(g: string, on: boolean) {
    const cur = grades().filter((x) => x !== g);
    if (on) cur.push(g);
    const order = app.catalog?.grade_options ?? [];
    cur.sort((a, b) => order.indexOf(a) - order.indexOf(b));
    onsave(JSON.stringify(cur), false);
  }

  const stateClass = $derived(
    field?.state === "Verified" ? "ok" : field?.state === "Entered" ? "" : field?.state?.startsWith("Extracted") ? "warn" : "grey",
  );
  const options = $derived(def.type === "yesno" ? (app.catalog?.yes_no_unknown ?? ["Yes", "No", "Unknown"]) : (def.options ?? []));
</script>

<div class="field">
  <div class="lbl">{def.label}{#if def.required}<span class="req" title="Needed for a complete response"> *</span>{/if}</div>
  <div>
    {#if def.type === "select" || def.type === "yesno"}
      <select value={effective} onchange={(e) => commit((e.target as HTMLSelectElement).value)}>
        <option value="">— Not provided —</option>
        {#each options as o}<option value={o}>{o}</option>{/each}
        {#if effective && !options.includes(effective)}<option value={effective}>{effective}</option>{/if}
      </select>
    {:else if def.type === "date"}
      <div class="row">
        <input type="date" value={toIso(effective)} onchange={(e) => commit(fromIso((e.target as HTMLInputElement).value))} />
        {#if effective && !toIso(effective)}<span class="small muted" title="Value as written in the email">“{effective}”</span>{/if}
      </div>
    {:else if def.type === "checkbox"}
      <label class="inline"><input type="checkbox" checked={effective === "Yes"} onchange={(e) => commit((e.target as HTMLInputElement).checked ? "Yes" : "No")} /> Yes</label>
    {:else if def.type === "grades"}
      <div class="grades">
        {#each app.catalog?.grade_options ?? [] as g}
          <label class="inline small"><input type="checkbox" checked={grades().includes(g)} onchange={(e) => toggleGrade(g, (e.target as HTMLInputElement).checked)} />{g}</label>
        {/each}
      </div>
    {:else if def.type === "notes"}
      <textarea rows="3" bind:value={draft} onblur={() => commit(draft)}></textarea>
    {:else}
      <input
        type={def.type === "email" ? "email" : "text"}
        class:mono={def.type === "identifier"}
        bind:value={draft}
        onblur={() => commit(draft)}
        onkeydown={(e) => { if (e.key === "Enter") commit(draft); }}
        placeholder="Not provided"
      />
    {/if}
    <div class="meta small">
      <span class="badge {stateClass}">{field?.state ?? "Not Provided"}</span>
      {#if field?.extracted_value && field.manual_value != null && field.manual_value !== field.extracted_value}
        <span class="muted">email said: “{field.extracted_value}”</span>
        <button class="link small" onclick={() => onsave(null, false)} title="Discard the staff value and use the value extracted from the email">use email value</button>
      {/if}
      {#if field?.extraction_confidence && field.manual_value == null && field.extracted_value}
        <span class="muted">({field.extraction_confidence} confidence)</span>
      {/if}
      {#if effective && !field?.verified}
        <button class="link small" onclick={() => onsave(effective, true)} title="Mark this value as checked by staff">✓ verify</button>
      {/if}
      {#if field?.verified}
        <button class="link small" onclick={() => onsave(effective, false)}>unverify</button>
      {/if}
      {#if def.help}<span class="muted" title={def.help}>ⓘ</span>{/if}
    </div>
  </div>
</div>

<style>
  .meta { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; margin-top: 2px; }
  .grades { display: flex; flex-wrap: wrap; gap: 4px 12px; }
  .mono { font-family: Consolas, monospace; }
</style>
