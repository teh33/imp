/// Which field in the settings panel is focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Model,
    ChosenModels,
    Theme,
    ThinkingLevel,
    MaxTokens,
    MaxTurns,
    ObservationMask,
    ReadMaxLines,
    SidebarWidth,
    WordWrap,
    Animations,
    AutoOpenSidebar,
    SidebarAutoOpenWidth,
    ThinkingLines,
    StreamingLines,
    MouseScrollLines,
    KeyboardScrollLines,
    ShowTimestamps,
    ShowCost,
    ShowContextUsage,
    NotifyOnAgentComplete,
    ContinuePolicy,
    AgentMode,
    WriteOverwritePolicy,
    ShellBackend,
    LuaNativeTools,
    LuaShellExec,
    LuaHttp,
    LuaSecrets,
    ImproveAutoTurnBudget,
    LoopTurnBudget,
    WebSearchProvider,
    TavilyApiKey,
    ExaApiKey,
    WorkflowScope,
    WorkflowAutoCommit,
    WorkflowAutoCloseParent,
    WorkflowVerifyTimeout,
    WorkflowRunBackground,
    WorkflowMaxWorkers,
    WorkflowReviewAfterRun,
    WorkflowContinueAfterFailure,
    Save,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Model,
    Ui,
    Security,
    Web,
    Workflow,
}

pub const SETTINGS_TABS: &[SettingsTab] = &[
    SettingsTab::General,
    SettingsTab::Model,
    SettingsTab::Ui,
    SettingsTab::Security,
    SettingsTab::Web,
    SettingsTab::Workflow,
];

const GENERAL_FIELDS: &[SettingsField] = &[
    SettingsField::Theme,
    SettingsField::MaxTurns,
    SettingsField::NotifyOnAgentComplete,
    SettingsField::ContinuePolicy,
    SettingsField::ImproveAutoTurnBudget,
    SettingsField::LoopTurnBudget,
];

const MODEL_FIELDS: &[SettingsField] = &[
    SettingsField::Model,
    SettingsField::ChosenModels,
    SettingsField::ThinkingLevel,
    SettingsField::MaxTokens,
    SettingsField::ObservationMask,
];

const UI_FIELDS: &[SettingsField] = &[
    SettingsField::ReadMaxLines,
    SettingsField::SidebarWidth,
    SettingsField::WordWrap,
    SettingsField::Animations,
    SettingsField::AutoOpenSidebar,
    SettingsField::SidebarAutoOpenWidth,
    SettingsField::ThinkingLines,
    SettingsField::StreamingLines,
    SettingsField::MouseScrollLines,
    SettingsField::KeyboardScrollLines,
    SettingsField::ShowTimestamps,
    SettingsField::ShowCost,
    SettingsField::ShowContextUsage,
];

const SECURITY_FIELDS: &[SettingsField] = &[
    SettingsField::AgentMode,
    SettingsField::WriteOverwritePolicy,
    SettingsField::ShellBackend,
    SettingsField::LuaNativeTools,
    SettingsField::LuaShellExec,
    SettingsField::LuaHttp,
    SettingsField::LuaSecrets,
];

const WEB_FIELDS: &[SettingsField] = &[
    SettingsField::WebSearchProvider,
    SettingsField::TavilyApiKey,
    SettingsField::ExaApiKey,
];

const WORKFLOW_FIELDS: &[SettingsField] = &[
    SettingsField::WorkflowScope,
    SettingsField::WorkflowAutoCommit,
    SettingsField::WorkflowAutoCloseParent,
    SettingsField::WorkflowVerifyTimeout,
    SettingsField::WorkflowRunBackground,
    SettingsField::WorkflowMaxWorkers,
    SettingsField::WorkflowReviewAfterRun,
    SettingsField::WorkflowContinueAfterFailure,
];

pub(crate) const FIELDS: &[SettingsField] = &[
    SettingsField::Model,
    SettingsField::ChosenModels,
    SettingsField::Theme,
    SettingsField::ThinkingLevel,
    SettingsField::MaxTokens,
    SettingsField::MaxTurns,
    SettingsField::ObservationMask,
    SettingsField::ReadMaxLines,
    SettingsField::SidebarWidth,
    SettingsField::WordWrap,
    SettingsField::Animations,
    SettingsField::AutoOpenSidebar,
    SettingsField::SidebarAutoOpenWidth,
    SettingsField::ThinkingLines,
    SettingsField::StreamingLines,
    SettingsField::MouseScrollLines,
    SettingsField::KeyboardScrollLines,
    SettingsField::ShowTimestamps,
    SettingsField::ShowCost,
    SettingsField::ShowContextUsage,
    SettingsField::NotifyOnAgentComplete,
    SettingsField::ContinuePolicy,
    SettingsField::AgentMode,
    SettingsField::WriteOverwritePolicy,
    SettingsField::ShellBackend,
    SettingsField::LuaNativeTools,
    SettingsField::LuaShellExec,
    SettingsField::LuaHttp,
    SettingsField::LuaSecrets,
    SettingsField::ImproveAutoTurnBudget,
    SettingsField::LoopTurnBudget,
    SettingsField::WebSearchProvider,
    SettingsField::TavilyApiKey,
    SettingsField::ExaApiKey,
    SettingsField::WorkflowScope,
    SettingsField::WorkflowAutoCommit,
    SettingsField::WorkflowAutoCloseParent,
    SettingsField::WorkflowVerifyTimeout,
    SettingsField::WorkflowRunBackground,
    SettingsField::WorkflowMaxWorkers,
    SettingsField::WorkflowReviewAfterRun,
    SettingsField::WorkflowContinueAfterFailure,
    SettingsField::Save,
];

impl SettingsTab {
    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Model => "Model",
            SettingsTab::Ui => "UI",
            SettingsTab::Security => "Security",
            SettingsTab::Web => "Web",
            SettingsTab::Workflow => "Workflow",
        }
    }

    pub fn fields(self) -> &'static [SettingsField] {
        match self {
            SettingsTab::General => GENERAL_FIELDS,
            SettingsTab::Model => MODEL_FIELDS,
            SettingsTab::Ui => UI_FIELDS,
            SettingsTab::Security => SECURITY_FIELDS,
            SettingsTab::Web => WEB_FIELDS,
            SettingsTab::Workflow => WORKFLOW_FIELDS,
        }
    }

    pub fn empty_message(self) -> Option<&'static str> {
        match self {
            SettingsTab::Security => None,
            SettingsTab::Workflow => None,
            _ => None,
        }
    }
}

pub fn field_index(field: SettingsField) -> usize {
    FIELDS
        .iter()
        .position(|candidate| *candidate == field)
        .expect("settings field is registered")
}
