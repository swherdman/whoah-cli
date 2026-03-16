//! Oxide-branded TUI mockup for whoah-cli.
//!
//! Mirrors the current working UI layout and fields, styled with the
//! Oxide Design System palette. Fully self-contained — no SSH, config,
//! or ops dependencies.
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

struct P;
#[allow(dead_code)]
impl P {
    const BG_BASE: Color = Color::Rgb(11, 13, 18);           // #0B0D12 neutral-0
    const BG_PANEL: Color = Color::Rgb(18, 21, 25);           // #121519 neutral-50
    const BG_HOVER: Color = Color::Rgb(30, 33, 36);           // #1E2124 neutral-200
    const BORDER_DEFAULT: Color = Color::Rgb(48, 49, 52);     // #303134 neutral-300
    const BORDER_FOCUS: Color = Color::Rgb(67, 68, 71);       // #434447 neutral-400
    const TEXT_DISABLED: Color = Color::Rgb(93, 94, 96);       // #5D5E60 neutral-500
    const TEXT_TERTIARY: Color = Color::Rgb(128, 129, 131);    // #808183 neutral-600
    const TEXT_SECONDARY: Color = Color::Rgb(162, 163, 164);   // #A2A3A4 neutral-700
    const TEXT_DEFAULT: Color = Color::Rgb(185, 186, 187);     // #B9BABB neutral-800
    const TEXT_RAISED: Color = Color::Rgb(221, 221, 221);      // #DDDDDD neutral-900
    const TEXT_BRIGHT: Color = Color::Rgb(238, 238, 238);      // #EEEEEE neutral-1100
    const GREEN_PRIMARY: Color = Color::Rgb(0, 216, 145);      // #00D891 green-800
    const GREEN_BORDER: Color = Color::Rgb(0, 147, 102);       // #009366 green-600
    const YELLOW_WARN: Color = Color::Rgb(254, 187, 85);       // #FEBB55 yellow-800
    const RED_ERROR: Color = Color::Rgb(254, 103, 132);        // #FE6784 red-800
    const BLUE_INFO: Color = Color::Rgb(129, 153, 254);        // #8199FE blue-800
    const ASCII_STRUCTURAL: Color = Color::Rgb(0, 147, 102);   // #009366 green-600
}

const FILLED: &str = "\u{258A}"; // ▊  — progress bars
const EMPTY: &str = "\u{2395}";  // ⎕  — progress bars (kept for bar rendering)
const GRID_ON: &str = "\u{25A0}";  // ■  — zone distribution: present (single)
const GRID_OFF: &str = "\u{25A1}"; // □  — zone distribution: not present

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// App state
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Dashboard, Recovery }

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusedPanel { Status, Disk }

struct MockupApp {
    mode: Mode,
    focused: FocusedPanel,
}

impl MockupApp {
    fn new() -> Self {
        Self { mode: Mode::Dashboard, focused: FocusedPanel::Status }
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
        Span::styled(EMPTY.repeat(empty), Style::default().fg(P::ASCII_STRUCTURAL)),
    ])
}

fn panel_block(title: &str, focused: bool) -> Block<'static> {
    let border_color = if focused { P::BORDER_FOCUS } else { P::BORDER_DEFAULT };
    let title_color = if focused { P::TEXT_RAISED } else { P::TEXT_TERTIARY };
    Block::default()
        .title(format!(" {} ", title.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title_style(Style::default().fg(title_color))
        .style(Style::default().bg(P::BG_PANEL))
        .padding(Padding::new(2, 2, 1, 1))
}

fn panel_block_accent(title: &str) -> Block<'static> {
    Block::default()
        .title(format!(" {} ", title.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(P::GREEN_BORDER))
        .title_style(Style::default().fg(P::GREEN_PRIMARY))
        .style(Style::default().bg(P::BG_PANEL))
        .padding(Padding::new(2, 2, 1, 1))
}

fn section_header(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_uppercase(),
        Style::default().fg(P::TEXT_BRIGHT).add_modifier(Modifier::BOLD),
    ))
}

fn threshold_color(pct: u8, warning: u8, critical: u8) -> Color {
    if pct >= critical { P::RED_ERROR }
    else if pct >= warning { P::YELLOW_WARN }
    else { P::GREEN_PRIMARY }
}

fn service_span(state: &str) -> Span<'static> {
    let (text, color) = match state {
        "online" => ("online", P::GREEN_PRIMARY),
        "maintenance" => ("maintenance", P::RED_ERROR),
        "offline" => ("offline", P::YELLOW_WARN),
        "degraded" => ("degraded", P::YELLOW_WARN),
        _ => (state, P::TEXT_DISABLED),
    };
    Span::styled(text.to_string(), Style::default().fg(color))
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Rendering
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn render(frame: &mut Frame, app: &MockupApp) {
    frame.render_widget(
        Block::default().style(Style::default().bg(P::BG_BASE)),
        frame.area(),
    );
    match app.mode {
        Mode::Dashboard => render_dashboard(frame, app),
        Mode::Recovery => render_recovery(frame),
    }
}

// ── Dashboard ───────────────────────────────────────────────────
// Layout matches src/tui/layout.rs:
//   title_bar(1) | main(min 8) [left 45% | right 55%] | logs(8) | keybindings(1)

fn render_dashboard(frame: &mut Frame, app: &MockupApp) {
    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Min(8),   // main content
            Constraint::Length(8), // log panel
            Constraint::Length(1), // keybindings bar
        ])
        .split(area);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(vertical[1]);

    render_title_bar(frame, vertical[0]);
    render_status_panel(frame, horizontal[0], app.focused == FocusedPanel::Status);
    render_disk_panel(frame, horizontal[1], app.focused == FocusedPanel::Disk);
    render_log_panel(frame, vertical[2]);
    render_keybindings(frame, vertical[3]);
}

/// Title bar — 1 line: " whoah — deployment  host online"
/// When alert is active, this is replaced by the alert bar.
fn render_title_bar(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            " whoah — home-lab ",
            Style::default().fg(P::TEXT_BRIGHT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("192.168.2.209 ", Style::default().fg(P::TEXT_DEFAULT)),
        Span::styled("online ", Style::default().fg(P::GREEN_PRIMARY)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(P::BG_HOVER)),
        area,
    );
}

/// Keybindings bar — 1 line at bottom
fn render_keybindings(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" [r]", Style::default().fg(P::GREEN_PRIMARY)),
        Span::styled("ecover ", Style::default().fg(P::TEXT_SECONDARY)),
        Span::styled("[s]", Style::default().fg(P::GREEN_PRIMARY)),
        Span::styled("tatus ", Style::default().fg(P::TEXT_SECONDARY)),
        Span::styled("[q]", Style::default().fg(P::GREEN_PRIMARY)),
        Span::styled("uit  ", Style::default().fg(P::TEXT_SECONDARY)),
        Span::styled("Tab", Style::default().fg(P::GREEN_PRIMARY)),
        Span::styled(":panels ", Style::default().fg(P::TEXT_SECONDARY)),
        Span::styled("j/k", Style::default().fg(P::GREEN_PRIMARY)),
        Span::styled(":scroll", Style::default().fg(P::TEXT_SECONDARY)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(P::BG_HOVER)),
        area,
    );
}

/// Status panel — matches src/tui/components/status_panel.rs
/// Services, per-service zone counts, instances, network, zone placement
fn render_status_panel(frame: &mut Frame, area: Rect, focused: bool) {
    let block = panel_block("Services", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Service states
    lines.push(Line::from(vec![
        Span::styled("sled-agent: ", Style::default().fg(P::TEXT_RAISED)),
        service_span("online"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("baseline:   ", Style::default().fg(P::TEXT_RAISED)),
        service_span("online"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("nexus:      ", Style::default().fg(P::TEXT_RAISED)),
        Span::styled("reachable", Style::default().fg(P::GREEN_PRIMARY)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("dns:        ", Style::default().fg(P::TEXT_RAISED)),
        Span::styled("resolving", Style::default().fg(P::GREEN_PRIMARY)),
    ]));

    // Zone distribution table — services × pools
    // ■ = 1 zone on this pool, number = multiple, □ = not present
    lines.push(Line::from(""));
    lines.push(section_header("Zone Distribution"));

    let pools = ["0", "1", "2"];

    // Column header row
    let mut header_spans = vec![
        Span::styled(format!("  {:<18}", ""), Style::default().fg(P::TEXT_DISABLED)),
    ];
    for pool in &pools {
        header_spans.push(Span::styled(
            format!("  {pool}"),
            Style::default().fg(P::TEXT_TERTIARY),
        ));
    }
    lines.push(Line::from(header_spans));

    // Service placement grid: (service_name, [count_on_pool0, pool1, pool2])
    // Uses real service names from KNOWN_SERVICES in parse/zones.rs
    let distribution: Vec<(&str, [u32; 3])> = vec![
        ("clickhouse",       [0, 0, 1]),
        ("cockroachdb",      [1, 1, 1]),
        ("crucible",         [1, 1, 1]),
        ("crucible_pantry",  [1, 1, 1]),
        ("external_dns",     [0, 0, 1]),
        ("internal_dns",     [1, 0, 0]),
        ("nexus",            [0, 1, 1]),
        ("ntp",              [1, 0, 0]),
        ("oximeter",         [0, 1, 0]),
    ];

    for (svc, counts) in &distribution {
        let mut spans = vec![
            Span::styled(format!("  {svc:<18}"), Style::default().fg(P::TEXT_DEFAULT)),
        ];
        for &count in counts {
            if count == 0 {
                spans.push(Span::styled(
                    format!("  {GRID_OFF}"),
                    Style::default().fg(P::TEXT_DISABLED),
                ));
            } else if count == 1 {
                spans.push(Span::styled(
                    format!("  {GRID_ON}"),
                    Style::default().fg(P::GREEN_PRIMARY),
                ));
            } else {
                spans.push(Span::styled(
                    format!("  {count}"),
                    Style::default().fg(P::GREEN_PRIMARY),
                ));
            }
        }
        lines.push(Line::from(spans));
    }

    // Instances — separated visually, count per pool
    lines.push(Line::from(""));
    lines.push(section_header("Instances"));
    let instance_dist: Vec<(&str, [u32; 3])> = vec![
        ("propolis-server",  [1, 1, 0]),
    ];
    for (svc, counts) in &instance_dist {
        let mut spans = vec![
            Span::styled(format!("  {svc:<18}"), Style::default().fg(P::TEXT_DEFAULT)),
        ];
        for &count in counts {
            if count == 0 {
                spans.push(Span::styled(
                    format!("  {GRID_OFF}"),
                    Style::default().fg(P::TEXT_DISABLED),
                ));
            } else if count == 1 {
                spans.push(Span::styled(
                    format!("  {GRID_ON}"),
                    Style::default().fg(P::BLUE_INFO),
                ));
            } else {
                spans.push(Span::styled(
                    format!("  {count}"),
                    Style::default().fg(P::BLUE_INFO),
                ));
            }
        }
        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(P::BG_PANEL));
    frame.render_widget(paragraph, inner);
}

/// Disk panel — ▊/⎕ bars for pool usage
fn render_disk_panel(frame: &mut Frame, area: Rect, focused: bool) {
    let block = panel_block("Disk Usage", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let bar_width = inner.width;
    let mut lines: Vec<Line> = Vec::new();

    // rpool
    let rpool_color = threshold_color(30, 85, 92);
    lines.push(Line::from(vec![
        Span::styled("rpool: ", Style::default().fg(rpool_color)),
        Span::styled("30% ", Style::default().fg(rpool_color)),
        Span::styled("(172 GiB free)", Style::default().fg(rpool_color)),
    ]));
    lines.push(render_bar(0.30, bar_width, rpool_color));

    // oxp pools
    let oxp_pools = [
        ("oxp_aaa111", 38u8),
        ("oxp_bbb222", 31),
        ("oxp_ccc333", 29),
    ];
    for (name, pct) in &oxp_pools {
        let color = threshold_color(*pct, 85, 95);
        lines.push(Line::from(vec![
            Span::styled(format!("{name}: "), Style::default().fg(color)),
            Span::styled(format!("{pct}%"), Style::default().fg(color)),
        ]));
        lines.push(render_bar(*pct as f64 / 100.0, bar_width, color));
    }

    // Spacer
    lines.push(Line::from(""));

    // Vdev files
    let vdevs = [("0.vdev", 11.6_f64), ("1.vdev", 10.0), ("2.vdev", 10.3)];
    for (name, gib) in &vdevs {
        let color = if *gib > 35.0 { P::RED_ERROR } else { P::TEXT_DEFAULT };
        lines.push(Line::from(Span::styled(
            format!("{name}: {gib:.1} GiB"),
            Style::default().fg(color),
        )));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(P::BG_PANEL));
    frame.render_widget(paragraph, inner);
}

/// Log panel — matches src/tui/components/log_panel.rs
fn render_log_panel(frame: &mut Frame, area: Rect) {
    let block = panel_block("Logs", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let logs = vec![
        "[14:32:01] Status: 22 zones, rpool 30%",
        "[14:32:11] Status: 22 zones, rpool 30%",
        "[14:32:21] Status: 22 zones, rpool 30%",
        "[14:32:31] All services healthy",
        "[14:32:41] Status: 22 zones, rpool 30%",
        "[14:32:51] Status: 22 zones, rpool 30%",
    ];

    let lines: Vec<Line> = logs
        .iter()
        .map(|msg| Line::from(Span::styled(*msg, Style::default().fg(P::TEXT_TERTIARY))))
        .collect();

    let paragraph = Paragraph::new(lines).style(Style::default().bg(P::BG_PANEL));
    frame.render_widget(paragraph, inner);
}

// ── Recovery View ───────────────────────────────────────────────
// Layout matches app.rs render_recovery:
//   title_bar(1) | recovery(min 5) | keybindings(1)

fn render_recovery(frame: &mut Frame) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Min(5),   // recovery view
            Constraint::Length(1), // keybindings
        ])
        .split(area);

    render_title_bar(frame, chunks[0]);
    render_recovery_view(frame, chunks[1]);
    render_keybindings(frame, chunks[2]);
}

fn render_recovery_view(frame: &mut Frame, area: Rect) {
    let block = panel_block_accent("Recovery");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Layout matches recovery_view.rs: progress(2) | steps(9) | output(rest)
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

    // ETA text — matches recovery_view.rs format
    let eta = Paragraph::new(Line::from(vec![
        Span::styled(
            "Step 5/7",
            Style::default().fg(P::YELLOW_WARN).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ~360s remaining", Style::default().fg(P::TEXT_TERTIARY)),
    ]))
    .style(Style::default().bg(P::BG_PANEL));
    frame.render_widget(eta, progress_rows[0]);

    let bar = render_bar(4.0 / 7.0, progress_rows[1].width, P::YELLOW_WARN);
    let bar_paragraph = Paragraph::new(vec![bar]).style(Style::default().bg(P::BG_PANEL));
    frame.render_widget(bar_paragraph, progress_rows[1]);

    // ── Step List ───────────────────────────────────────────────
    // Icons and format match recovery_view.rs:
    //   "  " pending, ">>" running, "OK" completed, "!!" failed

    struct Step { label: &'static str, icon: &'static str, state: u8, time: &'static str }

    let steps = [
        Step { label: "Wait for baseline service", icon: "OK", state: 2, time: "(45.2s)" },
        Step { label: "Uninstall broken state",    icon: "OK", state: 2, time: "(28.1s)" },
        Step { label: "Destroy virtual hardware",  icon: "OK", state: 2, time: "(8.4s)"  },
        Step { label: "Recreate virtual hardware",  icon: "OK", state: 2, time: "(31.0s)" },
        Step { label: "Install packages",           icon: ">>", state: 1, time: "(42s...)" },
        Step { label: "Monitor zone startup",       icon: "  ", state: 0, time: "[~360s]" },
        Step { label: "Verify services",            icon: "  ", state: 0, time: "[~10s]"  },
    ];

    let step_lines: Vec<Line> = steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let style = match step.state {
                2 => Style::default().fg(P::GREEN_PRIMARY),            // completed
                1 => Style::default().fg(P::YELLOW_WARN).add_modifier(Modifier::BOLD), // running
                _ => Style::default().fg(P::TEXT_DISABLED),            // pending
            };
            // Format matches: " {icon}  Step N: {label}  {time}"
            Line::from(vec![
                Span::styled(format!(" {} ", step.icon), style),
                Span::styled(format!("Step {}: {}", i + 1, step.label), style),
                Span::styled(format!(" {}", step.time), Style::default().fg(P::TEXT_TERTIARY)),
            ])
        })
        .collect();

    let step_paragraph = Paragraph::new(step_lines).style(Style::default().bg(P::BG_PANEL));
    frame.render_widget(step_paragraph, sections[1]);

    // ── Output Pane ─────────────────────────────────────────────
    // Matches recovery_view.rs: Block with Borders::TOP, title " Output "

    let output_block = Block::default()
        .title(Line::from(Span::styled(" Output ", Style::default().fg(P::TEXT_TERTIARY))))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(P::BORDER_DEFAULT))
        .style(Style::default().bg(P::BG_PANEL));

    let output_inner = output_block.inner(sections[2]);
    frame.render_widget(output_block, sections[2]);

    let output_lines = vec![
        "pfexec cargo xtask virtual-hardware create \\",
        "  --gateway-ip 192.168.2.1 \\",
        "  --pxa-start 192.168.2.70 --pxa-end 192.168.2.90 \\",
        "  --vdev-size 42949672960",
        "Running omicron-package install...",
        "Installing crucible-pantry ...",
        "Installing internal-dns ...",
        "Installing nexus ...",
    ];

    let output: Vec<Line> = output_lines
        .iter()
        .map(|l| Line::from(Span::styled(*l, Style::default().fg(P::TEXT_SECONDARY))))
        .collect();

    let output_paragraph = Paragraph::new(output).style(Style::default().bg(P::BG_PANEL));
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
