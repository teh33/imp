use super::*;

#[test]
fn uses_manual_session_name_when_present() {
    let mut app = make_app();
    app.session.set_name("my chat");
    assert_eq!(app.terminal_title(), "imp — my chat");
}

#[test]
fn falls_back_to_summarized_first_prompt() {
    let mut app = make_app();
    app.session
        .append(SessionEntry::Message {
            id: "m1".into(),
            parent_id: None,
            message: Message::user(
                "can we adjust the information that is displayed in the top bar",
            ),
        })
        .unwrap();
    assert_eq!(app.terminal_title(), "imp — adjust top bar");
}

#[test]
fn uses_nine_dot_spinner_while_streaming() {
    let mut app = make_app();
    app.session.set_name("my chat");
    app.is_streaming = true;
    app.tick = 0;
    assert_eq!(app.terminal_title(), "⠋ — my chat");
    app.tick = 16;
    assert_eq!(app.terminal_title(), "⠼ — my chat");
}

#[test]
fn uses_question_mark_while_waiting_for_answer() {
    let mut app = make_app();
    app.session.set_name("my chat");
    app.is_streaming = true;
    app.ask_state = Some(crate::views::ask_bar::AskState::new(
        "Which option?".into(),
        String::new(),
        Vec::new(),
        false,
    ));
    assert_eq!(app.terminal_title(), "? — my chat");
}

#[tokio::test]
async fn spins_while_agent_start_is_pending() {
    let mut app = make_app();
    app.session.set_name("my chat");
    app.agent_start_task = Some(tokio::spawn(async {}));
    app.tick = 4;
    assert_eq!(app.terminal_title(), "⠙ — my chat");
}

#[test]
fn uses_static_working_glyph_when_animations_are_off() {
    let mut app = make_app();
    app.config.ui.animations = imp_core::config::AnimationLevel::None;
    app.session.set_name("my chat");
    app.is_streaming = true;
    app.tick = 36;
    assert_eq!(app.terminal_title(), "• — my chat");
}

#[test]
fn uses_loop_icon_when_loop_is_active() {
    let mut app = make_app();
    app.session.set_name("my chat");
    app.loop_state = Some(LoopState {
        message: "keep going".into(),
        completed_turns: 1,
        budget: Some(3),
    });
    app.is_streaming = true;

    app.tick = 0;
    assert_eq!(app.terminal_title(), "↻ — my chat");
    app.tick = 8;
    assert_eq!(app.terminal_title(), "↻ — my chat");
}

#[test]
fn uses_static_loop_glyph_when_animations_are_off() {
    let mut app = make_app();
    app.config.ui.animations = imp_core::config::AnimationLevel::None;
    app.session.set_name("my chat");
    app.loop_state = Some(LoopState {
        message: "keep going".into(),
        completed_turns: 1,
        budget: Some(3),
    });
    app.is_streaming = true;
    app.tick = 8;

    assert_eq!(app.terminal_title(), "↻ — my chat");
}

#[test]
fn defaults_to_chat_when_empty() {
    let app = make_app();
    assert_eq!(app.terminal_title(), "imp — chat");
}
