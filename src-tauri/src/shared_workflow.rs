//! Shared workflow rules (Claim, Start Work, Review, Mark Done, reopening on new replies).
//!
//! P0.1 provides the extension point only: the storage/sync foundation accepts no workflow operations
//! yet, so unknown workflow types are rejected and attached emails never change the workflow status.

use crate::shared_apply::{EmailRec, OpResult, TicketCore, WorkflowTx};
use crate::shared_store::SResult;

/// Apply a workflow operation. Returns false when the type is not a workflow operation.
pub fn apply_workflow(_tx: &mut WorkflowTx<'_, '_>, _op_type: &str, _ticket_id: &str, _res: &mut OpResult) -> SResult<bool> {
    Ok(false)
}

/// Called after an email was attached to a case inside the same transaction.
pub fn after_email_attached(_tx: &mut WorkflowTx<'_, '_>, _ticket_id: &str, _email_id: &str, _email: &EmailRec) -> SResult<()> {
    Ok(())
}

/// P0.1: workflow status and owner are not yet managed through the shared database (a pulled case keeps
/// the local status/owner it already has). P0.2 turns this on together with the workflow operations.
pub const WORKFLOW_SYNCED: bool = false;

/// Local ticket keys changed by a workflow operation type (protected from being overwritten by a pull).
pub fn touched_by(_op_type: &str) -> &'static [&'static str] {
    &[]
}

/// Mirror the shared workflow status/owner into the local ticket (P0.2).
pub fn mirror_status(_conn: &rusqlite::Connection, _tid: i64, _core: &TicketCore, _owner_local: Option<i64>) -> crate::error::AppResult<()> {
    Ok(())
}
