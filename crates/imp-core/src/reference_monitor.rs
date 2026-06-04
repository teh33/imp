use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{AgentMode, PolicyAction, PolicyConfig};
use crate::policy::{RunPolicy, ToolPolicyDecision as RunToolDecision, WritePolicyDecision};
use crate::workflow::{RiskLevel, WorkflowContract, WorkflowType, WorkspaceScope};
use crate::{guardrails::GuardrailLevel, hooks::HookResult, trust::Provenance};

/// Central policy boundary for deciding whether a tool/action may proceed.
///
/// This initial type is a model-only facade. Later tasks route tool execution
/// through it while preserving current behavior.
#[derive(Debug, Clone, Default)]
pub struct ReferenceMonitor;

impl ReferenceMonitor {
    pub fn check_tool_action(
        &self,
        context: &ToolPolicyContext,
        run_policy: &RunPolicy,
    ) -> ToolPolicyDecision {
        if !context.mode.allows_tool(&context.tool_name) {
            return ToolPolicyDecision::Deny {
                reason: PolicyReason::new(
                    PolicySource::AgentMode,
                    "agent_mode_tool_denied",
                    format!(
                        "Tool `{}` is not available in {:?} mode.",
                        context.tool_name, context.mode
                    ),
                ),
            };
        }

        if context.metadata.extension && context.metadata.secrets {
            return ToolPolicyDecision::Deny {
                reason: PolicyReason::new(
                    PolicySource::ToolManifest,
                    "extension_secret_denied",
                    "TypeScript extension tools cannot receive secrets until explicit secret grants are implemented.",
                ),
            };
        }

        match run_policy.check_tool(&context.tool_name) {
            RunToolDecision::Allowed => {}
            RunToolDecision::Denied(message) => {
                return ToolPolicyDecision::Deny {
                    reason: PolicyReason::new(
                        PolicySource::RunPolicy,
                        "run_policy_tool_denied",
                        message,
                    ),
                };
            }
        }

        if context.metadata.workspace_write
            || matches!(
                context.action_kind,
                ToolActionKind::Write | ToolActionKind::Edit
            )
        {
            if let (Some(cwd), Some(path)) = (context.cwd.as_deref(), context.resource_scope.path())
            {
                match run_policy.check_write_path(cwd, path) {
                    WritePolicyDecision::Allowed => {}
                    WritePolicyDecision::Denied(message) => {
                        return ToolPolicyDecision::Deny {
                            reason: PolicyReason::new(
                                PolicySource::RunPolicy,
                                "run_policy_write_path_denied",
                                message,
                            ),
                        };
                    }
                }
            }
        }

        if context.metadata.extension && context.metadata.network {
            return self.apply_policy_action(
                context.policy.extension_network,
                "policy_extension_network_requires_approval",
                "Extension network capability requires approval by policy.",
                "policy_extension_network_denied",
                "Extension network capability is denied by policy.",
            );
        }

        let trust_decision = self.check_trust_escalation(context);
        if !trust_decision.is_allowed() {
            return trust_decision;
        }

        let config_decision = self.check_config_policy(context);
        if !config_decision.is_allowed() {
            return config_decision;
        }

        ToolPolicyDecision::allow()
    }

    pub fn record(
        &self,
        context: &ToolPolicyContext,
        decision: ToolPolicyDecision,
        details: Value,
    ) -> PolicyTraceRecord {
        let mut record = PolicyTraceRecord::from_context(context, decision);
        record.details = details;
        record
    }

    pub fn ask_user_record(
        &self,
        context: &ToolPolicyContext,
        message: impl Into<String>,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            ToolPolicyDecision::AskUser {
                reason: PolicyReason::new(
                    PolicySource::WorkflowAutonomy,
                    "ask_user_required",
                    message.into(),
                ),
            },
            serde_json::json!({ "unsupported_decision": "ask_user" }),
        )
    }

    pub fn dry_run_only_record(
        &self,
        context: &ToolPolicyContext,
        message: impl Into<String>,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            ToolPolicyDecision::DryRunOnly {
                reason: PolicyReason::new(
                    PolicySource::ToolManifest,
                    "dry_run_required",
                    message.into(),
                ),
            },
            serde_json::json!({ "unsupported_decision": "dry_run_only" }),
        )
    }

    pub fn sandbox_only_record(
        &self,
        context: &ToolPolicyContext,
        message: impl Into<String>,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            ToolPolicyDecision::SandboxOnly {
                reason: PolicyReason::new(
                    PolicySource::ToolManifest,
                    "sandbox_required",
                    message.into(),
                ),
            },
            serde_json::json!({ "unsupported_decision": "sandbox_only" }),
        )
    }

    pub fn require_verification_record(
        &self,
        context: &ToolPolicyContext,
        message: impl Into<String>,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            ToolPolicyDecision::RequireVerification {
                reason: PolicyReason::new(
                    PolicySource::WorkflowAutonomy,
                    "require_verification",
                    message.into(),
                ),
            },
            serde_json::json!({ "unsupported_decision": "require_verification" }),
        )
    }

    pub fn hook_blocked_record(
        &self,
        context: &ToolPolicyContext,
        hook: &HookResult,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            ToolPolicyDecision::Deny {
                reason: PolicyReason::new(
                    PolicySource::Hook,
                    "hook_blocked",
                    hook.reason
                        .clone()
                        .unwrap_or_else(|| "Hook blocked tool execution".into()),
                ),
            },
            serde_json::json!({ "hook": { "reason": hook.reason, "block": hook.block } }),
        )
    }

    pub fn bash_equivalent_record(
        &self,
        context: &ToolPolicyContext,
        hint: &str,
    ) -> PolicyTraceRecord {
        let mut reason = PolicyReason::new(
            PolicySource::BashEquivalent,
            "policy_blocked",
            hint.to_string(),
        );
        reason.suggestion = Some(
            "Use the native workflow tool instead of shelling out to legacy workflow shell".into(),
        );
        self.record(
            context,
            ToolPolicyDecision::Deny { reason },
            serde_json::json!({ "bash_equivalent_hint": hint }),
        )
    }

    pub fn repeated_call_record(
        &self,
        context: &ToolPolicyContext,
        blocked: bool,
        message: &str,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            if blocked {
                ToolPolicyDecision::Deny {
                    reason: PolicyReason::new(
                        PolicySource::RepeatedCall,
                        "repeated_tool_call_blocked",
                        message,
                    ),
                }
            } else {
                ToolPolicyDecision::Allow {
                    reasons: vec![PolicyReason::new(
                        PolicySource::RepeatedCall,
                        "repeated_tool_call_warned",
                        message,
                    )],
                }
            },
            serde_json::json!({ "repeated_call": { "blocked": blocked, "message": message } }),
        )
    }

    pub fn validation_error_record(
        &self,
        context: &ToolPolicyContext,
        message: &str,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            ToolPolicyDecision::Deny {
                reason: PolicyReason::new(PolicySource::Schema, "validation_error", message),
            },
            serde_json::json!({ "validation_error": message }),
        )
    }

    pub fn dangerous_grant_required_record(
        &self,
        context: &ToolPolicyContext,
        rail: DangerousRail,
    ) -> PolicyTraceRecord {
        self.record(
            context,
            ToolPolicyDecision::Deny {
                reason: PolicyReason::new(
                    PolicySource::DangerousGrant,
                    rail.reason_code(),
                    rail.message(),
                ),
            },
            serde_json::json!({ "dangerous_rail": rail }),
        )
    }

    pub fn guardrail_record(
        &self,
        context: &ToolPolicyContext,
        level: GuardrailLevel,
        failed: bool,
        message: &str,
    ) -> PolicyTraceRecord {
        let decision = if failed && matches!(level, GuardrailLevel::Enforce) {
            ToolPolicyDecision::Deny {
                reason: PolicyReason::new(PolicySource::Guardrail, "guardrail_enforced", message),
            }
        } else {
            ToolPolicyDecision::Allow {
                reasons: vec![PolicyReason::new(
                    PolicySource::Guardrail,
                    if failed {
                        "guardrail_advisory_failed"
                    } else {
                        "guardrail_passed"
                    },
                    message,
                )],
            }
        };
        self.record(
            context,
            decision,
            serde_json::json!({ "guardrail": { "level": format!("{level:?}"), "failed": failed, "message": message } }),
        )
    }
    pub fn evaluate(
        &self,
        context: &ToolPolicyContext,
        run_policy: &RunPolicy,
    ) -> PolicyTraceRecord {
        let decision = self.check_tool_action(context, run_policy);
        PolicyTraceRecord::from_context(context, decision)
    }

    fn check_trust_escalation(&self, context: &ToolPolicyContext) -> ToolPolicyDecision {
        if context.supporting_provenance.is_empty() {
            return ToolPolicyDecision::allow();
        }
        if context
            .supporting_provenance
            .iter()
            .any(|provenance| !provenance.is_low_trust())
        {
            return ToolPolicyDecision::allow();
        }
        if !context.is_high_risk_action() {
            return ToolPolicyDecision::allow();
        }

        let source_summary = context
            .supporting_provenance
            .iter()
            .filter_map(|provenance| provenance.origin.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
        let mut reason = PolicyReason::new(
            PolicySource::TrustLabel,
            "low_trust_escalation_denied",
            if source_summary.is_empty() {
                "Low-trust context cannot authorize this high-risk action.".to_string()
            } else {
                format!(
                    "Low-trust context cannot authorize this high-risk action. Source: {source_summary}"
                )
            },
        );
        reason.suggestion = Some(
            "Ask the user to explicitly authorize the action or provide trusted workflow policy."
                .into(),
        );
        if context.action_kind == ToolActionKind::Network {
            ToolPolicyDecision::AskUser { reason }
        } else {
            ToolPolicyDecision::Deny { reason }
        }
    }

    fn check_config_policy(&self, context: &ToolPolicyContext) -> ToolPolicyDecision {
        if !context.policy.allow_side_effects
            && !matches!(
                context.action_kind,
                ToolActionKind::Read | ToolActionKind::Search | ToolActionKind::AskUser
            )
        {
            return ToolPolicyDecision::Deny {
                reason: PolicyReason::new(
                    PolicySource::ConfigPolicy,
                    "policy_side_effect_denied",
                    "Side-effecting tool actions are denied by policy.",
                ),
            };
        }

        if context.metadata.secrets || context.action_kind == ToolActionKind::Secret {
            return self.apply_policy_action(
                context.policy.secrets,
                "policy_secret_requires_approval",
                "Secret access requires approval by policy.",
                "policy_secret_denied",
                "Secret reveal or direct secret access is denied by policy.",
            );
        }

        if context.policy.deny_approval_required
            && (context.metadata.requires_approval || context.metadata.default_requires_approval)
        {
            return ToolPolicyDecision::Deny {
                reason: PolicyReason::new(
                    PolicySource::ConfigPolicy,
                    "policy_approval_required_denied",
                    "This action would require approval, but approval-required actions are denied by policy.",
                ),
            };
        }

        if context.metadata.network
            || matches!(context.resource_scope, ResourceScope::Network { .. })
        {
            return self.apply_policy_action(
                context.policy.network,
                "policy_network_requires_approval",
                "Network actions require approval by policy.",
                "policy_network_denied",
                "Network actions are denied by policy.",
            );
        }

        if context.metadata.workspace_write
            || matches!(
                context.action_kind,
                ToolActionKind::Write | ToolActionKind::Edit
            )
        {
            let action = if self.is_outside_workspace(context) {
                context.policy.outside_workspace_writes
            } else {
                context.policy.workspace_writes
            };
            return self.apply_policy_action(
                action,
                "policy_write_requires_approval",
                "File writes require approval by policy.",
                "policy_write_denied",
                "File writes are denied by policy.",
            );
        }

        if context.action_kind == ToolActionKind::Execute || context.metadata.external_side_effect {
            return self.apply_policy_action(
                context.policy.shell,
                "policy_shell_requires_approval",
                "Shell or external side-effect actions require approval by policy.",
                "policy_shell_denied",
                "Shell or external side-effect actions are denied by policy.",
            );
        }

        ToolPolicyDecision::allow()
    }

    fn apply_policy_action(
        &self,
        action: PolicyAction,
        ask_code: &'static str,
        ask_message: &'static str,
        deny_code: &'static str,
        deny_message: &'static str,
    ) -> ToolPolicyDecision {
        match action {
            PolicyAction::Allow => ToolPolicyDecision::allow(),
            PolicyAction::Ask => self.ask_user_decision(ask_code, ask_message),
            PolicyAction::Deny => ToolPolicyDecision::Deny {
                reason: PolicyReason::new(PolicySource::ConfigPolicy, deny_code, deny_message),
            },
        }
    }

    fn ask_user_decision(&self, code: &'static str, message: &'static str) -> ToolPolicyDecision {
        ToolPolicyDecision::AskUser {
            reason: PolicyReason::new(PolicySource::ConfigPolicy, code, message),
        }
    }

    fn is_outside_workspace(&self, context: &ToolPolicyContext) -> bool {
        let Some(cwd) = context.cwd.as_deref() else {
            return false;
        };
        let Some(path) = context.resource_scope.path() else {
            return false;
        };
        if path.as_os_str().is_empty() {
            return false;
        }
        !path.starts_with(cwd)
    }
}

/// Context supplied to the reference monitor for a single tool/action decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolPolicyContext {
    pub run_id: Option<String>,
    pub workflow_id: Option<String>,
    pub turn: Option<u32>,
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub action_kind: ToolActionKind,
    pub args: Value,
    pub args_hash: Option<String>,
    pub cwd: Option<PathBuf>,
    pub resource_scope: ResourceScope,
    pub mode: AgentMode,
    pub policy: PolicyConfig,
    pub workflow_type: WorkflowType,
    pub risk_level: RiskLevel,
    pub workspace_scope: WorkspaceScope,
    pub trust_scope: TrustScopeContext,
    pub trust_labels: Vec<String>,
    pub supporting_provenance: Vec<Provenance>,
    pub metadata: ToolMetadata,
}

impl ToolPolicyContext {
    pub fn new(tool_name: impl Into<String>, action_kind: ToolActionKind) -> Self {
        let tool_name = tool_name.into();
        Self {
            metadata: ToolMetadata::new(tool_name.clone(), action_kind),
            run_id: None,
            workflow_id: None,
            turn: None,
            tool_call_id: None,
            tool_name,
            action_kind,
            args: Value::Null,
            args_hash: None,
            cwd: None,
            resource_scope: ResourceScope::default(),
            mode: AgentMode::default(),
            policy: PolicyConfig::default(),
            workflow_type: WorkflowType::default(),
            risk_level: RiskLevel::default(),
            workspace_scope: WorkspaceScope::default(),
            trust_scope: TrustScopeContext::default(),
            trust_labels: Vec::new(),
            supporting_provenance: Vec::new(),
        }
    }
    pub fn apply_workflow_contract(&mut self, contract: &WorkflowContract) {
        self.workflow_id = contract
            .id
            .clone()
            .or_else(|| contract.workflow_unit_ref.clone());
        self.workflow_type = contract.workflow_type;
        self.risk_level = contract.risk_level;
        self.workspace_scope = contract.workspace_scope.clone();
        self.trust_scope = TrustScopeContext::from_contract(contract);
        self.trust_labels = self.trust_scope.labels();
    }

    pub fn with_workflow_contract(mut self, contract: &WorkflowContract) -> Self {
        self.apply_workflow_contract(contract);
        self
    }
    pub fn with_supporting_provenance(mut self, provenance: Provenance) -> Self {
        self.supporting_provenance.push(provenance);
        self
    }

    fn is_high_risk_action(&self) -> bool {
        self.metadata.workspace_write
            || self.metadata.external_side_effect
            || self.metadata.network
            || self.metadata.secrets
            || matches!(
                self.action_kind,
                ToolActionKind::Write
                    | ToolActionKind::Edit
                    | ToolActionKind::Execute
                    | ToolActionKind::Network
                    | ToolActionKind::Git
                    | ToolActionKind::Workflow
                    | ToolActionKind::Secret
                    | ToolActionKind::Extension
            )
            || self.resource_scope.path().is_some_and(|path| {
                self.cwd
                    .as_deref()
                    .is_some_and(|cwd| !path.starts_with(cwd))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustScopeContext {
    pub allow_external_context: bool,
    pub allow_durable_memory_writes: bool,
    pub low_trust_requires_review: bool,
}

impl TrustScopeContext {
    pub fn from_contract(contract: &WorkflowContract) -> Self {
        Self {
            allow_external_context: contract.trust_scope.allow_external_context,
            allow_durable_memory_writes: contract.trust_scope.allow_durable_memory_writes,
            low_trust_requires_review: contract.trust_scope.low_trust_requires_review,
        }
    }

    pub fn labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        labels.push(
            if self.allow_external_context {
                "external-context-allowed"
            } else {
                "external-context-blocked"
            }
            .to_string(),
        );
        labels.push(
            if self.allow_durable_memory_writes {
                "durable-memory-writes-allowed"
            } else {
                "durable-memory-writes-blocked"
            }
            .to_string(),
        );
        labels.push(
            if self.low_trust_requires_review {
                "low-trust-review-required"
            } else {
                "low-trust-review-not-required"
            }
            .to_string(),
        );
        labels
    }
}

impl Default for TrustScopeContext {
    fn default() -> Self {
        Self {
            allow_external_context: true,
            allow_durable_memory_writes: true,
            low_trust_requires_review: true,
        }
    }
}

/// Coarse kind of action a tool can perform.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolActionKind {
    Read,
    Write,
    Edit,
    Execute,
    Search,
    Network,
    Git,
    Workflow,
    AskUser,
    Secret,
    Extension,
    #[default]
    Unknown,
}

/// Minimal tool manifest subset needed by the reference monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub name: String,
    pub action_kind: ToolActionKind,
    pub readonly: bool,
    pub workspace_write: bool,
    pub external_side_effect: bool,
    pub network: bool,
    pub secrets: bool,
    pub extension: bool,
    pub default_requires_approval: bool,
    pub resource_scopes: Vec<ResourceScope>,
    pub supports_dry_run: bool,
    pub supports_sandbox: bool,
    pub requires_approval: bool,
    pub extension_id: Option<String>,
    pub manifest_version: Option<String>,
}

impl ToolMetadata {
    pub fn new(name: impl Into<String>, action_kind: ToolActionKind) -> Self {
        Self {
            name: name.into(),
            action_kind,
            readonly: matches!(
                action_kind,
                ToolActionKind::Read | ToolActionKind::Search | ToolActionKind::AskUser
            ),
            workspace_write: matches!(action_kind, ToolActionKind::Write | ToolActionKind::Edit),
            external_side_effect: matches!(
                action_kind,
                ToolActionKind::Execute
                    | ToolActionKind::Network
                    | ToolActionKind::Git
                    | ToolActionKind::Workflow
                    | ToolActionKind::Secret
                    | ToolActionKind::Extension
            ),
            network: matches!(action_kind, ToolActionKind::Network),
            secrets: matches!(action_kind, ToolActionKind::Secret),
            extension: matches!(action_kind, ToolActionKind::Extension),
            default_requires_approval: false,
            resource_scopes: Vec::new(),
            supports_dry_run: false,
            supports_sandbox: false,
            requires_approval: false,
            extension_id: None,
            manifest_version: None,
        }
    }

    pub fn resource_scope_for_args(
        &self,
        cwd: Option<&std::path::Path>,
        args: &Value,
    ) -> ResourceScope {
        let path_arg = args
            .get("path")
            .or_else(|| args.get("file"))
            .or_else(|| args.get("directory"))
            .and_then(Value::as_str);
        if let Some(path) = path_arg {
            let path = PathBuf::from(path);
            let path = match cwd {
                Some(cwd) if path.is_relative() => cwd.join(path),
                _ => path,
            };
            return match self.action_kind {
                ToolActionKind::Search => ResourceScope::Directory { path },
                _ => ResourceScope::File { path },
            };
        }
        if self.action_kind == ToolActionKind::Execute {
            if let Some(command) = args.get("command").and_then(Value::as_str) {
                let program = command
                    .split_whitespace()
                    .next()
                    .unwrap_or(command)
                    .to_string();
                return ResourceScope::Command { program };
            }
        }
        if self.action_kind == ToolActionKind::Workflow {
            return ResourceScope::Workflow {
                action: args
                    .get("action")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
        }
        if self.action_kind == ToolActionKind::Network {
            return ResourceScope::Network {
                host: args
                    .get("url")
                    .and_then(Value::as_str)
                    .and_then(extract_host),
            };
        }
        self.resource_scopes
            .first()
            .cloned()
            .unwrap_or(ResourceScope::None)
    }

    pub fn for_tool_name(name: impl Into<String>, readonly: bool) -> Self {
        let name = name.into();
        let mut metadata = Self::new(name.clone(), ToolActionKind::from_tool_name(&name));
        metadata.readonly = readonly || metadata.readonly;
        match name.as_str() {
            "read" => {
                metadata.resource_scopes.push(ResourceScope::File {
                    path: PathBuf::new(),
                });
            }
            "write" | "edit" | "multi_edit" => {
                metadata.workspace_write = true;
                metadata.resource_scopes.push(ResourceScope::File {
                    path: PathBuf::new(),
                });
            }
            "bash" => {
                metadata.external_side_effect = true;
                metadata.resource_scopes.push(ResourceScope::Command {
                    program: String::new(),
                });
            }
            "git" => {
                metadata.external_side_effect = true;
                metadata.workspace_write = true;
            }
            "workflow" => {
                metadata.external_side_effect = true;
                metadata
                    .resource_scopes
                    .push(ResourceScope::Workflow { action: None });
            }
            "web" => {
                metadata.network = true;
                metadata.external_side_effect = true;
                metadata
                    .resource_scopes
                    .push(ResourceScope::Network { host: None });
            }
            "extend" => {
                metadata.workspace_write = true;
                metadata.extension = true;
                metadata.external_side_effect = true;
            }
            name if name.starts_with("lua:") || name.starts_with("extension:") => {
                metadata.extension = true;
                metadata.extension_id = Some(name.to_string());
                metadata.external_side_effect = !metadata.readonly;
            }
            _ => {}
        }
        metadata.default_requires_approval = metadata.external_side_effect && !metadata.readonly;
        metadata
    }
}

impl ToolActionKind {
    pub fn from_tool_name(name: &str) -> Self {
        match name {
            "read" => Self::Read,
            "scan" | "search" | "memory" => Self::Search,
            "write" => Self::Write,
            "edit" | "multi_edit" => Self::Edit,
            "bash" | "shell" => Self::Execute,
            "git" => Self::Git,
            "workflow" => Self::Workflow,
            "web" => Self::Network,
            "ask" | "ask_user" => Self::AskUser,
            "extend" => Self::Extension,
            name if name.starts_with("lua:") || name.starts_with("extension:") => Self::Extension,
            _ => Self::Unknown,
        }
    }
}

impl Default for ToolMetadata {
    fn default() -> Self {
        Self::new("unknown", ToolActionKind::Unknown)
    }
}

/// Resource touched by a tool action.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceScope {
    #[default]
    None,
    File {
        path: PathBuf,
    },
    Directory {
        path: PathBuf,
    },
    Command {
        program: String,
    },
    Network {
        host: Option<String>,
    },
    Workflow {
        action: Option<String>,
    },
    Secret {
        name: Option<String>,
    },
    Extension {
        id: String,
    },
}

impl ResourceScope {
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            ResourceScope::File { path } | ResourceScope::Directory { path } => {
                Some(path.as_path())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DangerousRail {
    SecretExfiltration,
    PrivateKeyRead,
    OutsideWorkspaceDestructiveWrite,
    ForcePush,
    GlobalGitConfigMutation,
    ProductionDeploy,
    CloudResourceDeletion,
    AuditLogDisable,
}

impl DangerousRail {
    pub fn reason_code(self) -> &'static str {
        match self {
            Self::SecretExfiltration => "dangerous_secret_exfiltration",
            Self::PrivateKeyRead => "dangerous_private_key_read",
            Self::OutsideWorkspaceDestructiveWrite => {
                "dangerous_outside_workspace_destructive_write"
            }
            Self::ForcePush => "dangerous_force_push",
            Self::GlobalGitConfigMutation => "dangerous_global_git_config_mutation",
            Self::ProductionDeploy => "dangerous_production_deploy",
            Self::CloudResourceDeletion => "dangerous_cloud_resource_deletion",
            Self::AuditLogDisable => "dangerous_audit_log_disable",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::SecretExfiltration => "Secret exfiltration requires an explicit dangerous grant.",
            Self::PrivateKeyRead => "Reading private keys requires an explicit dangerous grant.",
            Self::OutsideWorkspaceDestructiveWrite => {
                "Destructive writes outside the workspace require an explicit dangerous grant."
            }
            Self::ForcePush => "Force-push requires an explicit dangerous grant.",
            Self::GlobalGitConfigMutation => {
                "Global git config mutation requires an explicit dangerous grant."
            }
            Self::ProductionDeploy => "Production deploys require an explicit dangerous grant.",
            Self::CloudResourceDeletion => {
                "Cloud resource deletion requires an explicit dangerous grant."
            }
            Self::AuditLogDisable => "Disabling audit logs requires an explicit dangerous grant.",
        }
    }
}

/// Stable source/reason metadata for a policy decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyReason {
    pub source: PolicySource,
    pub code: String,
    pub message: String,
    pub suggestion: Option<String>,
}

impl PolicyReason {
    pub fn new(source: PolicySource, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            source,
            code: code.into(),
            message: message.into(),
            suggestion: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    AgentMode,
    RunPolicy,
    WorkflowLoop,
    BashEquivalent,
    RepeatedCall,
    Hook,
    Schema,
    Guardrail,
    ConfigPolicy,
    WorkflowAutonomy,
    TrustLabel,
    ToolManifest,
    DangerousGrant,
    Unknown,
}

/// Decision returned by the monitor for a tool/action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ToolPolicyDecision {
    Allow { reasons: Vec<PolicyReason> },
    Deny { reason: PolicyReason },
    AskUser { reason: PolicyReason },
    DryRunOnly { reason: PolicyReason },
    SandboxOnly { reason: PolicyReason },
    RequireVerification { reason: PolicyReason },
}

impl ToolPolicyDecision {
    pub fn allow() -> Self {
        Self::Allow {
            reasons: Vec::new(),
        }
    }

    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }
}

impl Default for ToolPolicyDecision {
    fn default() -> Self {
        Self::allow()
    }
}

/// Serializable policy record suitable for trace/evidence pipelines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyTraceRecord {
    pub run_id: Option<String>,
    pub workflow_id: Option<String>,
    pub turn: Option<u32>,
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub action_kind: ToolActionKind,
    pub decision: ToolPolicyDecision,
    pub args_hash: Option<String>,
    pub resource_scope: ResourceScope,
    pub workflow_type: WorkflowType,
    pub risk_level: RiskLevel,
    pub trust_scope: TrustScopeContext,
    pub trust_labels: Vec<String>,
    pub details: Value,
}

impl PolicyTraceRecord {
    pub fn from_context(context: &ToolPolicyContext, decision: ToolPolicyDecision) -> Self {
        Self {
            run_id: context.run_id.clone(),
            workflow_id: context.workflow_id.clone(),
            turn: context.turn,
            tool_call_id: context.tool_call_id.clone(),
            tool_name: context.tool_name.clone(),
            action_kind: context.action_kind,
            decision,
            args_hash: context.args_hash.clone(),
            resource_scope: context.resource_scope.clone(),
            workflow_type: context.workflow_type,
            risk_level: context.risk_level,
            trust_scope: context.trust_scope.clone(),
            trust_labels: context.trust_labels.clone(),
            details: Value::Null,
        }
    }
    pub fn to_trace_event(&self, run_id: impl Into<String>) -> crate::trace::TraceEvent {
        let mut event = crate::trace::TraceEvent::new(
            run_id,
            "policy.checked",
            serde_json::json!({
                "tool_name": self.tool_name,
                "action_kind": self.action_kind,
                "decision": self.decision,
                "resource_scope": self.resource_scope_summary(),
                "args_hash": self.args_hash,
                "workflow_type": self.workflow_type,
                "risk_level": self.risk_level,
                "trust_scope": self.trust_scope,
                "trust_labels": self.trust_labels,
                "details": self.details,
            }),
        );
        event.workflow_id = self.workflow_id.clone();
        event.turn = self.turn;
        if let Some(tool_call_id) = &self.tool_call_id {
            event = event.with_tool_call_id(tool_call_id.clone());
        }
        event.redaction.contains_redactions = true;
        if self.args_hash.is_some() {
            event.redaction.content_hash = self.args_hash.clone();
        }
        event
    }

    fn resource_scope_summary(&self) -> Value {
        match &self.resource_scope {
            ResourceScope::None => Value::Null,
            ResourceScope::File { path } => {
                serde_json::json!({ "kind": "file", "path": path.display().to_string() })
            }
            ResourceScope::Directory { path } => {
                serde_json::json!({ "kind": "directory", "path": path.display().to_string() })
            }
            ResourceScope::Command { program } => {
                serde_json::json!({ "kind": "command", "program": program })
            }
            ResourceScope::Network { host } => {
                serde_json::json!({ "kind": "network", "host": host })
            }
            ResourceScope::Workflow { action } => {
                serde_json::json!({ "kind": "workflow", "action": action })
            }
            ResourceScope::Secret { name } => serde_json::json!({ "kind": "secret", "name": name }),
            ResourceScope::Extension { id } => serde_json::json!({ "kind": "extension", "id": id }),
        }
    }
}

fn extract_host(url: &str) -> Option<String> {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    without_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "reference_monitor/tests.rs"]
mod reference_monitor_types_tests;
