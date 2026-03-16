use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::ops::pipeline::{Phase, Pipeline, StepStatus};
use crate::tui::theme::{panel_block_accent, render_bar, Palette};

use super::Component;

pub struct BuildView {
    scroll: u16,
}

impl BuildView {
    pub fn new() -> Self {
        Self { scroll: 0 }
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn render_pipeline(&self, frame: &mut Frame, area: Rect, pipeline: &Pipeline) {
        let p = Palette::default();
        let block = panel_block_accent("Build Pipeline", &p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Layout: progress bar (2) | step list (rest) | log pane (6)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
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

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(area);

        frame.render_widget(
            Paragraph::new(status_text).style(Style::default().fg(bar_color)),
            rows[0],
        );
        let bar = render_bar(ratio, rows[1].width, bar_color, p.border_default);
        frame.render_widget(Paragraph::new(vec![bar]), rows[1]);
    }

    fn render_steps(
        &self,
        frame: &mut Frame,
        area: Rect,
        pipeline: &Pipeline,
        p: &Palette,
    ) {
        let mut lines: Vec<Line> = Vec::new();

        for phase in &pipeline.phases {
            // Phase header
            lines.push(self.phase_header_line(phase, p));

            // Steps
            for step in &phase.steps {
                lines.push(self.step_line(step, p));

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
            }

            // Blank line between phases
            lines.push(Line::from(""));
        }

        // Apply scroll
        let height = area.height as usize;
        let max_scroll = lines.len().saturating_sub(height);
        let scroll = (self.scroll as usize).min(max_scroll);
        let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();

        frame.render_widget(Paragraph::new(visible), area);
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

    fn step_line<'a>(&self, step: &crate::ops::pipeline::Step, p: &Palette) -> Line<'a> {
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

        Line::from(vec![
            Span::styled(format!("    {icon} "), Style::default().fg(icon_color)),
            Span::styled(step.name.to_string(), name_style),
            Span::styled(time_str, Style::default().fg(p.text_tertiary)),
        ])
    }

    fn render_log_pane(
        &self,
        frame: &mut Frame,
        area: Rect,
        pipeline: &Pipeline,
        p: &Palette,
    ) {
        // Find the currently running step's detail, or the last failure
        let active_detail = pipeline
            .phases
            .iter()
            .flat_map(|ph| &ph.steps)
            .find(|s| matches!(s.status, StepStatus::Running { .. }))
            .or_else(|| {
                pipeline
                    .phases
                    .iter()
                    .flat_map(|ph| &ph.steps)
                    .rev()
                    .find(|s| matches!(s.status, StepStatus::Failed { .. }))
            });

        let block = ratatui::widgets::Block::default()
            .title(" STEP LOG ")
            .borders(ratatui::widgets::Borders::TOP)
            .border_style(Style::default().fg(p.border_default))
            .title_style(Style::default().fg(p.text_tertiary));

        let log_inner = block.inner(area);
        frame.render_widget(block, area);

        let content = if let Some(step) = active_detail {
            match &step.status {
                StepStatus::Running { detail: Some(d), .. } => {
                    format!("{}: {d}", step.name)
                }
                StepStatus::Running { .. } => {
                    format!("{}: running...", step.name)
                }
                StepStatus::Failed { error, .. } => {
                    format!("{}: FAILED — {error}", step.name)
                }
                _ => String::new(),
            }
        } else if pipeline.is_complete() {
            "Build complete.".to_string()
        } else {
            "No active step.".to_string()
        };

        frame.render_widget(
            Paragraph::new(content).style(Style::default().fg(p.text_secondary)),
            log_inner,
        );
    }
}

impl Component for BuildView {
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

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}h {:02}m {:02}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}
