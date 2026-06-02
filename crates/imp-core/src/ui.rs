use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Abstraction over user interaction. Tools and extensions use this
/// without knowing whether they're in a TUI, headless, or print mode.
#[async_trait]
pub trait UserInterface: Send + Sync {
    /// Whether this interface can show interactive UI.
    fn has_ui(&self) -> bool;

    /// Non-blocking notification.
    async fn notify(&self, message: &str, level: NotifyLevel);

    /// Yes/no confirmation. Returns None if no UI or cancelled.
    async fn confirm(&self, title: &str, message: &str) -> Option<bool>;

    /// Select from options. Returns None if no UI or cancelled.
    async fn select(&self, title: &str, options: &[SelectOption]) -> Option<usize> {
        self.select_with_context(title, "", options).await
    }

    /// Select from options with context. Returns None if no UI or cancelled.
    async fn select_with_context(
        &self,
        title: &str,
        context: &str,
        options: &[SelectOption],
    ) -> Option<usize>;

    /// Select from options, or return a custom typed answer when supported.
    ///
    /// Default implementations preserve the older select-only behavior.
    async fn select_or_input_with_context(
        &self,
        title: &str,
        context: &str,
        options: &[SelectOption],
        _placeholder: &str,
    ) -> Option<SelectionAnswer> {
        self.select_with_context(title, context, options)
            .await
            .map(SelectionAnswer::Choice)
    }

    /// Select multiple options. Returns None if no UI or cancelled.
    async fn multi_select_with_context(
        &self,
        title: &str,
        context: &str,
        options: &[SelectOption],
    ) -> Option<Vec<usize>> {
        self.select_with_context(title, context, options)
            .await
            .map(|index| vec![index])
    }

    /// Text input. Returns None if no UI or cancelled.
    async fn input(&self, title: &str, placeholder: &str) -> Option<String> {
        self.input_with_context(title, "", placeholder).await
    }

    /// Text input with context. Returns None if no UI or cancelled.
    async fn input_with_context(
        &self,
        title: &str,
        context: &str,
        placeholder: &str,
    ) -> Option<String>;

    /// Persistent status in footer.
    async fn set_status(&self, key: &str, text: Option<&str>);

    /// Widget above/below editor.
    async fn set_widget(&self, key: &str, content: Option<WidgetContent>);

    /// Full declarative custom component. Returns the serialized result.
    async fn custom(&self, component: ComponentSpec) -> Option<serde_json::Value>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotifyLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionAnswer {
    Choice(usize),
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WidgetContent {
    Lines(Vec<String>),
    Component(ComponentSpec),
}

/// Declarative component specification (from Lua tables or native code).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub component_type: String,
    pub props: serde_json::Value,
    pub children: Vec<ComponentSpec>,
}

/// Null interface for print mode — returns None for everything.
pub struct NullInterface;

#[async_trait]
impl UserInterface for NullInterface {
    fn has_ui(&self) -> bool {
        false
    }
    async fn notify(&self, _message: &str, _level: NotifyLevel) {}
    async fn confirm(&self, _title: &str, _message: &str) -> Option<bool> {
        None
    }
    async fn select_with_context(
        &self,
        _title: &str,
        _context: &str,
        _options: &[SelectOption],
    ) -> Option<usize> {
        None
    }
    async fn multi_select_with_context(
        &self,
        _title: &str,
        _context: &str,
        _options: &[SelectOption],
    ) -> Option<Vec<usize>> {
        None
    }
    async fn input_with_context(
        &self,
        _title: &str,
        _context: &str,
        _placeholder: &str,
    ) -> Option<String> {
        None
    }
    async fn set_status(&self, _key: &str, _text: Option<&str>) {}
    async fn set_widget(&self, _key: &str, _content: Option<WidgetContent>) {}
    async fn custom(&self, _component: ComponentSpec) -> Option<serde_json::Value> {
        None
    }
}
