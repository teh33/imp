use super::*;
use serde::Deserialize;
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
struct GuardrailToml {
    guardrails: GuardrailConfig,
}

#[tokio::test]
async fn guardrail_command_closes_stdin() {
    let dir = TempDir::new().unwrap();
    let manager = ProcessManager::new();
    let output = execute(
        &manager,
        "if IFS= read -r line; then printf unexpected; else printf eof; fi",
        dir.path(),
        std::time::Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert!(output.success);
    assert_eq!(output.stdout, "eof");
}

#[tokio::test]
async fn guardrail_command_timeout_returns_error() {
    let dir = TempDir::new().unwrap();
    let started = std::time::Instant::now();

    let manager = ProcessManager::new();
    let result = execute(
        &manager,
        "sleep 5",
        dir.path(),
        std::time::Duration::from_millis(50),
    )
    .await;

    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(matches!(
        result,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut
    ));
}

#[test]
fn guardrail_toml_deserializes() {
    let parsed: GuardrailToml = toml::from_str(
        r#"
[guardrails]
enabled = true
level = "enforce"
profile = "zig"
critical_paths = ["src/**", "lib/**"]
after_write = ["zig fmt --check .", "zig build"]
"#,
    )
    .unwrap();

    assert_eq!(parsed.guardrails.enabled, Some(true));
    assert_eq!(parsed.guardrails.level, Some(GuardrailLevel::Enforce));
    assert_eq!(parsed.guardrails.profile, Some(GuardrailProfile::Zig));
    assert_eq!(
        parsed.guardrails.critical_paths,
        Some(vec!["src/**".into(), "lib/**".into()])
    );
    assert_eq!(
        parsed.guardrails.after_write,
        Some(vec!["zig fmt --check .".into(), "zig build".into()])
    );
}

#[test]
fn guardrail_auto_profile_resolves_zig() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("build.zig"), "").unwrap();

    let config = GuardrailConfig {
        profile: Some(GuardrailProfile::Auto),
        ..Default::default()
    };

    assert_eq!(
        config.resolve_effective_profile(dir.path()),
        GuardrailProfile::Zig
    );
}

#[test]
fn guardrail_auto_profile_resolves_rust_from_subdirectory() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname='x'\nversion='0.1.0'\n",
    )
    .unwrap();
    let nested = dir.path().join("src").join("nested");
    std::fs::create_dir_all(&nested).unwrap();

    let config = GuardrailConfig {
        profile: Some(GuardrailProfile::Auto),
        ..Default::default()
    };

    assert_eq!(
        config.resolve_effective_profile(&nested),
        GuardrailProfile::Rust
    );
}

#[test]
fn guardrail_auto_profile_resolves_go() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("go.mod"), "module example.com/test\n").unwrap();

    let config = GuardrailConfig {
        profile: Some(GuardrailProfile::Auto),
        ..Default::default()
    };

    assert_eq!(
        config.resolve_effective_profile(dir.path()),
        GuardrailProfile::Go
    );
}

#[test]
fn guardrail_auto_profile_resolves_elixir() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("mix.exs"),
        "defmodule Demo.MixProject do end\n",
    )
    .unwrap();

    let config = GuardrailConfig {
        profile: Some(GuardrailProfile::Auto),
        ..Default::default()
    };

    assert_eq!(
        config.resolve_effective_profile(dir.path()),
        GuardrailProfile::Elixir
    );
}

#[test]
fn guardrail_auto_profile_resolves_kotlin_gradle() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.gradle.kts"),
        "pluginManagement {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("build.gradle.kts"),
        "plugins { kotlin(\"jvm\") version \"2.0.0\" }\n",
    )
    .unwrap();

    let config = GuardrailConfig {
        profile: Some(GuardrailProfile::Auto),
        ..Default::default()
    };

    assert_eq!(
        config.resolve_effective_profile(dir.path()),
        GuardrailProfile::Kotlin
    );
}

#[test]
fn guardrail_auto_profile_falls_back_to_generic() {
    let dir = TempDir::new().unwrap();
    let config = GuardrailConfig {
        profile: Some(GuardrailProfile::Auto),
        ..Default::default()
    };

    assert_eq!(
        config.resolve_effective_profile(dir.path()),
        GuardrailProfile::Generic
    );
}

#[test]
fn guardrail_prompt_guidance_varies_by_profile() {
    let zig = GuardrailProfile::Zig.prompt_guidance();
    let rust = GuardrailProfile::Rust.prompt_guidance();
    let generic = GuardrailProfile::Generic.prompt_guidance();

    assert!(zig.contains("catch unreachable"));
    assert!(zig.contains("allocator"));
    assert!(rust.contains("clippy"));
    assert!(rust.contains("unwrap"));
    assert!(generic.contains("warning-free"));
    assert_ne!(zig, rust);
    assert_ne!(zig, generic);
}

#[test]
fn guardrail_default_after_write_zig() {
    let cmds = GuardrailProfile::Zig.default_after_write();
    assert_eq!(cmds.len(), 3);
    assert!(cmds[0].contains("zig fmt"));
}

#[test]
fn guardrail_default_after_write_generic_is_empty() {
    assert!(GuardrailProfile::Generic.default_after_write().is_empty());
}

#[test]
fn guardrail_layer_contains_header() {
    let layer = guardrails_layer(GuardrailProfile::Zig);
    assert!(layer.starts_with("## Engineering Guardrails"));
    assert!(layer.contains("catch unreachable"));
}

#[test]
fn guardrail_format_check_results_all_passed() {
    let results = vec![CheckResult {
        command: "zig build".into(),
        success: true,
        output: String::new(),
    }];
    let msg = format_check_results(&results, GuardrailLevel::Advisory);
    assert_eq!(msg, "Guardrail checks passed.");
}

#[test]
fn guardrail_format_check_results_failure_enforce() {
    let results = vec![CheckResult {
        command: "cargo clippy".into(),
        success: false,
        output: "warning: unused variable".into(),
    }];
    let msg = format_check_results(&results, GuardrailLevel::Enforce);
    assert!(msg.contains("GUARDRAIL CHECK FAILED"));
    assert!(msg.contains("enforce"));
    assert!(msg.contains("cargo clippy"));
}

#[test]
fn guardrail_format_check_results_failure_advisory() {
    let results = vec![CheckResult {
        command: "mix test".into(),
        success: false,
        output: "1 test failed".into(),
    }];
    let msg = format_check_results(&results, GuardrailLevel::Advisory);
    assert!(msg.contains("advisory"));
    assert!(msg.contains("mix test"));
}

#[test]
fn guardrail_merge_only_overrides_present_fields() {
    let mut base = GuardrailConfig {
        enabled: Some(true),
        level: Some(GuardrailLevel::Advisory),
        profile: Some(GuardrailProfile::Rust),
        critical_paths: Some(vec!["src/**".into()]),
        after_write: None,
    };

    let overlay = GuardrailConfig {
        enabled: None,
        level: Some(GuardrailLevel::Enforce),
        profile: None,
        critical_paths: None,
        after_write: Some(vec!["cargo test".into()]),
    };

    base.merge(overlay);

    assert_eq!(base.enabled, Some(true));
    assert_eq!(base.level, Some(GuardrailLevel::Enforce));
    assert_eq!(base.profile, Some(GuardrailProfile::Rust));
    assert_eq!(base.critical_paths, Some(vec!["src/**".into()]));
    assert_eq!(base.after_write, Some(vec!["cargo test".into()]));
}
