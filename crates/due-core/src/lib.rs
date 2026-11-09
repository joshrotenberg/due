//! Scoped, multi-provider delegation over native Claude and Codex wrappers.
//!
//! The default `resident` feature provides an asynchronous engine. Disabling it
//! leaves the shared provider and result types available to foreground callers.

#[doc(hidden)]
pub mod diagnostics;
#[doc(hidden)]
pub mod provenance;
pub mod result;
pub mod types;

pub use types::{Effort, Provider};

#[cfg(feature = "resident")]
mod core;
#[cfg(feature = "resident")]
pub use core::*;

// Temporary source-level bridge for Due's existing single-session RPC adapter.
#[cfg(feature = "resident")]
mod session;
#[cfg(feature = "resident")]
#[doc(hidden)]
pub mod legacy {
    pub use crate::session::{Manager, OpenOptions, PushQueue};
}
