<script lang="ts">
  import type { FolderNode, Settings } from "../lib/api";
  import { descendants, ref, rowState } from "../lib/folders.svelte";
  import SubfolderPicker from "./SubfolderPicker.svelte";

  /**
   * Checkbox tree below the monitored folder.
   * - "selected" mode: tick a folder to scan it; "+ subfolders" also scans everything below it.
   * - "all" mode (and below a "+ subfolders" folder): every folder is scanned; untick one to EXCLUDE it
   *   together with its subfolders (e.g. Completed folders).
   * `s` is the page's settings state and is changed in place.
   */
  let { s, root, node, depth = 0, expanded }: { s: Settings; root: FolderNode; node: FolderNode; depth?: number; expanded: Record<string, boolean> } = $props();

  function indeterminate(el: HTMLInputElement, on: boolean) {
    el.indeterminate = on;
    return { update(v: boolean) { el.indeterminate = v; } };
  }
  const someBelow = (n: FolderNode) => descendants(n).some((d) => rowState(s, root, d).included);

  function toggleSelected(n: FolderNode, on: boolean) {
    s.selected_subfolders = s.selected_subfolders.filter((f) => f.entry_id !== n.entry_id);
    s.selected_with_children = s.selected_with_children.filter((id) => id !== n.entry_id);
    if (on) s.selected_subfolders.push(ref(n));
  }
  function toggleWithChildren(n: FolderNode, on: boolean) {
    s.selected_with_children = s.selected_with_children.filter((id) => id !== n.entry_id);
    if (on) {
      s.selected_with_children.push(n.entry_id);
      // folders below are now covered: drop their own selections so nothing is scanned twice
      const below = new Set(descendants(n).map((d) => d.entry_id));
      s.selected_subfolders = s.selected_subfolders.filter((f) => !below.has(f.entry_id));
      s.selected_with_children = s.selected_with_children.filter((id) => !below.has(id));
    }
  }
  function toggleExcluded(n: FolderNode, exclude: boolean) {
    s.excluded_folders = (s.excluded_folders ?? []).filter((f) => f.entry_id !== n.entry_id);
    if (exclude) s.excluded_folders.push(ref(n));
  }
</script>

{#each node.children as c (c.entry_id || c.path)}
  {@const st = rowState(s, root, c)}
  {@const key = "pick:" + c.path}
  {@const open = expanded[key] ?? depth < 1}
  {@const kids = c.children.length > 0}
  <div class="row small item" style="padding-left:{depth * 18}px">
    {#if kids}
      <button class="tog" onclick={() => (expanded[key] = !open)} aria-label={open ? `Collapse ${c.name}` : `Expand ${c.name}`}>{open ? "▾" : "▸"}</button>
    {:else}<span class="tog"></span>{/if}
    {#if c.accessible === false}
      <label class="inline muted" title={c.error_detail ?? ""}><input type="checkbox" disabled />🔒 {c.name} <span class="badge grey">no access — not scanned</span></label>
    {:else if st.excludedVia}
      <label class="inline muted"><input type="checkbox" disabled />{c.name} <span class="badge grey">excluded with {st.excludedVia}</span></label>
    {:else if st.coveredBy}
      <!-- scanned because a parent includes its subfolders: unticking excludes it -->
      <label class="inline" class:muted={st.excluded} title="Untick to exclude this folder and its subfolders">
        <input type="checkbox" checked={!st.excluded} onchange={(e) => toggleExcluded(c, !(e.target as HTMLInputElement).checked)} />
        <span class:struck={st.excluded}>{c.name}</span>
        {#if st.excluded}<span class="badge warn">excluded</span>{:else}<span class="badge grey">included via {st.coveredBy}</span>{/if}
      </label>
    {:else}
      <label class="inline">
        <input type="checkbox" checked={st.selected} use:indeterminate={!st.selected && someBelow(c)}
          onchange={(e) => toggleSelected(c, (e.target as HTMLInputElement).checked)} />{c.name}
      </label>
      {#if st.selected && kids}
        <label class="inline muted" title="Also scan every folder below {c.name}">
          <input type="checkbox" checked={st.withChildren} onchange={(e) => toggleWithChildren(c, (e.target as HTMLInputElement).checked)} />+ its {descendants(c).length} subfolder(s)
        </label>
      {/if}
    {/if}
    {#if c.error}<span class="badge err" title={c.error_detail ?? ""}>subfolders could not be read</span>{/if}
  </div>
  {#if kids && open}
    <SubfolderPicker {s} {root} node={c} depth={depth + 1} {expanded} />
  {/if}
{/each}

<style>
  .item { min-height: 24px; gap: 4px; flex-wrap: nowrap; }
  .tog { width: 18px; height: 20px; border: none; background: none; padding: 0; color: var(--muted); flex: none; }
  .struck { text-decoration: line-through; }
</style>
