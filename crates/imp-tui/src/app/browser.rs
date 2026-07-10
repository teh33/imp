use imp_core::agent::{BrowserEvent, BrowserEventKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct BrowserUiState {
    pub session_id: Option<String>,
    pub domain: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub approval_scope: Option<String>,
    pub last_action: Option<String>,
    pub sequence: Option<u64>,
    pub active: bool,
    pub failed: bool,
}

impl BrowserUiState {
    pub(super) fn apply(&mut self, event: &BrowserEvent) {
        if let Some(session_id) = &event.session_id {
            self.session_id = Some(session_id.clone());
        }
        if let Some(domain) = &event.domain {
            self.domain = Some(domain.clone());
        }
        if let Some(url) = &event.url {
            self.url = Some(url.clone());
        }
        if let Some(title) = &event.title {
            self.title = Some(title.clone());
        }
        if let Some(action) = &event.action {
            self.last_action = Some(action.clone());
        }
        if let Some(sequence) = event.sequence {
            self.sequence = Some(sequence);
        }
        match event.kind {
            BrowserEventKind::SessionStarted => {
                self.active = true;
                self.failed = false;
            }
            BrowserEventKind::InputApproved => {
                self.approval_scope = event.approval_scope.clone();
            }
            BrowserEventKind::SessionFailed => {
                self.active = false;
                self.failed = true;
                self.approval_scope = None;
            }
            BrowserEventKind::SessionStopped => {
                self.active = false;
                self.failed = false;
                self.approval_scope = None;
            }
            BrowserEventKind::Navigated
            | BrowserEventKind::Observation
            | BrowserEventKind::InputRequested
            | BrowserEventKind::InputDenied
            | BrowserEventKind::ActionCompleted => {}
        }
    }

    pub(super) fn status(&self, event: &BrowserEvent) -> String {
        let location = self
            .domain
            .as_deref()
            .or(self.title.as_deref())
            .unwrap_or("Lightpanda");
        let action = browser_event_label(event);
        match self.sequence {
            Some(sequence) => format!("{action} · {location} · #{sequence}"),
            None => format!("{action} · {location}"),
        }
    }
}

fn browser_event_label(event: &BrowserEvent) -> &'static str {
    match event.kind {
        BrowserEventKind::SessionStarted => "Started",
        BrowserEventKind::Navigated => "Navigated",
        BrowserEventKind::Observation => "Observed",
        BrowserEventKind::InputRequested => "Approval requested",
        BrowserEventKind::InputApproved => "Input approved",
        BrowserEventKind::InputDenied => "Input denied",
        BrowserEventKind::ActionCompleted => "Action completed",
        BrowserEventKind::SessionFailed => "Failed",
        BrowserEventKind::SessionStopped => "Stopped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_ui_state_tracks_session_and_clears_approval_on_stop() {
        let mut state = BrowserUiState::default();
        let mut started = BrowserEvent::new(BrowserEventKind::SessionStarted);
        started.session_id = Some("browser-1".into());
        state.apply(&started);
        assert!(state.active);
        assert_eq!(state.session_id.as_deref(), Some("browser-1"));

        let mut approved = BrowserEvent::new(BrowserEventKind::InputApproved);
        approved.approval_scope = Some("domain".into());
        state.apply(&approved);
        assert_eq!(state.approval_scope.as_deref(), Some("domain"));

        state.apply(&BrowserEvent::new(BrowserEventKind::SessionStopped));
        assert!(!state.active);
        assert_eq!(state.approval_scope, None);
    }
}
