use super::*;
use crate::workflow::{CheckStatus, StepStatus, load_workflow, validate_workflow};
use files::load_selected_workflow;
use std::path::Path;
use std::sync::Arc;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn workflow_tool_list_discovers_workflows() {
    let output = list_action(&repo_root().join(".imp/workflows")).expect("list succeeds");
    let text = output.text_content().expect("text output");
    assert!(text.contains("prototype-imp-workflow-engine"));
    assert!(text.contains("prototype-workflow-tool"));
}

#[test]
fn workflow_tool_show_renders_status() {
    let output = show_action(
        &repo_root().join(".imp/workflows"),
        Some("update-imp-after-workflow-engine"),
        WorkflowValidationModeParam::Strict,
    )
    .expect("show succeeds");
    let text = output.text_content().expect("text output");
    assert!(text.contains("Workflow: update-imp-after-workflow-engine"));
    assert!(text.contains("Acceptance:"));
    assert!(text.contains("Steps:"));
}

#[test]
fn workflow_tool_validate_all_passes_for_dogfood_workflows() {
    let output = validate_action(
        &repo_root().join(".imp/workflows"),
        None,
        WorkflowValidationModeParam::Strict,
    )
    .expect("validate succeeds");
    assert!(!output.is_error);
    let text = output.text_content().expect("text output");
    assert!(text.contains("Validated"));
    assert!(text.contains("0 with diagnostics"), "{text}");
}

#[tokio::test]
async fn workflow_run_returns_next_runnable_step() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);
    set_step_status(
        &workflows_root,
        "implement-workflow-run-engine",
        "execute",
        "todo",
    );

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("implement-workflow-run-engine"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(
        text.contains("Next workflow action: run step execute [build]"),
        "{text}"
    );
    assert!(text.contains("Worker: builder"), "{text}");
    assert!(
        text.contains("Worker assignment: builder (builder)"),
        "{text}"
    );
    assert!(text.contains("Writes: code, tests"), "{text}");
    assert!(text.contains("Worktree: workflow"), "{text}");

    let assignment = output.details["result"]["next_action"]["worker_assignment"]
        .as_object()
        .expect("worker assignment details");
    let contract = assignment["contract"]
        .as_object()
        .expect("worker assignment contract");
    assert_eq!(contract["workflow_id"], "implement-workflow-run-engine");
    assert_eq!(contract["step"], "execute");
    assert_eq!(contract["step_kind"], "build");
    assert_eq!(contract["role"], "coder");
    assert_eq!(contract["worker"], "builder");
    assert_eq!(
        contract["result_path"],
        ".imp/workflows/implement-workflow-run-engine/results.md"
    );
    assert_eq!(contract["writes_code"], true);
    assert_eq!(contract["worktree"], "workflow");
    assert!(
        contract["objective"]
            .as_str()
            .expect("objective")
            .contains("Complete workflow step `execute`")
    );
    let instructions = contract["instructions"].as_array().expect("instructions");
    assert!(instructions.iter().any(|instruction| {
        instruction
            .as_str()
            .is_some_and(|text| text.contains("do not broaden scope"))
    }));
    assert!(instructions.iter().any(|instruction| {
        instruction
            .as_str()
            .is_some_and(|text| text.contains("implementation_ready"))
    }));
}

#[tokio::test]
async fn workflow_run_executes_pending_command_checks() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let (workflows_root, workflow_root) = write_command_check_workflow(temp.path(), true);

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("command-check-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(text.contains("ran 1 command check"), "{text}");
    assert!(text.contains("command_check: passed"), "{text}");

    let doc =
        load_workflow(&workflow_root.join("workflow.yaml")).expect("updated workflow should load");
    assert!(matches!(
        doc.steps.get("verify").expect("step exists").status,
        StepStatus::Done
    ));
    assert!(matches!(doc.status, crate::workflow::WorkflowStatus::Done));
    assert!(matches!(
        doc.spec
            .acceptance
            .get("command_check_passes")
            .expect("acceptance exists")
            .status,
        crate::workflow::AcceptanceStatus::Done
    ));
    let events = std::fs::read_to_string(workflow_root.join("events.jsonl"))
        .expect("events should be written");
    assert!(events.contains("checks.command_check.status"), "{events}");
    assert!(
        events.contains("spec.acceptance.command_check_passes.status"),
        "{events}"
    );
    assert!(events.contains("\"path\":\"status\""), "{events}");
}

#[tokio::test]
async fn workflow_run_executes_presence_absence_checks() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_presence_absence_workflow(temp.path());
    std::fs::write(temp.path().join("subject.txt"), "alpha\nbeta\n").expect("write subject");

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("presence-absence-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(text.contains("has_alpha: passed"), "{text}");
    assert!(text.contains("no_gamma: passed"), "{text}");

    let doc = load_workflow(&workflows_root.join("presence-absence-workflow/workflow.yaml"))
        .expect("updated workflow loads");
    assert!(matches!(
        doc.checks.get("has_alpha").expect("check exists").status,
        CheckStatus::Passed
    ));
    assert!(matches!(
        doc.checks.get("no_gamma").expect("check exists").status,
        CheckStatus::Passed
    ));
    assert!(matches!(
        doc.steps.get("verify").expect("step exists").status,
        StepStatus::Done
    ));
}

#[tokio::test]
async fn workflow_run_executes_changed_files_check() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    init_git_repo(temp.path());
    std::fs::write(temp.path().join("tracked.txt"), "before\n").expect("write tracked");
    git(temp.path(), &["add", "tracked.txt"]);
    git(temp.path(), &["commit", "-m", "initial"]);
    std::fs::write(temp.path().join("tracked.txt"), "after\n").expect("modify tracked");
    let workflows_root = write_changed_files_workflow(temp.path());

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("changed-files-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(text.contains("source_changed: passed"), "{text}");

    let doc = load_workflow(&workflows_root.join("changed-files-workflow/workflow.yaml"))
        .expect("updated workflow loads");
    assert!(matches!(
        doc.checks
            .get("source_changed")
            .expect("check exists")
            .status,
        CheckStatus::Passed
    ));
    assert!(matches!(
        doc.steps.get("verify").expect("step exists").status,
        StepStatus::Done
    ));
}

#[tokio::test]
async fn workflow_run_does_not_complete_build_from_broad_checks_only() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_broad_only_build_workflow(temp.path());

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("broad-only-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run returns validation diagnostics");
    let text = output.text_content().expect("text output");
    assert!(text.contains("blocked by validation diagnostics"), "{text}");
    assert!(text.contains("broad command checks alone"), "{text}");

    let doc = load_workflow(&workflows_root.join("broad-only-workflow/workflow.yaml"))
        .expect("workflow still loads");
    assert!(matches!(
        doc.steps.get("implement").expect("step exists").status,
        StepStatus::Ready
    ));
    assert!(matches!(
        doc.checks.get("broad_tests").expect("check exists").status,
        CheckStatus::Pending
    ));
}

#[tokio::test]
async fn workflow_run_marks_step_failed_when_command_check_fails() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let (workflows_root, workflow_root) = write_command_check_workflow(temp.path(), false);

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("command-check-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(text.contains("command_check: failed"), "{text}");
    assert!(text.contains("- step verify: failed"), "{text}");

    let doc =
        load_workflow(&workflow_root.join("workflow.yaml")).expect("updated workflow should load");
    assert!(matches!(
        doc.checks
            .get("command_check")
            .expect("check exists")
            .status,
        CheckStatus::Failed
    ));
    assert!(matches!(
        doc.steps.get("verify").expect("step exists").status,
        StepStatus::Failed
    ));
}

#[tokio::test]
async fn workflow_run_reports_no_runnable_steps_when_dependencies_block() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);
    set_step_status(
        &workflows_root,
        "implement-workflow-run-engine",
        "verify",
        "todo",
    );

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("implement-workflow-run-engine"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(
        text.contains("Workflow blocked: missing action contract for step verify [verify]."),
        "{text}"
    );
}

#[tokio::test]
async fn workflow_run_reports_readiness_summary_when_no_steps_are_runnable() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_no_runnable_workflow(temp.path());

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("no-runnable-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(text.contains("No runnable workflow steps."), "{text}");
    assert!(text.contains("Readiness summary:"), "{text}");
    assert!(text.contains("- waiting: 2"), "{text}");
    assert!(text.contains("- terminal: 0"), "{text}");
    assert!(text.contains("step status is active"), "{text}");
    assert!(text.contains("dependency `inspect` is active"), "{text}");

    let next_action = &output.details["result"]["next_action"];
    assert_eq!(next_action["kind"], "no_runnable_steps");
    assert_eq!(next_action["summary"]["waiting"], 2);
    assert_eq!(next_action["summary"]["terminal"], 0);
    assert_eq!(
        next_action["blocked_steps"][1]["reason_details"][0]["kind"],
        "dependency_not_ready"
    );
}

#[tokio::test]
async fn workflow_complete_step_marks_step_checks_and_workflow_done() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_agent_action_workflow(temp.path());
    let ctx = test_ctx(temp.path());

    let output = complete_step_action(
        &workflows_root,
        Some("agent-action-workflow"),
        &json!({
            "step": "inspect",
            "reason": "inspection artifact written"
        }),
        &ctx,
    )
    .expect("complete step succeeds");

    assert_eq!(output.details["action"], "complete_step");
    assert_eq!(output.details["step"], "inspect");
    let doc = load_workflow(&workflows_root.join("agent-action-workflow/workflow.yaml"))
        .expect("updated workflow loads");
    assert!(matches!(
        doc.steps.get("inspect").expect("step exists").status,
        StepStatus::Done
    ));
    assert!(matches!(
        doc.checks.get("inspected").expect("check exists").status,
        CheckStatus::Passed
    ));
    assert!(matches!(doc.status, crate::workflow::WorkflowStatus::Done));
    assert!(matches!(
        doc.spec
            .acceptance
            .get("inspected")
            .expect("acceptance exists")
            .status,
        crate::workflow::AcceptanceStatus::Done
    ));
}

#[tokio::test]
async fn workflow_run_renders_agent_action_contract() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_agent_action_workflow(temp.path());

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("agent-action-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(
        text.contains("Workflow needs main agent action: inspect [context]"),
        "{text}"
    );
    assert!(text.contains("Role: reviewer"), "{text}");
    assert!(
        text.contains("Objective: Review workflow action support."),
        "{text}"
    );
    assert!(text.contains("Instructions:"), "{text}");
    assert!(text.contains("Allowed writes:"), "{text}");
    assert!(text.contains("Completion:"), "{text}");
    assert!(text.contains("complete_step"), "{text}");
    assert!(text.contains("Review adversarially"), "{text}");
    assert!(
        text.contains("Review rubric: Check acceptance criteria."),
        "{text}"
    );
    assert!(
        text.contains("Output sections: Decision, Evidence, Blocking Findings"),
        "{text}"
    );
    assert!(text.contains("Review required: yes"), "{text}");
    assert!(text.contains("Communication:"), "{text}");
    assert!(text.contains("main_agent_artifact_mailbox"), "{text}");
    assert!(
        text.contains("artifacts/communication/inspect/inbox.md"),
        "{text}"
    );
    assert!(text.contains("- check: inspected"), "{text}");
    assert_eq!(
        output.details["result"]["next_action"]["kind"],
        "agent_action"
    );
    assert_eq!(output.details["action"], "run");
    assert_eq!(output.details["id"], "agent-action-workflow");
    assert_eq!(output.details["status"], "active");
}

#[tokio::test]
async fn workflow_run_renders_subagent_action_contract() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_agent_action_workflow(temp.path());

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("agent-action-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::Subagents,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(
        text.contains("Workflow recommends subagent action: inspect [context]"),
        "{text}"
    );
    assert!(text.contains("subagent_artifact_mailbox"), "{text}");
    assert!(
        text.contains("workflow-agent-action-workflow-inspect"),
        "{text}"
    );
    assert!(
        text.contains("Launch this with the Subagent tool"),
        "{text}"
    );
    assert_eq!(output.details["result"]["execution_mode"], "subagents");
    assert_eq!(
        output.details["result"]["next_action"]["kind"],
        "subagent_action"
    );
    assert_eq!(
        output.details["result"]["next_action"]["contract"]["communication"]["channel"],
        "subagent_artifact_mailbox"
    );
    assert_eq!(
        output.details["result"]["next_action"]["input"]["child_run_id"],
        "workflow-agent-action-workflow-inspect"
    );
}

#[tokio::test]
async fn workflow_run_renders_subagent_batch_for_parallel_action_steps() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_parallel_action_workflow(temp.path(), false);

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("parallel-action-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::Subagents,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(
        text.contains("Workflow recommends 3 parallel subagent action(s)."),
        "{text}"
    );
    assert!(text.contains("- cli [build]: CLI coder"), "{text}");
    assert!(text.contains("- core [build]: Core coder"), "{text}");
    assert_eq!(
        output.details["result"]["next_action"]["kind"],
        "subagent_batch"
    );
    assert_eq!(
        output.details["result"]["next_action"]["assignments"]
            .as_array()
            .expect("assignments")
            .len(),
        3
    );
}

#[tokio::test]
async fn workflow_run_holds_back_overlapping_subagent_write_scope() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_parallel_action_workflow(temp.path(), true);

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("parallel-action-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::Subagents,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(
        text.contains("Workflow recommends 2 parallel subagent action(s)."),
        "{text}"
    );
    assert!(text.contains("Held back:"), "{text}");
    assert!(text.contains("write scope overlaps with cli"), "{text}");
    assert_eq!(
        output.details["result"]["next_action"]["assignments"]
            .as_array()
            .expect("assignments")
            .len(),
        2
    );
    assert_eq!(
        output.details["result"]["next_action"]["held_back"]
            .as_array()
            .expect("held back")
            .len(),
        1
    );
}

#[tokio::test]
async fn workflow_run_blocks_missing_action_contract() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_missing_action_contract_workflow(temp.path());

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("missing-action-workflow"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run succeeds");
    let text = output.text_content().expect("text output");
    assert!(
        text.contains("Workflow blocked: missing action contract for step inspect [context]."),
        "{text}"
    );
    assert!(
        text.contains("Add command checks, a child workflow, worker, or action contract."),
        "{text}"
    );
    assert_eq!(
        output.details["result"]["next_action"]["kind"],
        "missing_action_contract"
    );
}

#[test]
fn workflow_validation_rejects_bad_action_contract() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = write_invalid_action_workflow(temp.path());
    let (_, root, doc) = load_selected_workflow(&workflows_root, Some("invalid-action-workflow"))
        .expect("workflow loads");
    let diagnostics = validate_workflow(&doc, &ValidateOptions::strict(root));
    let messages = diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("steps.inspect.action.objective: action objective must not be empty"),
        "{messages}"
    );
    assert!(
        messages.contains("steps.inspect.action.worker: unknown worker `missing_worker`"),
        "{messages}"
    );
    assert!(
        messages.contains("steps.inspect.action.completion.checks: unknown check `missing_check`"),
        "{messages}"
    );
}

#[tokio::test]
async fn workflow_run_reports_validation_diagnostics() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);
    let workflow_path = workflows_root
        .join("implement-workflow-run-engine")
        .join("workflow.yaml");
    let raw = std::fs::read_to_string(&workflow_path)
        .expect("fixture copied")
        .replace(
            "checks:\n      - implementation_ready",
            "checks:\n      - missing_check",
        );
    std::fs::write(&workflow_path, raw).expect("write broken fixture");

    let ctx = test_ctx(temp.path());
    let output = run_action(
        &workflows_root,
        Some("implement-workflow-run-engine"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    .expect("run returns diagnostics instead of error");
    let text = output.text_content().expect("text output");
    assert!(text.contains("blocked by validation diagnostics"), "{text}");
    assert!(text.contains("unknown check `missing_check`"), "{text}");
}

#[test]
fn workflow_update_status_updates_yaml_and_appends_event() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-update-events", &workflows_root);

    let ctx = test_ctx(temp.path());
    let output = update_action(
        &workflows_root,
        Some("implement-workflow-update-events"),
        &json!({
            "path": "steps.execute.status",
            "value": "done",
            "reason": "unit test completed execute step"
        }),
        &ctx,
    )
    .expect("update succeeds");
    assert!(
        output
            .text_content()
            .expect("text output")
            .contains("Updated workflow")
    );

    let workflow_path = workflows_root
        .join("implement-workflow-update-events")
        .join("workflow.yaml");
    let doc = load_workflow(&workflow_path).expect("updated workflow should load");
    assert!(matches!(
        doc.steps.get("execute").expect("step exists").status,
        crate::workflow::StepStatus::Done
    ));

    let events = std::fs::read_to_string(
        workflows_root
            .join("implement-workflow-update-events")
            .join("events.jsonl"),
    )
    .expect("events should be written");
    assert!(events.contains("steps.execute.status"));
    assert!(events.contains("unit test completed execute step"));
}

#[tokio::test]
async fn workflow_tool_execute_enforces_mode_action_policy() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);

    let mut ctx = test_ctx(temp.path());
    ctx.mode = crate::config::AgentMode::Auditor;
    let output = WorkflowTool
        .execute(
            "test-call",
            json!({
                "action": "update",
                "id": "implement-workflow-run-engine",
                "path": "steps.execute.status",
                "value": "done",
                "reason": "unit test should be blocked"
            }),
            ctx,
        )
        .await
        .expect("policy denial returns tool output");

    assert!(output.is_error);
    let text = output.text_content().expect("text output");
    assert!(text.contains("not available in auditor mode"), "{text}");
    assert!(
        !workflows_root
            .join("implement-workflow-run-engine")
            .join("events.jsonl")
            .exists()
    );
}

#[test]
fn workflow_update_rejects_invalid_status_without_writing() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-update-events", &workflows_root);
    let workflow_path = workflows_root
        .join("implement-workflow-update-events")
        .join("workflow.yaml");
    let before = std::fs::read_to_string(&workflow_path).expect("fixture copied");

    let ctx = test_ctx(temp.path());
    let error = match update_action(
        &workflows_root,
        Some("implement-workflow-update-events"),
        &json!({
            "path": "steps.execute.status",
            "value": "not_a_status",
            "reason": "unit test invalid status"
        }),
        &ctx,
    ) {
        Ok(_) => panic!("invalid status should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("invalid YAML/schema"));
    let after = std::fs::read_to_string(&workflow_path).expect("fixture remains");
    assert_eq!(before, after);
    assert!(
        !workflows_root
            .join("implement-workflow-update-events")
            .join("events.jsonl")
            .exists()
    );
}

#[tokio::test]
async fn workflow_rejects_absolute_or_parent_directory_ids() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-update-events", &workflows_root);
    let ctx = test_ctx(temp.path());

    let show_error = match show_action(
        &workflows_root,
        Some("../implement-workflow-update-events"),
        WorkflowValidationModeParam::Strict,
    ) {
        Ok(_) => panic!("parent traversal id should fail"),
        Err(error) => error,
    };
    assert!(show_error.to_string().contains("invalid workflow id"));

    let run_error = match run_action(
        &workflows_root,
        Some("/tmp/implement-workflow-update-events"),
        WorkflowValidationModeParam::Strict,
        WorkflowExecutionMode::MainAgent,
        &ctx,
    )
    .await
    {
        Ok(_) => panic!("absolute id should fail"),
        Err(error) => error,
    };
    assert!(run_error.to_string().contains("invalid workflow id"));

    let nested_error = match show_action(
        &workflows_root,
        Some("nested/implement-workflow-update-events"),
        WorkflowValidationModeParam::Strict,
    ) {
        Ok(_) => panic!("nested id should fail"),
        Err(error) => error,
    };
    assert!(nested_error.to_string().contains("invalid workflow id"));

    let update_error = match update_action(
        &workflows_root,
        Some("../implement-workflow-update-events"),
        &json!({
            "path": "steps.execute.status",
            "value": "done",
            "reason": "unit test invalid id"
        }),
        &ctx,
    ) {
        Ok(_) => panic!("update traversal id should fail"),
        Err(error) => error,
    };
    assert!(update_error.to_string().contains("invalid workflow id"));
}

#[test]
fn workflow_update_rejects_unwritable_event_log_without_replacing_yaml() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflows_root = temp.path().join(".imp/workflows");
    copy_workflow_fixture("implement-workflow-update-events", &workflows_root);
    let workflow_root = workflows_root.join("implement-workflow-update-events");
    let workflow_path = workflow_root.join("workflow.yaml");
    let before = std::fs::read_to_string(&workflow_path).expect("fixture copied");
    std::fs::create_dir(workflow_root.join("events.jsonl"))
        .expect("create conflicting event log directory");

    let ctx = test_ctx(temp.path());
    let error = match update_action(
        &workflows_root,
        Some("implement-workflow-update-events"),
        &json!({
            "path": "steps.execute.status",
            "value": "done",
            "reason": "unit test event log failure"
        }),
        &ctx,
    ) {
        Ok(_) => panic!("unwritable event log should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("Is a directory"), "{error}");
    let after = std::fs::read_to_string(&workflow_path).expect("fixture remains");
    assert_eq!(before, after);
    assert!(!workflow_path.with_extension("yaml.tmp").exists());
}

fn set_step_status(workflows_root: &Path, workflow_id: &str, step_id: &str, status: &str) {
    let workflow_path = workflows_root.join(workflow_id).join("workflow.yaml");
    let raw = std::fs::read_to_string(&workflow_path).expect("fixture copied");
    let marker = format!("  {step_id}:\n");
    let start = raw.find(&marker).expect("step marker exists");
    let rest = &raw[start..];
    let status_marker = "    status: ";
    let status_start =
        start + rest.find(status_marker).expect("step status exists") + status_marker.len();
    let status_end = raw[status_start..]
        .find('\n')
        .map(|offset| status_start + offset)
        .expect("status line ends");
    let mut updated = raw;
    updated.replace_range(status_start..status_end, status);
    std::fs::write(&workflow_path, updated).expect("write fixture");
}

fn test_ctx(dir: &Path) -> ToolContext {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    ToolContext {
        cwd: dir.to_path_buf(),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: Arc::new(crate::ui::NullInterface),
        file_cache: Arc::new(crate::tools::FileCache::new()),
        checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
        anchor_store: Arc::new(crate::tools::AnchorStore::new()),
        lua_tool_loader: None,
        mode: crate::config::AgentMode::Full,
        read_max_lines: 500,
        turn_workflow_review: Arc::new(std::sync::Mutex::new(
            crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
        )),
        config: Arc::new(crate::config::Config::default()),
        run_policy: Default::default(),
        supporting_provenance: Vec::new(),
    }
}

fn write_command_check_workflow(root: &Path, command_succeeds: bool) -> (PathBuf, PathBuf) {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("command-check-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    let command = if command_succeeds { "true" } else { "false" };
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        format!(
            r#"schema: imp.workflow/v1
id: command-check-workflow
title: Command check workflow
status: active
kind: implementation
settings:
  worktree: none
  strictness: medium
  durable: true
  disposable: false
  commit_traces: false
spec:
  goal: Run command checks.
  acceptance:
command_check_passes:
  text: Command check passes.
  status: todo
  checks:
    - command_check
steps:
  verify:
kind: verify
status: ready
checks:
  - command_check
checks:
  command_check:
kind: command
status: pending
command: {command}
results:
  path: .imp/workflows/command-check-workflow/results.md
workers: {{}}
closeout:
  done:
requires:
  - command_check
"#
        ),
    )
    .expect("write workflow");
    (workflows_root, workflow_root)
}

fn write_presence_absence_workflow(root: &Path) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("presence-absence-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        r#"schema: imp.workflow/v1
id: presence-absence-workflow
title: Presence absence workflow
status: active
kind: implementation
spec:
  goal: Run presence and absence checks.
  acceptance:
content_checked:
  text: Content is checked.
  status: todo
  checks: [has_alpha, no_gamma]
steps:
  verify:
kind: verify
status: ready
checks: [has_alpha, no_gamma]
checks:
  has_alpha:
kind: presence
status: pending
path: subject.txt
pattern: alpha
  no_gamma:
kind: absence
status: pending
path: subject.txt
pattern: gamma
results:
  path: .imp/workflows/presence-absence-workflow/results.md
workers: {}
closeout:
  done:
requires: [has_alpha, no_gamma]
"#,
    )
    .expect("write workflow");
    workflows_root
}

fn write_changed_files_workflow(root: &Path) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("changed-files-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        r#"schema: imp.workflow/v1
id: changed-files-workflow
title: Changed files workflow
status: active
kind: implementation
spec:
  goal: Run changed files check.
  acceptance:
source_changed:
  text: Source changed.
  status: todo
  checks: [source_changed]
steps:
  verify:
kind: verify
status: ready
checks: [source_changed]
checks:
  source_changed:
kind: changed_files
status: pending
paths: [tracked.txt]
results:
  path: .imp/workflows/changed-files-workflow/results.md
workers: {}
closeout:
  done:
requires: [source_changed]
"#,
    )
    .expect("write workflow");
    workflows_root
}

fn write_broad_only_build_workflow(root: &Path) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("broad-only-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        r#"schema: imp.workflow/v1
id: broad-only-workflow
title: Broad only workflow
status: active
kind: implementation
spec:
  goal: Broad command checks alone should not complete implementation.
  acceptance:
done:
  text: Work is done.
  status: todo
  checks: [broad_tests]
steps:
  implement:
kind: build
status: ready
checks: [broad_tests]
checks:
  broad_tests:
kind: command
status: pending
broad: true
command: true
results:
  path: .imp/workflows/broad-only-workflow/results.md
workers: {}
closeout:
  done:
requires: [broad_tests]
"#,
    )
    .expect("write workflow");
    workflows_root
}

fn init_git_repo(root: &Path) {
    git(root, &["init"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test User"]);
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_no_runnable_workflow(root: &Path) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("no-runnable-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        r#"schema: imp.workflow/v1
id: no-runnable-workflow
title: No runnable workflow
status: active
kind: test
spec:
  goal: Report readiness when blocked.
  acceptance:
done:
  text: Readiness is reported.
  status: todo
steps:
  inspect:
kind: context
status: active
  verify:
kind: verify
status: todo
depends_on:
  - inspect
checks: {}
results:
  path: .imp/workflows/no-runnable-workflow/results.md
workers: {}
closeout:
  done:
requires:
  - no_unapproved_goal_or_acceptance_changes
"#,
    )
    .expect("write workflow");
    workflows_root
}

fn write_agent_action_workflow(root: &Path) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("agent-action-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        r#"schema: imp.workflow/v1
id: agent-action-workflow
title: Agent action workflow
status: active
kind: implementation
spec:
  goal: Dispatch agent action.
  acceptance:
    inspected:
      text: Agent action is dispatched.
      status: todo
      checks:
        - inspected
context: {}
steps:
  inspect:
    kind: context
    status: ready
    checks:
      - inspected
    action:
      kind: agent
      role: reviewer
      objective: Review workflow action support.
      instructions:
        - Read the workflow runner.
        - Report the action contract.
      output:
        required_sections:
          - Decision
          - Evidence
      review:
        required: true
        rubric:
          - Check acceptance criteria.
        output:
          required_sections:
            - Blocking Findings
      write_scope:
        - .imp/workflows/agent-action-workflow/artifacts/inspection.md
      completion:
        checks:
          - inspected
        artifacts:
          - .imp/workflows/agent-action-workflow/artifacts/inspection.md
prototypes: {}
checks:
  inspected:
    kind: review
    status: pending
    question: Was the agent action inspected?
results:
  path: .imp/workflows/agent-action-workflow/results.md
workers: {}
closeout:
  done:
    requires:
      - inspected
"#,
    )
    .expect("write workflow");
    workflows_root
}

fn write_parallel_action_workflow(root: &Path, overlap: bool) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("parallel-action-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    let docs_scope = if overlap {
        "crates/imp-cli/src/lib.rs"
    } else {
        "docs/workflows.md"
    };
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        format!(
            r#"schema: imp.workflow/v1
id: parallel-action-workflow
title: Parallel action workflow
status: active
kind: test
spec:
  goal: Dispatch parallel action steps.
  acceptance:
done:
  text: Parallel actions are dispatched.
  status: todo
  checks:
    - cli_done
    - core_done
    - docs_done
steps:
  cli:
kind: build
status: ready
checks:
  - cli_done
action:
  kind: agent
  role: CLI coder
  objective: Update CLI workflow behavior.
  write_scope:
    - crates/imp-cli/src/lib.rs
  completion:
    checks:
      - cli_done
  core:
kind: build
status: ready
checks:
  - core_done
action:
  kind: agent
  role: Core coder
  objective: Update core workflow behavior.
  write_scope:
    - crates/imp-core/src/tools/workflow.rs
  completion:
    checks:
      - core_done
  docs:
kind: build
status: ready
checks:
  - docs_done
action:
  kind: agent
  role: Docs writer
  objective: Update workflow docs.
  write_scope:
    - {docs_scope}
  completion:
    checks:
      - docs_done
checks:
  cli_done:
kind: review
status: pending
  core_done:
kind: review
status: pending
  docs_done:
kind: review
status: pending
results:
  path: .imp/workflows/parallel-action-workflow/results.md
workers: {{}}
closeout:
  done:
requires:
  - cli_done
  - core_done
  - docs_done
"#
        ),
    )
    .expect("write workflow");
    workflows_root
}

fn write_missing_action_contract_workflow(root: &Path) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("missing-action-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        r#"schema: imp.workflow/v1
id: missing-action-workflow
title: Missing action workflow
status: active
kind: implementation
spec:
  goal: Report missing action contract.
  acceptance:
inspected:
  text: Missing action is reported.
  status: todo
  checks:
    - inspected
steps:
  inspect:
kind: context
status: ready
checks:
  - inspected
checks:
  inspected:
kind: review
status: pending
results:
  path: .imp/workflows/missing-action-workflow/results.md
workers: {}
closeout:
  done:
requires:
  - inspected
"#,
    )
    .expect("write workflow");
    workflows_root
}

fn write_invalid_action_workflow(root: &Path) -> PathBuf {
    let workflows_root = root.join(".imp/workflows");
    let workflow_root = workflows_root.join("invalid-action-workflow");
    std::fs::create_dir_all(&workflow_root).expect("create workflow root");
    std::fs::write(
        workflow_root.join("workflow.yaml"),
        r#"schema: imp.workflow/v1
id: invalid-action-workflow
title: Invalid action workflow
status: active
kind: implementation
spec:
  goal: Validate action contracts.
  acceptance:
inspected:
  text: Invalid action is rejected.
  status: todo
  checks:
    - inspected
steps:
  inspect:
kind: context
status: ready
action:
  kind: worker
  worker: missing_worker
  objective: ""
  completion:
    checks:
      - missing_check
checks:
  inspected:
kind: review
status: pending
results:
  path: .imp/workflows/invalid-action-workflow/results.md
workers: {}
closeout:
  done:
requires:
  - inspected
"#,
    )
    .expect("write workflow");
    workflows_root
}

fn copy_workflow_fixture(id: &str, workflows_root: &Path) {
    let source = repo_root()
        .join(".imp/workflows")
        .join(id)
        .join("workflow.yaml");
    let destination_dir = workflows_root.join(id);
    std::fs::create_dir_all(&destination_dir).expect("create fixture workflow dir");
    std::fs::copy(source, destination_dir.join("workflow.yaml")).expect("copy fixture workflow");

    if id == "implement-workflow-update-events" || id == "implement-workflow-run-engine" {
        copy_workflow_fixture("prototype-imp-workflow-engine", workflows_root);
        for relative_path in ["artifacts/plan.md", "results.md"] {
            let path = destination_dir.join(relative_path);
            std::fs::create_dir_all(path.parent().expect("fixture artifact parent"))
                .expect("create fixture artifact dir");
            std::fs::write(path, "fixture artifact").expect("write fixture artifact");
        }

        let project_root = workflows_root
            .parent()
            .and_then(Path::parent)
            .expect("workflow root should be under .imp/workflows");
        let source_artifact = project_root.join("crates/imp-core/src/tools/workflow.rs");
        std::fs::create_dir_all(source_artifact.parent().expect("source artifact parent"))
            .expect("create source artifact dir");
        std::fs::write(source_artifact, "fixture source artifact").expect("write source artifact");
    }
}
