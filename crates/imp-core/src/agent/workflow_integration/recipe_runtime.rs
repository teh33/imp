//! Recipe/runtime support for imp workflow integration.

use super::workflow_compat::tool_results_indicate_execution_evidence;
use crate::agent::{
    autonomy::{
        edited_files_verification_obligation, failed_command_recovery_obligation, ObligationKind,
    },
    Agent, AgentEvent, ContentBlock, LoopDecision, Message, RunFinalStatus, StopReason,
};
use crate::config::AgentMode;
use crate::eval_candidate::{redact_eval_candidate, EvalArtifactRef};
use crate::eval_candidate_closeout::{eval_candidate_for_closeout, EvalCandidateCloseoutContext};
use crate::evidence::{
    EvidenceActions, EvidenceArtifact, EvidencePacket, EvidencePolicy, EvidenceTrustSummary,
    EvidenceVerificationGate,
};
use crate::storage;
use crate::trust::{Provenance, RiskLabel, TrustLabel};
use crate::workflow::{WorkflowContract, WorkflowRunController, WorktreeRunMetadata};
use crate::workflow_review::TurnWorkflowReviewAccumulator;

use super::super::{
    tool_results_include_successful_check, tool_results_include_successful_edit,
    tool_results_indicate_failed_bash_command,
};

#[derive(Debug)]
pub(crate) struct WorkflowRuntimeLayer {
    controller: WorkflowRunController,
    contract: WorkflowContract,
    pub(crate) turn_workflow_review:
        std::sync::Arc<std::sync::Mutex<TurnWorkflowReviewAccumulator>>,
    pub(crate) has_workflow_skill: bool,
    pub(crate) has_workflow_basics_skill: bool,
    pub(crate) has_workflow_delegation_skill: bool,
    pub(crate) queued_workflow_externalization_nudge: bool,
}

impl WorkflowRuntimeLayer {
    pub(crate) fn new(contract: WorkflowContract) -> Self {
        Self {
            controller: WorkflowRunController::new(),
            contract,
            turn_workflow_review: std::sync::Arc::new(std::sync::Mutex::new(
                TurnWorkflowReviewAccumulator::default(),
            )),
            has_workflow_skill: false,
            has_workflow_basics_skill: false,
            has_workflow_delegation_skill: false,
            queued_workflow_externalization_nudge: false,
        }
    }

    pub(crate) fn controller(&self) -> &WorkflowRunController {
        &self.controller
    }

    pub(crate) fn controller_mut(&mut self) -> &mut WorkflowRunController {
        &mut self.controller
    }

    pub(crate) fn contract(&self) -> &WorkflowContract {
        &self.contract
    }

    pub(crate) fn contract_mut(&mut self) -> &mut WorkflowContract {
        &mut self.contract
    }

    pub(crate) fn replace_contract(&mut self, contract: WorkflowContract) {
        self.contract = contract;
    }

    pub(crate) fn turn_workflow_review(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<TurnWorkflowReviewAccumulator>> {
        self.turn_workflow_review.clone()
    }

    pub(crate) fn set_workflow_skill_available(&mut self, available: bool) {
        self.has_workflow_skill = available;
    }

    pub(crate) fn set_workflow_basics_skill_available(&mut self, available: bool) {
        self.has_workflow_basics_skill = available;
    }

    pub(crate) fn set_workflow_delegation_skill_available(&mut self, available: bool) {
        self.has_workflow_delegation_skill = available;
    }

    pub(crate) fn mark_externalization_nudge_queued(&mut self) {
        self.queued_workflow_externalization_nudge = true;
    }

    pub(crate) fn replace_controller(&mut self, controller: WorkflowRunController) {
        self.controller = controller;
    }
}

pub(crate) fn workflow_layer_may_override_finish(decision: &LoopDecision) -> bool {
    matches!(
        decision,
        LoopDecision::Finish {
            status: crate::agent::RunFinalStatus::Done { .. }
                | crate::agent::RunFinalStatus::DoneWithConcerns { .. }
        }
    )
}

fn evidence_trust_summary_from_messages(messages: &[Message]) -> EvidenceTrustSummary {
    let mut summary = EvidenceTrustSummary::default();
    for message in messages {
        let Message::ToolResult(result) = message else {
            continue;
        };
        let Some(provenance) = result
            .details
            .get("provenance")
            .and_then(|value| serde_json::from_value::<Provenance>(value.clone()).ok())
        else {
            continue;
        };
        record_evidence_provenance(&mut summary, &provenance);
    }
    summary.sources.sort();
    summary.sources.dedup();
    summary.low_trust_influences.sort();
    summary.low_trust_influences.dedup();
    summary.warnings.sort();
    summary.warnings.dedup();
    summary
}

fn record_evidence_provenance(summary: &mut EvidenceTrustSummary, provenance: &Provenance) {
    summary.sources.push(format!(
        "source={:?}; trust={:?}; origin={}",
        provenance.source,
        provenance.trust,
        provenance.origin.as_deref().unwrap_or("unknown")
    ));
    if provenance.is_low_trust() {
        summary.low_trust_influences.push(format!(
            "low-trust source observed: {}",
            provenance.origin.as_deref().unwrap_or("unknown")
        ));
    }
    if provenance.risk.iter().any(|risk| {
        matches!(
            risk,
            RiskLabel::PossiblePromptInjection | RiskLabel::ContainsInstructions
        )
    }) {
        summary.warnings.push(format!(
            "instruction-like low-trust content observed from {}",
            provenance.origin.as_deref().unwrap_or("unknown")
        ));
    }
    if provenance.trust == TrustLabel::ExternalUntrusted {
        summary
            .warnings
            .push("external/untrusted content cannot authorize policy or tool escalation".into());
    }
}

fn evidence_verification_gate(
    gate: &crate::workflow::VerificationGate,
) -> EvidenceVerificationGate {
    EvidenceVerificationGate {
        name: if gate.name.is_empty() {
            gate.id.clone()
        } else {
            gate.name.clone()
        },
        required: gate.is_required(),
        status: format!("{:?}", gate.status).to_lowercase(),
        command: gate.command.as_ref().map(|command| command.command.clone()),
        exit_code: gate.result.as_ref().and_then(|result| result.exit_code),
        artifact_path: gate
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == "status")
            .or_else(|| gate.artifacts.first())
            .map(|artifact| artifact.path.clone()),
    }
}

fn evidence_policy_for_config(policy_config: &crate::config::PolicyConfig) -> EvidencePolicy {
    let mut policy = EvidencePolicy::default();
    policy.decisions.push(format!(
        "config policy: side_effects={}, workspace_writes={}, outside_workspace_writes={}, shell={}, network={}, secrets={}, extension_network={}, deny_approval_required={}",
        policy_config.allow_side_effects,
        policy_config.workspace_writes,
        policy_config.outside_workspace_writes,
        policy_config.shell,
        policy_config.network,
        policy_config.secrets,
        policy_config.extension_network,
        policy_config.deny_approval_required,
    ));
    policy.decisions.push(
        "policy.checked trace events record explicit config policy, scope, and decision context when policy checks run".into(),
    );
    policy
        .denials
        .push("hard-rail bypass: none recorded; dangerous grants are not implemented".into());
    policy
}

fn evidence_actions_from_messages(messages: &[Message]) -> EvidenceActions {
    let mut actions = EvidenceActions::default();
    for message in messages {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        for block in &assistant.content {
            let ContentBlock::ToolCall {
                name, arguments, ..
            } = block
            else {
                continue;
            };
            actions.tools.push(name.clone());
            match name.as_str() {
                "read" => {
                    if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
                        actions.files_inspected.push(path.to_string());
                    }
                }
                "write" | "edit" => {
                    if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
                        actions.files_changed.push(path.to_string());
                    }
                }
                "bash" => {
                    if let Some(command) = arguments.get("command").and_then(|value| value.as_str())
                    {
                        actions.commands_run.push(command.to_string());
                    }
                }
                "scan" => actions.searches.push(format!("scan {arguments}")),
                _ => {}
            }
        }
    }
    actions.files_inspected.sort();
    actions.files_inspected.dedup();
    actions.files_changed.sort();
    actions.files_changed.dedup();
    actions.commands_run.sort();
    actions.commands_run.dedup();
    actions.searches.sort();
    actions.searches.dedup();
    actions.tools.sort();
    actions.tools.dedup();
    actions
}

impl Agent {
    pub fn workflow_contract(&self) -> &WorkflowContract {
        self.workflow_layer.contract()
    }

    pub fn workflow_contract_mut(&mut self) -> &mut WorkflowContract {
        self.workflow_layer.contract_mut()
    }

    pub fn set_workflow_contract(&mut self, contract: WorkflowContract) {
        self.workflow_layer.replace_contract(contract);
    }

    pub(in crate::agent) fn write_workflow_contract_snapshot(
        &self,
        artifacts: &storage::RunArtifacts,
    ) {
        let _ = std::fs::write(
            artifacts.workflow_contract_path(),
            serde_json::to_string_pretty(self.workflow_contract()).unwrap_or_default(),
        );
    }

    pub(in crate::agent) async fn write_closeout_eval_candidate(
        &self,
        run_id: &str,
        artifacts: &storage::RunArtifacts,
        prompt: &str,
        status: &RunFinalStatus,
    ) {
        let candidate_id = format!("{run_id}-closeout");
        let context = EvalCandidateCloseoutContext {
            candidate_id: candidate_id.clone(),
            run_id: Some(run_id.to_string()),
            workflow_id: self
                .workflow_contract()
                .id
                .clone()
                .or_else(|| self.workflow_contract().workflow_unit_ref.clone()),
            session_id: None,
            prompt: Some(prompt.to_string()),
            expected_summary: Some(
                "Agent run should satisfy its workflow contract and required verification gates"
                    .into(),
            ),
            verifiers: Vec::new(),
            artifact_refs: vec![
                EvalArtifactRef {
                    kind: "trace".into(),
                    path: artifacts.trace_path(),
                    summary: Some("Structured runtime event trace".into()),
                    sha256: None,
                },
                EvalArtifactRef {
                    kind: "evidence".into(),
                    path: artifacts.evidence_path(),
                    summary: Some("Run evidence packet".into()),
                    sha256: None,
                },
                EvalArtifactRef {
                    kind: "workflow-contract".into(),
                    path: artifacts.workflow_contract_path(),
                    summary: Some("Workflow contract snapshot".into()),
                    sha256: None,
                },
            ],
        };
        let Some(candidate) =
            eval_candidate_for_closeout(status, &self.verification_gates, context)
        else {
            return;
        };
        let path = artifacts.eval_candidate_path(&candidate_id);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        if let Ok(json) = serde_json::to_string_pretty(&redact_eval_candidate(candidate)) {
            let _ = std::fs::write(path, json);
        }
    }

    pub(in crate::agent) async fn write_run_evidence(
        &self,
        run_id: &str,
        artifacts: &storage::RunArtifacts,
        worktree_metadata: Option<&WorktreeRunMetadata>,
        prompt: &str,
        status: &RunFinalStatus,
    ) {
        let mut packet = EvidencePacket::new(run_id, prompt);
        packet.workflow_id = self
            .workflow_contract()
            .id
            .clone()
            .or_else(|| self.workflow_contract().workflow_unit_ref.clone());
        packet.workflow_type = Some(format!("{:?}", self.workflow_contract().workflow_type));
        packet.risk_level = Some(format!("{:?}", self.workflow_contract().risk_level));
        packet.final_status = Some(format!("{:?}", status));
        packet.policy = evidence_policy_for_config(&self.config.policy);
        packet.trust = evidence_trust_summary_from_messages(&self.messages);
        packet
            .summary
            .push("Agent run completed; inspect trace.jsonl for structured event details.".into());
        packet.actions = evidence_actions_from_messages(&self.messages);
        packet.verification = self
            .verification_gates
            .iter()
            .map(evidence_verification_gate)
            .collect();
        if let Some(metadata) = worktree_metadata {
            packet.summary.push(format!(
                "Worktree-auto changes ran in {} on branch {}; inspect worktree artifacts for status and patch.",
                metadata.worktree_path.display(),
                metadata.branch
            ));
        }
        packet.artifacts = vec![
            EvidenceArtifact {
                kind: "trace".into(),
                path: artifacts.trace_path(),
                summary: Some("Structured runtime event trace".into()),
            },
            EvidenceArtifact {
                kind: "workflow-contract".into(),
                path: artifacts.workflow_contract_path(),
                summary: Some("Workflow contract snapshot".into()),
            },
        ];
        if let Some(metadata) = worktree_metadata {
            let worktree_dir = artifacts.root().join("worktree");
            packet.artifacts.extend([
                EvidenceArtifact {
                    kind: "worktree-status".into(),
                    path: metadata.status_path.clone(),
                    summary: Some(format!(
                        "Worktree status for {} ({})",
                        metadata.worktree_path.display(),
                        if metadata.clean { "clean" } else { "dirty" }
                    )),
                },
                EvidenceArtifact {
                    kind: "worktree-diff-stat".into(),
                    path: metadata.stat_path.clone(),
                    summary: Some("Worktree diff summary".into()),
                },
                EvidenceArtifact {
                    kind: "worktree-diff".into(),
                    path: metadata.patch_path.clone(),
                    summary: Some("Binary-safe worktree patch".into()),
                },
                EvidenceArtifact {
                    kind: "worktree-metadata".into(),
                    path: worktree_dir.join("worktree-metadata.json"),
                    summary: Some(format!(
                        "Worktree {} on branch {}",
                        metadata.worktree_path.display(),
                        metadata.branch
                    )),
                },
            ]);
        }
        let evidence_path = artifacts.evidence_path();
        if packet.write_markdown(&evidence_path).is_ok() {
            self.write_trace_event(&AgentEvent::EvidenceWritten {
                path: evidence_path.clone(),
            });
            let _ = self.event_tx.send(AgentEvent::EvidenceWritten {
                path: evidence_path,
            });
        }
    }

    pub(in crate::agent) fn override_finish_with_workflow_decision(
        &self,
        decision: LoopDecision,
    ) -> LoopDecision {
        if !workflow_layer_may_override_finish(&decision) {
            return decision;
        }
        let controller_decision = self.workflow_controller_continue_decision();
        if controller_decision.is_none()
            && matches!(
                decision,
                LoopDecision::Finish {
                    status: RunFinalStatus::Done {
                        reason: StopReason::NoAutomaticFollowUp | StopReason::NoProgress
                    }
                }
            )
        {
            return decision;
        }
        controller_decision.unwrap_or(decision)
    }

    pub(in crate::agent) fn should_retry_unanswered_execution_debt(
        &self,
        tool_results: &[imp_llm::ToolResultMessage],
        execution_evidence: bool,
    ) -> bool {
        !matches!(self.mode, AgentMode::Planner)
            && self.queued_execution_debt_follow_up_count == 1
            && !execution_evidence
            && tool_results
                .iter()
                .all(|result| result.is_error || result.tool_name == "work")
    }

    pub(in crate::agent) fn record_obligations_from_tool_results(
        &mut self,
        tool_results: &[imp_llm::ToolResultMessage],
    ) {
        if tool_results_indicate_failed_bash_command(tool_results, self.mode) {
            self.obligation_ledger
                .add(failed_command_recovery_obligation());
        }
        if tool_results_include_successful_check(tool_results) {
            self.obligation_ledger
                .resolve_kind(ObligationKind::EditedFilesVerification);
        }
        if tool_results_include_successful_edit(tool_results) {
            self.obligation_ledger
                .add(edited_files_verification_obligation());
        }
        if tool_results_indicate_execution_evidence(tool_results, self.mode) {
            self.obligation_ledger
                .resolve_kind(ObligationKind::FailedCommandRecovery);
        }
    }
}
