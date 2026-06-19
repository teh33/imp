use imp_core::config::AnimationLevel;
use imp_llm::ThinkingLevel;

pub(super) fn theme_options(current: Option<&str>) -> Vec<String> {
    let mut options = vec!["default".to_string(), "light".to_string()];
    if let Some(current) = current.filter(|value| !value.trim().is_empty()) {
        if !options.iter().any(|option| option == current) {
            options.push(current.to_string());
        }
    }
    options
}

pub(super) fn next_thinking(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Off => ThinkingLevel::Low,
        ThinkingLevel::Minimal => ThinkingLevel::Low,
        ThinkingLevel::Low => ThinkingLevel::Medium,
        ThinkingLevel::Medium => ThinkingLevel::High,
        ThinkingLevel::High => ThinkingLevel::XHigh,
        ThinkingLevel::XHigh => ThinkingLevel::Off,
    }
}

pub(super) fn prev_thinking(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Off => ThinkingLevel::XHigh,
        ThinkingLevel::Minimal => ThinkingLevel::Off,
        ThinkingLevel::Low => ThinkingLevel::Off,
        ThinkingLevel::Medium => ThinkingLevel::Low,
        ThinkingLevel::High => ThinkingLevel::Medium,
        ThinkingLevel::XHigh => ThinkingLevel::High,
    }
}

pub(super) fn thinking_label(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "Off",
        ThinkingLevel::Minimal => "Minimal",
        ThinkingLevel::Low => "Low",
        ThinkingLevel::Medium => "Medium",
        ThinkingLevel::High => "High",
        ThinkingLevel::XHigh => "XHigh",
    }
}

pub(super) fn animation_label(level: AnimationLevel) -> &'static str {
    match level {
        AnimationLevel::None => "none",
        AnimationLevel::Spinner => "spinner",
        AnimationLevel::Minimal => "minimal",
    }
}
