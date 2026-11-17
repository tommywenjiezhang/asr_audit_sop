//! ASR/Audit Support Desk - Tauri backend.

pub mod analyzer;
pub mod assignment;
pub mod catalog;
#[cfg(feature = "app")]
pub mod commands;
pub mod completion;
pub mod db;
#[cfg(test)]
mod demo;
pub mod drafts;
pub mod error;
pub mod export;
pub mod outlook;
pub mod overview;
pub mod prompt;
pub mod render;
pub mod scope;
pub mod search;
pub mod settings;
pub mod staff;
#[cfg(feature = "app")]
pub mod sync;
pub mod sync_core;
pub mod templates_store;
pub mod tickets;

#[cfg(feature = "app")]
mod shell;
#[cfg(feature = "app")]
pub use shell::*;
