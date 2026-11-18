<script lang="ts">
  import type { FolderNode } from "../lib/api";
  import FolderTree from "./FolderTree.svelte";

  /**
   * Outlook folder picker. `expanded` (keyed by folder path) is owned by the page, so expansion survives
   * Refresh Folders; folders on the way to the selected one open automatically.
   */
  let { node, selectedId, onselect, depth = 0, expanded }: {
    node: FolderNode; selectedId: string; onselect: (n: FolderNode) => void; depth?: number; expanded: Record<string, boolean>;
  } = $props();

  const key = $derived(node.path || node.name);
  const hasKids = $derived(node.children.length > 0 || !!node.has_children);
  const containsSelected = $derived(!!selectedId && node.entry_id !== selectedId && contains(node, selectedId));
  const open = $derived(expanded[key] ?? (depth < 1 || containsSelected));
  const blocked = $derived(node.accessible === false);

  function contains(n: FolderNode, id: string): boolean {
    return n.children.some((c) => c.entry_id === id || contains(c, id));
  }
  function toggle() {
    expanded[key] = !open;
  }
</script>

<div role="treeitem" aria-selected={node.entry_id === selectedId} aria-expanded={hasKids ? open : undefined}>
  <div class="node" style="padding-left:{depth * 16}px">
    {#if hasKids}
      <button class="tog" onclick={toggle} aria-label={open ? `Collapse ${node.name}` : `Expand ${node.name}`} title={open ? "Collapse" : "Expand"}>{open ? "▾" : "▸"}</button>
    {:else}<span class="tog"></span>{/if}
    {#if blocked}
      <span class="name muted" title={node.error_detail ?? node.error ?? ""}>🔒 {node.name} <span class="badge grey">no access</span></span>
    {:else if node.path}
      <button class="name" class:sel={node.entry_id === selectedId} onclick={() => onselect(node)} title={node.outlook_path || node.path}>📁 {node.name}</button>
    {:else}
      <span class="muted">{node.name}</span>
    {/if}
    {#if node.unread_count}<span class="count" title="Unread items">{node.unread_count}</span>{/if}
    {#if node.error}<span class="badge err" title={node.error_detail ?? ""}>subfolders could not be read</span>{/if}
  </div>
  {#if open}
    {#each node.children as c (c.entry_id || c.path)}
      <FolderTree node={c} {selectedId} {onselect} depth={depth + 1} {expanded} />
    {/each}
    {#if hasKids && node.children_loaded === false && !node.error}
      <div class="small muted" style="padding-left:{(depth + 1) * 16 + 18}px">{node.child_count ?? ""} subfolder(s) not loaded — select this folder to load them</div>
    {/if}
  {/if}
</div>

<style>
  .node { display: flex; align-items: center; gap: 4px; min-height: 24px; }
  .tog { width: 18px; height: 20px; border: none; background: none; padding: 0; color: var(--muted); flex: none; }
  .name { border: none; background: none; height: 22px; padding: 0 6px; text-align: left; }
  .name.sel { background: var(--accent); color: #fff; }
  .count { font-size: 11px; color: var(--muted); }
</style>
