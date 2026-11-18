// Outlook folder discovery for every folder picker (monitored folders, completion folder, staff folders):
// one cached, de-duplicated loader plus pure helpers that compute exactly which folders a scan reads.
import { api, type FolderListing, type FolderNode, type FolderRef, type Settings } from "./api";

type Key = string;
const cache = new Map<Key, { listing: FolderListing; at: number }>();
const inflight = new Map<Key, Promise<FolderListing>>();

/** The last finished discovery (shown with "Copy Diagnostics"). */
export const discovery = $state<{ last: (FolderListing & { at: number; request: string }) | null; error: string | null }>({ last: null, error: null });

const keyOf = (storeId: string | null, shared: string | null, root: string | null) =>
  `${storeId ?? ""}|${(shared ?? "").trim().toLowerCase()}|${root ?? ""}`;

/**
 * Load a folder tree. Results are cached per mailbox/subtree until `refresh` (Refresh Folders) or a
 * different mailbox is opened; identical concurrent requests share one Outlook call.
 */
export async function loadFolders(storeId: string | null, shared: string | null, rootEntryId: string | null = null, refresh = false): Promise<FolderListing> {
  const key = keyOf(storeId, shared, rootEntryId);
  if (!refresh) {
    const hit = cache.get(key);
    if (hit) return hit.listing;
  }
  const running = inflight.get(key);
  if (running) return running;
  const p = api
    .folders(storeId, shared, rootEntryId)
    .then((listing) => {
      cache.set(key, { listing, at: Date.now() });
      // a whole-mailbox listing makes cached subtrees of that mailbox stale
      if (!rootEntryId) for (const k of [...cache.keys()]) if (k !== key && k.startsWith(`${listing.store_id}|`)) cache.delete(k);
      discovery.last = { ...listing, at: Date.now(), request: describe(storeId, shared, rootEntryId) };
      discovery.error = null;
      return listing;
    })
    .catch((e) => {
      discovery.error = `${describe(storeId, shared, rootEntryId)}: ${e?.message ?? e}`;
      throw e;
    })
    .finally(() => inflight.delete(key));
  inflight.set(key, p);
  return p;
}

export function clearFolderCache() {
  cache.clear();
}

function describe(storeId: string | null, shared: string | null, root: string | null): string {
  if (root) return "monitored folder subtree";
  if (shared) return `mailbox "${shared}"`;
  return storeId ? "selected mailbox" : "default mailbox";
}

export function findNode(n: FolderNode | null | undefined, id: string): FolderNode | null {
  if (!n || !id) return null;
  if (n.entry_id === id) return n;
  for (const c of n.children) {
    const f = findNode(c, id);
    if (f) return f;
  }
  return null;
}

/** All descendants (depth first). */
export function descendants(n: FolderNode): FolderNode[] {
  return n.children.flatMap((c) => [c, ...descendants(c)]);
}

/** Paths of the ancestors of `path` ("Inbox/A/B" -> ["Inbox", "Inbox/A"]). */
export function ancestorPaths(path: string): string[] {
  const parts = path.split("/");
  return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
}

export const isBelow = (path: string, parent: string) => path.length > parent.length + 1 && path.startsWith(parent + "/");

export const ref = (n: FolderNode): FolderRef => ({ store_id: n.store_id, entry_id: n.entry_id, path: n.path });

export type RowState = {
  /** this folder is scanned */
  included: boolean;
  /** explicitly selected ("selected" mode) */
  selected: boolean;
  /** selected together with all its subfolders */
  withChildren: boolean;
  /** covered by a selected ancestor that includes its subfolders, or by "all" mode */
  coveredBy: string | null;
  /** this folder is explicitly excluded */
  excluded: boolean;
  /** an ancestor is excluded (the whole subtree is skipped) */
  excludedVia: string | null;
};

/** How one descendant of the monitored folder is treated by the current settings. */
export function rowState(s: Settings, root: FolderNode, n: FolderNode): RowState {
  const excl = s.excluded_folders ?? [];
  const excluded = excl.some((x) => x.entry_id === n.entry_id);
  const excludedViaRef = excl.find((x) => x.entry_id !== n.entry_id && isBelow(n.path, x.path));
  const selected = s.subfolder_mode === "selected" && s.selected_subfolders.some((f) => f.entry_id === n.entry_id);
  const withChildren = selected && s.selected_with_children.includes(n.entry_id);
  let coveredBy: string | null = null;
  if (s.subfolder_mode === "all") coveredBy = root.path;
  else if (s.subfolder_mode === "selected") {
    const anc = s.selected_subfolders.find((f) => s.selected_with_children.includes(f.entry_id) && isBelow(n.path, f.path));
    coveredBy = anc ? anc.path : null;
  }
  const blocked = excluded || !!excludedViaRef || n.accessible === false;
  return { included: !blocked && (selected || !!coveredBy), selected, withChildren, coveredBy, excluded, excludedVia: excludedViaRef?.path ?? null };
}

/** Exactly the folders a scan reads with these settings (as far as the loaded tree shows them). */
export function scanPlan(s: Settings, root: FolderNode | null): { path: string; note: string }[] {
  if (!s.folder) return [];
  const out = [{ path: s.folder.path, note: s.subfolder_mode === "all" ? "with its subfolders" : "this folder only" }];
  if (s.subfolder_mode === "none") return out;
  if (!root) {
    // tree not loaded: describe the selection
    if (s.subfolder_mode === "selected")
      for (const f of s.selected_subfolders) out.push({ path: f.path, note: s.selected_with_children.includes(f.entry_id) ? "with its subfolders" : "this folder only" });
    return out;
  }
  for (const n of descendants(root)) {
    const st = rowState(s, root, n);
    if (st.included) out.push({ path: n.path, note: st.selected ? (st.withChildren ? "selected, with its subfolders" : "selected") : `via ${st.coveredBy}` });
  }
  return out;
}

/** Plain-text discovery report for "Copy Diagnostics" (folder names and counts only; no email content). */
export function discoveryReport(s: Settings | null): string {
  const lines: string[] = ["ASR/Audit Support Desk - Outlook folder discovery diagnostics", `Copied: ${new Date().toISOString()}`];
  if (s) {
    lines.push(`Monitored: ${s.mailbox_name || "(mailbox not set)"} > ${s.folder?.path ?? "(no folder)"} [store ...${(s.folder?.store_id ?? s.mailbox_store_id).slice(-12)}, folder ...${(s.folder?.entry_id ?? "").slice(-12)}]`);
    lines.push(`Child folder mode: ${s.subfolder_mode}; selected: ${s.selected_subfolders.map((f) => f.path + (s.selected_with_children.includes(f.entry_id) ? " (+subfolders)" : "")).join(", ") || "none"}`);
    lines.push(`Excluded: ${(s.excluded_folders ?? []).map((f) => f.path).join(", ") || "none"}; completion folder: ${s.archive_folder?.path ?? "none"}`);
  }
  const d = discovery.last;
  if (discovery.error) lines.push(`Last discovery FAILED: ${discovery.error}`);
  if (d) {
    const x = d.discovery;
    lines.push(`Last discovery: ${d.request}, ${new Date(d.at).toISOString()}, mailbox "${d.mailbox ?? ""}" [store ...${d.store_id.slice(-12)}]`);
    if (x) {
      lines.push(`Source: ${x.hierarchy_source}${x.mapi_error ? ` (MAPI unavailable: ${x.mapi_error})` : ""}; immediate child folders: ${x.immediate_children}; folders in total: ${x.descendants}; with problems: ${x.problems}; ${x.elapsed_ms} ms`);
      if (x.repaired.length) lines.push(`Subfolders recovered through MAPI (Outlook listed none or too few): ${x.repaired.join(", ")}`);
      for (const e of x.errors) lines.push(`[ERROR] ${e}`);
      lines.push("--- log ---", ...x.log);
    }
  } else if (!discovery.error) lines.push("No folder discovery has run in this session yet (use Refresh Folders).");
  return lines.join("\n");
}
