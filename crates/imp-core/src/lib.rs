#![recursion_limit = "256"]

pub mod agent;
pub mod builder;
pub(crate) mod child_process;
pub mod code_index;
pub mod codeintel;
pub mod compaction;
pub mod config;
pub mod context;
pub mod context_prefill;
pub mod contracts;
pub mod error;
pub mod error_display;
pub mod eval_candidate;
pub mod eval_candidate_closeout;
pub mod evidence;
mod guardrail_execution;
pub mod guardrails;
pub mod hooks;
pub mod imp_session;
pub mod import;
pub mod learning;
pub mod managed_workspace;
pub mod memory;
pub mod policy;
pub mod process;
pub mod reference_monitor;
pub mod repo_index;
pub mod repo_intelligence;
pub mod resources;
pub mod retry;
pub mod roles;
pub mod run_evidence;
pub mod runtime;
pub mod sdk;
pub mod session;
pub mod session_index;
pub mod soul;
pub mod storage;
pub mod system_prompt;
pub mod tools;
pub mod trace;
pub mod trust;
pub mod ui;
pub mod usage;
pub mod workflow;
pub mod workflow_review;

pub use agent::{RecoveryCheckpoint, RecoveryCheckpointKind, TimingEvent, TimingStage};
pub use error::{Error, Result};
pub use error_display::format_error_for_display;
pub use imp_llm::{
    CancellationMode, ContinuationMode, PersistentSessionMode, ResumabilityMode,
    TransportCapabilities,
};
pub use sdk::*;
pub use workflow_review::{
    TurnWorkflowReview, WorkflowReviewState, WorkflowReviewUnitKind, WorkflowUnitRef,
};

// Re-export imp-llm for downstream crates
pub use imp_llm;
