use super::*;

#[test]
fn parser_is_available_for_requested_languages() {
    let cases = [
        ("script.sh", "echo hello"),
        ("main.py", "def hello():\n    pass\n"),
        ("lib.rs", "fn hello() {}"),
        ("app.js", "function hello() {}"),
        ("app.ts", "function hello(): void {}"),
        ("main.go", "package main\nfunc hello() {}\n"),
        ("mod.ex", "defmodule Hello do\n  def hi, do: :ok\nend\n"),
        ("app.rb", "def hello\nend\n"),
        ("app.pl", "sub hello { return 1; }"),
        ("app.lua", "function hello() end"),
        ("main.zig", "pub fn hello() void {}"),
        ("main.odin", "hello :: proc() {}"),
        ("App.swift", "func hello() {}"),
        ("Main.kt", "fun hello() {}"),
        ("Main.java", "class Main { void hello() {} }"),
        ("main.c", "void hello() {}"),
        ("Program.cs", "class Program { void Hello() {} }"),
        ("main.cpp", "void hello() {}"),
        ("index.php", "<?php function hello() {}"),
        ("Main.scala", "class Main { def hello(): Unit = () }"),
        ("main.dart", "void hello() {}"),
    ];

    for (file_name, source) in cases {
        let path = Path::new(file_name);
        let mut parser =
            get_parser(path).unwrap_or_else(|| panic!("missing parser for {file_name}"));
        let tree = parser
            .parse(source, None)
            .unwrap_or_else(|| panic!("failed to parse {file_name}"));
        assert!(
            !tree.root_node().has_error(),
            "parser reported errors for {file_name}"
        );
    }
}

#[test]
fn generic_extractor_indexes_representative_new_languages() {
    let cases = [
        (
            "src/app.rb",
            "class Greeter\n  def hello(name)\n  end\nend\n",
            "Greeter",
            "hello",
        ),
        (
            "src/Main.java",
            "class Greeter { void hello(String name) {} }",
            "Greeter",
            "hello",
        ),
        (
            "src/main.c",
            "struct Greeter { int id; };\nvoid hello(void) {}",
            "Greeter",
            "hello",
        ),
        (
            "src/App.swift",
            "struct Greeter { func hello() {} }",
            "Greeter",
            "hello",
        ),
        ("src/script.sh", "hello() { echo hi; }", "", "hello"),
        (
            "src/app.lua",
            "function hello(name) return name end",
            "",
            "hello",
        ),
        (
            "src/main.odin",
            "Greeter :: struct {}
hello :: proc() {}",
            "Greeter",
            "hello",
        ),
        (
            "src/app.pl",
            "package Greeter;
sub hello { return 1; }",
            "",
            "hello",
        ),
        (
            "src/main.cpp",
            "class Greeter { void hello(); };
void hello() {}",
            "Greeter",
            "hello",
        ),
        (
            "src/Program.cs",
            "class Greeter { void Hello() {} }",
            "Greeter",
            "Hello",
        ),
        (
            "src/index.php",
            "<?php class Greeter { function hello($name) { return $name; } }",
            "Greeter",
            "hello",
        ),
        (
            "src/Main.scala",
            "class Greeter { def hello(name: String): String = name }",
            "Greeter",
            "hello",
        ),
        (
            "src/main.dart",
            "class Greeter { void hello(String name) {} }",
            "Greeter",
            "hello",
        ),
        (
            "src/mod.ex",
            "defmodule Greeter do
  def hello(name), do: name
end",
            "Greeter",
            "hello",
        ),
        (
            "src/main.zig",
            "const Greeter = struct {
};
pub fn hello() void {}",
            "Greeter",
            "hello",
        ),
    ];

    for (file, source, type_name, function_name) in cases {
        let mut result = ScanResult::default();
        let ext = Path::new(file).extension().unwrap().to_str().unwrap();
        let language = language_for_extension(ext)
            .unwrap_or_else(|| panic!("missing generic language for {file}"));
        generic::parse(source, file, language, &mut result);

        if !type_name.is_empty() {
            assert!(
                result.types.contains_key(type_name),
                "missing type {type_name} for {file}; got {:?}",
                result.types.keys().collect::<Vec<_>>()
            );
        }
        assert!(
            result.functions.contains_key(function_name),
            "missing function {function_name} for {file}; got {:?}",
            result.functions.keys().collect::<Vec<_>>()
        );
    }
}

#[test]
fn collect_source_files_uses_ignore_files_in_fallback_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join(".ignore"), "ignored.py\n").unwrap();
    std::fs::write(root.join("kept.py"), "def kept(): pass\n").unwrap();
    std::fs::write(root.join("ignored.py"), "def ignored(): pass\n").unwrap();
    std::fs::write(root.join("notes.txt"), "not source\n").unwrap();

    let files = collect_source_files(root).unwrap();
    let names = files
        .iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .collect::<Vec<_>>();

    assert!(names.contains(&"kept.py"));
    assert!(!names.contains(&"ignored.py"));
    assert!(!names.contains(&"notes.txt"));
}

#[test]
fn scan_non_git_ignore_prunes_common_noise_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(root.join("target/debug/build")).unwrap();
    std::fs::create_dir_all(root.join(".venv/lib")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn kept() {}\n").unwrap();
    std::fs::write(
        root.join("node_modules/pkg/index.ts"),
        "export const ignored = 1;\n",
    )
    .unwrap();
    std::fs::write(
        root.join("target/debug/build/generated.rs"),
        "fn ignored() {}\n",
    )
    .unwrap();
    std::fs::write(root.join(".venv/lib/site.py"), "def ignored(): pass\n").unwrap();

    let files = collect_source_files(root).unwrap();
    let relative = files
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();

    assert_eq!(relative, vec!["src/lib.rs"]);
}

#[test]
fn schema_uses_directory_files_extract_and_targets() {
    let schema = ScanTool.parameters();
    let properties = schema["properties"].as_object().unwrap();
    let actions = properties["action"]["enum"].as_array().unwrap();
    assert!(actions.iter().any(|value| value == "directory"));
    assert!(actions.iter().any(|value| value == "files"));
    assert!(actions.iter().any(|value| value == "extract"));
    assert!(actions.iter().any(|value| value == "search"));
    assert!(actions.iter().any(|value| value == "tests"));
    assert!(actions.iter().any(|value| value == "related"));
    assert!(!actions.iter().any(|value| value == "references"));
    assert!(!actions.iter().any(|value| value == "impact"));
    assert!(properties.contains_key("targets"));
    assert!(properties.contains_key("query"));
    assert!(properties.contains_key("mode"));
    assert!(properties.contains_key("max_results"));
    assert!(!properties.contains_key("preset"));
    assert!(properties.contains_key("target"));
    assert!(!properties.contains_key("task"));
}

#[test]
fn search_index_returns_ranked_symbol_hits() {
    let index = vec![
        IndexedSymbol {
            file: "src/auth/session.rs".to_string(),
            name: "resolve_auth_fallback".to_string(),
            kind: "function".to_string(),
            line: 12,
            text: "fn resolve_auth_fallback()".to_string(),
            is_test: false,
        },
        IndexedSymbol {
            file: "src/cache.rs".to_string(),
            name: "load_cache".to_string(),
            kind: "function".to_string(),
            line: 4,
            text: "fn load_cache()".to_string(),
            is_test: false,
        },
    ];

    let hits = search_index(&index, "auth fallback", "concept", 5);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].symbol.as_deref(), Some("resolve_auth_fallback"));
    assert!(hits[0]
        .why
        .iter()
        .any(|why| why.contains("symbol contains")));
}

#[test]
fn discover_tests_suggests_cargo_test_for_rust_tests() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let index = vec![IndexedSymbol {
        file: "src/session.rs".to_string(),
        name: "falls_back_to_env_token".to_string(),
        kind: "function".to_string(),
        line: 42,
        text: "fn falls_back_to_env_token()".to_string(),
        is_test: true,
    }];

    let tests = discover_tests(
        &index,
        "src/session.rs#resolve_auth_fallback",
        tmp.path(),
        5,
    );

    assert_eq!(tests.len(), 1);
    assert_eq!(
        tests[0].command.as_deref(),
        Some("cargo test falls_back_to_env_token")
    );
}

#[test]
fn related_symbols_returns_same_file_context() {
    let index = vec![
        IndexedSymbol {
            file: "src/session.rs".to_string(),
            name: "resolve_auth_fallback".to_string(),
            kind: "function".to_string(),
            line: 10,
            text: "fn resolve_auth_fallback()".to_string(),
            is_test: false,
        },
        IndexedSymbol {
            file: "src/session.rs".to_string(),
            name: "SessionConfig".to_string(),
            kind: "struct".to_string(),
            line: 2,
            text: "SessionConfig".to_string(),
            is_test: false,
        },
    ];

    let related = related_symbols(
        &index,
        Some("src/session.rs"),
        Some("resolve_auth_fallback"),
        5,
    );

    assert_eq!(related.len(), 1);
    assert_eq!(related[0].name, "SessionConfig");
}

#[test]
fn scan_repo_intelligence_counts_reuse_symbol_index_shape() {
    let index = vec![
        IndexedSymbol {
            file: "src/lib.rs".to_string(),
            name: "Widget".to_string(),
            kind: "struct".to_string(),
            line: 1,
            text: "Widget".to_string(),
            is_test: false,
        },
        IndexedSymbol {
            file: "src/lib.rs".to_string(),
            name: "widget_builds".to_string(),
            kind: "function".to_string(),
            line: 5,
            text: "fn widget_builds()".to_string(),
            is_test: true,
        },
    ];

    assert_eq!(
        repo_index_line(&index),
        "Repo intelligence: 2 symbols, 1 tests"
    );
    assert_eq!(repo_index_details(&index)["symbols"], 2);
    assert_eq!(repo_index_details(&index)["tests"], 1);
}

#[test]
fn parse_extract_target_rejects_invalid_lines() {
    assert!(parse_extract_target("src/lib.rs:0").is_none());
    assert!(parse_extract_target("src/lib.rs:10-2").is_none());
    assert!(parse_extract_target("src/lib.rs:1-2").is_some());
}

#[test]
fn execute_extract_reports_invalid_target_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    let ctx = ToolContext {
        cwd: tmp.path().to_path_buf(),
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: std::sync::Arc::new(crate::ui::NullInterface),
        file_cache: std::sync::Arc::new(crate::tools::FileCache::new()),
        checkpoint_state: std::sync::Arc::new(crate::tools::CheckpointState::new()),
        file_tracker: std::sync::Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
        anchor_store: std::sync::Arc::new(crate::tools::AnchorStore::new()),
        lua_tool_loader: None,
        mode: crate::config::AgentMode::Full,
        read_max_lines: 500,
        turn_workflow_review: std::sync::Arc::new(std::sync::Mutex::new(
            crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
        )),
        run_policy: Default::default(),
        config: std::sync::Arc::new(crate::config::Config::default()),
        supporting_provenance: Vec::new(),
    };

    let output = execute_extract(&["not-a-target".to_string()], &ctx);

    assert!(output.is_error);
    assert_eq!(output.details["action"], "extract");
    assert_eq!(output.details["errors"].as_array().unwrap().len(), 1);
}

#[test]
fn extract_rust_file() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("sample.rs");
    std::fs::write(
        &file,
        r#"
pub struct User {
    pub name: String,
    pub age: u32,
}

pub enum Status { Active, Inactive }

pub trait Validate {
    fn validate(&self) -> bool;
}

impl Validate for User {
    fn validate(&self) -> bool { true }
}

pub async fn load_user(id: &str) -> Result<User> { todo!() }
fn internal_helper() {}
"#,
    )
    .unwrap();

    let result = extract_files(&[file], tmp.path());

    // Types extracted
    assert!(result.types.contains_key("User"));
    assert!(result.types.contains_key("Status"));
    assert!(result.types.contains_key("Validate"));

    // User has fields
    let user = &result.types["User"];
    assert_eq!(user.fields.len(), 2);
    assert_eq!(user.visibility, Visibility::Public);

    // Status has variants
    let status = &result.types["Status"];
    assert_eq!(status.variants, vec!["Active", "Inactive"]);

    // Validate has methods
    let validate = &result.types["Validate"];
    assert!(validate.methods.contains(&"validate".to_string()));

    // User implements Validate
    assert!(user.implements.contains(&"Validate".to_string()));

    // Functions extracted with signatures
    let load = &result.functions["load_user"];
    assert!(load.is_async);
    assert!(load.signature.contains("-> Result<User>"));
    assert_eq!(load.visibility, Visibility::Public);

    let helper = &result.functions["internal_helper"];
    assert_eq!(helper.visibility, Visibility::Private);
}

#[test]
fn extract_swift_file_with_rich_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("Greeter.swift");
    std::fs::write(
        &file,
        r#"
public protocol GreetingService { func greet(name: String) }
struct Greeter: GreetingService {
    enum Tone { case friendly, formal }
    public func greet(name: String) async {}
    private func helper() {}
}
extension Greeter {
    func extra() {}
}
func makeGreeter() -> Greeter { Greeter() }
"#,
    )
    .unwrap();

    let result = extract_files(&[file], tmp.path());

    assert_eq!(result.types["GreetingService"].kind, TypeKind::Protocol);
    assert_eq!(
        result.types["GreetingService"].visibility,
        Visibility::Public
    );
    assert_eq!(result.types["Greeter"].kind, TypeKind::Struct);
    assert!(result.types["Greeter"]
        .implements
        .contains(&"GreetingService".to_string()));
    assert!(result.types["Greeter"]
        .methods
        .contains(&"greet".to_string()));
    assert!(result.types["Greeter"]
        .methods
        .contains(&"helper".to_string()));
    assert!(result.types["Greeter"]
        .methods
        .contains(&"extra".to_string()));
    assert_eq!(result.types["Greeter::Tone"].kind, TypeKind::Enum);
    assert_eq!(
        result.functions["Greeter::greet"].visibility,
        Visibility::Public
    );
    assert!(result.functions["Greeter::greet"].is_async);
    assert_eq!(
        result.functions["Greeter::helper"].visibility,
        Visibility::Private
    );
    assert!(result.functions.contains_key("Greeter::extra"));
    assert!(result.functions.contains_key("makeGreeter"));
    assert!(result.functions["makeGreeter"].source.ends_with(":11"));
}

#[test]
fn extract_zig_odin_shell_perl_files_with_rich_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let zig_file = tmp.path().join("main.zig");
    std::fs::write(
        &zig_file,
        r#"
pub const Greeter = struct {};
pub fn hello() void {}
fn helper() void {}
"#,
    )
    .unwrap();
    let odin_file = tmp.path().join("main.odin");
    std::fs::write(
        &odin_file,
        r#"
Greeter :: struct {}
hello :: proc() {}
"#,
    )
    .unwrap();
    let shell_file = tmp.path().join("script.sh");
    std::fs::write(
        &shell_file,
        r#"
hello() { echo hi; }
function helper { echo ok; }
"#,
    )
    .unwrap();
    let perl_file = tmp.path().join("Greeter.pm");
    std::fs::write(
        &perl_file,
        r#"
package Greeter;
sub hello { return 1; }
"#,
    )
    .unwrap();

    let result = extract_files(&[zig_file, odin_file, shell_file, perl_file], tmp.path());

    assert_eq!(result.types["Greeter"].kind, TypeKind::Struct);
    assert_eq!(result.functions["hello"].visibility, Visibility::Public);
    assert_eq!(result.functions["helper"].visibility, Visibility::Private);
    assert!(result.functions["hello"].signature.contains("hello"));
    assert!(result.functions.contains_key("helper"));
    assert!(result.types.contains_key("Greeter"));
    assert!(result.functions.contains_key("Greeter::hello"));
}

#[test]
fn extract_ruby_elixir_lua_ocaml_files_with_rich_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let ruby_file = tmp.path().join("greeter.rb");
    std::fs::write(
        &ruby_file,
        r#"
module Services
  class Greeter
    def hello(name)
      name
    end
  end
end
"#,
    )
    .unwrap();
    let elixir_file = tmp.path().join("greeter.ex");
    std::fs::write(
        &elixir_file,
        r#"
defmodule Services.Greeter do
  def hello(name), do: name
  defp normalize(name), do: name
end
"#,
    )
    .unwrap();
    let lua_file = tmp.path().join("greeter.lua");
    std::fs::write(
        &lua_file,
        r#"
local Greeter = {}
function Greeter:hello(name) return name end
function helper(name) return name end
"#,
    )
    .unwrap();
    let ocaml_file = tmp.path().join("greeter.ml");
    std::fs::write(
        &ocaml_file,
        r#"
module Greeter = struct
  type user = { name : string }
  let hello name = name
end
"#,
    )
    .unwrap();

    let result = extract_files(&[ruby_file, elixir_file, lua_file, ocaml_file], tmp.path());

    assert!(result.types.contains_key("Services"));
    assert!(result.types.contains_key("Services::Greeter"));
    assert!(result.types["Services::Greeter"]
        .methods
        .contains(&"hello".to_string()));
    assert!(result.functions.contains_key("Services::Greeter::hello"));

    assert!(result.types.contains_key("Services.Greeter"));
    assert_eq!(
        result.functions["Services.Greeter::normalize"].visibility,
        Visibility::Private
    );

    assert!(result.types.contains_key("Greeter"));
    assert!(result.types["Greeter"]
        .methods
        .contains(&"hello".to_string()));
    assert!(result.functions.contains_key("Greeter:hello"));
    assert!(result.functions.contains_key("helper"));

    assert!(result.types.contains_key("Greeter::user"));
    assert!(result.functions.contains_key("Greeter::hello"));
    assert!(result.functions["Greeter::hello"].source.ends_with(":4"));
}

#[test]
fn extract_c_file_with_rich_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("sample.c");
    std::fs::write(
        &file,
        r#"
typedef unsigned long Size;
struct User { int id; const char *name; };
enum Status { Active, Inactive };
void greet(struct User *user) {}
"#,
    )
    .unwrap();

    let result = extract_files(&[file], tmp.path());

    assert_eq!(result.types["Size"].kind, TypeKind::TypeAlias);
    assert_eq!(result.types["User"].kind, TypeKind::Struct);
    assert!(result.types["User"]
        .fields
        .iter()
        .any(|field| field.name == "id"));
    assert_eq!(result.types["Status"].kind, TypeKind::Enum);
    assert!(result.types["Status"]
        .variants
        .contains(&"Active".to_string()));
    assert!(result.functions["greet"].signature.contains("void greet"));
    assert!(result.functions["greet"].source.ends_with(":5"));
}

#[test]
fn extract_cpp_file_with_rich_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("sample.cpp");
    std::fs::write(
        &file,
        r#"
using Size = unsigned long;
enum class Status { Active, Inactive };
class Greeter {
    int count;
    void helper() {}
};
void greet(Greeter& greeter) {}
"#,
    )
    .unwrap();

    let result = extract_files(&[file], tmp.path());

    assert_eq!(result.types["Size"].kind, TypeKind::TypeAlias);
    assert_eq!(result.types["Status"].kind, TypeKind::Enum);
    assert!(result.types["Status"]
        .variants
        .contains(&"Active".to_string()));
    assert_eq!(result.types["Greeter"].kind, TypeKind::Class);
    assert!(result.types["Greeter"]
        .fields
        .iter()
        .any(|field| field.name == "count"));
    assert!(result.types["Greeter"]
        .methods
        .contains(&"helper".to_string()));
    assert!(result.functions.contains_key("Greeter::helper"));
    assert!(result.functions["greet"].signature.contains("void greet"));
}

#[test]
fn extract_java_file_with_rich_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("Greeter.java");
    std::fs::write(
        &file,
        r#"
public interface GreetingService { void greet(String name); }
public enum Tone { Friendly, Formal }
class Greeter implements GreetingService {
    public Greeter() {}
    private void helper() {}
    @Test public void greet(String name) {}
}
"#,
    )
    .unwrap();

    let result = extract_files(&[file], tmp.path());

    assert_eq!(result.types["GreetingService"].kind, TypeKind::Interface);
    assert_eq!(result.types["Tone"].kind, TypeKind::Enum);
    assert!(result.types["Tone"]
        .variants
        .contains(&"Friendly".to_string()));
    assert_eq!(result.types["Greeter"].kind, TypeKind::Class);
    assert_eq!(result.types["Greeter"].visibility, Visibility::Internal);
    assert!(result.types["Greeter"]
        .methods
        .contains(&"Greeter".to_string()));
    assert!(result.types["Greeter"]
        .methods
        .contains(&"greet".to_string()));

    assert_eq!(
        result.functions["Greeter::Greeter"].visibility,
        Visibility::Public
    );
    assert_eq!(
        result.functions["Greeter::helper"].visibility,
        Visibility::Private
    );
    assert!(result.functions["Greeter::greet"].is_test);
}

#[test]
fn extract_csharp_file_with_rich_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("Greeter.cs");
    std::fs::write(
        &file,
        r#"
public interface IGreeting { void Greet(string name); }
public enum Tone { Friendly, Formal }
internal class Greeter : IGreeting {
    public Greeter() {}
    private void Helper() {}
    [Fact] public async Task Greet(string name) {}
}
"#,
    )
    .unwrap();

    let result = extract_files(&[file], tmp.path());

    assert_eq!(result.types["IGreeting"].kind, TypeKind::Interface);
    assert_eq!(result.types["Tone"].kind, TypeKind::Enum);
    assert!(result.types["Tone"]
        .variants
        .contains(&"Friendly".to_string()));
    assert_eq!(result.types["Greeter"].kind, TypeKind::Class);
    assert_eq!(result.types["Greeter"].visibility, Visibility::Internal);
    assert!(result.types["Greeter"]
        .methods
        .contains(&"Greeter".to_string()));
    assert!(result.types["Greeter"]
        .methods
        .contains(&"Greet".to_string()));

    assert_eq!(
        result.functions["Greeter::Greeter"].visibility,
        Visibility::Public
    );
    assert_eq!(
        result.functions["Greeter::Helper"].visibility,
        Visibility::Private
    );
    assert!(result.functions["Greeter::Greet"].is_async);
    assert!(result.functions["Greeter::Greet"].is_test);
}

#[test]
fn extract_typescript_file() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("models.ts");
    std::fs::write(
        &file,
        r#"
export interface User {
    name: string;
    email: string;
}

export enum Status {
    Active = "active",
    Inactive = "inactive",
}

export async function fetchUser(id: string): Promise<User> {
    return {} as User;
}

function internalHelper(): void {}
"#,
    )
    .unwrap();

    let result = extract_files(&[file], tmp.path());
    assert!(result.types.contains_key("User"));
    assert!(result.types.contains_key("Status"));
    assert_eq!(result.types["User"].visibility, Visibility::Public);
    assert_eq!(result.types["Status"].variants, vec!["Active", "Inactive"]);
    assert!(result.functions["fetchUser"].is_async);
    assert_eq!(
        result.functions["internalHelper"].visibility,
        Visibility::Private
    );
}

#[test]
fn format_output_shows_rich_info() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("lib.rs");
    std::fs::write(
        &file,
        r#"
pub struct Config { pub host: String, pub port: u16 }
pub enum Mode { Debug, Release }
pub fn start(config: &Config) -> Result<()> { todo!() }
"#,
    )
    .unwrap();

    let result = extract_files(std::slice::from_ref(&file), tmp.path());
    let output = format_result(&result, &[file], tmp.path(), "extract", None);

    assert!(output.contains("pub struct Config { host, port }"));
    assert!(output.contains("pub enum Mode { Debug, Release }"));
    assert!(output.contains("pub fn start"));
    assert!(output.contains("-> Result<()>"));
}

#[test]
fn skeleton_output_includes_line_ranges_and_target_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("lib.rs");
    std::fs::write(
        &file,
        r#"
pub struct Config { pub host: String, pub port: u16 }
pub fn start(config: &Config) -> Result<()> { todo!() }
"#,
    )
    .unwrap();

    let result = extract_files(std::slice::from_ref(&file), tmp.path());
    let output = format_result(&result, &[file], tmp.path(), "build", None);

    assert!(output.contains("compact code skeleton"));
    assert!(output.contains("file#symbol"));
    assert!(output.contains("pub struct Config"));
    assert!(output.contains(" @ lib.rs:2"));
    assert!(output.contains("pub fn start"));
    assert!(output.contains(" @ lib.rs:3"));
}

#[test]
fn symbol_extract_includes_structured_details() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("lib.rs");
    std::fs::write(
        &file,
        r#"
pub struct Config {
    pub host: String,
}

pub fn start(config: &Config) -> Result<()> { todo!() }
"#,
    )
    .unwrap();

    let found = extract_symbol(&std::fs::read_to_string(&file).unwrap(), &file, "Config").unwrap();
    let output = format_blocks(&[CodeBlock {
        file: PathBuf::from("lib.rs"),
        ..found
    }]);

    assert!(output.contains("Details:"));
    assert!(output.contains("\"symbol\":\"Config\""));
    assert!(output.contains("\"language\":\"rust\""));
    assert!(output.contains("\"start_line\":2"));
    assert!(output.contains("pub struct Config"));
}

#[test]
fn typescript_skeleton_output_includes_line_ranges() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("models.ts");
    std::fs::write(
        &file,
        r#"
export interface User { name: string; }
export async function fetchUser(id: string): Promise<User> { return {} as User; }
"#,
    )
    .unwrap();

    let result = extract_files(std::slice::from_ref(&file), tmp.path());
    let output = format_result(&result, &[file], tmp.path(), "build", None);

    assert!(output.contains("pub interface User @ models.ts:2"));
    assert!(output.contains("pub async function fetchUser"));
    assert!(output.contains(" @ models.ts:3"));
}
