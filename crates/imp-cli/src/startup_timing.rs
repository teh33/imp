#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStage {
    ProcessStart,
    CwdResolved,
    ConfigResolved,
    SessionReady,
    AuthLoaded,
    ModelRegistryReady,
    ModelResolved,
    ProviderReady,
    ApiKeyResolved,
    AgentBuilt,
    PromptReady,
    RunLoopStarted,
}

impl StartupStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessStart => "process_start",
            Self::CwdResolved => "cwd_resolved",
            Self::ConfigResolved => "config_resolved",
            Self::SessionReady => "session_ready",
            Self::AuthLoaded => "auth_loaded",
            Self::ModelRegistryReady => "model_registry_ready",
            Self::ModelResolved => "model_resolved",
            Self::ProviderReady => "provider_ready",
            Self::ApiKeyResolved => "api_key_resolved",
            Self::AgentBuilt => "agent_built",
            Self::PromptReady => "prompt_ready",
            Self::RunLoopStarted => "run_loop_started",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupTiming {
    pub stage: StartupStage,
    pub since_start_ms: u64,
    pub since_previous_ms: u64,
}

#[derive(Debug)]
pub(crate) struct StartupTimer {
    started_at: std::time::Instant,
    last_mark_at: std::time::Instant,
    enabled: bool,
}

impl StartupTimer {
    pub(crate) fn new(enabled: bool) -> Self {
        let now = std::time::Instant::now();
        Self {
            started_at: now,
            last_mark_at: now,
            enabled,
        }
    }

    fn mark(&mut self, stage: StartupStage) -> Option<StartupTiming> {
        if !self.enabled {
            return None;
        }
        let now = std::time::Instant::now();
        let timing = StartupTiming {
            stage,
            since_start_ms: now.duration_since(self.started_at).as_millis() as u64,
            since_previous_ms: now.duration_since(self.last_mark_at).as_millis() as u64,
        };
        self.last_mark_at = now;
        Some(timing)
    }
}

pub(crate) fn emit_startup_timing(timer: &mut StartupTimer, stage: StartupStage) {
    if let Some(timing) = timer.mark(stage) {
        eprintln!(
            "[startup stage={} total={}ms delta={}ms]",
            timing.stage.as_str(),
            timing.since_start_ms,
            timing.since_previous_ms,
        );
    }
}
