use super::*;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn workflow_root(id: &str) -> PathBuf {
    repo_root().join(".imp/workflows").join(id)
}

fn load_fixture(id: &str) -> WorkflowDocument {
    load_workflow(&workflow_root(id).join("workflow.yaml")).expect("fixture should load")
}

fn validate_fixture(id: &str) -> Vec<WorkflowDiagnostic> {
    let doc = load_fixture(id);
    validate_workflow(&doc, &ValidateOptions::strict(workflow_root(id)))
}

#[test]
fn workflow_schema_dogfood_workflows_parse_and_validate() {
    for id in [
        "prototype-imp-workflow-engine",
        "define-workflow-schema",
        "prototype-rust-workflow-schema-parser",
    ] {
        let diagnostics = validate_fixture(id);
        assert_eq!(
            diagnostics,
            Vec::new(),
            "{id} diagnostics: {diagnostics:#?}"
        );
    }
}

#[test]
fn workflow_schema_reference_validation_rejects_missing_refs() {
    let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
    doc.steps
        .get_mut("add_schema_module")
        .expect("step exists")
        .depends_on
        .push("missing_step".to_owned());
    doc.steps
        .get_mut("add_schema_module")
        .expect("step exists")
        .checks
        .push("missing_check".to_owned());
    doc.steps
        .get_mut("add_schema_module")
        .expect("step exists")
        .worker = Some("missing_worker".to_owned());
    doc.steps
        .get_mut("record_parser_considerations")
        .expect("step exists")
        .prototypes
        .push("missing_prototype".to_owned());
    doc.closeout
        .done
        .requires
        .push("missing_closeout_check".to_owned());

    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(workflow_root("prototype-rust-workflow-schema-parser")),
    );
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("unknown step `missing_step`"),
        "{diagnostics:#?}"
    );
    assert!(
        messages.contains("unknown check `missing_check`"),
        "{diagnostics:#?}"
    );
    assert!(
        messages.contains("unknown worker `missing_worker`"),
        "{diagnostics:#?}"
    );
    assert!(
        messages.contains("unknown prototype `missing_prototype`"),
        "{diagnostics:#?}"
    );
    assert!(
        messages.contains("unknown check or built-in predicate `missing_closeout_check`"),
        "{diagnostics:#?}"
    );
}

#[test]
fn workflow_schema_shape_validation_rejects_bad_status_and_acceptance() {
    let yaml = r#"
schema: imp.workflow/v1
id: broken
title: Broken
status: active
kind: test
settings: {}
spec:
  goal: Broken workflow.
  acceptance:
    missing_status:
      text: This should fail.
context: {}
steps:
  first:
    kind: context
    status: nope
prototypes: {}
checks: {}
workers: {}
results:
  path: .imp/workflows/broken/results.md
closeout:
  done:
    requires: []
"#;

    let error = serde_yaml::from_str::<WorkflowDocument>(yaml)
        .expect_err("invalid status and missing acceptance status should fail during parse");
    let message = error.to_string();
    assert!(message.contains("status"), "{message}");
}

#[test]
fn workflow_readiness_explains_runnable_and_blocked_steps() {
    let yaml = r#"
schema: imp.workflow/v1
id: readiness
title: Readiness
status: active
kind: test
settings: {}
spec:
  goal: Test readiness.
  acceptance:
    done:
      text: Work is ready.
      status: todo
context: {}
steps:
  inspect:
    kind: context
    status: done
  build:
    kind: build
    status: todo
    depends_on: [inspect]
    worker: missing_builder
    checks: [pending_check]
  verify:
    kind: verify
    status: todo
    depends_on: [build]
  missing_dep:
    kind: verify
    status: todo
    depends_on: [does_not_exist]
  active_step:
    kind: build
    status: active
  failed_step:
    kind: build
    status: failed
prototypes: {}
checks:
  pending_check:
    kind: command
    status: pending
    command: cargo test -p imp-core workflow
workers: {}
results:
  path: .imp/workflows/readiness/results.md
closeout:
  done:
    requires: [pending_check]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let readiness = workflow_step_readiness(&doc);
    let find = |step: &str| {
        readiness
            .iter()
            .find(|entry| entry.step == step)
            .unwrap_or_else(|| panic!("missing readiness for {step}"))
    };

    assert_eq!(find("inspect").state, WorkflowReadinessState::Terminal);
    assert_eq!(find("build").state, WorkflowReadinessState::Blocked);
    assert!(find("build").reasons.iter().any(|reason| {
        reason.kind == WorkflowReadinessReasonKind::WorkerMissing
            && reason.subject.as_deref() == Some("missing_builder")
    }));
    assert!(find("build").reasons.iter().any(|reason| {
        reason.kind == WorkflowReadinessReasonKind::CheckPending
            && reason.subject.as_deref() == Some("pending_check")
    }));
    assert_eq!(find("verify").state, WorkflowReadinessState::Waiting);
    assert!(find("verify").reasons.iter().any(|reason| {
        reason.kind == WorkflowReadinessReasonKind::DependencyNotReady
            && reason.subject.as_deref() == Some("build")
    }));
    assert_eq!(find("missing_dep").state, WorkflowReadinessState::Blocked);
    assert!(find("missing_dep").reasons.iter().any(|reason| {
        reason.kind == WorkflowReadinessReasonKind::DependencyMissing
            && reason.subject.as_deref() == Some("does_not_exist")
    }));
    assert_eq!(find("active_step").state, WorkflowReadinessState::Waiting);
    assert!(
        find("active_step")
            .reasons
            .iter()
            .any(|reason| { reason.kind == WorkflowReadinessReasonKind::StatusNotRunnable })
    );
    assert_eq!(find("failed_step").state, WorkflowReadinessState::Terminal);
}

#[test]
fn workflow_readiness_preserves_next_runnable_steps_compatibility() {
    let yaml = r#"
schema: imp.workflow/v1
id: readiness-compat
title: Readiness compatibility
status: active
kind: test
settings: {}
spec:
  goal: Test next runnable compatibility.
  acceptance:
    done:
      text: Work is ready.
      status: todo
context: {}
steps:
  add_schema_module:
    kind: build
    status: todo
  add_validation_tests:
    kind: verify
    status: todo
    depends_on: [add_schema_module]
prototypes: {}
checks: {}
workers: {}
results:
  path: .imp/workflows/readiness-compat/results.md
closeout:
  done:
    requires: []
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");

    let readiness = workflow_step_readiness(&doc)
        .into_iter()
        .filter(|entry| matches!(entry.state, WorkflowReadinessState::Runnable))
        .map(|entry| entry.step)
        .collect::<Vec<_>>();

    assert_eq!(readiness, vec!["add_schema_module".to_owned()]);
    assert_eq!(next_runnable_steps(&doc), readiness);
}

#[test]
fn workflow_schema_next_runnable_steps_respects_dependencies() {
    let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
    doc.steps
        .get_mut("add_schema_module")
        .expect("step exists")
        .status = StepStatus::Todo;
    doc.steps
        .get_mut("add_validation_tests")
        .expect("step exists")
        .status = StepStatus::Todo;

    assert_eq!(
        next_runnable_steps(&doc),
        vec!["add_schema_module".to_owned()]
    );
}

#[test]
fn evidence_quality_check_kinds() {
    let yaml = r#"
schema: imp.workflow/v1
id: evidence-quality
title: Evidence Quality
status: active
kind: implementation
settings: {}
spec:
  goal: Validate evidence check kinds.
  acceptance:
    done:
      text: Evidence is checked.
      status: todo
      checks: [source_changed, classifier_removed, prompt_present]
context: {}
steps:
  implement:
    kind: build
    status: todo
    checks: [source_changed, classifier_removed, prompt_present]
prototypes: {}
checks:
  source_changed:
    kind: changed_files
    status: pending
    paths: [crates/imp-core/src/workflow/schema.rs]
  classifier_removed:
    kind: absence
    status: pending
    path: crates/imp-core/src/agent/mod.rs
    pattern: classify_stop_reason_from_text
  prompt_present:
    kind: presence
    status: pending
    file: crates/imp-core/src/system_prompt.rs
    pattern: Tool routing
workers: {}
results:
  path: .imp/workflows/evidence-quality/results.md
closeout:
  done:
    requires: [source_changed]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(PathBuf::from(".imp/workflows/evidence-quality")),
    );
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:#?}");

    let broken_yaml = yaml
        .replace(
            "paths: [crates/imp-core/src/workflow/schema.rs]",
            "paths: []",
        )
        .replace("pattern: classify_stop_reason_from_text", "pattern: ''")
        .replace(
            "file: crates/imp-core/src/system_prompt.rs",
            "question: missing file",
        );
    let broken_doc: WorkflowDocument = serde_yaml::from_str(&broken_yaml).expect("workflow parses");
    let diagnostics = validate_workflow(
        &broken_doc,
        &ValidateOptions::draft(PathBuf::from(".imp/workflows/evidence-quality")),
    );
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("changed_files check must include at least one path"),
        "{diagnostics:#?}"
    );
    assert!(
        messages.contains("presence/absence check must include a non-empty pattern"),
        "{diagnostics:#?}"
    );
    assert!(
        messages.contains("presence/absence check must include path or file"),
        "{diagnostics:#?}"
    );
}

#[test]
fn weak_implementation_checks_are_rejected() {
    let yaml = r#"
schema: imp.workflow/v1
id: weak-implementation
title: Weak Implementation
status: active
kind: implementation
settings: {}
spec:
  goal: Reject broad checks alone.
  acceptance:
    done:
      text: Work is done.
      status: todo
      checks: [broad_tests]
context: {}
steps:
  implement:
    kind: build
    status: todo
    checks: [broad_tests]
prototypes: {}
checks:
  broad_tests:
    kind: command
    status: pending
    broad: true
    command: cargo test -p imp-core --lib
workers: {}
results:
  path: .imp/workflows/weak-implementation/results.md
closeout:
  done:
    requires: [broad_tests]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(PathBuf::from(".imp/workflows/weak-implementation")),
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path == "steps.implement.checks"
                && diagnostic.message.contains("broad command checks alone")),
        "{diagnostics:#?}"
    );
}

#[test]
fn implementation_steps_require_change_sensitive_evidence() {
    let yaml = r#"
schema: imp.workflow/v1
id: strong-implementation
title: Strong Implementation
status: active
kind: implementation
settings: {}
spec:
  goal: Accept change-sensitive evidence.
  acceptance:
    done:
      text: Work is done.
      status: todo
      checks: [source_changed, broad_tests]
context: {}
steps:
  implement:
    kind: build
    status: todo
    checks: [source_changed, broad_tests]
prototypes: {}
checks:
  source_changed:
    kind: changed_files
    status: pending
    paths: [crates/imp-core/src/workflow/schema.rs]
  broad_tests:
    kind: command
    status: pending
    broad: true
    command: cargo test -p imp-core --lib
workers: {}
results:
  path: .imp/workflows/strong-implementation/results.md
closeout:
  done:
    requires: [broad_tests]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(PathBuf::from(".imp/workflows/strong-implementation")),
    );
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:#?}");
}

#[test]
fn workflow_iteration_schema_parses_loop_until_done_strategy() {
    let yaml = r#"
schema: imp.workflow/v1
id: loop-workflow
title: Loop Workflow
status: active
kind: investigation
settings: {}
strategy:
  kind: loop_until_done
  max_rounds: 3
  stop_when: [all_checks_pass, no_new_findings]
spec:
  goal: Investigate until done.
  acceptance:
    done:
      text: Done.
      status: todo
      checks: [reviewed]
context: {}
steps:
  investigate:
    kind: context
    status: todo
    checks: [reviewed]
prototypes: {}
checks:
  reviewed:
    kind: review
    status: pending
    question: Investigation complete?
workers: {}
results:
  path: .imp/workflows/loop-workflow/results.md
closeout:
  done:
    requires: [reviewed]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let strategy = doc.strategy.as_ref().expect("strategy exists");
    let WorkflowStrategy::LoopUntilDone {
        max_rounds,
        stop_when,
    } = strategy;
    assert_eq!(*max_rounds, 3);
    assert_eq!(
        stop_when,
        &vec![
            WorkflowStopCondition::AllChecksPass,
            WorkflowStopCondition::NoNewFindings
        ]
    );

    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(PathBuf::from(".imp/workflows/loop-workflow")),
    );
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:#?}");
}

#[test]
fn workflow_iteration_runtime_rejects_unbounded_loop_strategy() {
    let yaml = r#"
schema: imp.workflow/v1
id: unbounded-loop-workflow
title: Unbounded Loop Workflow
status: active
kind: investigation
settings: {}
strategy:
  kind: loop_until_done
  max_rounds: 0
  stop_when: []
spec:
  goal: Investigate until done.
  acceptance:
    done:
      text: Done.
      status: todo
      checks: [reviewed]
context: {}
steps:
  investigate:
    kind: context
    status: todo
    checks: [reviewed]
prototypes: {}
checks:
  reviewed:
    kind: review
    status: pending
    question: Investigation complete?
workers: {}
results:
  path: .imp/workflows/unbounded-loop-workflow/results.md
closeout:
  done:
    requires: [reviewed]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(PathBuf::from(
            ".imp/workflows/unbounded-loop-workflow",
        )),
    );

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.path == "strategy.max_rounds"
                && diagnostic.message.contains("greater than 0")
        }),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.path == "strategy.stop_when"
                && diagnostic.message.contains("at least one stop condition")
        }),
        "{diagnostics:#?}"
    );
}

#[test]
fn workflow_adversarial_schema_parses_review_requirement() {
    let yaml = r#"
schema: imp.workflow/v1
id: adversarial-review-schema
title: Adversarial Review Schema
status: active
kind: implementation
settings: {}
spec:
  goal: Review support parses.
  acceptance:
    done:
      text: Done.
      status: todo
      checks: [source_changed, review_done]
context: {}
steps:
  implement:
    kind: build
    status: todo
    checks: [source_changed, review_done]
    action:
      kind: agent
      objective: Implement the change.
      write_scope: [crates/imp-core/src/workflow/schema.rs]
      completion:
        checks: [review_done]
      review:
        required: true
        role: reviewer
        rubric:
        - Check evidence.
        output:
          required_sections: [Decision, Evidence]
prototypes: {}
checks:
  source_changed:
    kind: changed_files
    status: pending
    paths: [crates/imp-core/src/workflow/schema.rs]
  review_done:
    kind: review
    status: pending
    question: Did review pass?
workers: {}
results:
  path: .imp/workflows/adversarial-review-schema/results.md
closeout:
  done:
    requires: [review_done]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let action = doc
        .steps
        .get("implement")
        .and_then(|step| step.action.as_ref())
        .expect("action exists");
    let review = action.review.as_ref().expect("review requirement exists");

    assert!(review.required);
    assert_eq!(review.role.as_deref(), Some("reviewer"));
    assert_eq!(review.rubric, vec!["Check evidence."]);
    assert_eq!(
        review.output.required_sections,
        vec!["Decision", "Evidence"]
    );

    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(PathBuf::from(".imp/workflows/adversarial-review-schema")),
    );
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:#?}");
}

#[test]
fn workflow_adversarial_closeout_requires_review_check_gate() {
    let yaml = r#"
schema: imp.workflow/v1
id: adversarial-review-closeout
title: Adversarial Review Closeout
status: active
kind: implementation
settings: {}
spec:
  goal: Review support validates.
  acceptance:
    done:
      text: Done.
      status: todo
      checks: [tests_passed]
context: {}
steps:
  implement:
    kind: build
    status: todo
    action:
      kind: agent
      objective: Implement the change.
      write_scope: [crates/imp-core/src/workflow/schema.rs]
      completion:
        checks: [tests_passed]
      review:
        required: true
prototypes: {}
checks:
  tests_passed:
    kind: command
    status: pending
    command: cargo +nightly test -p imp-core workflow_adversarial_closeout --lib
workers: {}
results:
  path: .imp/workflows/adversarial-review-closeout/results.md
closeout:
  done:
    requires: [tests_passed]
"#;
    let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::draft(PathBuf::from(".imp/workflows/adversarial-review-closeout")),
    );

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.path == "steps.implement.action.review"
                && diagnostic
                    .message
                    .contains("required adversarial review must be gated")
        }),
        "{diagnostics:#?}"
    );
}

#[test]
fn workflow_schema_strict_validation_rejects_parent_mismatch() {
    let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
    doc.parent.as_mut().expect("parent exists").step = "plan_rust_validator".to_owned();

    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::strict(workflow_root("prototype-rust-workflow-schema-parser")),
    );

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("does not call workflow")),
        "{diagnostics:#?}"
    );
}

#[test]
fn workflow_schema_load_rejects_oversized_workflow_yaml() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = temp.path().join("workflow.yaml");
    std::fs::write(&path, vec![b'a'; (MAX_WORKFLOW_YAML_BYTES + 1) as usize])
        .expect("write oversized workflow");

    let error = load_workflow(&path).expect_err("oversized workflow should fail before parse");
    let message = error.to_string();
    assert!(message.contains("above the"), "{message}");
    assert!(message.contains("byte limit"), "{message}");
}

#[test]
fn workflow_schema_rejects_path_like_workflow_references() {
    let mut doc = load_fixture("define-workflow-schema");
    doc.steps
        .get_mut("prototype_rust_parser")
        .expect("workflow step exists")
        .workflow = Some("../prototype-rust-workflow-schema-parser".to_owned());

    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::strict(workflow_root("define-workflow-schema")),
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.path == "steps.prototype_rust_parser.workflow"
                && diagnostic
                    .message
                    .contains("must be a workflow directory name")
        }),
        "{diagnostics:#?}"
    );

    let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
    doc.parent.as_mut().expect("parent exists").workflow = "/tmp/parent".to_owned();

    let diagnostics = validate_workflow(
        &doc,
        &ValidateOptions::strict(workflow_root("prototype-rust-workflow-schema-parser")),
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.path == "parent.workflow"
                && diagnostic
                    .message
                    .contains("must be a workflow directory name")
        }),
        "{diagnostics:#?}"
    );
}
