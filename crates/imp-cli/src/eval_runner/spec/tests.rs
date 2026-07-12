use super::*;

fn valid_spec() -> EvalTaskSpec {
    EvalTaskSpec {
        id: "rename-method".to_string(),
        repo: "https://example.test/repo.git".to_string(),
        commit: "a".repeat(40),
        prompt: "Rename the method and update callers.".to_string(),
        verifier: "cargo test rename_method".to_string(),
        fixture: None,
        expectations: EvalExpectations::default(),
        setup: None,
        max_turns: None,
        timeout_seconds: None,
        verifier_timeout_seconds: None,
    }
}

#[test]
fn execution_prompt_includes_verification_command() {
    let spec = valid_spec();
    assert_eq!(
            spec.execution_prompt(),
            "Rename the method and update callers.\n\nRequired verification command: `cargo test rename_method`"
        );
}

#[test]
fn validation_accepts_pinned_task_with_concrete_verifier() {
    let validation = valid_spec().validate(Path::new("rename-method.json"));
    assert!(validation.valid, "{:?}", validation.errors);
}

#[test]
fn validation_rejects_moving_commit_and_placeholder_verifier() {
    let mut spec = valid_spec();
    spec.commit = "main".to_string();
    spec.verifier = "repo test command TBD".to_string();

    let validation = spec.validate(Path::new("rename-method.json"));

    assert!(!validation.valid);
    assert!(validation
        .errors
        .iter()
        .any(|error| error.contains("40-character")));
    assert!(validation
        .errors
        .iter()
        .any(|error| error.contains("unresolved")));
}

#[test]
fn fixture_task_does_not_require_remote_source() {
    let dir = tempfile::tempdir().unwrap();
    let tasks = dir.path().join("tasks");
    let fixture = dir.path().join("fixtures/fixture-task");
    fs::create_dir_all(&tasks).unwrap();
    fs::create_dir_all(&fixture).unwrap();
    let spec = EvalTaskSpec {
        id: "fixture-task".into(),
        repo: String::new(),
        commit: String::new(),
        prompt: "Fix it".into(),
        verifier: "python3 -m unittest".into(),
        fixture: Some(PathBuf::from("../fixtures/fixture-task")),
        expectations: EvalExpectations::default(),
        setup: None,
        max_turns: None,
        timeout_seconds: None,
        verifier_timeout_seconds: None,
    };

    let validation = spec.validate(&tasks.join("fixture-task.json"));

    assert!(validation.valid, "{:?}", validation.errors);
}

#[test]
fn validation_rejects_parent_traversal_in_expected_paths() {
    let mut spec = valid_spec();
    spec.expectations.required_changed_paths = vec!["../outside".into()];
    let validation = spec.validate(Path::new("rename-method.json"));
    assert!(validation
        .errors
        .iter()
        .any(|error| error.contains("repository-relative")));
}

#[test]
fn validation_rejects_unsafe_task_id() {
    let mut spec = valid_spec();
    spec.id = "../outside".into();
    let validation = spec.validate(Path::new("../outside.json"));
    assert!(validation
        .errors
        .iter()
        .any(|error| error.contains("ASCII letters")));
}

#[test]
fn task_paths_are_stably_sorted() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("z.json"), "{}").unwrap();
    fs::write(dir.path().join("a.json"), "{}").unwrap();
    fs::write(dir.path().join("ignore.md"), "x").unwrap();

    let paths = task_paths(dir.path()).unwrap();

    assert_eq!(
        paths
            .iter()
            .filter_map(|path| path.file_name().and_then(|value| value.to_str()))
            .collect::<Vec<_>>(),
        vec!["a.json", "z.json"]
    );
}
