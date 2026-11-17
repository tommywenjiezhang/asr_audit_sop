<script lang="ts">
  import type { FolderNode } from "../lib/api";
  import FolderTree from "./FolderTree.svelte";

  let { node, selectedId, onselect, depth = 0 }: { node: FolderNode; selectedId: string; onselect: (n: FolderNode) => void; depth?: number } = $props();
  let open = $state(true);
  $effect(() => { open = depth < 2; });
</script>

<div>
  <div class="node" style="padding-left:{depth * 16}px">
    {#if node.children.length}
      <button class="tog" onclick={() => (open = !open)} aria-label="Expand">{open ? "▾" : "▸"}</button>
    {:else}<span class="tog"></span>{/if}
    {#if node.path}
      <button class="name" class:sel={node.entry_id === selectedId} onclick={() => onselect(node)}>📁 {node.name}</button>
    {:else}
      <span class="muted">{node.name}</span>
    {/if}
  </div>
  {#if open}
    {#each node.children as c (c.entry_id)}
      <FolderTree node={c} {selectedId} {onselect} depth={depth + 1} />
    {/each}
  {/if}
</div>

<style>
  .node { display: flex; align-items: center; gap: 2px; height: 24px; }
  .tog { width: 18px; height: 20px; border: none; background: none; padding: 0; color: var(--muted); }
  .name { border: none; background: none; height: 22px; padding: 0 6px; }
  .name.sel { background: var(--accent); color: #fff; }
</style>
