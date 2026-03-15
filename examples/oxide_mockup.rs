//! Oxide-branded TUI mockup for whoah-cli.
//!
//! Renders the dashboard and recovery views with hardcoded fake data
//! using the Oxide Design System palette. Fully self-contained — no SSH,
//! config, or ops dependencies.
//!
//! Run with: cargo run --example oxide_mockup
//!
//! Keys:
//!   q     — quit
//!   r     — toggle recovery view
//!   Tab   — cycle focused panel
//!   Esc   — return to dashboard

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Terminal;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Palette — Design System (oxidecomputer/design-system OKLCH dark theme)
// Duplicated from src/tui/theme.rs — [[bin]] crate, examples can't import.
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Clone, Copy)]
struct Palette {
    bg_base: Color,
    bg_panel: Color,
    bg_hover: Color,
    border_default: Color,
    border_focus: Color,
    text_disabled: Color,
    text_tertiary: Color,
    text_secondary: Color,
    text_default: Color,
    text_raised: Color,
    text_bright: Color,
    green_primary: Color,
    green_border: Color,
    yellow_warn: Color,
    red_error: Color,
    ascii_structural: Color,
}

const P: Palette = Palette {
    bg_base: Color::Rgb(11, 13, 18),          // #0B0D12 neutral-0
    bg_panel: Color::Rgb(18, 21, 25),          // #121519 neutral-50
    bg_hover: Color::Rgb(30, 33, 36),          // #1E2124 neutral-200
    border_default: Color::Rgb(48, 49, 52),    // #303134 neutral-300
    border_focus: Color::Rgb(67, 68, 71),      // #434447 neutral-400
    text_disabled: Color::Rgb(93, 94, 96),     // #5D5E60 neutral-500
    text_tertiary: Color::Rgb(128, 129, 131),  // #808183 neutral-600
    text_secondary: Color::Rgb(162, 163, 164), // #A2A3A4 neutral-700
    text_default: Color::Rgb(185, 186, 187),   // #B9BABB neutral-800
    text_raised: Color::Rgb(221, 221, 221),    // #DDDDDD neutral-900
    text_bright: Color::Rgb(238, 238, 238),    // #EEEEEE neutral-1100
    green_primary: Color::Rgb(0, 216, 145),    // #00D891 green-800
    green_border: Color::Rgb(0, 147, 102),     // #009366 green-600
    yellow_warn: Color::Rgb(254, 187, 85),     // #FEBB55 yellow-800
    red_error: Color::Rgb(254, 103, 132),      // #FE6784 red-800
    ascii_structural: Color::Rgb(0, 147, 102), // #009366 green-600
};

// Bar characters
const FILLED: &str = "\u{258A}"; // ▊
const EMPTY: &str = "\u{2395}";  // ⎕

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// App state
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Dashboard,
    Recovery,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusedPanel {
    Status,
    Disk,
}

struct MockupApp {
    mode: Mode,
    focused: FocusedPanel,
}

impl MockupApp {
    fn new() -> Self {
        Self {
            mode: Mode::Dashboard,
            focused: FocusedPanel::Status,
        }
    }

    fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Dashboard => Mode::Recovery,
            Mode::Recovery => Mode::Dashboard,
        };
    }

    fn cycle_focus(&mut self) {
        self.focused = match self.focused {
            FocusedPanel::Status => FocusedPanel::Disk,
            FocusedPanel::Disk => FocusedPanel::Status,
        };
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Theme helpers
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn render_bar(ratio: f64, width: u16, fg: Color) -> Line<'static> {
    let ratio = ratio.clamp(0.0, 1.0);
    let total = width as usize;
    let filled = (ratio * total as f64).round() as usize;
    let empty = total.saturating_sub(filled);

    Line::from(vec![
        Span::styled(FILLED.repeat(filled), Style::default().fg(fg)),
        Span::styled(EMPTY.repeat(empty), Style::default().fg(P.ascii_structural)),
    ])
}

fn panel_block(title: &str, focused: bool) -> Block<'static> {
    let border_color = if focused { P.border_focus } else { P.border_default };
    let title_color = if focused { P.text_raised } else { P.text_tertiary };

    Block::default()
        .title(format!(" {} ", title.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title_style(Style::default().fg(title_color))
        .style(Style::default().bg(P.bg_panel))
        .padding(Padding::new(2, 2, 1, 1))
}

fn panel_block_accent(title: &str) -> Block<'static> {
    Block::default()
        .title(format!(" {} ", title.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(P.green_border))
        .title_style(Style::default().fg(P.green_primary))
        .style(Style::default().bg(P.bg_panel))
        .padding(Padding::new(2, 2, 1, 1))
}

fn section_header(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_uppercase(),
        Style::default()
            .fg(P.text_bright)
            .add_modifier(Modifier::BOLD),
    ))
}

fn threshold_color(pct: u8, warning: u8, critical: u8) -> Color {
    if pct >= critical {
        P.red_error
    } else if pct >= warning {
        P.yellow_warn
    } else {
        P.green_primary
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Rendering
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn render(frame: &mut Frame, app: &MockupApp) {
    frame.render_widget(
        Block::default().style(Style::default().bg(P.bg_base)),
        frame.area(),
    );

    match app.mode {
        Mode::Dashboard => render_dashboard(frame, app),
        Mode::Recovery => render_recovery(frame),
    }
}

// ── Dashboard ───────────────────────────────────────────────────

fn render_dashboard(frame: &mut Frame, app: &MockupApp) {
    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(2),
        ])
        .split(area);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(vertical[1]);

    render_alert_bar(frame, vertical[0]);
    render_status_panel(frame, horizontal[0], app.focused == FocusedPanel::Status);
    render_disk_panel(frame, horizontal[1], app.focused == FocusedPanel::Disk);
    render_log_panel(frame, vertical[2]);
    render_status_bar(frame, vertical[3]);
}

fn render_alert_bar(frame: &mut Frame, area: Rect) {
    let style = Style::default()
        .fg(P.text_bright)
        .bg(P.red_error)
        .add_modifier(Modifier::BOLD);

    let alert = Paragraph::new(Line::from(vec![
        Span::styled("  !! ", style),
        Span::styled("Reboot detected. Press [r] to start recovery.", style),
    ]))
    .style(Style::default().bg(P.red_error));

    frame.render_widget(alert, area);
}

fn render_status_panel(frame: &mut Frame, area: Rect, focused: bool) {
    let block = panel_block("Services", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(vec![
            Span::styled("SLED-AGENT  ", Style::default().fg(P.text_raised)),
            Span::styled("online", Style::default().fg(P.green_primary)),
        ]),
        Line::from(vec![
            Span::styled("BASELINE    ", Style::default().fg(P.text_raised)),
            Span::styled("online", Style::default().fg(P.green_primary)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("ZONES       ", Style::default().fg(P.text_raised)),
            Span::styled("22/22", Style::default().fg(P.green_primary)),
            Span::styled(" running", Style::default().fg(P.text_default)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("NEXUS       ", Style::default().fg(P.text_raised)),
            Span::styled("reachable", Style::default().fg(P.green_primary)),
        ]),
        Line::from(vec![
            Span::styled("DNS         ", Style::default().fg(P.text_raised)),
            Span::styled("resolving", Style::default().fg(P.green_primary)),
        ]),
        Line::from(""),
        section_header("Zone Placement"),
        Line::from(vec![
            Span::styled("  oxp_aaa111  ", Style::default().fg(P.text_default)),
            Span::styled("cockroachdb, internal_dns", Style::default().fg(P.text_secondary)),
        ]),
        Line::from(vec![
            Span::styled("  oxp_bbb222  ", Style::default().fg(P.text_default)),
            Span::styled("nexus, crucible, cockroachdb", Style::default().fg(P.text_secondary)),
        ]),
        Line::from(vec![
            Span::styled("  oxp_ccc333  ", Style::default().fg(P.text_default)),
            Span::styled("external_dns, clickhouse, crucible", Style::default().fg(P.text_secondary)),
        ]),
    ];

    let paragraph = Paragraph::new(lines).style(Style::default().bg(P.bg_panel));
    frame.render_widget(paragraph, inner);
}

fn render_disk_panel(frame: &mut Frame, area: Rect, focused: bool) {
    let block = panel_block("Disk Usage", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let bar_width = inner.width;
    let mut lines: Vec<Line> = Vec::new();

    let rpool_color = threshold_color(30, 85, 92);
    lines.push(Line::from(vec![
        Span::styled("RPOOL  ", Style::default().fg(P.text_raised)),
        Span::styled("30%", Style::default().fg(rpool_color)),
        Span::styled("  172 GiB free", Style::default().fg(P.text_secondary)),
    ]));
    lines.push(render_bar(0.30, bar_width, rpool_color));
    lines.push(Line::from(""));

    let pools = [("OXP_AAA111", 38u8), ("OXP_BBB222", 31), ("OXP_CCC333", 29)];
    for (name, pct) in &pools {
        let color = threshold_color(*pct, 85, 95);
        lines.push(Line::from(vec![
            Span::styled(format!("{name}  "), Style::default().fg(P.text_raised)),
            Span::styled(format!("{pct}%"), Style::default().fg(color)),
        ]));
        lines.push(render_bar(*pct as f64 / 100.0, bar_width, color));
    }

    lines.push(Line::from(""));
    lines.push(section_header("Vdev Files"));

    let vdevs = [("u2_0.vdev", "11.6"), ("u2_1.vdev", "10.0"), ("u2_2.vdev", "10.3")];
    for (name, gib) in &vdevs {
        lines.push(Line::from(vec![
            Span::styled(format!("  {name}  "), Style::default().fg(P.text_default)),
            Span::styled(format!("{gib} GiB"), Style::default().fg(P.text_secondary)),
        ]));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(P.bg_panel));
    frame.render_widget(paragraph, inner);
}

fn render_log_panel(frame: &mut Frame, area: Rect) {
    let block = panel_block("Logs", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let logs = vec![
        "[14:32:01] Status: 22 zones, rpool 30%",
        "[14:32:11] Status: 22 zones, rpool 30%",
        "[14:32:21] Status: 22 zones, rpool 30%",
        "[14:32:31] Reboot detected on 192.168.2.209",
        "[14:32:31] Services: sled-agent offline, baseline offline",
        "[14:32:32] Alert: Reboot detected. Press [r] to start recovery.",
    ];

    let lines: Vec<Line> = logs
        .iter()
        .map(|msg| Line::from(Span::styled(*msg, Style::default().fg(P.text_tertiary))))
        .collect();

    let paragraph = Paragraph::new(lines).style(Style::default().bg(P.bg_panel));
    frame.render_widget(paragraph, inner);
}

fn render_status_bar(frame: &mut Frame, area: Rect) {
    let line1 = Line::from(vec![
        Span::styled(
            "  WHOAH — home-lab ",
            Style::default().fg(P.text_bright).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("192.168.2.209: online", Style::default().fg(P.green_primary)),
    ]);

    let line2 = Line::from(vec![
        Span::styled("  [r]", Style::default().fg(P.green_primary)),
        Span::styled("ecover ", Style::default().fg(P.text_secondary)),
        Span::styled("[s]", Style::default().fg(P.green_primary)),
        Span::styled("tatus ", Style::default().fg(P.text_secondary)),
        Span::styled("[q]", Style::default().fg(P.green_primary)),
        Span::styled("uit  ", Style::default().fg(P.text_secondary)),
        Span::styled("Tab", Style::default().fg(P.green_primary)),
        Span::styled(":panels ", Style::default().fg(P.text_secondary)),
        Span::styled("j/k", Style::default().fg(P.green_primary)),
        Span::styled(":scroll", Style::default().fg(P.text_secondary)),
    ]);

    let paragraph = Paragraph::new(vec![line1, line2]).style(Style::default().bg(P.bg_hover));
    frame.render_widget(paragraph, area);
}

// ── Recovery View ───────────────────────────────────────────────

fn render_recovery(frame: &mut Frame) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(2)])
        .split(area);

    render_recovery_view(frame, chunks[0]);
    render_status_bar(frame, chunks[1]);
}

fn render_recovery_view(frame: &mut Frame, area: Rect) {
    let block = panel_block_accent("Recovery");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(9),
            Constraint::Min(3),
        ])
        .split(inner);

    // ── Progress ────────────────────────────────────────────────

    let progress_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(sections[0]);

    let eta = Paragraph::new(Line::from(vec![
        Span::styled(
            "STEP 5/7",
            Style::default().fg(P.green_primary).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ~360s remaining", Style::default().fg(P.text_secondary)),
    ]))
    .style(Style::default().bg(P.bg_panel));
    frame.render_widget(eta, progress_rows[0]);

    let bar = render_bar(4.0 / 7.0, progress_rows[1].width, P.green_primary);
    let bar_paragraph = Paragraph::new(vec![bar]).style(Style::default().bg(P.bg_panel));
    frame.render_widget(bar_paragraph, progress_rows[1]);

    // ── Step List ───────────────────────────────────────────────

    struct Step {
        label: &'static str,
        icon: &'static str,
        state: StepState,
        time: &'static str,
    }

    #[derive(Clone, Copy)]
    enum StepState { Done, Running, Pending }

    let steps = [
        Step { label: "WAIT FOR BASELINE SERVICE", icon: "OK", state: StepState::Done, time: "(45.2s)" },
        Step { label: "UNINSTALL BROKEN STATE", icon: "OK", state: StepState::Done, time: "(28.1s)" },
        Step { label: "DESTROY VIRTUAL HARDWARE", icon: "OK", state: StepState::Done, time: "(8.4s)" },
        Step { label: "RECREATE VIRTUAL HARDWARE", icon: "OK", state: StepState::Done, time: "(31.0s)" },
        Step { label: "INSTALL PACKAGES", icon: ">>", state: StepState::Running, time: "(42s...)" },
        Step { label: "MONITOR ZONE STARTUP", icon: "  ", state: StepState::Pending, time: "[~360s]" },
        Step { label: "VERIFY SERVICES", icon: "  ", state: StepState::Pending, time: "[~10s]" },
    ];

    let step_lines: Vec<Line> = steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let style = match step.state {
                StepState::Done => Style::default().fg(P.green_primary),
                StepState::Running => Style::default().fg(P.yellow_warn).add_modifier(Modifier::BOLD),
                StepState::Pending => Style::default().fg(P.text_disabled),
            };
            Line::from(vec![
                Span::styled(format!(" {} ", step.icon), style),
                Span::styled(format!(" STEP {}: {}", i + 1, step.label), style),
                Span::styled(format!("  {}", step.time), Style::default().fg(P.text_tertiary)),
            ])
        })
        .collect();

    let step_paragraph = Paragraph::new(step_lines).style(Style::default().bg(P.bg_panel));
    frame.render_widget(step_paragraph, sections[1]);

    // ── Output Pane ─────────────────────────────────────────────

    let output_block = Block::default()
        .title(Line::from(Span::styled(" OUTPUT ", Style::default().fg(P.text_tertiary))))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(P.border_default))
        .style(Style::default().bg(P.bg_panel));

    let output_inner = output_block.inner(sections[2]);
    frame.render_widget(output_block, sections[2]);

    let output_lines = vec![
        "Running omicron-package install...",
        "Installing crucible-pantry ...",
        "Installing internal-dns ...",
        "Installing nexus ...",
        "Installing external-dns ...",
        "Installing clickhouse ...",
        "Installing cockroachdb ...",
        "Zones: 0/22 running",
    ];

    let output: Vec<Line> = output_lines
        .iter()
        .map(|l| Line::from(Span::styled(*l, Style::default().fg(P.text_secondary))))
        .collect();

    let output_paragraph = Paragraph::new(output).style(Style::default().bg(P.bg_panel));
    frame.render_widget(output_paragraph, output_inner);
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = MockupApp::new();

    loop {
        terminal.draw(|frame| render(frame, &app))?;

        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => break,
                        KeyCode::Char('r') => app.toggle_mode(),
                        KeyCode::Tab => app.cycle_focus(),
                        KeyCode::Esc => app.mode = Mode::Dashboard,
                        _ => {}
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
