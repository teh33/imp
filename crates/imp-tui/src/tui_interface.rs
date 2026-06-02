use std::sync::Arc;

use async_trait::async_trait;
use imp_core::ui::{
    ComponentSpec, NotifyLevel, SelectOption, SelectionAnswer, UserInterface, WidgetContent,
};
use tokio::sync::mpsc;

/// Events sent from the TuiInterface to the main App event loop.
#[derive(Debug)]
pub enum UiRequest {
    Notify {
        message: String,
        level: NotifyLevel,
    },
    Confirm {
        title: String,
        message: String,
        reply: tokio::sync::oneshot::Sender<Option<bool>>,
    },
    Select {
        title: String,
        context: String,
        options: Vec<SelectOption>,
        reply: tokio::sync::oneshot::Sender<Option<usize>>,
    },
    SelectOrInput {
        title: String,
        context: String,
        options: Vec<SelectOption>,
        placeholder: String,
        reply: tokio::sync::oneshot::Sender<Option<SelectionAnswer>>,
    },
    MultiSelect {
        title: String,
        context: String,
        options: Vec<SelectOption>,
        reply: tokio::sync::oneshot::Sender<Option<Vec<usize>>>,
    },
    Input {
        title: String,
        context: String,
        placeholder: String,
        reply: tokio::sync::oneshot::Sender<Option<String>>,
    },
    SetStatus {
        key: String,
        text: Option<String>,
    },
    SetWidget {
        key: String,
        content: Option<WidgetContent>,
    },
    Custom {
        component: ComponentSpec,
        reply: tokio::sync::oneshot::Sender<Option<serde_json::Value>>,
    },
}

/// UserInterface implementation that sends requests to the TUI event loop.
///
/// Tools and extensions call methods on this trait. The implementation
/// sends a request to the main event loop, which renders the appropriate
/// UI element and sends back the response.
pub struct TuiInterface {
    tx: mpsc::Sender<UiRequest>,
}

impl TuiInterface {
    pub fn new(tx: mpsc::Sender<UiRequest>) -> Arc<Self> {
        Arc::new(Self { tx })
    }
}

#[async_trait]
impl UserInterface for TuiInterface {
    fn has_ui(&self) -> bool {
        true
    }

    async fn notify(&self, message: &str, level: NotifyLevel) {
        let _ = self
            .tx
            .send(UiRequest::Notify {
                message: message.to_string(),
                level,
            })
            .await;
    }

    async fn confirm(&self, title: &str, message: &str) -> Option<bool> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self
            .tx
            .send(UiRequest::Confirm {
                title: title.to_string(),
                message: message.to_string(),
                reply: reply_tx,
            })
            .await;
        reply_rx.await.ok().flatten()
    }

    async fn select_with_context(
        &self,
        title: &str,
        context: &str,
        options: &[SelectOption],
    ) -> Option<usize> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self
            .tx
            .send(UiRequest::Select {
                title: title.to_string(),
                context: context.to_string(),
                options: options.to_vec(),
                reply: reply_tx,
            })
            .await;
        reply_rx.await.ok().flatten()
    }

    async fn select_or_input_with_context(
        &self,
        title: &str,
        context: &str,
        options: &[SelectOption],
        placeholder: &str,
    ) -> Option<SelectionAnswer> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self
            .tx
            .send(UiRequest::SelectOrInput {
                title: title.to_string(),
                context: context.to_string(),
                options: options.to_vec(),
                placeholder: placeholder.to_string(),
                reply: reply_tx,
            })
            .await;
        reply_rx.await.ok().flatten()
    }

    async fn multi_select_with_context(
        &self,
        title: &str,
        context: &str,
        options: &[SelectOption],
    ) -> Option<Vec<usize>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self
            .tx
            .send(UiRequest::MultiSelect {
                title: title.to_string(),
                context: context.to_string(),
                options: options.to_vec(),
                reply: reply_tx,
            })
            .await;
        reply_rx.await.ok().flatten()
    }

    async fn input_with_context(
        &self,
        title: &str,
        context: &str,
        placeholder: &str,
    ) -> Option<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self
            .tx
            .send(UiRequest::Input {
                title: title.to_string(),
                context: context.to_string(),
                placeholder: placeholder.to_string(),
                reply: reply_tx,
            })
            .await;
        reply_rx.await.ok().flatten()
    }

    async fn set_status(&self, key: &str, text: Option<&str>) {
        let _ = self
            .tx
            .send(UiRequest::SetStatus {
                key: key.to_string(),
                text: text.map(String::from),
            })
            .await;
    }

    async fn set_widget(&self, key: &str, content: Option<WidgetContent>) {
        let _ = self
            .tx
            .send(UiRequest::SetWidget {
                key: key.to_string(),
                content,
            })
            .await;
    }

    async fn custom(&self, component: ComponentSpec) -> Option<serde_json::Value> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self
            .tx
            .send(UiRequest::Custom {
                component,
                reply: reply_tx,
            })
            .await;
        reply_rx.await.ok().flatten()
    }
}
