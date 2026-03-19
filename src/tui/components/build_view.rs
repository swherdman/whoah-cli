use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::ops::pipeline::{Phase, Pipeline, StepStatus};
use crate::tui::theme::{format_duration, panel_block_accent, render_bar, Palette};

use crate::action::Action;

use super::Component;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildFocus {
    StepList,
    LogPanel,
}

pub struct BuildView {
    scroll: u16,
    log_scroll: u16,
    /// Track the output length last time we rendered, so we can auto-scroll
    /// when new output arrives.
    prev_log_len: usize,
    /// Track which step id was active last render, to reset scroll on step change.
    prev_active_step: Option<&'static str>,
    /// Which panel currently has focus.
    focus: BuildFocus,
    /// Total log lines from last render (used for scroll clamping).
    total_log_lines: usize,
    /// Visible height of the log pane from last render (used for scroll clamping).
    log_visible_height: usize,
    /// Selected step index (flat across all phases) for reviewing completed output.
    selected_step: usize,
    /// Total step count from last render.
    total_steps: usize,
}

impl BuildView {
    pub fn new() -> Self {
        Self {
            scroll: 0,
            log_scroll: 0,
            prev_log_len: 0,
            prev_active_step: None,
            focus: BuildFocus::StepList,
            total_log_lines: 0,
            log_visible_height: 0,
            selected_step: 0,
            total_steps: 0,
        }
    }

    /// Toggle focus between the step list and the log panel.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            BuildFocus::StepList => BuildFocus::LogPanel,
            BuildFocus::LogPanel => {
                // Returning to step list: re-enable auto-scroll by snapping to bottom
                self.log_scroll =
                    self.total_log_lines.saturating_sub(self.log_visible_height) as u16;
                BuildFocus::StepList
            }
        };
    }

    /// Returns true when the log panel has focus.
    pub fn is_log_focused(&self) -> bool {
        self.focus == BuildFocus::LogPanel
    }

    /// Scroll the log panel down by one line, clamped to content length.
    pub fn scroll_log_down(&mut self) {
        let max = self.total_log_lines.saturating_sub(self.log_visible_height) as u16;
        self.log_scroll = self.log_scroll.saturating_add(1).min(max);
    }

    /// Scroll the log panel up by one line.
    pub fn scroll_log_up(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(1);
    }

    /// Move the step selection cursor down.
    pub fn select_next_step(&mut self) {
        if self.total_steps > 0 && self.selected_step < self.total_steps - 1 {
            self.selected_step += 1;
            // Reset log scroll when step selection changes
            self.log_scroll = 0;
            self.prev_log_len = 0;
        }
    }

    /// Move the step selection cursor up.
    pub fn select_prev_step(&mut self) {
        if self.selected_step > 0 {
            self.selected_step -= 1;
            self.log_scroll = 0;
            self.prev_log_len = 0;
        }
    }

    pub fn render_pipeline(&mut self, frame: &mut Frame, area: Rect, pipeline: &Pipeline) {
        let p = Palette::default();
        let block = panel_block_accent("Build Pipeline", &p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Layout: progress bar (2) | step list (rest) | log pane (6)
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(6),
        ])
        .split(inner);

        self.render_progress(frame, chunks[0], pipeline, &p);
        self.render_steps(frame, chunks[1], pipeline, &p);
        self.render_log_pane(frame, chunks[2], pipeline, &p);
    }

    fn render_progress(
        &self,
        frame: &mut Frame,
        area: Rect,
        pipeline: &Pipeline,
        p: &Palette,
    ) {
        let (done, total) = pipeline.progress();
        let ratio = if total > 0 {
            done as f64 / total as f64
        } else {
            0.0
        };

        let bar_color = if pipeline.has_failure() {
            p.red_error
        } else if pipeline.is_complete() {
            p.green_primary
        } else if done > 0 {
            p.yellow_warn
        } else {
            p.text_disabled
        };

        let elapsed = pipeline.total_elapsed();
        let status_text = if pipeline.is_complete() {
            format!(
                "Complete — {done}/{total} steps in {}",
                format_duration(elapsed)
            )
        } else if pipeline.has_failure() {
            format!("Failed at step {done}/{total} — {}", format_duration(elapsed))
        } else if done > 0 {
            format!(
                "Step {done}/{total} — {}",
                format_duration(elapsed)
            )
        } else {
            format!("0/{total} steps — ready")
        };

        let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
            .split(area);

        frame.render_widget(
            Paragraph::new(status_text).style(Style::default().fg(bar_color)),
            rows[0],
        );
        let bar = render_bar(ratio, rows[1].width, bar_color, p.border_default);
        frame.render_widget(Paragraph::new(vec![bar]), rows[1]);
    }

    fn render_steps(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        pipeline: &Pipeline,
        p: &Palette,
    ) {
        let mut lines: Vec<Line> = Vec::new();
        let mut flat_idx: usize = 0;
        // Track which line index the selected step starts at (for auto-scroll)
        let mut selected_line: Option<usize> = None;

        // Count total steps for cursor clamping
        self.total_steps = pipeline.phases.iter().map(|ph| ph.steps.len()).sum();

        for phase in &pipeline.phases {
            // Phase header
            lines.push(self.phase_header_line(phase, p));

            // Steps
            for step in &phase.steps {
                let is_selected = self.focus == BuildFocus::StepList
                    && flat_idx == self.selected_step;

                if is_selected {
                    selected_line = Some(lines.len());
                }

                lines.push(self.step_line(step, p, is_selected));

                // Show detail line for running/failed steps
                if let Some(detail) = step.detail() {
                    let detail_color = match &step.status {
                        StepStatus::Failed { .. } => p.red_error,
                        _ => p.text_tertiary,
                    };
                    lines.push(Line::from(Span::styled(
                        format!("        {detail}"),
                        Style::default().fg(detail_color),
                    )));
                }

                flat_idx += 1;
            }

            // Blank line between phases
            lines.push(Line::from(""));
        }

        // Auto-scroll to keep selected step visible
        if let Some(sel_line) = selected_line {
            let visible = area.height as usize;
            let scroll = self.scroll as usize;
            if sel_line < scroll {
                self.scroll = sel_line as u16;
            } else if sel_line >= scroll + visible {
                self.scroll = (sel_line - visible + 2) as u16;
            }
        }

        let total_lines = lines.len();
        frame.render_widget(
            Paragraph::new(lines).scroll((self.scroll, 0)),
            area,
        );

        // Scrollbar: accent color when step list is focused
        let scrollbar_color = if self.focus == BuildFocus::StepList {
            p.green_border
        } else {
            p.border_default
        };
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(scrollbar_color));
        let mut scrollbar_state = ScrollbarState::new(total_lines)
            .position(self.scroll as usize);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }

    fn phase_header_line<'a>(&self, phase: &Phase, p: &Palette) -> Line<'a> {
        let phase_color = if phase.has_failure() {
            p.red_error
        } else if phase.is_complete() {
            p.green_primary
        } else if !phase.is_pending() {
            p.yellow_warn
        } else {
            p.text_secondary
        };

        let elapsed = phase.elapsed();
        let time_str = if elapsed.as_secs() > 0 {
            format!("  {}", format_duration(elapsed))
        } else {
            String::new()
        };

        Line::from(vec![
            Span::styled(
                format!("  {}", phase.name),
                Style::default()
                    .fg(phase_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(time_str, Style::default().fg(p.text_tertiary)),
        ])
    }

    fn step_line<'a>(
        &self,
        step: &crate::ops::pipeline::Step,
        p: &Palette,
        is_selected: bool,
    ) -> Line<'a> {
        let (icon, name_style) = match &step.status {
            StepStatus::Pending => ("○", Style::default().fg(p.text_disabled)),
            StepStatus::Running { .. } => (
                "●",
                Style::default()
                    .fg(p.yellow_warn)
                    .add_modifier(Modifier::BOLD),
            ),
            StepStatus::Completed { .. } => ("✓", Style::default().fg(p.green_primary)),
            StepStatus::Failed { .. } => (
                "✗",
                Style::default()
                    .fg(p.red_error)
                    .add_modifier(Modifier::BOLD),
            ),
            StepStatus::Skipped => ("–", Style::default().fg(p.text_disabled)),
        };

        let icon_color = match &step.status {
            StepStatus::Pending => p.text_disabled,
            StepStatus::Running { .. } => p.yellow_warn,
            StepStatus::Completed { .. } => p.green_primary,
            StepStatus::Failed { .. } => p.red_error,
            StepStatus::Skipped => p.text_disabled,
        };

        let time_str = match step.elapsed() {
            Some(d) if d.as_secs() > 0 => format!("  {}", format_duration(d)),
            _ => String::new(),
        };

        // Selection indicator: ▸ prefix when selected
        let prefix = if is_selected { "  ▸ " } else { "    " };

        Line::from(vec![
            Span::styled(
                format!("{prefix}{icon} "),
                Style::default().fg(icon_color),
            ),
            Span::styled(step.name.to_string(), name_style),
            Span::styled(time_str, Style::default().fg(p.text_tertiary)),
        ])
    }

    fn render_log_pane(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        pipeline: &Pipeline,
        p: &Palette,
    ) {
        // Determine which step's output to display:
        // 1. If step list has focus, show the selected step's output
        // 2. Otherwise, show the running step (or last failed)
        let all_steps: Vec<&crate::ops::pipeline::Step> = pipeline
            .phases
            .iter()
            .flat_map(|ph| &ph.steps)
            .collect();

        let display_step = if self.focus == BuildFocus::StepList {
            // Show selected step
            all_steps.get(self.selected_step).copied()
        } else {
            // Show running or last failed
            all_steps
                .iter()
                .find(|s| matches!(s.status, StepStatus::Running { .. }))
                .or_else(|| {
                    all_steps
                        .iter()
                        .rev()
                        .find(|s| matches!(s.status, StepStatus::Failed { .. }))
                })
                .copied()
        };

        // Build the block title
        let title = if let Some(step) = display_step {
            format!(" {} ", step.name)
        } else {
            " STEP LOG ".to_string()
        };

        let border_color = if self.focus == BuildFocus::LogPanel {
            p.green_border
        } else {
            p.border_default
        };
        let block = ratatui::widgets::Block::default()
            .title(title)
            .borders(ratatui::widgets::Borders::TOP)
            .border_style(Style::default().fg(border_color))
            .title_style(Style::default().fg(p.text_tertiary));

        let log_inner = block.inner(area);
        frame.render_widget(block, area);

        // If no step to display
        let Some(step) = display_step else {
            let msg = if pipeline.is_complete() {
                "Build complete."
            } else {
                "No active step."
            };
            frame.render_widget(
                Paragraph::new(msg)
                    .style(Style::default().fg(p.text_secondary))
                    .alignment(Alignment::Center),
                log_inner,
            );
            return;
        };

        let output = &step.output;
        let visible_height = log_inner.height as usize;

        // Track dimensions for scroll clamping in scroll_log_down/up
        self.total_log_lines = output.len();
        self.log_visible_height = visible_height;

        // Auto-scroll: snap to bottom when step changes or output grows
        // — but only when the user isn't manually scrolling (log panel focus)
        let step_changed = self.prev_active_step != Some(step.id);
        let output_grew = output.len() > self.prev_log_len;
        if self.focus != BuildFocus::LogPanel && (step_changed || output_grew) {
            self.log_scroll = output.len().saturating_sub(visible_height) as u16;
        }
        self.prev_active_step = Some(step.id);
        self.prev_log_len = output.len();

        // Show "(no output)" for steps with empty buffers
        if output.is_empty() {
            frame.render_widget(
                Paragraph::new("  (no output)")
                    .style(Style::default().fg(p.text_disabled)),
                log_inner,
            );
            return;
        }

        // Build display lines with a dim line-number prefix
        let lines: Vec<Line> = output
            .iter()
            .enumerate()
            .map(|(i, line)| {
                Line::from(vec![
                    Span::styled(
                        format!("{:>4} ", i + 1),
                        Style::default().fg(p.text_tertiary),
                    ),
                    Span::styled(line.clone(), Style::default().fg(p.text_default)),
                ])
            })
            .collect();

        let para = Paragraph::new(lines).scroll((self.log_scroll, 0));
        frame.render_widget(para, log_inner);
    }
}

impl Component for BuildView {
    fn update(&mut self, action: &Action) {
        match action {
            Action::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            Action::ScrollDown => self.scroll = self.scroll.saturating_add(1),
            _ => {}
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        // Fallback: render without pipeline (shouldn't normally be called)
        let p = Palette::default();
        let block = panel_block_accent("Build Pipeline", &p);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("  No pipeline loaded.")
                .style(Style::default().fg(p.text_tertiary)),
            inner,
        );
    }
}
