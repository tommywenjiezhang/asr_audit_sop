import { api, type CatalogInfo, type SharedStatus, type SyncStatus } from "./api";

export const app = $state<{
  catalog: CatalogInfo | null;
  sync: SyncStatus | null;
  /** shared (Access) database connection and queue state */
  shared: SharedStatus | null;
  toast: { text: string; kind: "ok" | "error" | "info" } | null;
  ticketsVersion: number;
}>({ catalog: null, sync: null, shared: null, toast: null, ticketsVersion: 0 });

/** One-line, plain-language shared database state for badges and the sidebar. */
export function sharedLabel(s: SharedStatus | null): { text: string; kind: "ok" | "warn" | "err" | "grey" } {
  if (!s) return { text: "Shared database: …", kind: "grey" };
  const pend = s.queue.pending + s.queue.held;
  const tail = pend ? ` · ${pend} pending` : "";
  switch (s.state) {
    case "standalone": return { text: "Standalone (this computer only)", kind: "grey" };
    case "connected": return { text: `Shared: connected${tail}`, kind: pend ? "warn" : "ok" };
    case "read_only": return { text: `Shared: read-only${tail}`, kind: "warn" };
    case "not_checked": return { text: `Shared: not checked yet${tail}`, kind: "grey" };
    case "identity_changed": return { text: "Shared: different database — review", kind: "err" };
    case "maintenance": return { text: `Shared: maintenance${tail}`, kind: "warn" };
    default: return { text: `Shared: offline — saved locally${tail}`, kind: "warn" };
  }
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(text: string, kind: "ok" | "error" | "info" = "info") {
  app.toast = { text, kind };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (app.toast = null), kind === "error" ? 8000 : 3500);
}

export async function loadCatalog() {
  if (!app.catalog) app.catalog = await api.catalog();
  return app.catalog;
}

export function issueName(id: string): string {
  return app.catalog?.issue_types.find((i) => i.id === id)?.name ?? id;
}
