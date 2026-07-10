//! Agent-native audit models, configuration, and reporting for imp.
//!
//! This crate owns pure audit data structures and validation logic. Runtime
//! integration with tools, hooks, workflows, and subagents belongs in
//! `imp-core`.

pub mod config;
pub mod doctor;
pub mod error;
pub mod model;
pub mod requirements;
pub mod summarize;

pub use config::{AuditConfig, CheckConfig, ConfigDefaults, InstallRecipe, ToolRequirement};
pub use doctor::{doctor_report_for_profile, format_doctor_report, DoctorReport, DoctorSummary};
pub use error::{AuditError, Result};
pub use model::{
    AuditCheckRun, AuditFinding, AuditLocation, AuditReport, AuditSeverity, CheckStatus,
};
pub use requirements::{
    availability_for_profile, PathRequirementProbe, RequirementAvailability, RequirementProbe,
    RequirementStatus,
};
pub use summarize::{summarize_report, ReportSummary};
