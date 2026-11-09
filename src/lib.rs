//! Shared types and compatibility exports for Due.
//!
//! New Rust consumers can depend on the `due-core` crate directly.

#[doc(hidden)]
pub use due_core::diagnostics;
pub use due_core::{result, types};

#[cfg(feature = "core")]
pub mod core {
    pub use due_core::*;
}

// Temporary compatibility boundary for the existing single-session adapter.
#[cfg(feature = "rpc")]
#[doc(hidden)]
pub use due_core::legacy as session;
