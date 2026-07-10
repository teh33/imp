//! Durable, bounded child-agent execution for imp.
//!
//! `imp-core` authorizes the model-facing contract. This crate owns durable
//! child RPC-process and session lifecycle.

mod events;
mod executor;
mod model;
mod store;
mod worker;

pub use executor::Executor;
pub use model::{ArtifactPaths, Error, LaunchRequest, Record, Result, Status};
pub use worker::run_worker;
