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
  updated_at: string; last_email_received_at?: string; next_followup_date?: string; possible_duplicate_of?: number;
  first_received_at?: string; completed_at?: string; needs_review: boolean; low_confidence: boolean; issue_count?: number;
  assignee_staff_id?: number | null; assignee_name?: string | null; assigned_at?: string | null; ownership_conflict: boolean;
  scope_state: "current" | "history" | "unverified"; scope_reason?: string | null; ticket_ref?: string | null; in_scope_count?: number;
  /** shared-database state of this case */
  sync_state: SyncState; review_reason?: string | null; new_reply?: boolean;
}
export type SyncState = "local" | "pending" | "synced" | "conflict" | "failed";
export interface Ticket extends TicketSummary {
  issue_type_source: string; classification_confidence?: number; conversation_id?: string; internal_notes?: string;
  resolution_notes?: string; source: string; created_at: string; last_response_at?: string; staff_actions?: string; last_prompt_at?: string;
  scope_changed_at?: string | null;
  shared_version: number; shared_state: "local_only" | "queued" | "shared"; shared_refreshed_at?: string | null;
}
export interface IssueRow { issue_type: string; source: string; position: number }
export interface CompletionRow {
  message_id: number; flagged_at?: string; moved_at?: string; dest_path?: string; last_error?: string; last_error_at?: string; attempts: number;
}
export interface CompletionOutcome { done: boolean; flagged: number; moved: number; failed: { message_id: number; subject: string; error: string }[]; dest_path: string }
export interface DoneDecision { completion_id: string; covered: number; state: "pending" | "accepted" }
export interface MarkDoneResult { decision: DoneDecision; outlook?: CompletionOutcome | null; outlook_message: string }
export interface ListQuery {
  view?: string; sync?: string; status?: string; assignee?: string; reason?: string; school?: string; sender?: string; min_age_days?: number;
  assigned_from?: string; assigned_to?: string;
  search?: string; issue_type?: string; received_from?: string; received_to?: string;
  completed_from?: string; completed_to?: string; sort?: string; limit?: number; offset?: number;
}
export interface Staff {
  id: number; display_name: string; email: string; mailbox_name?: string | null; store_id: string; folder_entry_id: string;
  folder_path: string; active: boolean; created_at: string; updated_at: string; shared_id?: string | null; windows_account?: string | null;
}
export interface StaffInput { id?: number | null; display_name: string; email: string; mailbox_name: string; store_id: string; folder_entry_id: string; folder_path: string; windows_account: string }
export interface AssignPreview { ticket_id: number; display_id: string; from_name?: string | null; to: Staff; emails: number; already_in_folder: number; will_notify: boolean; blocked?: string | null }
export interface MoveFailure { message_id: number; subject: string; error: string }
export interface AssignOutcome {
  ticket_id: number; display_id: string; result: "assigned" | "no_change" | "moves_failed" | "blocked" | "busy"; message: string;
  failures: MoveFailure[]; op_id?: number | null; notify_state: string; notify_error?: string | null;
}
export interface MoveRow { message_id: number; subject?: string; moved_at?: string | null; new_folder_path?: string | null; last_error?: string | null; attempts: number }
export interface AssignmentOp {
  id: number; ticket_id: number; from_staff_id?: number | null; from_name?: string | null; to_staff_id: number; to_name?: string | null;
  operator: string; note?: string | null; created_at: string; state: string; dest_path: string; prev_folders?: string | null; finished_at?: string | null;
  notify_state: string; notify_id?: string | null; notify_recipient?: string | null; notify_account?: string | null; notify_attempts: number;
  notify_at?: string | null; notify_error?: string | null; notify_entry_id?: string | null; notify_checked_at?: string | null; moves: MoveRow[];
}
export interface ScopeStatus {
  configured: boolean; mailbox: string; summary: string; last_ok_at?: string | null; last_attempt_at?: string | null; last_error?: string | null;
  stale: boolean; running: boolean; current: number; history: number; unverified: number;
}
export interface OverviewRow { staff_id?: number | null; name: string; active: boolean; folder_path?: string | null; folder_in_scope: boolean; unassigned: number; assigned: number; in_progress: number; review: number; done: number }
export interface Overview { scope_summary: string; last_reconciled_at?: string | null; done_from: string; done_to: string; rows: OverviewRow[]; awaiting_verification: number; notes: string[] }

export const HISTORY_REASONS: [string, string][] = [
  ["completed", "Completed"], ["reassigned", "Reassigned outside the selected scope"], ["moved", "Moved elsewhere"],
  ["not_found", "No longer found after successful reconciliation"], ["unknown", "Outside the scope; location not verified"],
];
export function reasonLabel(r?: string | null): string {
  return HISTORY_REASONS.find(([k]) => k === r)?.[1] ?? (r || "");
}
/** "Unassigned" / "Assigned to Aaron" / "In Progress — Aaron" / "Review — Aaron" / "Done — Aaron" */
export function statusText(status: string, assignee?: string | null): string {
  if (status === "Unassigned" || !assignee) return status;
  if (status === "Assigned") return `Assigned to ${assignee}`;
  return `${status} — ${assignee}`;
}
export function statusBadge(status: string): string {
  return status === "Done" ? "grey" : status === "Unassigned" ? "warn" : status === "Review" ? "err" : "ok";
}
export interface TicketField {
  key: string; extracted_value?: string; extraction_confidence?: string; manual_value?: string | null; verified: boolean;
  effective: string; state: string; updated_at: string;
}
export interface MessageRow {
  id: number; subject?: string; sender_name?: string; sender_email?: string; received_at?: string; body_text?: string;
  body_truncated: boolean; folder_path?: string; link_type: string; is_internal: boolean; has_attachments: boolean; entry_id?: string;
  scope_state: "in" | "out" | "unknown"; out_reason?: string | null; location_status?: string | null; location_checked_at?: string | null; owner_name?: string | null;
}
export interface HistoryRow { id: number; at: string; actor: string; action: string; field?: string; old_value?: string; new_value?: string }
export interface TicketDetail {
  ticket: Ticket; fields: TicketField[]; messages: MessageRow[]; history: HistoryRow[];
  issues: IssueRow[]; schools: number[]; completion: CompletionRow[]; assignments: AssignmentOp[];
  notes: Note[]; ownership: OwnershipRow[]; completions: CompletionDecision[]; conflicts: LocalConflict[]; pending_ops: OpRow[];
}
export interface Note { note_id: string; author_name: string; body: string; created_at: string; updated_at: string; origin: string; sync_state: "" | "pending" | "conflict" }
export interface OwnershipRow { staff_name?: string | null; started_at: string; ended_at?: string | null; started_by?: string | null; start_reason?: string | null; end_reason?: string | null; origin: string }
export interface CompletionDecision { completion_id: string; actor_name?: string | null; decided_at: string; covered: number; state: string; state_reason?: string | null }
export interface LocalConflict {
  conflict_id: string; op_id: string; ticket_id?: number | null; ticket_ref?: string | null; field_name?: string | null; conflict_type: string;
  original_value?: string | null; proposed_value?: string | null; shared_value?: string | null; reason?: string | null;
  status: "open" | "resolving" | "resolved"; op_type?: string | null; staff_name?: string | null; created_at: string;
}
export interface OpRow { op_id: string; op_type: string; state: string; created_at: string; attempts: number; last_error?: string | null; hold_reason?: string | null; staff_name: string }
export interface QueueSummary { pending: number; held: number; conflicts: number; rejected: number; isolated: number; oldest_pending_at?: string | null; last_error?: string | null }
export interface DbIdentity { database_id: string; display_name: string; environment: string; schema_version: number; min_client_protocol: number; maintenance: boolean; maintenance_message: string; created_at: string }
export interface CycleReport {
  identity?: DbIdentity | null; binding: string; writable: boolean; pushed: number; accepted: number; conflicts: number; rejected: number;
  replayed: number; pulled: number; staff: number; full_reconcile: boolean; held: number; elapsed_ms: number;
}
export interface SharedStatus {
  mode: "shared" | "standalone"; path: string;
  state: "standalone" | "not_checked" | "connected" | "read_only" | "unavailable" | "driver_missing" | "locked" | "no_permission" | "incompatible" | "maintenance" | "identity_changed" | "error";
  reachable?: boolean | null; readable?: boolean | null; writable?: boolean | null;
  database_id?: string | null; database_name?: string | null; environment?: string | null; schema_version?: number | null; compatibility?: string | null;
  app_schema_version: number; bound_database_id?: string | null; bound_database_name?: string | null; driver?: string | null;
  error_code?: string | null; error_message?: string | null; provider_detail?: string | null;
  last_success_at?: string | null; last_attempt_at?: string | null; next_attempt_at?: string | null;
  queue: QueueSummary; running: boolean; last_report?: CycleReport | null; consecutive_failures: number;
}
export interface ImportPreview { local_only: number; with_message_id: number; without_message_id: number; done: number; open: number; staff: number; already_shared?: number | null; held_ops: number }
export interface DiagEvent { id: number; at: string; severity: "info" | "warning" | "error"; component: string; code?: string | null; summary: string; detail?: string | null; correlation?: string | null }
export const SYNC_LABEL: Record<SyncState, string> = { local: "Local only", pending: "Saved locally — pending sync", synced: "Synced", conflict: "Conflict — review required", failed: "Failed — retry available" };
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
  selected_with_children: string[];
  sync_enabled: boolean; interval_minutes: number; check_on_startup: boolean; launch_at_login: boolean; close_to_tray: boolean;
  initial_lookback_days: number; overlap_minutes: number; full_rescan_hours: number; max_messages_per_sync: number; body_max_chars: number;
  python_path: string; staff_name: string; default_assigned_staff: string; asr_deadline: string; internal_senders: string[];
  duplicate_window_days: number; open_outlook_draft_after_create: boolean;
  scan_mode: "all" | "unread"; archive_folder: FolderRef | null; archive_mailbox_name: string; prompt_instructions: string;
  notify_account: string; shared_mode: "shared" | "standalone"; shared_db_path: string; shared_sync_minutes: number;
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
export interface AppInfo { version: string; data_dir: string; db_path: string; log_dir: string; startup_ms: number; schema_version: number; catalog_version: number; template_count: number; worker?: string }
export interface OutlookDraftResult { created: boolean; needs_confirmation: boolean; check: DraftCheck; entry_id?: string; mode?: string }

export interface ApiError { code: string; message: string }
export function errMsg(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as ApiError).message);
  return String(e);
}

export const api = {
  catalog: () => invoke<CatalogInfo>("catalog_get"),
  tickets: (query: ListQuery) => invoke<ListResult>("tickets_list", { query }),
  ticket: (id: number) => invoke<TicketDetail>("ticket_detail", { id }),
  createTicket: (input: Record<string, string>) => invoke<number>("ticket_create", { input }),
  updateTicket: (id: number, patch: Record<string, unknown>) => invoke<Ticket>("ticket_update", { id, patch }),
  setField: (id: number, key: string, value: string | null, verified: boolean) => invoke<TicketField>("ticket_set_field", { id, key, value, verified }),
  setIssues: (id: number, issues: string[]) => invoke<IssueRow[]>("ticket_set_issues", { id, issues }),
  addSchool: (id: number) => invoke<number>("ticket_add_school", { id }),
  removeSchool: (id: number, slot: number) => invoke<void>("ticket_remove_school", { id, slot }),
  reopen: (id: number) => invoke<Ticket>("ticket_reopen", { id }),
  complete: (id: number) => invoke<MarkDoneResult>("ticket_complete", { id }),
  archiveOutlook: (id: number) => invoke<CompletionOutcome>("ticket_archive_outlook", { id }),
  claim: (id: number, takeover: boolean) => invoke<Ticket>("ticket_claim", { id, takeover }),
  release: (id: number) => invoke<Ticket>("ticket_release", { id }),
  flagReview: (id: number, reason: string) => invoke<Ticket>("ticket_flag_review", { id, reason }),
  resume: (id: number) => invoke<Ticket>("ticket_resume", { id }),
  join: (id: number, target: string) => invoke<Ticket>("ticket_join", { id, target }),
  conflictRetry: (conflictId: string) => invoke<void>("conflict_retry", { conflictId }),
  prompt: (id: number) => invoke<string>("prompt_get", { id }),
  promptCopied: (id: number, text: string) => invoke<string>("prompt_copied", { id, text }),
  promptDefault: () => invoke<string>("prompt_default_instructions"),
  openReply: (ticketId: number, replyAll: boolean, messageId: number | null = null) =>
    invoke<{ opened: boolean; reply_all: boolean; message_id: number; relocated: boolean }>("outlook_open_reply", { ticketId, messageId, replyAll }),
  openNewMessage: (ticketId: number) => invoke<void>("outlook_open_new", { ticketId }),
  validateFolder: (storeId: string, entryId: string) =>
    invoke<{ entry_id: string; store_id: string; path: string; name: string; mailbox: string }>("outlook_validate_folder", { storeId, entryId }),
  refreshMessage: (messageId: number) => invoke<string>("message_refresh", { messageId }),
  startWork: (id: number) => invoke<Ticket>("ticket_start_work", { id }),

  staffList: (includeInactive: boolean) => invoke<Staff[]>("staff_list", { includeInactive }),
  staffSave: (input: StaffInput) => invoke<Staff>("staff_save", { input }),
  staffSetActive: (id: number, active: boolean) => invoke<Staff>("staff_set_active", { id, active }),
  currentStaff: () => invoke<number | null>("staff_current_get"),
  setCurrentStaff: (id: number | null) => invoke<void>("staff_current_set", { id }),
  accounts: () => invoke<{ accounts: { display_name: string; smtp: string }[] }>("outlook_list_accounts"),
  assignPreview: (ticketIds: number[], staffId: number) => invoke<AssignPreview[]>("assign_preview", { ticketIds, staffId }),
  assign: (ticketIds: number[], staffId: number, note: string) => invoke<AssignOutcome[]>("tickets_assign", { ticketIds, staffId, note }),
  assignmentRetry: (ticketId: number) => invoke<AssignOutcome>("assignment_retry", { ticketId }),
  notificationRetry: (opId: number) => invoke<[string, string | null]>("notification_retry", { opId }),
  notificationCheck: (opId: number) => invoke<string>("notification_check", { opId }),
  notificationResend: (opId: number) => invoke<[string, string | null]>("notification_resend", { opId }),
  scopeStatus: () => invoke<ScopeStatus>("scope_status"),
  overview: (doneFrom: string, doneTo: string) => invoke<Overview>("overview_get", { doneFrom, doneTo }),

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
  folders: (storeId: string | null, sharedMailbox: string | null, rootEntryId: string | null = null) =>
    invoke<{ store_id: string; root: FolderNode }>("outlook_list_folders", { storeId, sharedMailbox, rootEntryId }),
  syncNow: () => invoke<{ report: IngestReport; warnings: string[]; folders: number }>("sync_now"),
  syncStatus: () => invoke<SyncStatus>("sync_status"),

  settings: () => invoke<Settings>("settings_get"),
  saveSettings: (settings: Settings) => invoke<Settings>("settings_save", { settings }),
  report: () => invoke<Report>("report_get"),
  exportCsv: (path: string, status: string) => invoke<number>("export_csv", { path, status }),
  backup: (path: string) => invoke<void>("db_backup", { path }),
  restore: (path: string) => invoke<string>("db_restore", { path }),
  appInfo: () => invoke<AppInfo>("app_info"),

  sharedStatus: () => invoke<SharedStatus>("shared_status"),
  sharedTest: () => invoke<SharedStatus>("shared_test_connection"),
  sharedSyncNow: () => invoke<SharedStatus>("shared_sync_now"),
  sharedSaveConfig: (mode: string, path: string, minutes: number | null) => invoke<Settings>("shared_save_config", { mode, path, minutes }),
  sharedReleaseHeld: () => invoke<number>("shared_release_held"),
  sharedDiscardHeld: () => invoke<number>("shared_discard_held"),
  sharedSwitchDatabase: (databaseId: string) => invoke<number>("shared_switch_database", { databaseId }),
  sharedImportPreview: () => invoke<ImportPreview>("shared_import_preview"),
  sharedImportRun: (limit: number | null) => invoke<number>("shared_import_run", { limit }),
  sharedCreateDatabase: (path: string, name: string, environment: string) => invoke<DbIdentity>("shared_create_database", { path, name, environment }),
  notesAdd: (ticketId: number, body: string) => invoke<Note>("notes_add", { ticketId, body }),
  notesEdit: (noteId: string, body: string) => invoke<void>("notes_edit", { noteId, body }),
  notesDelete: (noteId: string) => invoke<void>("notes_delete", { noteId }),
  conflicts: (ticketId: number | null, includeResolved: boolean) => invoke<LocalConflict[]>("conflicts_list", { ticketId, includeResolved }),
  conflictAcceptShared: (conflictId: string) => invoke<void>("conflict_accept_shared", { conflictId }),
  diagRecent: (minSeverity: string, limit: number) => invoke<DiagEvent[]>("diag_recent", { minSeverity, limit }),
};

// ------------------------------------------------------------ helpers

/** UTC instant of local midnight at the start of a "YYYY-MM-DD" date (dayOffset 1 = the next day). */
export function localDayStart(date: string, dayOffset = 0): string {
  const m = date.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!m) return "";
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]) + dayOffset, 0, 0, 0).toISOString();
}

/** Put text on the clipboard without any manual selection. Throws if both methods fail. */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.left = "-9999px";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    if (!ok) throw new Error("The clipboard is not available.");
  }
}

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
