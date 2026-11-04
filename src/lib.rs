//! Experimental transport-independent delegation core.
//!
//! Enable `core` without `rpc` or `harness` to use the library without a wire
//! adapter or embedded interpreter. Existing CLI contracts remain independent.

#[doc(hidden)]
pub mod diagnostics;
pub mod result;
pub mod types;

#[cfg(feature = "core")]
pub mod core;

// Temporary compatibility boundary for the existing RPC adapter. New clients
// use core::Engine, not this single-session manager.
#[cfg(feature = "core")]
#[doc(hidden)]
pub mod session;

#[cfg(feature = "core")]
mod providers {
    pub(crate) mod provenance;
}
