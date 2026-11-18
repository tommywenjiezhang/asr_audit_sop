<script lang="ts">
  import { api, errMsg, fmtDate, localDayStart, statusBadge, type ActivityRow, type ActivitySummary, type Overview, type TeamOverview } from "../lib/api";
  import { app, toast } from "../lib/stores.svelte";

  let { onopenticket }: { onopenticket?: (id: number) => void } = $props();

  type Tab = "team" | "activity" | "scope";
  let tab = $state<Tab>("team");
  let oldDays = $state(14);
  let staleDays = $state(7);
  let t = $state<TeamOverview | null>(null);
  let actor = $state("");
  let actSum = $state<ActivitySummary[]>([]);
  let actRows = $state<ActivityRow[]>([]);

  // folder-scope report (this computer's selected Outlook folders)
  const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  const today = new Date();
  let from = $state(iso(new Date(today.getFullYear(), today.getMonth(), today.getDate() - 30)));
  let to = $state(iso(today));
  let o = $state<Overview | null>(null);

  async function load() {
    try {
      if (tab === "team") t = await api.teamOverview(oldDays, staleDays);
      else if (tab === "activity") {
        const r = await api.staffActivity(actor || null, 300);
        actSum = r.summary;
        actRows = r.rows;
      } else o = await api.overview(from ? localDayStart(from) : "", to ? localDayStart(to, 1) : "");
    } catch (e) {
      toast(errMsg(e), "error");
    }
  }
  $effect(() => { void tab; void oldDays; void staleDays; void actor; void from; void to; void app.ticketsVersion; void app.shared?.last_success_at; load(); });
  const total = (k: "unassigned" | "assigned" | "in_progress" | "review" | "done") => (o?.rows ?? []).reduce((n, r) => n + r[k], 0);
</script>

<div class="scroll" style="height:100%;padding:16px">
  <div class="row wrap">
    <h2 style="margin:0">Team Overview</h2>
    <div class="tabs">
      <button class="tab" class:on={tab === "team"} onclick={() => (tab = "team")}>Workload</button>
      <button class="tab" class:on={tab === "activity"} onclick={() => (tab = "activity")}>Staff activity</button>
      <button class="tab" class:on={tab === "scope"} onclick={() => (tab = "scope")}>Folder report</button>
    </div>
    <span class="grow"></span><button onclick={load}>Refresh</button>
  </div>

  {#if tab === "team" && t}
    <div class="tiles">
      <div class="tile"><div class="n">{t.total_open}</div><div>open cases</div></div>
      <div class="tile" class:warn={t.unassigned > 0}><div class="n">{t.unassigned}</div><div>unassigned</div></div>
      <div class="tile" class:err={t.review > 0}><div class="n">{t.review}</div><div>in Review</div></div>
      <div class="tile" class:warn={t.older > 0}><div class="n">{t.older}</div><div>older than {t.old_days} days</div></div>
      <div class="tile" class:warn={t.stale > 0}><div class="n">{t.stale}</div><div>no activity {staleDays}+ days</div></div>
      <div class="tile" class:warn={t.outlook_pending > 0}><div class="n">{t.outlook_pending}</div><div>pending Outlook work</div></div>
      <div class="tile" class:err={t.conflicts_open > 0}><div class="n">{t.conflicts_open}</div><div>sync conflicts</div></div>
      <div class="tile" class:warn={t.sync_pending + t.sync_held > 0}><div class="n">{t.sync_pending + t.sync_held}</div><div>changes not yet shared</div></div>
      <div class="tile ok"><div class="n">{t.done_7d}</div><div>Done last 7 days</div></div>
    </div>
    <div class="row small" style="margin:6px 0">
      <span class="muted">Older than</span><input type="number" min="1" style="width:70px" bind:value={oldDays} /><span class="muted">days · stale after</span><input type="number" min="1" style="width:70px" bind:value={staleDays} /><span class="muted">days</span>
      <span class="grow"></span><span class="muted">Shared data refreshed {t.shared_last_success_at ? fmtDate(t.shared_last_success_at) : "never"}</span>
    </div>
    <table class="grid panel">
      <thead><tr><th>Staff</th><th class="num">Assigned</th><th class="num">In Progress</th><th class="num">Review</th><th class="num">Unresolved</th><th class="num">Older</th><th class="num">Oldest (days)</th><th class="num">Done 7d</th><th class="num">Done 30d</th><th class="num">Outlook pending</th><th class="num">Conflicts</th></tr></thead>
      <tbody>
        {#each t.rows as r (r.staff_id ?? 0)}
          <tr class:muted={!r.active}>
            <td>{r.name}{#if !r.active} <span class="badge grey">inactive</span>{/if}</td>
            <td class="num">{r.staff_id == null ? "" : r.assigned}</td><td class="num">{r.in_progress}</td><td class="num">{r.review}</td>
            <td class="num"><b>{r.unresolved}</b></td><td class="num" class:hot={r.older > 0}>{r.older}</td><td class="num">{r.oldest_days ?? ""}</td>
            <td class="num">{r.done_7d}</td><td class="num">{r.done_30d}</td><td class="num" class:hot={r.outlook_pending > 0}>{r.outlook_pending}</td><td class="num" class:hot={r.conflicts > 0}>{r.conflicts}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <h3 style="margin-top:14px">Oldest open cases</h3>
    <table class="grid panel small">
      <thead><tr><th>Case</th><th>Status</th><th>Owner</th><th>School / subject</th><th class="num">Age (days)</th><th class="num">Idle (days)</th></tr></thead>
      <tbody>
        {#each t.oldest as c (c.ticket_id)}
          <tr>
            <td>{#if onopenticket}<button class="link small" onclick={() => onopenticket?.(c.ticket_id)}>{c.display_id}</button>{:else}{c.display_id}{/if}<div class="muted">{c.ticket_ref ?? ""}</div></td>
            <td><span class="badge {statusBadge(c.status)}">{c.status}</span></td><td>{c.owner ?? "—"}</td>
            <td>{c.school ?? ""}<div class="muted ellipsis">{c.subject ?? ""}</div></td>
            <td class="num">{c.age_days}</td><td class="num" class:hot={c.idle_days >= staleDays}>{c.idle_days}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <ul class="small muted notes">{#each t.notes as n}<li>{n}</li>{/each}</ul>
  {:else if tab === "activity"}
    <div class="row small" style="margin:8px 0">
      <span class="muted">Person</span>
      <select bind:value={actor} style="width:auto"><option value="">Everyone</option>{#each actSum as s}<option value={s.actor}>{s.actor}</option>{/each}</select>
    </div>
    <table class="grid panel small">
      <thead><tr><th>Person</th><th class="num">Actions (7 days)</th><th class="num">Cases (7 days)</th><th>Last activity</th></tr></thead>
      <tbody>{#each actSum as s}<tr><td>{s.actor}</td><td class="num">{s.actions_7d}</td><td class="num">{s.cases_7d}</td><td>{fmtDate(s.last_at)}</td></tr>{/each}</tbody>
    </table>
    <h3 style="margin-top:14px">Recent actions{actor ? ` — ${actor}` : ""}</h3>
    <table class="grid panel small">
      <thead><tr><th>When</th><th>Who</th><th>Action</th><th>Case</th><th>Detail</th></tr></thead>
      <tbody>
        {#each actRows as r, i (i)}
          <tr><td style="white-space:nowrap">{fmtDate(r.at)}</td><td>{r.actor}</td><td>{r.action}</td>
            <td>{#if onopenticket}<button class="link small" onclick={() => onopenticket?.(r.ticket_id)}>{r.ticket_ref ?? r.display_id}</button>{:else}{r.ticket_ref ?? r.display_id}{/if}</td>
            <td class="muted">{r.detail ?? ""}</td></tr>
        {:else}<tr><td colspan="5" class="muted">No activity recorded yet.</td></tr>{/each}
      </tbody>
    </table>
  {:else if tab === "scope" && o}
    <div class="small muted" style="margin:6px 0 10px"><b>Reporting scope:</b> {o.scope_summary} · last verified {o.last_reconciled_at ? fmtDate(o.last_reconciled_at) : "never"}</div>
    <div class="row small" style="margin-bottom:8px">
      <span class="muted">Done between</span><input type="date" style="width:150px" bind:value={from} /><span class="muted">and</span><input type="date" style="width:150px" bind:value={to} /><span class="muted">(end date included)</span>
    </div>
    <table class="grid panel">
      <thead><tr><th>Staff</th><th>Folder</th><th class="num">Unassigned</th><th class="num">Assigned</th><th class="num">In Progress</th><th class="num">Review</th><th class="num">Done (local)</th></tr></thead>
      <tbody>
        {#each o.rows as r (r.staff_id ?? 0)}
          <tr class:muted={!r.active}>
            <td>{r.name}{#if !r.active} <span class="badge grey">inactive</span>{/if}</td>
            <td class="small">{r.folder_path ?? "—"}{#if r.folder_path && !r.folder_in_scope} <span class="badge warn" title="Not in the selected folders: current tickets there are not reconciled on this computer">not reconciled</span>{/if}</td>
            <td class="num">{r.staff_id == null ? r.unassigned : ""}</td><td class="num">{r.staff_id == null ? "" : r.assigned}</td>
            <td class="num">{r.in_progress}</td><td class="num">{r.review}</td><td class="num">{r.done}</td>
          </tr>
        {/each}
        <tr><td><b>Total</b></td><td></td><td class="num"><b>{total("unassigned")}</b></td><td class="num"><b>{total("assigned")}</b></td><td class="num"><b>{total("in_progress")}</b></td><td class="num"><b>{total("review")}</b></td><td class="num"><b>{total("done")}</b></td></tr>
      </tbody>
    </table>
    <ul class="small muted notes">{#each o.notes as n}<li>{n}</li>{/each}</ul>
  {/if}
</div>

<style>
  .num { text-align: right; font-variant-numeric: tabular-nums; width: 90px; }
  .hot { color: var(--err); font-weight: 600; }
  .notes { margin-top: 10px; padding-left: 18px; line-height: 1.6; }
  table { background: #fff; }
  .tabs { display: flex; gap: 2px; margin-left: 12px; }
  .tab { border: none; border-bottom: 2px solid transparent; border-radius: 0; background: none; font-weight: 600; color: var(--muted); padding: 0 10px; height: 32px; }
  .tab.on { color: var(--accent); border-bottom-color: var(--accent); }
  .tiles { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 8px; margin-top: 12px; }
  .tile { background: #fff; border: 1px solid var(--border); border-radius: 8px; padding: 8px 10px; font-size: 12px; color: var(--muted); }
  .tile .n { font-size: 22px; font-weight: 700; color: var(--text); font-variant-numeric: tabular-nums; }
  .tile.warn .n { color: var(--warn); } .tile.err .n { color: var(--err); } .tile.ok .n { color: var(--ok); }
  .ellipsis { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 380px; }
</style>
