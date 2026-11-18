# Changelog

## 1.2.0 — 2026-11-18

- Complete shared-mailbox child-folder discovery, including nested folders recovered from the Outlook MAPI hierarchy when the ordinary Outlook folder collection hides them.
- Support monitoring the top folder only, all descendants, or selected descendants with optional recursive children.
- Preserve completion-folder exclusions across synchronization and scope reconciliation.
- Keep folder discovery read-only and surface inaccessible subfolders as diagnostics instead of silently skipping them.
- Verified with 50 Python worker tests, 150 Rust tests, a zero-warning Svelte check, and the recorded 24-scenario dual-instance shared-database run.
- Removed the Svelte state-initialization warning in the merge dialog.
