//! ASR/Audit Support Desk - Tauri backend.

pub mod analyzer;
pub mod catalog;
#[cfg(feature = "app")]
pub mod commands;
pub mod db;
pub mod drafts;
pub mod error;
pub mod export;
pub mod outlook;
pub mod render;
pub mod settings;
#[cfg(feature = "app")]
pub mod sync;
pub mod sync_core;
pub mod templates_store;
pub mod tickets;

#[cfg(feature = "app")]
mod shell;
#[cfg(feature = "app")]
pub use shell::*;
