//! Read-oriented code-intelligence contracts.
//!
//! These types are the imp-facing normalization seam for semantic code queries.
//! They are intentionally independent from the AST-backed `scan` tool types and
//! do not model write-oriented semantic actions.

pub mod repo_index_adapter;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    pub range: Option<TextRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolIdentity {
    pub name: String,
    pub kind: SymbolKind,
    pub container: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    File,
    Module,
    Namespace,
    Package,
    Class,
    Struct,
    Interface,
    Enum,
    Trait,
    Function,
    Method,
    Constructor,
    Field,
    Property,
    Variable,
    Constant,
    TypeAlias,
    Macro,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedLocation {
    pub location: Location,
    pub relationship: RelationshipKind,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    Definition,
    Declaration,
    Implementation,
    Reference,
    Caller,
    Callee,
    EnclosingSymbol,
    RelatedDiagnostic,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemItem {
    pub message: String,
    pub severity: ProblemSeverity,
    pub location: Option<Location>,
    pub code: Option<String>,
    pub source: ProblemSource,
    pub related: Vec<RelatedLocation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemSeverity {
    Error,
    Warning,
    Info,
    Hint,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemSource {
    SemanticBackend { backend: String },
    Compiler,
    Linter,
    Test,
    Build,
    Other { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticSummary {
    pub total: usize,
    pub by_severity: Vec<SeverityCount>,
    pub by_source: Vec<SourceCount>,
    pub primary_blocker: Option<ProblemItem>,
    pub compact: String,
}

impl DiagnosticSummary {
    pub fn from_problems(problems: &[ProblemItem]) -> Self {
        let mut by_severity = Vec::new();
        for severity in [
            ProblemSeverity::Error,
            ProblemSeverity::Warning,
            ProblemSeverity::Info,
            ProblemSeverity::Hint,
            ProblemSeverity::Unknown,
        ] {
            let count = problems
                .iter()
                .filter(|problem| problem.severity == severity)
                .count();
            if count > 0 {
                by_severity.push(SeverityCount { severity, count });
            }
        }

        let mut by_source: Vec<SourceCount> = Vec::new();
        for problem in problems {
            let source = problem.source.summary_label();
            if let Some(existing) = by_source.iter_mut().find(|entry| entry.source == source) {
                existing.count += 1;
            } else {
                by_source.push(SourceCount { source, count: 1 });
            }
        }

        let primary_blocker = problems
            .iter()
            .find(|problem| problem.severity == ProblemSeverity::Error)
            .or_else(|| problems.first())
            .cloned();
        let compact =
            compact_diagnostic_summary(problems.len(), &by_severity, primary_blocker.as_ref());

        Self {
            total: problems.len(),
            by_severity,
            by_source,
            primary_blocker,
            compact,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeverityCount {
    pub severity: ProblemSeverity,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCount {
    pub source: String,
    pub count: usize,
}

impl ProblemSource {
    pub fn summary_label(&self) -> String {
        match self {
            Self::SemanticBackend { backend } => format!("semantic:{backend}"),
            Self::Compiler => "compiler".to_string(),
            Self::Linter => "linter".to_string(),
            Self::Test => "test".to_string(),
            Self::Build => "build".to_string(),
            Self::Other { name } => name.clone(),
        }
    }
}

fn compact_diagnostic_summary(
    total: usize,
    by_severity: &[SeverityCount],
    primary_blocker: Option<&ProblemItem>,
) -> String {
    if total == 0 {
        return "no diagnostics".to_string();
    }

    let counts = by_severity
        .iter()
        .map(|entry| format!("{} {:?}", entry.count, entry.severity))
        .collect::<Vec<_>>()
        .join(", ");
    match primary_blocker {
        Some(problem) => format!(
            "{total} diagnostics ({counts}); primary: {}",
            problem.message
        ),
        None => format!("{total} diagnostics ({counts})"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendProvenance {
    pub adapter: String,
    pub backend: String,
    pub language: Option<String>,
    pub workspace_root: Option<String>,
    pub freshness: Freshness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale { reason: String },
    Degraded { reason: String },
    Unknown,
}

impl Freshness {
    pub fn is_trustworthy(&self) -> bool {
        matches!(self, Self::Fresh)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeIntelEnvelope<T> {
    pub surface: CodeIntelSurface,
    pub query: CodeIntelQuery,
    pub summary: String,
    pub items: Vec<T>,
    pub provenance: BackendProvenance,
    pub truncated: Option<Truncation>,
    pub warnings: Vec<String>,
    pub suggested_next_queries: Vec<CodeIntelQuery>,
}

impl<T> CodeIntelEnvelope<T> {
    pub fn is_complete_and_fresh(&self) -> bool {
        self.truncated.is_none() && self.provenance.freshness.is_trustworthy()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeIntelSurface {
    Diagnostics,
    Definition,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
    Hover,
    SignatureHelp,
    StructuralContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CodeIntelQuery {
    Diagnostics {
        path: Option<String>,
    },
    Definition {
        path: String,
        position: Position,
    },
    References {
        path: String,
        position: Position,
    },
    DocumentSymbols {
        path: String,
    },
    WorkspaceSymbols {
        query: String,
    },
    Hover {
        path: String,
        position: Position,
    },
    SignatureHelp {
        path: String,
        position: Position,
    },
    StructuralContext {
        path: String,
        position: Option<Position>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truncation {
    pub returned: usize,
    pub available: Option<usize>,
    pub reason: String,
    pub continuation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterDescriptor {
    pub id: String,
    pub display_name: String,
    pub languages: Vec<String>,
    pub surfaces: Vec<CodeIntelSurface>,
    pub source: AdapterSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterSource {
    BuiltIn,
    Project,
    Extension,
    ExternalProcess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionItem {
    pub symbol: Option<SymbolIdentity>,
    pub target: Location,
    pub provenance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceItem {
    pub symbol: Option<SymbolIdentity>,
    pub location: Location,
    pub usage: ReferenceUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceUsage {
    Read,
    Write,
    Call,
    Import,
    Type,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSymbolItem {
    pub symbol: SymbolIdentity,
    pub location: Location,
    pub children: Vec<DocumentSymbolItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoverItem {
    pub summary: String,
    pub symbol: Option<SymbolIdentity>,
    pub location: Option<Location>,
    pub range: Option<TextRange>,
    pub contents: Vec<HoverContentBlock>,
}

impl HoverItem {
    pub fn compact_summary(&self) -> &str {
        &self.summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoverContentBlock {
    pub kind: HoverContentKind,
    pub value: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoverContentKind {
    PlainText,
    Markdown,
    Code,
    Type,
    Documentation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureHelpItem {
    pub active_signature: Option<usize>,
    pub active_parameter: Option<usize>,
    pub signatures: Vec<SignatureItem>,
    pub trigger: Option<SignatureTrigger>,
}

impl SignatureHelpItem {
    pub fn selected_signature(&self) -> Option<&SignatureItem> {
        self.active_signature
            .and_then(|index| self.signatures.get(index))
            .or_else(|| self.signatures.first())
    }

    pub fn selected_parameter(&self) -> Option<&ParameterItem> {
        let signature = self.selected_signature()?;
        self.active_parameter
            .and_then(|index| signature.parameters.get(index))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureItem {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<ParameterItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterItem {
    pub label: String,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureTrigger {
    pub character: Option<String>,
    pub reason: SignatureTriggerReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureTriggerReason {
    Invoked,
    TriggerCharacter,
    ContentChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeIntelError {
    NoAdapter {
        language: Option<String>,
        surface: CodeIntelSurface,
    },
    AdapterUnavailable {
        adapter: String,
        reason: String,
    },
}

impl std::fmt::Display for CodeIntelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdapter { language, surface } => write!(
                f,
                "no code-intelligence adapter available for surface {:?}{}",
                surface,
                language
                    .as_deref()
                    .map(|language| format!(" and language {language}"))
                    .unwrap_or_default()
            ),
            Self::AdapterUnavailable { adapter, reason } => {
                write!(
                    f,
                    "code-intelligence adapter {adapter} unavailable: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for CodeIntelError {}

pub type CodeIntelResult<T> = Result<T, CodeIntelError>;

pub trait CodeIntelAdapter: Send + Sync {
    fn descriptor(&self) -> &AdapterDescriptor;

    fn supports(&self, language: Option<&str>, surface: CodeIntelSurface) -> bool {
        let descriptor = self.descriptor();
        descriptor.surfaces.contains(&surface)
            && language.is_none_or(|language| {
                descriptor
                    .languages
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(language))
            })
    }
}

#[derive(Default)]
pub struct CodeIntelRegistry {
    adapters: Vec<Box<dyn CodeIntelAdapter>>,
}

impl CodeIntelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: impl CodeIntelAdapter + 'static) {
        self.adapters.push(Box::new(adapter));
    }

    pub fn descriptors(&self) -> Vec<&AdapterDescriptor> {
        self.adapters
            .iter()
            .map(|adapter| adapter.descriptor())
            .collect()
    }

    pub fn find(
        &self,
        language: Option<&str>,
        surface: CodeIntelSurface,
    ) -> CodeIntelResult<&dyn CodeIntelAdapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.supports(language, surface))
            .map(|adapter| adapter.as_ref())
            .ok_or_else(|| CodeIntelError::NoAdapter {
                language: language.map(str::to_string),
                surface,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolOrientationPlan {
    pub subject: SymbolOrientationSubject,
    pub first_step: SymbolOrientationStep,
    pub follow_up: Option<CodeIntelQuery>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SymbolOrientationSubject {
    LocalFile {
        path: String,
        language: Option<String>,
    },
    Workspace {
        query: String,
        language: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolOrientationStep {
    AstStructuralScan { path: String },
    SemanticDocumentSymbols { path: String },
    SemanticWorkspaceSymbols { query: String },
}

pub fn plan_symbol_orientation(subject: SymbolOrientationSubject) -> SymbolOrientationPlan {
    match subject {
        SymbolOrientationSubject::LocalFile { path, language } => SymbolOrientationPlan {
            first_step: SymbolOrientationStep::AstStructuralScan { path: path.clone() },
            follow_up: Some(CodeIntelQuery::DocumentSymbols { path: path.clone() }),
            reason: "use AST scan first for fast local shape before semantic document symbols"
                .to_string(),
            subject: SymbolOrientationSubject::LocalFile { path, language },
        },
        SymbolOrientationSubject::Workspace { query, language } => SymbolOrientationPlan {
            first_step: SymbolOrientationStep::SemanticWorkspaceSymbols {
                query: query.clone(),
            },
            follow_up: None,
            reason: "workspace-wide symbol discovery requires the semantic symbol index"
                .to_string(),
            subject: SymbolOrientationSubject::Workspace { query, language },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance(freshness: Freshness) -> BackendProvenance {
        BackendProvenance {
            adapter: "rust-analyzer".to_string(),
            backend: "lsp".to_string(),
            language: Some("rust".to_string()),
            workspace_root: Some("/repo".to_string()),
            freshness,
        }
    }

    #[test]
    fn envelope_reports_complete_only_when_fresh_and_untruncated() {
        let envelope: CodeIntelEnvelope<ProblemItem> = CodeIntelEnvelope {
            surface: CodeIntelSurface::Diagnostics,
            query: CodeIntelQuery::Diagnostics { path: None },
            summary: "no problems".to_string(),
            items: Vec::new(),
            provenance: provenance(Freshness::Fresh),
            truncated: None,
            warnings: Vec::new(),
            suggested_next_queries: Vec::new(),
        };

        assert!(envelope.is_complete_and_fresh());

        let stale = CodeIntelEnvelope {
            provenance: provenance(Freshness::Stale {
                reason: "index is rebuilding".to_string(),
            }),
            ..envelope.clone()
        };
        assert!(!stale.is_complete_and_fresh());

        let truncated = CodeIntelEnvelope {
            truncated: Some(Truncation {
                returned: 50,
                available: Some(200),
                reason: "too many references".to_string(),
                continuation: Some("next-page".to_string()),
            }),
            ..envelope
        };
        assert!(!truncated.is_complete_and_fresh());
    }

    #[test]
    fn problem_item_distinguishes_semantic_and_deterministic_sources() {
        let semantic = ProblemItem {
            message: "unknown field".to_string(),
            severity: ProblemSeverity::Error,
            location: None,
            code: Some("E0609".to_string()),
            source: ProblemSource::SemanticBackend {
                backend: "rust-analyzer".to_string(),
            },
            related: Vec::new(),
        };
        let compiler = ProblemItem {
            source: ProblemSource::Compiler,
            ..semantic.clone()
        };

        assert_ne!(semantic.source, compiler.source);
    }

    #[test]
    fn adapter_descriptor_names_supported_surfaces_without_backend_payloads() {
        let descriptor = AdapterDescriptor {
            id: "ra".to_string(),
            display_name: "rust-analyzer".to_string(),
            languages: vec!["rust".to_string()],
            surfaces: vec![CodeIntelSurface::Diagnostics, CodeIntelSurface::Definition],
            source: AdapterSource::ExternalProcess,
        };

        assert!(descriptor.surfaces.contains(&CodeIntelSurface::Diagnostics));
        assert_eq!(descriptor.languages, ["rust"]);
    }

    #[test]
    fn symbol_orientation_uses_ast_first_for_local_file_shape() {
        let plan = plan_symbol_orientation(SymbolOrientationSubject::LocalFile {
            path: "src/lib.rs".to_string(),
            language: Some("rust".to_string()),
        });

        assert_eq!(
            plan.first_step,
            SymbolOrientationStep::AstStructuralScan {
                path: "src/lib.rs".to_string(),
            }
        );
        assert_eq!(
            plan.follow_up,
            Some(CodeIntelQuery::DocumentSymbols {
                path: "src/lib.rs".to_string(),
            })
        );
        assert!(plan.reason.contains("AST scan first"));
    }

    #[test]
    fn symbol_orientation_uses_semantic_index_for_workspace_queries() {
        let plan = plan_symbol_orientation(SymbolOrientationSubject::Workspace {
            query: "Agent".to_string(),
            language: Some("rust".to_string()),
        });

        assert_eq!(
            plan.first_step,
            SymbolOrientationStep::SemanticWorkspaceSymbols {
                query: "Agent".to_string(),
            }
        );
        assert_eq!(plan.follow_up, None);
        assert!(plan.reason.contains("semantic symbol index"));
    }

    #[test]
    fn diagnostic_summary_counts_severity_and_preserves_source_distinctions() {
        let problems = vec![
            ProblemItem {
                message: "unknown field".to_string(),
                severity: ProblemSeverity::Error,
                location: None,
                code: Some("E0609".to_string()),
                source: ProblemSource::SemanticBackend {
                    backend: "rust-analyzer".to_string(),
                },
                related: Vec::new(),
            },
            ProblemItem {
                message: "test failed".to_string(),
                severity: ProblemSeverity::Error,
                location: None,
                code: None,
                source: ProblemSource::Test,
                related: Vec::new(),
            },
            ProblemItem {
                message: "unused import".to_string(),
                severity: ProblemSeverity::Warning,
                location: None,
                code: None,
                source: ProblemSource::Linter,
                related: Vec::new(),
            },
        ];

        let summary = DiagnosticSummary::from_problems(&problems);

        assert_eq!(summary.total, 3);
        assert!(summary.by_severity.contains(&SeverityCount {
            severity: ProblemSeverity::Error,
            count: 2,
        }));
        assert!(summary.by_source.contains(&SourceCount {
            source: "semantic:rust-analyzer".to_string(),
            count: 1,
        }));
        assert!(summary.by_source.contains(&SourceCount {
            source: "test".to_string(),
            count: 1,
        }));
        assert_eq!(
            summary
                .primary_blocker
                .as_ref()
                .map(|problem| problem.message.as_str()),
            Some("unknown field")
        );
        assert!(summary.compact.contains("primary: unknown field"));
    }

    #[test]
    fn diagnostic_summary_handles_empty_problem_sets_compactly() {
        let summary = DiagnosticSummary::from_problems(&[]);

        assert_eq!(summary.total, 0);
        assert!(summary.by_severity.is_empty());
        assert!(summary.by_source.is_empty());
        assert_eq!(summary.primary_blocker, None);
        assert_eq!(summary.compact, "no diagnostics");
    }

    #[test]
    fn hover_item_exposes_compact_summary_without_raw_payload() {
        let hover = HoverItem {
            summary: "AgentConfig configures model and tool policy".to_string(),
            symbol: Some(SymbolIdentity {
                name: "AgentConfig".to_string(),
                kind: SymbolKind::Struct,
                container: Some("imp_core::agent".to_string()),
                language: Some("rust".to_string()),
            }),
            location: None,
            range: None,
            contents: vec![HoverContentBlock {
                kind: HoverContentKind::Documentation,
                value: "Configuration for an agent session.".to_string(),
                language: None,
            }],
        };
        let envelope = CodeIntelEnvelope {
            surface: CodeIntelSurface::Hover,
            query: CodeIntelQuery::Hover {
                path: "src/agent/mod.rs".to_string(),
                position: Position {
                    line: 10,
                    column: 4,
                },
            },
            summary: hover.compact_summary().to_string(),
            items: vec![hover],
            provenance: provenance(Freshness::Fresh),
            truncated: None,
            warnings: Vec::new(),
            suggested_next_queries: Vec::new(),
        };

        assert_eq!(envelope.items[0].compact_summary(), envelope.summary);
        assert!(envelope.is_complete_and_fresh());
    }

    #[test]
    fn signature_help_selects_active_signature_and_parameter() {
        let help = SignatureHelpItem {
            active_signature: Some(1),
            active_parameter: Some(0),
            signatures: vec![
                SignatureItem {
                    label: "run()".to_string(),
                    documentation: None,
                    parameters: Vec::new(),
                },
                SignatureItem {
                    label: "run(config: AgentConfig)".to_string(),
                    documentation: Some("Run an agent turn.".to_string()),
                    parameters: vec![ParameterItem {
                        label: "config".to_string(),
                        documentation: Some("Agent runtime configuration.".to_string()),
                    }],
                },
            ],
            trigger: Some(SignatureTrigger {
                character: Some("(".to_string()),
                reason: SignatureTriggerReason::TriggerCharacter,
            }),
        };

        assert_eq!(
            help.selected_signature()
                .map(|signature| signature.label.as_str()),
            Some("run(config: AgentConfig)")
        );
        assert_eq!(
            help.selected_parameter()
                .map(|parameter| parameter.label.as_str()),
            Some("config")
        );
    }

    struct FakeAdapter {
        descriptor: AdapterDescriptor,
    }

    impl CodeIntelAdapter for FakeAdapter {
        fn descriptor(&self) -> &AdapterDescriptor {
            &self.descriptor
        }
    }

    fn fake_adapter(
        id: &str,
        languages: Vec<&str>,
        surfaces: Vec<CodeIntelSurface>,
    ) -> FakeAdapter {
        FakeAdapter {
            descriptor: AdapterDescriptor {
                id: id.to_string(),
                display_name: id.to_string(),
                languages: languages.into_iter().map(str::to_string).collect(),
                surfaces,
                source: AdapterSource::BuiltIn,
            },
        }
    }

    #[test]
    fn registry_finds_adapter_by_language_and_surface() {
        let mut registry = CodeIntelRegistry::new();
        registry.register(fake_adapter(
            "rust",
            vec!["rust"],
            vec![CodeIntelSurface::Diagnostics, CodeIntelSurface::Definition],
        ));
        registry.register(fake_adapter(
            "typescript",
            vec!["typescript", "javascript"],
            vec![CodeIntelSurface::References],
        ));

        let adapter = registry
            .find(Some("Rust"), CodeIntelSurface::Definition)
            .expect("rust definition adapter");
        assert_eq!(adapter.descriptor().id, "rust");
    }

    #[test]
    fn registry_reports_clear_no_backend_error() {
        let registry = CodeIntelRegistry::new();

        let error = match registry.find(Some("rust"), CodeIntelSurface::References) {
            Ok(adapter) => panic!("unexpected adapter: {}", adapter.descriptor().id),
            Err(error) => error,
        };

        assert_eq!(
            error,
            CodeIntelError::NoAdapter {
                language: Some("rust".to_string()),
                surface: CodeIntelSurface::References,
            }
        );
        assert!(error.to_string().contains("no code-intelligence adapter"));
    }
}
