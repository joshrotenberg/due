//! Scoped, concurrent delegation through native resident wrapper transports.
//!
//! This spike implements the five functions without MCP, raw RPC or an
//! interpreter. Call [`Engine::shutdown`] before dropping the runtime. A
//! wait timeout leaves work running; an assignment timeout stops that run.
//!
//! ```no_run
//! use due::core::{Assignment, Engine, Limits, Provider, WaitOptions, WorkerOptions};
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let engine = Engine::new(Limits::default())?;
//! let worker = WorkerOptions::new(Provider::Claude, std::env::current_dir()?);
//! let run = engine.spawn(Assignment::new(worker, "Review the parser. Do not edit.")).await?;
//! let results = engine.wait(&[run], WaitOptions::default()).await?;
//! // Inspect execution status and independently verify effects before acceptance.
//! println!("{:?}", results.runs[0].result);
//! engine.shutdown().await?;
//! # Ok(())
//! # }
//! ```

mod engine;
mod types;

pub use crate::types::{Effort, Provider};
pub use engine::Engine;
pub use types::*;
