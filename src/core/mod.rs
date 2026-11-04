//! Scoped, concurrent delegation through native resident wrapper transports.
//!
//! This spike adds explicit interaction responses and full-item streams to the
//! five-function interface, without MCP, raw RPC or an
//! interpreter. Call [`Engine::shutdown`] before dropping the runtime. A
//! wait timeout leaves work running; an assignment timeout stops that run.
//!
//! ```no_run
//! use due::core::{Assignment, Engine, InteractionMode, Limits, Provider, WaitOptions, WorkerOptions};
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let engine = Engine::new(Limits::default())?;
//! let mut worker = WorkerOptions::new(Provider::Claude, std::env::current_dir()?);
//! // This unattended example fails explicitly if native approval is needed.
//! worker.interaction_mode = InteractionMode::Reject;
//! let run = engine.spawn(Assignment::new(worker, "Review the parser. Do not edit.")).await?;
//! let results = engine.wait(&[run], WaitOptions::default()).await?;
//! // Inspect execution status and independently verify effects before acceptance.
//! println!("{:?}", results.runs[0].result);
//! engine.shutdown().await?;
//! # Ok(())
//! # }
//! ```

//! For interactive callers, keep the default manual mode, consume
//! [`Engine::subscribe`] items, and pass each caller-chosen native JSON response
//! to [`Engine::respond`]. Pending tokens are also available in
//! [`RunSnapshot::interactions`]. See the core spike guide for provider examples.

mod engine;
mod stream;
mod types;

pub use crate::types::{Effort, Provider};
pub use engine::Engine;
pub use stream::{SessionItem, SessionStream, StreamError};
pub use types::*;
