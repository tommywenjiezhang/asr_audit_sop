import { api, type CatalogInfo, type SyncStatus } from "./api";

export const app = $state<{
  catalog: CatalogInfo | null;
  sync: SyncStatus | null;
  toast: { text: string; kind: "ok" | "error" | "info" } | null;
  ticketsVersion: number;
}>({ catalog: null, sync: null, toast: null, ticketsVersion: 0 });

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
