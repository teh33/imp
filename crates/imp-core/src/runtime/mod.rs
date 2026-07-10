//! Versioned runtime events and deterministic execution-state reduction.

mod event;
mod reducer;
mod session_projection;
mod state;

pub use event::{RuntimeAssistantDelta, RuntimeEvent, RuntimeEventKind, RUNTIME_SCHEMA_VERSION};
pub use reducer::{RuntimeApplyOutcome, RuntimeStateAccumulator};
pub use session_projection::RuntimeSessionProjection;
pub use state::*;

#[cfg(test)]
mod runtime_events;
