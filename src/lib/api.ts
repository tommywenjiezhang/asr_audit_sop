import { invoke } from "@tauri-apps/api/core";

export interface Condition { field: string; op: string; value?: string; values?: string[]; fields?: string[]; label?: string }
export interface FieldDef { key: string; label: string; type: string; options?: string[]; required?: boolean; show_if?: Condition; help?: string }
export interface IssueType { id: string; name: string; sort: number; sop_section: string; sop_page: string; description: string; fields: FieldDef[]; default_template: string; internal_steps?: string | null }
export interface PlaceholderDef { key: string; label: string; source: string; sample: string; verified_only?: boolean }
export interface CatalogInfo {
  catalog_version: number; sop_document: string; yes_no_unknown: string[]; grade_options: string[];
  common_fields: FieldDef[]; placeholders: PlaceholderDef[]; issue_types: IssueType[]; statuses: string[]; priorities: string[];
}
export interface TicketSummary {
  id: number; display_id: string; status: string; priority: string; issue_type: string; school_name?: string; school_njiis_id?: string;
  school_verification: string; sender_name?: string; sender_email?: string; original_subject?: string; assigned_staff?: string;
  updated_at: string; last_email_received_at?: string; next_followup_date?: string; possible_duplicate_of?: number; has_draft: boolean;
}
export interface Ticket extends TicketSummary {
  issue_type_source: string; classification_confidence?: number; conversation_id?: string; internal_notes?: string;
  resolution_notes?: string; source: string; created_at: string; last_response_at?: string;
}
export interface TicketField {
  key: string; extracted_value?: string; extraction_confidence?: string; manual_value?: string | null; verified: boolean;
  effective: string; state: string; updated_at: string;
}
export interface MessageRow {
  id: number; subject?: string; sender_name?: string; sender_email?: string; received_at?: string; body_text?: string;
  body_truncated: boolean; folder_path?: string; link_type: string; is_internal: boolean; has_attachments: boolean;
}
export interface HistoryRow { id: number; at: string; actor: string; action: string; field?: string; old_value?: string; new_value?: string }
export interface TicketDetail { ticket: Ticket; fields: TicketField[]; messages: MessageRow[]; history: HistoryRow[] }
export interface ListResult { rows: TicketSummary[]; total: number; counts: [string, number][] }
export interface Unresolved { key: string; label: string; known: boolean }
export interface DraftCheck { unresolved: Unresolved[]; unverified_claims: string[]; sop_status?: string; sop_reference?: string }
export interface Proposal { template_id: string; template_name: string; subject: string; body_html: string; reason: string; check: DraftCheck }
export interface Draft {
  id: number; ticket_id: number; template_id?: string; subject: string; body_html: string; is_user_edited: boolean; status: string;
  outlook_entry_id?: string; outlook_created_at?: string; version: number; created_at: string; updated_at: string;
}
export interface DraftVersion { id: number; version: number; template_id?: string; subject: string; body_html: string; reason?: string; created_at: string }
export interface TemplateRow {
  id: string; issue_type: string; name: string; description?: string; sop_section?: string; sop_page?: string; sop_status: string;
  version: number; system_version?: number; priority: number; required_fields: string[]; optional_fields: string[];
  rules: Condition[]; asserts: Condition[]; subject_template: string; body_html: string; default_subject?: string;
  default_body_html?: string; is_system: boolean; is_customized: boolean; system_update_available: boolean; enabled: boolean;
  created_at: string; last_modified: string;
}
export interface TemplateVersion { id: number; version: number; subject_template: string; body_html: string; note?: string; saved_at: string }
export interface FolderRef { store_id: string; entry_id: string; path: string }
export interface Settings {
  mailbox_name: string; mailbox_store_id: string; folder: FolderRef | null; subfolder_mode: string; selected_subfolders: FolderRef[];
  sync_enabled: boolean; interval_minutes: number; check_on_startup: boolean; launch_at_login: boolean; close_to_tray: boolean;
  initial_lookback_days: number; overlap_minutes: number; full_rescan_hours: number; max_messages_per_sync: number; body_max_chars: number;
  python_path: string; staff_name: string; default_assigned_staff: string; asr_deadline: string; internal_senders: string[];
  duplicate_window_days: number; open_outlook_draft_after_create: boolean;
}
export interface IngestReport { seen: number; new_messages: number; already_imported: number; moved: number; created_tickets: number; updated_tickets: number; skipped_internal: number; duplicates_flagged: number }
export interface SyncStatus {
  running: boolean; last_run_at?: string; last_success_at?: string; last_error?: string; last_error_code?: string;
  last_report?: IngestReport; next_run_at?: string; folder_display?: string; warnings: string[];
}
export interface FolderNode { name: string; entry_id: string; store_id: string; path: string; children: FolderNode[] }
export interface Report {
  total: number; open: number; by_status: [string, number][]; by_issue: [string, string, number][]; by_school_verification: [string, number][];
  created_last_7_days: number; created_last_30_days: number; open_older_than_7_days: number; followups_due: number;
  outlook_drafts_created: number; by_week: [string, number][];
}
export interface AppInfo { version: string; data_dir: string; db_path: string; log_dir: string; startup_ms: number; schema_version: number; catalog_version: number; template_count: number }
export interface OutlookDraftResult { created: boolean; needs_confirmation: boolean; check: DraftCheck; entry_id?: string; mode?: string }

export interface ApiError { code: string; message: string }
export function errMsg(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as ApiError).message);
  return String(e);
}

export const api = {
  catalog: () => invoke<CatalogInfo>("catalog_get"),
  tickets: (query: { group?: string; search?: string; issue_type?: string; limit?: number; offset?: number }) =>
    invoke<ListResult>("tickets_list", { query }),
  ticket: (id: number) => invoke<TicketDetail>("ticket_detail", { id }),
  createTicket: (input: Record<string, string>) => invoke<number>("ticket_create", { input }),
  updateTicket: (id: number, patch: Record<string, unknown>) => invoke<Ticket>("ticket_update", { id, patch }),
  setField: (id: number, key: string, value: string | null, verified: boolean) => invoke<TicketField>("ticket_set_field", { id, key, value, verified }),
  refreshMessage: (messageId: number) => invoke<string>("message_refresh", { messageId }),

  templates: (issueType?: string) => invoke<TemplateRow[]>("templates_list", { issueType: issueType ?? null }),
  template: (id: string) => invoke<TemplateRow>("template_get", { id }),
  saveTemplate: (id: string, subject: string, bodyHtml: string, name?: string, note?: string) =>
    invoke<TemplateRow>("template_save", { id, subject, bodyHtml, name: name ?? null, note: note ?? null }),
  duplicateTemplate: (id: string, name: string) => invoke<TemplateRow>("template_duplicate", { id, name }),
  setTemplateEnabled: (id: string, enabled: boolean) => invoke<TemplateRow>("template_set_enabled", { id, enabled }),
  restoreTemplate: (id: string) => invoke<TemplateRow>("template_restore_default", { id }),
  templateVersions: (id: string) => invoke<TemplateVersion[]>("template_versions", { id }),
  preview: (templateId: string | null, subject: string, bodyHtml: string, ticketId: number | null) =>
    invoke<{ subject: string; body_html: string; check: DraftCheck }>("template_preview", { templateId, subject, bodyHtml, ticketId }),

  draft: (ticketId: number) => invoke<{ draft: Draft | null; check: DraftCheck | null }>("draft_get", { ticketId }),
  draftCheck: (ticketId: number, templateId: string | null, subject: string, bodyHtml: string) =>
    invoke<DraftCheck>("draft_check", { ticketId, templateId, subject, bodyHtml }),
  loadDefault: (ticketId: number, templateId: string | null) => invoke<Proposal>("draft_load_default", { ticketId, templateId }),
  resetDefault: (ticketId: number, templateId: string | null) => invoke<Proposal>("draft_reset", { ticketId, templateId }),
  generate: (ticketId: number) => invoke<Proposal>("draft_generate", { ticketId }),
  fill: (ticketId: number, templateId: string | null, subject: string, bodyHtml: string) =>
    invoke<Proposal>("draft_fill", { ticketId, templateId, subject, bodyHtml }),
  saveDraft: (ticketId: number, templateId: string | null, subject: string, bodyHtml: string, userEdited: boolean, reason: string) =>
    invoke<Draft>("draft_save", { ticketId, templateId, subject, bodyHtml, userEdited, reason }),
  draftVersions: (ticketId: number) => invoke<DraftVersion[]>("draft_versions", { ticketId }),
  createOutlookDraft: (ticketId: number, confirmIncomplete: boolean) =>
    invoke<OutlookDraftResult>("draft_create_outlook", { ticketId, confirmIncomplete }),

  outlookTest: () => invoke<Record<string, unknown>>("outlook_test"),
  mailboxes: () => invoke<{ mailboxes: { display_name: string; store_id: string }[] }>("outlook_list_mailboxes"),
  folders: (storeId: string | null, sharedMailbox: string | null) =>
    invoke<{ store_id: string; root: FolderNode }>("outlook_list_folders", { storeId, sharedMailbox }),
  syncNow: () => invoke<{ report: IngestReport; warnings: string[]; folders: number }>("sync_now"),
  syncStatus: () => invoke<SyncStatus>("sync_status"),

  settings: () => invoke<Settings>("settings_get"),
  saveSettings: (settings: Settings) => invoke<Settings>("settings_save", { settings }),
  report: () => invoke<Report>("report_get"),
  exportCsv: (path: string, group: string) => invoke<number>("export_csv", { path, group }),
  backup: (path: string) => invoke<void>("db_backup", { path }),
  restore: (path: string) => invoke<string>("db_restore", { path }),
  appInfo: () => invoke<AppInfo>("app_info"),
};

// ------------------------------------------------------------ helpers

export function fmtDate(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

/** Evaluate a show_if condition against current field values (mirrors the Rust rules). */
export function condMatches(c: Condition | undefined, values: Record<string, string>): boolean {
  if (!c) return true;
  const v = (values[c.field] ?? "").trim();
  switch (c.op) {
    case "eq": return v === (c.value ?? "");
    case "ne": return v !== (c.value ?? "");
    case "in": return (c.values ?? []).includes(v);
    case "not_in": return !(c.values ?? []).includes(v);
    case "empty": return v === "" || v === "[]";
    case "not_empty": return !(v === "" || v === "[]");
    default: return true;
  }
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Wrap {{tokens}} in <mark class="ph"> for display; the backend strips these marks on save. */
export function highlightTokens(html: string): string {
  return html.replace(/(<mark class="ph"[^>]*>)?(\{\{\s*([A-Za-z0-9_]+)\s*\}\})(<\/mark>)?/g,
    (_m, _a, tok, key) => `<mark class="ph" data-ph="${key}">${tok}</mark>`);
}

export function stripMarks(html: string): string {
  return html.replace(/<mark class="ph"[^>]*>(\{\{\s*[A-Za-z0-9_]+\s*\}\})<\/mark>/g, "$1");
}

export const debounce = <A extends unknown[]>(fn: (...a: A) => void, ms: number) => {
  let t: ReturnType<typeof setTimeout> | undefined;
  return (...a: A) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
};
