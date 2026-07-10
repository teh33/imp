use std::fs;
use std::process::Command;

const WORKFLOW_YAML: &str = r#"schema: imp.workflow/v1
id: cli-fixture
title: CLI fixture workflow
status: active
kind: feature

spec:
  goal: Exercise workflow CLI commands.
  acceptance:
    done:
      text: CLI commands work.
      status: todo

steps:
  plan:
    kind: plan
    status: todo

checks:
  ready:
    kind: aggregate
    status: passed

workers: {}

results:
  path: .imp/workflows/cli-fixture/results.md
  required_claims: []

closeout:
  done:
    requires:
      - ready
  allowed_terminal_statuses:
    - done
    - blocked
"#;

#[test]
fn workflow_cli_lists_shows_validates_runs_and_updates() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflow_dir = temp.path().join(".imp/workflows/cli-fixture");
    fs::create_dir_all(&workflow_dir).expect("create workflow dir");
    fs::write(workflow_dir.join("workflow.yaml"), WORKFLOW_YAML).expect("write workflow fixture");

    assert_success_contains(
        temp.path(),
        &["workflow", "list"],
        "cli-fixture [active] CLI fixture workflow",
    );
    assert_success_contains(
        temp.path(),
        &["workflow", "show", "cli-fixture"],
        "Workflow: cli-fixture",
    );
    assert_success_contains(
        temp.path(),
        &["workflow", "validate", "cli-fixture"],
        "1 ok, 0 with diagnostics",
    );
    assert_success_contains(
        temp.path(),
        &["workflow", "run", "cli-fixture"],
        "Workflow blocked: missing action contract for step plan [plan]",
    );
    assert_success_contains(
        temp.path(),
        &[
            "workflow",
            "update",
            "cli-fixture",
            "steps.plan.status",
            "done",
            "--reason",
            "integration test",
        ],
        "Updated workflow `cli-fixture`",
    );

    let updated =
        fs::read_to_string(workflow_dir.join("workflow.yaml")).expect("read updated workflow");
    assert!(updated.contains("status: done"), "{updated}");

    let events = fs::read_to_string(workflow_dir.join("events.jsonl")).expect("read events");
    assert!(events.contains("integration test"), "{events}");
}

#[test]
fn workflow_cli_rejects_path_like_ids() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let workflow_dir = temp.path().join(".imp/workflows/cli-fixture");
    fs::create_dir_all(&workflow_dir).expect("create workflow dir");
    fs::write(workflow_dir.join("workflow.yaml"), WORKFLOW_YAML).expect("write workflow fixture");

    let output = imp_command()
        .current_dir(temp.path())
        .args(["workflow", "show", "../cli-fixture"])
        .output()
        .expect("run imp workflow show");

    assert!(!output.status.success(), "command unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid workflow id"), "{stderr}");
}

fn assert_success_contains(cwd: &std::path::Path, args: &[&str], expected: &str) {
    let output = imp_command()
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run imp {args:?}: {error}"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "imp {args:?} failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(expected),
        "imp {args:?} stdout did not contain {expected:?}\nstdout:\n{stdout}"
    );
}

fn imp_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_imp"))
}
