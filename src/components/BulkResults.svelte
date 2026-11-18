<script lang="ts">
  import type { BulkOutcome } from "../lib/api";

  /** Per-case results of a bulk action, with a safe retry of the failed cases only. */
  let { title, outcome, onretry, onclose }: { title: string; outcome: BulkOutcome; onretry?: (ids: number[]) => void; onclose: () => void } = $props();
  const failed = $derived(outcome.results.filter((r) => !r.ok).map((r) => r.ticket_id));
</script>

<div class="modal-back">
  <div class="modal stack" role="dialog" aria-label={title}>
    <h3>{title}</h3>
    <div class="small"><span class="badge ok">{outcome.ok} succeeded</span> {#if outcome.failed}<span class="badge err">{outcome.failed} failed</span>{/if}</div>
    <table class="grid small">
      <thead><tr><th>Case</th><th>Result</th><th>Details</th></tr></thead>
      <tbody>
        {#each outcome.results as r (r.ticket_id)}
          <tr>
            <td>{r.display_id}</td>
            <td><span class="badge {r.ok ? (r.outcome === 'unchanged' ? 'grey' : 'ok') : 'err'}">{r.ok ? (r.outcome === "unchanged" ? "No change" : "Done") : "Failed"}</span></td>
            <td>{r.message}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="small muted">Each case was handled separately. Retrying runs the action again only for the failed cases; cases already in the target state are left as they are.</div>
    <div class="row">
      <span class="grow"></span>
      {#if failed.length && onretry}<button onclick={() => onretry?.(failed)}>Retry failed ({failed.length})</button>{/if}
      <button class="primary" onclick={onclose}>Close</button>
    </div>
  </div>
</div>
