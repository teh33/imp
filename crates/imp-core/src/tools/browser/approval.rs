use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;

use super::{BrowserAction, BrowserSessionId};
use crate::tools::ToolApproval;
use crate::ui::{SelectOption, UserInterface};

#[derive(Debug, Default)]
pub(crate) struct BrowserApprovalStore {
    sessions: HashSet<BrowserSessionId>,
    domains: HashSet<(BrowserSessionId, String)>,
}

impl BrowserApprovalStore {
    pub(crate) fn allows(
        &self,
        session: &BrowserSessionId,
        domain: Option<&str>,
    ) -> Option<&'static str> {
        if self.sessions.contains(session) {
            return Some("session");
        }
        domain.and_then(|domain| {
            self.domains
                .contains(&(session.clone(), domain.to_string()))
                .then_some("domain")
        })
    }

    pub(crate) fn grant_session(&mut self, session: BrowserSessionId) {
        self.sessions.insert(session);
    }

    pub(crate) fn grant_domain(&mut self, session: BrowserSessionId, domain: String) {
        self.domains.insert((session, domain));
    }

    pub(crate) fn remove_session(&mut self, session: &BrowserSessionId) {
        self.sessions.remove(session);
        self.domains.retain(|(id, _)| id != session);
    }
}

pub(crate) async fn request_browser_approval(
    store: &tokio::sync::Mutex<BrowserApprovalStore>,
    params: &Value,
    ui: Arc<dyn UserInterface>,
) -> ToolApproval {
    let request = match BrowserApprovalRequest::from_params(params) {
        Ok(request) => request,
        Err(error) => return ToolApproval::denied(error),
    };
    if !request.action.requires_fresh_approval(&request.params) {
        if let Some(scope) = store
            .lock()
            .await
            .allows(&request.session, request.domain.as_deref())
        {
            return ToolApproval::approved(scope);
        }
    }
    if !ui.has_ui() {
        return ToolApproval::denied("browser input requires interactive approval");
    }
    prompt_for_approval(store, request, ui).await
}

struct BrowserApprovalRequest {
    action: BrowserAction,
    session: BrowserSessionId,
    domain: Option<String>,
    params: Value,
}

impl BrowserApprovalRequest {
    fn from_params(params: &Value) -> Result<Self, String> {
        Ok(Self {
            action: BrowserAction::parse(params)?,
            session: params
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser action requires session_id".to_string())
                .and_then(BrowserSessionId::parse)?,
            domain: params
                .get("approval_domain")
                .and_then(Value::as_str)
                .map(str::to_string),
            params: params.clone(),
        })
    }
}

async fn prompt_for_approval(
    store: &tokio::sync::Mutex<BrowserApprovalStore>,
    request: BrowserApprovalRequest,
    ui: Arc<dyn UserInterface>,
) -> ToolApproval {
    let domain = request.domain.as_deref();
    let context = approval_context(request.action, &request.params, domain);
    let fresh = request.action.requires_fresh_approval(&request.params);
    let mut options = vec![option("Allow once", "Approve only this browser action")];
    if !fresh && domain.is_some() {
        options.push(option(
            "Allow for this domain",
            "Approve browser input for this session and domain",
        ));
    }
    if !fresh {
        options.push(option(
            "Allow for this session",
            "Approve browser input until this browser session stops",
        ));
    }
    options.push(option("Deny", "Do not run this browser action"));
    let choice = ui
        .select_with_context("Approve browser input", &context, &options)
        .await;
    let domain_choice = (!fresh && domain.is_some()).then_some(1);
    let session_choice = if fresh {
        None
    } else if domain.is_some() {
        Some(2)
    } else {
        Some(1)
    };
    match choice {
        Some(0) => ToolApproval::approved("once"),
        Some(index) if Some(index) == domain_choice => {
            let Some(domain) = domain else {
                return ToolApproval::denied("approval domain was unavailable");
            };
            store
                .lock()
                .await
                .grant_domain(request.session, domain.to_string());
            ToolApproval::approved("domain")
        }
        Some(index) if Some(index) == session_choice => {
            store.lock().await.grant_session(request.session);
            ToolApproval::approved("session")
        }
        _ => ToolApproval::denied("user denied or cancelled browser input"),
    }
}

fn approval_context(action: BrowserAction, params: &Value, domain: Option<&str>) -> String {
    let target = params
        .get("selector")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            params
                .get("backend_node_id")
                .and_then(Value::as_u64)
                .map(|node| format!("node {node}"))
        })
        .unwrap_or_else(|| "current page".into());
    let mut lines = vec![
        format!("Action: {}", action.name()),
        format!("Target: {target}"),
    ];
    if let Some(domain) = domain {
        lines.push(format!("Domain: {domain}"));
    }
    if action.has_sensitive_value() {
        let length = params
            .get("value")
            .and_then(Value::as_str)
            .map(str::len)
            .unwrap_or(0);
        lines.push(format!("Value: [redacted, {length} characters]"));
    }
    lines.join("\n")
}

fn option(label: &str, description: &str) -> SelectOption {
    SelectOption {
        label: label.into(),
        description: Some(description.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HeadlessUi;

    #[async_trait::async_trait]
    impl UserInterface for HeadlessUi {
        fn has_ui(&self) -> bool {
            false
        }
        async fn notify(&self, _: &str, _: crate::ui::NotifyLevel) {}
        async fn confirm(&self, _: &str, _: &str) -> Option<bool> {
            None
        }
        async fn select_with_context(&self, _: &str, _: &str, _: &[SelectOption]) -> Option<usize> {
            None
        }
        async fn input_with_context(&self, _: &str, _: &str, _: &str) -> Option<String> {
            None
        }
        async fn set_status(&self, _: &str, _: Option<&str>) {}
        async fn set_widget(&self, _: &str, _: Option<crate::ui::WidgetContent>) {}
        async fn custom(&self, _: crate::ui::ComponentSpec) -> Option<Value> {
            None
        }
    }

    #[tokio::test]
    async fn headless_request_fails_closed_without_lease() {
        let store = tokio::sync::Mutex::new(BrowserApprovalStore::default());
        let approval = request_browser_approval(
            &store,
            &serde_json::json!({
                "action": "click",
                "session_id": "browser_00000000000000000000000000000000",
                "selector": "button"
            }),
            Arc::new(HeadlessUi),
        )
        .await;
        assert!(!approval.approved);
        assert!(approval.reason.contains("interactive approval"));
    }

    #[tokio::test]
    async fn fresh_sensitive_action_ignores_existing_session_lease_headlessly() {
        let session = BrowserSessionId::parse("browser_00000000000000000000000000000000").unwrap();
        let mut leases = BrowserApprovalStore::default();
        leases.grant_session(session);
        let store = tokio::sync::Mutex::new(leases);
        let approval = request_browser_approval(
            &store,
            &serde_json::json!({
                "action": "click",
                "session_id": "browser_00000000000000000000000000000000",
                "selector": "button[type=submit]",
                "approval_domain": "example.com"
            }),
            Arc::new(HeadlessUi),
        )
        .await;
        assert!(!approval.approved);
    }

    #[test]
    fn approval_store_scopes_and_revokes_leases() {
        let session = BrowserSessionId::parse("browser_00000000000000000000000000000000").unwrap();
        let mut store = BrowserApprovalStore::default();
        assert_eq!(store.allows(&session, Some("example.com")), None);
        store.grant_domain(session.clone(), "example.com".into());
        assert_eq!(store.allows(&session, Some("example.com")), Some("domain"));
        assert_eq!(store.allows(&session, Some("other.com")), None);
        store.grant_session(session.clone());
        assert_eq!(store.allows(&session, Some("other.com")), Some("session"));
        store.remove_session(&session);
        assert_eq!(store.allows(&session, Some("example.com")), None);
    }

    #[test]
    fn approval_context_redacts_fill_value() {
        let params = serde_json::json!({
            "action": "fill",
            "selector": "#email",
            "value": "asher@example.com"
        });
        let context = approval_context(BrowserAction::Fill, &params, Some("example.com"));
        assert!(!context.contains("asher@example.com"));
        assert!(context.contains("[redacted, 17 characters]"));
    }
}
