<script lang="ts">
  import { api, errMsg, fmtDate, statusBadge, type LinkedCase, type Suggestion } from "../lib/api";
  import { toast } from "../lib/stores.svelte";

  /** Related-case suggestions (evidence only) and explicit links. Nothing is combined automatically. */
  let { id, onopen, onmerge, onchanged }: { id: number; onopen?: (id: number) => void; onmerge?: (sources: number[]) => void; onchanged?: () => void } = $props();

  let sugg = $state<Suggestion[]>([]);
  let links = $state<LinkedCase[]>([]);
  let loading = $state(false);
  let target = $state("");
  let open = $state(false);

  async function load() {
    loading = true;
    try {
      [sugg, links] = await Promise.all([api.relatedSuggestions(id), api.ticketLinks(id)]);
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      loading = false;
    }
  }
  $effect(() => {
    void id;
    sugg = [];
    links = [];
    if (open) load();
  });

  async function link(t: string) {
    try {
      await api.ticketLink(id, t);
      target = "";
      toast("Cases linked. Both stay separate cases.", "ok");
      await load();
      onchanged?.();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
  async function setLink(l: LinkedCase, status: "dismissed" | "removed") {
    try {
      await api.ticketLinkSet(id, l.relation_id, status);
      await load();
      onchanged?.();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
  const linkedIds = $derived(new Set(links.map((l) => l.ticket_id)));
</script>

<div class="panel pad">
  <button class="link" onclick={() => { open = !open; if (open) load(); }}>{open ? "▾" : "▸"} Related cases{links.length ? ` (${links.filter((l) => l.status === "linked").length} linked)` : ""}</button>
  {#if open}
    <div class="stack" style="margin-top:6px">
      {#if links.length}
        <table class="grid small">
          <thead><tr><th>Linked case</th><th>Status</th><th>Subject</th><th></th></tr></thead>
          <tbody>
            {#each links as l (l.relation_id)}
              <tr>
                <td>{#if l.ticket_id}<button class="link small" onclick={() => onopen?.(l.ticket_id!)}>{l.display_id}</button>{:else}{l.ticket_ref ?? "not on this computer yet"}{/if}
                  {#if l.relation_type === "possible_duplicate"}<span class="badge warn" title={l.evidence ?? ""}>possible duplicate</span>{/if}</td>
                <td>{#if l.case_status}<span class="badge {statusBadge(l.case_status)}">{l.case_status}</span>{/if}</td>
                <td class="ellipsis">{l.subject ?? ""}</td>
                <td style="white-space:nowrap">
                  {#if l.relation_type === "possible_duplicate" && l.status === "open"}
                    {#if l.ticket_id}<button class="small" onclick={() => onmerge?.([l.ticket_id!])}>Combine…</button>{/if}
                    <button class="small" onclick={() => setLink(l, "dismissed")}>Not a duplicate</button>
                  {:else}<button class="link small" onclick={() => setLink(l, "removed")}>Unlink</button>{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
      <div class="row small">
        <span>Link to case</span><input style="width:170px" bind:value={target} placeholder="T-000123 or ASR-…" />
        <button class="small" disabled={!target.trim()} onclick={() => link(target)}>Link</button>
        <span class="muted">Linked cases stay separate (use Combine for one support case).</span>
      </div>
      <div class="small muted">Suggestions — evidence only, never combined automatically:</div>
      {#if loading}<div class="small muted">Looking for related cases…</div>{/if}
      {#each sugg.filter((s) => !s.linked && !linkedIds.has(s.ticket_id)) as s (s.ticket_id)}
        <div class="sugg small">
          <div class="row">
            <button class="link small" onclick={() => onopen?.(s.ticket_id)}><b>{s.display_id}</b></button>
            <span class="badge {statusBadge(s.status)}">{s.status}{s.owner ? ` — ${s.owner}` : ""}</span>
            <span class="muted">{fmtDate(s.received_at)}</span>
            <span class="grow"></span>
            <button class="small" onclick={() => link(String(s.ticket_id))}>Link</button>
            <button class="small" onclick={() => onmerge?.([s.ticket_id])}>Combine…</button>
          </div>
          <div class="ellipsis">{s.subject ?? ""}{s.school ? ` · ${s.school}` : ""}</div>
          <div class="muted">{s.reasons.join(" · ")}</div>
        </div>
      {:else}
        {#if !loading}<div class="small muted">No related cases found.</div>{/if}
      {/each}
    </div>
  {/if}
</div>

<style>
  .pad { padding: 12px; }
  .sugg { border-left: 3px solid #c9d1db; padding: 2px 8px; }
  .ellipsis { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 360px; }
</style>
