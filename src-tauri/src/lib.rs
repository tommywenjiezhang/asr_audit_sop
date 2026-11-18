//! ASR/Audit Support Desk - Tauri backend.

pub mod analyzer;
pub mod assignment;
pub mod bulk;
pub mod cases;
pub mod catalog;
#[cfg(feature = "app")]
pub mod commands;
pub mod completion;
pub mod db;
pub mod diag;
#[cfg(test)]
mod demo;
pub mod drafts;
pub mod error;
pub mod export;
pub mod folders;
pub mod notes;
pub mod outlook;
pub mod outlook_mock;
pub mod overview;
pub mod profile;
pub mod prompt;
pub mod render;
pub mod scope;
pub mod search;
pub mod settings;
pub mod shared_admin;
pub mod shared_apply;
pub mod shared_cases;
pub mod shared_outlook;
pub mod shared_store;
pub mod shared_sync;
pub mod shared_workflow;
pub mod staff;
#[cfg(feature = "app")]
pub mod sync;
pub mod sync_core;
pub mod sync_queue;
pub mod team;
pub mod templates_store;
pub mod tickets;
pub mod workflow;

#[cfg(feature = "app")]
mod shell;
#[cfg(feature = "app")]
pub use shell::*;
