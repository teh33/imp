use imp_llm::auth::AuthStore;
use imp_llm::model::ModelMeta;

use crate::views::model_selector::ModelSelectorState;

use super::{filtered_model_options, include_current_model_option, App, UiMode};

impl App {
    /// Return models filtered by `config.enabled_models` (if set) and by
    /// available credentials. Models whose provider has no auth configured
    /// are hidden unless explicitly listed in `enabled_models`.
    pub(super) fn filtered_models(&self) -> Vec<ModelMeta> {
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
        filtered_model_options(&self.model_registry, &self.config, &auth_store)
    }

    pub(super) fn open_model_selector(&mut self) {
        let models = self.filtered_models();
        let (models, current_model) =
            include_current_model_option(models, &self.model_registry, &self.model_name);
        self.mode = UiMode::ModelSelector(ModelSelectorState::new(models, current_model));
    }

    pub(super) fn cycle_model(&mut self, forward: bool) {
        let models = self.filtered_models();
        if models.is_empty() {
            return;
        }
        let current_idx = models.iter().position(|m| m.id == self.model_name);
        let next_idx = match current_idx {
            Some(idx) => {
                if forward {
                    (idx + 1) % models.len()
                } else {
                    (idx + models.len() - 1) % models.len()
                }
            }
            None => 0,
        };
        self.model_name = models[next_idx].id.clone();
        self.context_window = models[next_idx].context_window;
        self.invalidate_chat_render_cache();
        self.push_system_msg(&format!("Model: {}", self.model_name));
    }
}
