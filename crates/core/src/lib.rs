//! Unlost engine.
//!
//! Platform code (raw device handles, IOCTLs, WMI, named pipes) and the recovery workflow live
//! here; the format parsers live in `unlost-formats`, which forbids `unsafe`. The Tauri shell
//! only translates between this crate and the UI.
//!
//! Process model: the UI process stays a normal user. Anything that needs administrator rights
//! (raw volume reads, shadow copies) runs in the broker, the same exe started through UAC, which
//! talks to the UI over a named pipe only the UI's user can open. The broker never writes files:
//! it streams bytes, and the UI process writes them to the destination the guard approved.

pub mod broker;
pub mod device;
pub mod disks;
pub mod engine;
pub mod error;
pub mod guard;
pub mod job;
pub mod pipe;
pub mod protocol;
pub mod recycle;
pub mod session;
pub mod settings;
pub mod shadow;
pub mod sources;
pub mod system;
mod winstr;
pub mod wmi;
pub mod writer;

pub use error::{Error, Result};
pub use job::{Job, ProgressSnapshot};
