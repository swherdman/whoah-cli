use std::time::{Duration, Instant};

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};

use crate::action::Action;
use crate::ops::recover::{RecoveryEvent, RecoveryStep};

use super::Component;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepState {
    Pending,
    Running,
    Completed,
    Failed,
}

struct StepInfo {
    step: RecoveryStep,
    state: StepState,
    elapsed: Option<Duration>,
}

pub struct RecoveryView {
    active: bool,
    steps: Vec<StepInfo>,
    current_step: Option<usize>,
    output_lines: Vec<String>,
    output_scroll: u16,
    zone_progress: Option<(u32, u32)>,
    start_time: Option<Instant>,
    total_elapsed: Option<Duration>,
    error: Option<(String, Option<String>)>, // (error, workaround hint)
}

impl RecoveryView {
    pub fn new() -> Self {
        let steps = RecoveryStep::all()
            .iter()
            .map(|&step| StepInfo {
                step,
                state: StepState::Pending,
                elapsed: None,
            })
            .collect();

        Self {
            active: false,
            steps,
            current_step: None,
            output_lines: Vec::new(),
            output_scroll: 0,
            zone_progress: None,
            start_time: None,
            total_elapsed: None,
            error: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn start(&mut self) {
        self.active = true;
        self.start_time = Some(Instant::now());
        self.error = None;
        self.total_elapsed = None;
        self.output_lines.clear();
        self.output_scroll = 0;
        self.zone_progress = None;
        self.current_step = None;
        for step in &mut self.steps {
            step.state = StepState::Pending;
            step.elapsed = None;
        }
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    pub fn handle_recovery_event(&mut self, event: &RecoveryEvent) {
        match event {
            RecoveryEvent::StepStarted(step) => {
                let idx = step.index();
                self.current_step = Some(idx);
                self.steps[idx].state = StepState::Running;
            }
            RecoveryEvent::StepOutput(line) => {
                self.output_lines.push(line.clone());
                // Auto-scroll to bottom
                let total = self.output_lines.len() as u16;
                self.output_scroll = total.saturating_sub(1);
            }
            RecoveryEvent::ZoneProgress { running, expected } => {
                self.zone_progress = Some((*running, *expected));
                // Update output with zone progress
                let line = format!("Zones: {running}/{expected} running");
                if let Some(last) = self.output_lines.last() {
                    if last.starts_with("Zones: ") {
                        self.output_lines.pop();
                    }
                }
                self.output_lines.push(line);
            }
            RecoveryEvent::StepCompleted(step, duration) => {
                let idx = step.index();
                self.steps[idx].state = StepState::Completed;
                self.steps[idx].elapsed = Some(*duration);
            }
            RecoveryEvent::StepFailed {
                step,
                error,
                workaround,
            } => {
                let idx = step.index();
                self.steps[idx].state = StepState::Failed;
                self.error = Some((
                    error.clone(),
                    workaround.as_ref().map(|w| w.description().to_string()),
                ));
            }
            RecoveryEvent::RecoveryComplete(duration) => {
                self.total_elapsed = Some(*duration);
            }
        }
    }

    fn estimated_remaining(&self) -> Duration {
        let mut remaining = Duration::ZERO;
        for info in &self.steps {
            if info.state == StepState::Pending {
                remaining += info.step.estimated_duration();
            }
            if info.state == StepState::Running {
                // Estimate remaining as half the expected duration
                let expected = info.step.estimated_duration();
                let elapsed = info.elapsed.unwrap_or(Duration::ZERO);
                if elapsed < expected {
                    remaining += expected - elapsed;
                }
            }
        }
        remaining
    }

    fn completed_count(&self) -> usize {
        self.steps.iter().filter(|s| s.state == StepState::Completed).count()
    }
}

impl Component for RecoveryView {
    fn update(&mut self, action: &Action) {
        match action {
            Action::ScrollUp => {
                self.output_scroll = self.output_scroll.saturating_sub(1);
            }
            Action::ScrollDown => {
                self.output_scroll = self.output_scroll.saturating_add(1);
            }
            Action::RecoveryProgress(event) => {
                self.handle_recovery_event(event);
            }
            _ => {}
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Recovery ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Layout: progress bar (2) | step list (9) | output pane (rest)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),   // progress bar + ETA
                Constraint::Length(9),   // step list (7 steps + header + spacer)
                Constraint::Min(3),      // output pane
            ])
            .split(inner);

        // --- Progress bar ---
        let completed = self.completed_count();
        let total = RecoveryStep::total_count();
        let ratio = if total > 0 {
            completed as f64 / total as f64
        } else {
            0.0
        };

        let remaining = self.estimated_remaining();
        let eta_text = if let Some(dur) = self.total_elapsed {
            format!("Complete in {:.0}s", dur.as_secs_f64())
        } else if remaining.as_secs() > 0 {
            format!("Step {}/{}  ~{}s remaining", completed + 1, total, remaining.as_secs())
        } else {
            format!("Step {}/{}", completed + 1, total)
        };

        let gauge_color = if self.error.is_some() {
            Color::Red
        } else if self.total_elapsed.is_some() {
            Color::Green
        } else {
            Color::Yellow
        };

        let progress_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(chunks[0]);

        frame.render_widget(
            Paragraph::new(eta_text).style(Style::default().fg(gauge_color)),
            progress_area[0],
        );
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .gauge_style(Style::default().fg(gauge_color).bg(Color::DarkGray)),
            progress_area[1],
        );

        // --- Step list ---
        let mut step_lines: Vec<Line> = Vec::new();
        for info in &self.steps {
            let (icon, style) = match info.state {
                StepState::Pending => ("  ", Style::default().fg(Color::DarkGray)),
                StepState::Running => (">>", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                StepState::Completed => ("OK", Style::default().fg(Color::Green)),
                StepState::Failed => ("!!", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            };

            let time_str = match (info.state, info.elapsed) {
                (StepState::Completed, Some(d)) => format!(" ({:.1}s)", d.as_secs_f64()),
                (StepState::Completed, None) => " (done)".to_string(),
                (StepState::Running, _) => {
                    if let Some(start) = self.start_time {
                        format!(" ({:.0}s...)", start.elapsed().as_secs_f64())
                    } else {
                        String::new()
                    }
                }
                (StepState::Pending, _) => format!(" [~{}s]", info.step.estimated_duration().as_secs()),
                (StepState::Failed, _) => " FAILED".to_string(),
            };

            step_lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), style),
                Span::styled(
                    format!("Step {}: {}", info.step.index() + 1, info.step.label()),
                    style,
                ),
                Span::styled(time_str, Style::default().fg(Color::DarkGray)),
            ]));
        }

        frame.render_widget(Paragraph::new(step_lines), chunks[1]);

        // --- Output pane ---
        let output_block = Block::default()
            .title(" Output ")
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray));

        let output_inner = output_block.inner(chunks[2]);
        frame.render_widget(output_block, chunks[2]);

        let height = output_inner.height as usize;
        let total_lines = self.output_lines.len();
        let scroll = self.output_scroll as usize;
        let start = scroll.min(total_lines.saturating_sub(height));
        let end = (start + height).min(total_lines);

        let visible: Vec<Line> = self.output_lines[start..end]
            .iter()
            .map(|l| Line::from(Span::raw(l.as_str())))
            .collect();

        frame.render_widget(Paragraph::new(visible), output_inner);

        // Error display
        if let Some((error, workaround)) = &self.error {
            // Render error at bottom of output
            let error_area = Rect {
                y: output_inner.y + output_inner.height.saturating_sub(3),
                height: 3.min(output_inner.height),
                ..output_inner
            };
            let mut error_lines = vec![
                Line::from(Span::styled(
                    format!("Error: {error}"),
                    Style::default().fg(Color::Red),
                )),
            ];
            if let Some(hint) = workaround {
                error_lines.push(Line::from(Span::styled(
                    format!("Hint: {hint}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
            error_lines.push(Line::from(Span::styled(
                "[Esc] back to dashboard",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(Paragraph::new(error_lines), error_area);
        }
    }
}
