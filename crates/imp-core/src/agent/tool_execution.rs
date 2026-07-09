use std::sync::Arc;
use std::time::Instant;

use futures::future::join_all;
use tokio::sync::mpsc;

use crate::agent::{
    Agent, AgentEvent, RecoveryCheckpointKind, TimingEvent, TimingStage, ToolExecutionMode,
    ToolPlan, ToolRisk,
};
use crate::guardrails::{self, GuardrailLevel};
use crate::hooks::HookEvent;
use crate::reference_monitor::{PolicyReason, PolicySource, ToolPolicyContext, ToolPolicyDecision};
use crate::trust::{Provenance, RiskLabel};

use super::{extract_file_path, RepeatedToolCallCheck, RepeatedToolCallState};

fn legacy_policy_error_message(
    tool_name: &str,
    mode: crate::config::AgentMode,
    reason: &PolicyReason,
) -> String {
    match reason.source {
        PolicySource::AgentMode => format!(
            "Tool '{tool_name}' is not available in {} mode",
            format!("{:?}", mode).to_lowercase()
        ),
        PolicySource::RunPolicy => reason.message.clone(),
        _ => reason.message.clone(),
    }
}

fn policy_block_reason(decision: &ToolPolicyDecision) -> Option<PolicyReason> {
    match decision {
        ToolPolicyDecision::Allow { .. } => None,
        ToolPolicyDecision::Deny { reason }
        | ToolPolicyDecision::AskUser { reason }
        | ToolPolicyDecision::DryRunOnly { reason }
        | ToolPolicyDecision::SandboxOnly { reason }
        | ToolPolicyDecision::RequireVerification { reason } => Some(reason.clone()),
    }
}

fn legacy_policy_checkpoint_reason(reason: &PolicyReason) -> &'static str {
    match reason.code.as_str() {
        "agent_mode_tool_denied" => "mode_blocked",
        "run_policy_tool_denied" | "run_policy_write_path_denied" => "run_policy_blocked",
        "require_verification" => "verification_required",
        "ask_user_required" => "approval_required",
        "dry_run_required" => "dry_run_required",
        "sandbox_required" => "sandbox_required",
        _ => "policy_blocked",
    }
}

fn tool_result_provenance(tool_name: &str, args: &serde_json::Value) -> Provenance {
    match tool_name {
        "read" => args
            .get("path")
            .and_then(|value| value.as_str())
            .map(Provenance::workspace_file)
            .unwrap_or_else(|| Provenance::tool_observation(tool_name)),
        "web" => args
            .get("url")
            .and_then(|value| value.as_str())
            .map(Provenance::external_web)
            .unwrap_or_else(|| {
                Provenance::tool_observation(tool_name).with_risk(RiskLabel::NetworkDerived)
            }),
        "browser" => args
            .get("url")
            .and_then(|value| value.as_str())
            .map(Provenance::external_web)
            .unwrap_or_else(|| {
                Provenance::tool_observation(tool_name).with_risk(RiskLabel::NetworkDerived)
            }),
        _ => Provenance::tool_observation(tool_name),
    }
}

fn attach_provenance_to_result(
    mut result: imp_llm::ToolResultMessage,
    provenance: &Provenance,
) -> imp_llm::ToolResultMessage {
    let mut details = match result.details {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Null => serde_json::Map::new(),
        other => {
            let mut map = serde_json::Map::new();
            map.insert("raw".into(), other);
            map
        }
    };
    details.insert(
        "provenance".into(),
        serde_json::to_value(provenance).unwrap_or(serde_json::Value::Null),
    );
    result.details = serde_json::Value::Object(details);
    result
}

fn attach_policy_trace_to_result(
    mut result: imp_llm::ToolResultMessage,
    policy_record: &crate::reference_monitor::PolicyTraceRecord,
) -> imp_llm::ToolResultMessage {
    let mut details = match result.details {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Null => serde_json::Map::new(),
        other => {
            let mut map = serde_json::Map::new();
            map.insert("raw".into(), other);
            map
        }
    };
    details.insert(
        "policy".into(),
        serde_json::to_value(policy_record).unwrap_or(serde_json::Value::Null),
    );
    result.details = serde_json::Value::Object(details);
    result
}

impl Agent {
    pub(super) fn plan_tools(&self, calls: Vec<(String, String, serde_json::Value)>) -> ToolPlan {
        let calls = calls
            .into_iter()
            .enumerate()
            .map(|(index, (id, name, args))| {
                let risk = if self.tools.is_readonly_call(&name, &args).unwrap_or(false) {
                    ToolRisk::ReadOnly
                } else if matches!(name.as_str(), "bash" | "git") {
                    ToolRisk::ExternalSideEffect
                } else {
                    ToolRisk::Mutable
                };
                let retry_safe = matches!(risk, ToolRisk::ReadOnly);
                super::PlannedToolCall {
                    index,
                    id,
                    name,
                    args,
                    risk,
                    retry_safe,
                }
            })
            .collect();

        ToolPlan {
            mode: ToolExecutionMode::ParallelReadonlyThenSequentialMutable,
            calls,
        }
    }

    pub(super) async fn execute_tool_plan(
        &self,
        turn: u32,
        turn_started_at: Instant,
        plan: ToolPlan,
        cancel_token: Arc<std::sync::atomic::AtomicBool>,
    ) -> Vec<imp_llm::ToolResultMessage> {
        let mut readonly = Vec::new();
        let mut mutable = Vec::new();

        for call in plan.calls {
            match call.risk {
                ToolRisk::ReadOnly => readonly.push(call),
                ToolRisk::Mutable | ToolRisk::ExternalSideEffect => mutable.push(call),
            }
        }

        let mut results = join_all(readonly.into_iter().map(|call| {
            let cancel_token = Arc::clone(&cancel_token);
            async move {
                let result = self
                    .execute_one_tool(
                        &call.id,
                        &call.name,
                        call.args,
                        cancel_token,
                        turn,
                        turn_started_at,
                    )
                    .await;
                (call.index, result)
            }
        }))
        .await;

        for call in mutable {
            let result = self
                .execute_one_tool(
                    &call.id,
                    &call.name,
                    call.args,
                    Arc::clone(&cancel_token),
                    turn,
                    turn_started_at,
                )
                .await;
            results.push((call.index, result));
        }

        results.sort_by_key(|(index, _)| *index);
        results.into_iter().map(|(_, result)| result).collect()
    }

    fn repeated_tool_call_check(
        &self,
        call_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> RepeatedToolCallCheck {
        let args_json = serde_json::to_string(args).unwrap_or_else(|_| "<unserializable>".into());
        let mut state = match self.last_tool_call.lock() {
            Ok(s) => s,
            Err(_) => return RepeatedToolCallCheck::Ok,
        };

        let consecutive = match state.as_mut() {
            Some(prev) if prev.tool_name == tool_name && prev.args_json == args_json => {
                prev.consecutive += 1;
                prev.consecutive
            }
            _ => {
                *state = Some(RepeatedToolCallState {
                    tool_name: tool_name.to_string(),
                    args_json,
                    consecutive: 1,
                });
                1
            }
        };

        if consecutive == 3 {
            return RepeatedToolCallCheck::Warn(format!(
                "Warning: identical tool call repeated 3 times in a row for '{tool_name}'. The result may not have changed. Consider using the information you already have or trying a different action."
            ));
        }

        if consecutive >= 4 {
            return RepeatedToolCallCheck::Block(
                crate::tools::ToolOutput::error(format!(
                    "Blocked: identical tool call repeated {consecutive} times in a row for '{tool_name}'. The result likely has not changed. Use the information you already have or try a different action."
                ))
                .into_tool_result(call_id, tool_name),
            );
        }

        RepeatedToolCallCheck::Ok
    }

    async fn execute_one_tool(
        &self,
        call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        cancel_token: Arc<std::sync::atomic::AtomicBool>,
        turn: u32,
        turn_started_at: Instant,
    ) -> imp_llm::ToolResultMessage {
        let args_hash = Self::tool_args_hash(&args);
        let repeat_check = self.repeated_tool_call_check(call_id, tool_name, &args);
        let tool_started_at = Instant::now();
        self.emit_timing_with_details(
            TimingEvent::new(turn, TimingStage::ToolExecutionStart, turn_started_at, None)
                .with_label(tool_name.to_string()),
        )
        .await;

        self.emit_recovery_checkpoint(Self::recovery_checkpoint(
            turn,
            RecoveryCheckpointKind::ToolExecutionStart,
            Some(call_id.to_string()),
            Some(tool_name.to_string()),
            Some(args_hash.clone()),
            None,
            None,
        ))
        .await;

        if let RepeatedToolCallCheck::Block(loop_result) = repeat_check {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call_id.to_string(),
                tool_name: tool_name.to_string(),
                args: args.clone(),
            })
            .await;
            self.emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: call_id.to_string(),
                result: loop_result.clone(),
                provenance: Some(tool_result_provenance(tool_name, &args)),
            })
            .await;
            self.emit_timing_with_details(
                TimingEvent::new(turn, TimingStage::ToolExecutionEnd, turn_started_at, None)
                    .with_duration_ms(tool_started_at.elapsed().as_millis() as u64)
                    .with_label(tool_name.to_string())
                    .with_success(false),
            )
            .await;
            self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                turn,
                RecoveryCheckpointKind::ToolExecutionEnd,
                Some(call_id.to_string()),
                Some(tool_name.to_string()),
                Some(args_hash.clone()),
                Some(false),
                Some("repeated_tool_call_blocked".to_string()),
            ))
            .await;
            return loop_result;
        }

        self.emit(AgentEvent::ToolExecutionStart {
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            args: args.clone(),
        })
        .await;

        let before_results = self
            .hooks
            .fire(&HookEvent::BeforeToolCall {
                tool_name,
                args: &args,
            })
            .await;

        let mut policy_context = ToolPolicyContext::new(
            tool_name,
            self.tools
                .policy_metadata_for(tool_name, &args)
                .map(|metadata| metadata.action_kind)
                .unwrap_or_default(),
        );
        policy_context.run_id = self.run_id.lock().ok().and_then(|run_id| run_id.clone());
        policy_context.workflow_id = self
            .workflow_contract()
            .id
            .clone()
            .or_else(|| self.workflow_contract().workflow_unit_ref.clone());
        policy_context.turn = Some(turn);
        policy_context.tool_call_id = Some(call_id.to_string());
        policy_context.args = args.clone();
        policy_context.args_hash = Some(args_hash.clone());
        policy_context.cwd = Some(self.cwd.clone());
        if let Some(metadata) = self.tools.policy_metadata_for(tool_name, &args) {
            policy_context.resource_scope =
                metadata.resource_scope_for_args(Some(&self.cwd), &args);
            policy_context.metadata = metadata;
        }
        policy_context.mode = self.mode;
        policy_context.policy = self.config.policy.clone();
        policy_context.apply_workflow_contract(self.workflow_contract());

        let policy_record =
            crate::reference_monitor::ReferenceMonitor.evaluate(&policy_context, &self.run_policy);
        let policy_block = policy_block_reason(&policy_record.decision);
        self.emit(AgentEvent::PolicyChecked {
            record: policy_record.clone(),
        })
        .await;
        if let Some(reason) = policy_block {
            let mut result = crate::tools::ToolOutput::error(legacy_policy_error_message(
                tool_name, self.mode, &reason,
            ))
            .into_tool_result(call_id, tool_name);
            result = attach_policy_trace_to_result(result, &policy_record);
            self.emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: call_id.to_string(),
                result: result.clone(),
                provenance: Some(tool_result_provenance(tool_name, &args)),
            })
            .await;
            self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                turn,
                RecoveryCheckpointKind::ToolExecutionEnd,
                Some(call_id.to_string()),
                Some(tool_name.to_string()),
                Some(args_hash.clone()),
                Some(false),
                Some(legacy_policy_checkpoint_reason(&reason).to_string()),
            ))
            .await;
            return result;
        }

        if let Some(blocking_result) = before_results.into_iter().find(|result| result.block) {
            let reason = blocking_result
                .reason
                .unwrap_or_else(|| format!("Tool call blocked by hook: {tool_name}"));
            let result =
                crate::tools::ToolOutput::error(reason).into_tool_result(call_id, tool_name);
            self.emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: call_id.to_string(),
                result: result.clone(),
                provenance: Some(tool_result_provenance(tool_name, &args)),
            })
            .await;
            self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                turn,
                RecoveryCheckpointKind::ToolExecutionEnd,
                Some(call_id.to_string()),
                Some(tool_name.to_string()),
                Some(args_hash.clone()),
                Some(false),
                Some("hook_blocked".to_string()),
            ))
            .await;
            return result;
        }

        // Validate args against the tool's JSON schema before execution so the
        // model can self-correct on bad types or missing required fields.
        if let Some(tool) = self.tools.get(tool_name) {
            let schema = tool.parameters();
            if let Err(e) = crate::tools::validate_tool_args(&schema, &args) {
                let result = crate::tools::ToolOutput::error(e.to_string())
                    .into_tool_result(call_id, tool_name);
                self.emit(AgentEvent::ToolExecutionEnd {
                    tool_call_id: call_id.to_string(),
                    result: result.clone(),
                    provenance: Some(tool_result_provenance(tool_name, &args)),
                })
                .await;
                self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                    turn,
                    RecoveryCheckpointKind::ToolExecutionEnd,
                    Some(call_id.to_string()),
                    Some(tool_name.to_string()),
                    Some(args_hash.clone()),
                    Some(false),
                    Some("validation_error".to_string()),
                ))
                .await;
                return result;
            }
        }

        let mut result = match self.tools.get(tool_name) {
            Some(tool) => {
                let (update_tx, mut update_rx) = mpsc::channel(64);
                let ctx = crate::tools::ToolContext {
                    cwd: self.cwd.clone(),
                    cancelled: Arc::clone(&cancel_token),
                    update_tx,
                    command_tx: self.command_tx.clone(),
                    ui: self.ui.clone(),
                    file_cache: self.file_cache.clone(),
                    checkpoint_state: self.checkpoint_state.clone(),
                    file_tracker: self.file_tracker.clone(),
                    anchor_store: self.anchor_store.clone(),
                    lua_tool_loader: self.lua_tool_loader.clone(),
                    mode: self.mode,
                    read_max_lines: self.read_max_lines,
                    config: self.config.clone(),
                    turn_workflow_review: self.turn_workflow_review_accumulator(),
                    run_policy: self.run_policy.clone(),
                    supporting_provenance: policy_context.supporting_provenance.clone(),
                };

                // Forward tool output deltas to event stream
                let event_tx = self.event_tx.clone();
                let delta_call_id = call_id.to_string();
                let forwarder = tokio::spawn(async move {
                    while let Some(update) = update_rx.recv().await {
                        for block in &update.content {
                            if let imp_llm::ContentBlock::Text { text } = block {
                                let _ = event_tx.send(AgentEvent::ToolOutputDelta {
                                    tool_call_id: delta_call_id.clone(),
                                    text: text.clone(),
                                });
                            }
                        }
                    }
                });

                let exec_result = match tool.execute(call_id, args.clone(), ctx).await {
                    Ok(output) => output.into_tool_result(call_id, tool_name),
                    Err(e) => crate::tools::ToolOutput::error(e.to_string())
                        .into_tool_result(call_id, tool_name),
                };
                forwarder.await.ok();
                exec_result
            }
            None => crate::tools::ToolOutput::error(format!("Unknown tool: {tool_name}"))
                .into_tool_result(call_id, tool_name),
        };

        let after_results = self
            .hooks
            .fire(&HookEvent::AfterToolCall {
                tool_name,
                result: &result,
            })
            .await;

        if let Some(modified_content) = after_results
            .into_iter()
            .filter_map(|hook_result| hook_result.modified_content)
            .next_back()
        {
            result.content = modified_content;
        }

        if !result.is_error && matches!(tool_name, "write" | "edit" | "multi_edit") {
            if let Some(path) = extract_file_path(self.cwd.as_path(), &args) {
                self.hooks
                    .fire(&HookEvent::AfterFileWrite {
                        file: path.as_path(),
                    })
                    .await;

                // Run guardrail after-write checks when enabled
                if let Some(profile) = self.guardrail_profile {
                    if self.guardrail_config.should_check_path(&path) {
                        let check_results = guardrails::run_after_write_checks(
                            &self.guardrail_config,
                            profile,
                            &self.cwd,
                        )
                        .await;

                        if !check_results.is_empty() {
                            let level = self.guardrail_config.effective_level();
                            let msg = guardrails::format_check_results(&check_results, level);
                            if !msg.is_empty() && msg != "Guardrail checks passed." {
                                // Append guardrail output to the tool result
                                result.content.push(imp_llm::ContentBlock::Text {
                                    text: format!("\n\n{msg}"),
                                });
                                if level == GuardrailLevel::Enforce
                                    && check_results.iter().any(|r| !r.success)
                                {
                                    result.is_error = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        let provenance = tool_result_provenance(tool_name, &args);
        result = attach_provenance_to_result(result, &provenance);
        result = attach_policy_trace_to_result(result, &policy_record);
        self.emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: call_id.to_string(),
            result: result.clone(),
            provenance: Some(provenance),
        })
        .await;
        self.emit_timing_with_details(
            TimingEvent::new(turn, TimingStage::ToolExecutionEnd, turn_started_at, None)
                .with_duration_ms(tool_started_at.elapsed().as_millis() as u64)
                .with_label(tool_name.to_string())
                .with_success(!result.is_error),
        )
        .await;

        self.emit_recovery_checkpoint(Self::recovery_checkpoint(
            turn,
            RecoveryCheckpointKind::ToolExecutionEnd,
            Some(call_id.to_string()),
            Some(tool_name.to_string()),
            Some(args_hash),
            Some(!result.is_error),
            result.is_error.then(|| "tool_error".to_string()),
        ))
        .await;

        if let RepeatedToolCallCheck::Warn(warning) = repeat_check {
            result.content.push(imp_llm::ContentBlock::Text {
                text: format!("\n\n{warning}"),
            });
        }

        result
    }
}
