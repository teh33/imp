use std::time::Instant;

use crossterm::event::{Event, KeyEventKind};

use crate::event_source::TerminalEventSource;
use crate::terminal::{set_window_title, InteractiveTerminal};

use super::{
    App, RuntimeSignal, ACTIVE_FRAME_INTERVAL, IDLE_FRAME_INTERVAL, MAX_RUNTIME_SIGNAL_BATCH,
    MAX_TERMINAL_EVENTS_PER_TICK, SLOW_TUI_EVENT_THRESHOLD, SLOW_TUI_RENDER_THRESHOLD,
};

impl App {
    pub(super) fn sync_window_title_if_needed(&mut self) {
        if self.is_streaming || self.agent_start_task.is_some() || self.compaction_task.is_some() {
            self.sync_window_title();
        }
    }

    pub(super) async fn render_if_dirty(
        &mut self,
        terminal: &mut InteractiveTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.sync_window_title();
        if self.needs_redraw {
            let started = Instant::now();
            terminal.draw(|frame| self.render(frame))?;
            let elapsed = started.elapsed();
            if elapsed >= SLOW_TUI_RENDER_THRESHOLD {
                self.trace_tui(format!("slow_render duration_ms={}", elapsed.as_millis()));
            }
            self.needs_redraw = false;
            self.start_pending_agent_after_redraw();
        }
        Ok(())
    }

    pub(super) async fn drain_terminal_events(
        &mut self,
        rx: &mut tokio::sync::mpsc::Receiver<Event>,
        first: Event,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let started = Instant::now();
        let mut count = 1usize;
        self.handle_terminal_event(first)?;
        for _ in 1..MAX_TERMINAL_EVENTS_PER_TICK {
            match rx.try_recv() {
                Ok(event) => {
                    count += 1;
                    self.handle_terminal_event(event)?;
                }
                Err(_) => break,
            }
            if !self.running {
                break;
            }
        }
        let elapsed = started.elapsed();
        if count > 1 || elapsed >= SLOW_TUI_EVENT_THRESHOLD {
            self.trace_tui(format!(
                "terminal_batch count={} duration_ms={}",
                count,
                elapsed.as_millis()
            ));
        }
        Ok(())
    }

    pub(super) async fn event_loop(
        &mut self,
        terminal: &mut InteractiveTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (_input_source, mut terminal_events) = TerminalEventSource::spawn();
        let mut frame_tick = tokio::time::interval(ACTIVE_FRAME_INTERVAL);
        frame_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut idle_tick = tokio::time::interval(IDLE_FRAME_INTERVAL);
        idle_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        self.render_if_dirty(terminal).await?;

        loop {
            tokio::select! {
                event = terminal_events.recv() => {
                    if let Some(event) = event {
                        self.drain_terminal_events(&mut terminal_events, event).await?;
                    } else {
                        break;
                    }
                }
                signal = self.runtime_signal_rx.recv() => {
                    if let Some(signal) = signal {
                        self.drain_runtime_signal_batch(signal);
                    }
                }
                _ = frame_tick.tick() => {
                    self.tick = self.tick.wrapping_add(1);
                    self.maybe_autoscroll_selection();
                    if self.is_streaming
                        || self.agent_start_task.is_some()
                        || self.compaction_task.is_some()
                        || self.drag_autoscroll.is_some()
                    {
                        self.sync_window_title_if_needed();
                        self.needs_redraw = true;
                    }
                }
                _ = idle_tick.tick() => {
                    self.pump_runtime_signals().await;
                }
            }

            self.pump_runtime_signals().await;
            self.render_if_dirty(terminal).await?;

            if !self.running {
                break;
            }
        }

        Ok(())
    }

    pub(super) fn trace_tui(&self, message: impl AsRef<str>) {
        if let Some(trace) = &self.tui_trace {
            trace.log(message);
        }
    }

    pub(super) fn drain_runtime_signal_batch(&mut self, first: RuntimeSignal) {
        let started = Instant::now();
        let mut count = 1usize;
        self.handle_runtime_signal(first);
        for _ in 1..MAX_RUNTIME_SIGNAL_BATCH {
            match self.runtime_signal_rx.try_recv() {
                Ok(signal) => {
                    count += 1;
                    self.handle_runtime_signal(signal);
                }
                Err(_) => break,
            }
            if !self.running {
                break;
            }
        }
        let elapsed = started.elapsed();
        if count > 1 || elapsed >= SLOW_TUI_EVENT_THRESHOLD {
            self.trace_tui(format!(
                "runtime_signal_batch count={} duration_ms={}",
                count,
                elapsed.as_millis()
            ));
        }
    }

    pub(super) fn handle_terminal_event(
        &mut self,
        event: Event,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_key(key)?;
            }
            Event::Paste(text) => {
                self.handle_paste(text);
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(mouse);
            }
            Event::Resize(_, _) => {
                self.needs_redraw = true;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn sync_window_title(&mut self) {
        let title = self.terminal_title();
        if self.last_terminal_title.as_deref() == Some(title.as_str()) {
            return;
        }
        let _ = set_window_title(&title);
        self.last_terminal_title = Some(title);
    }
}
