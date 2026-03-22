use crossterm::event::KeyEvent;
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use tui_input::Input;

use crate::git::RepoRefs;
use crate::tui::theme::Palette;

// ── ConfigPanel trait ──────────────────────────────────────────────────

/// Trait that all right-panel config types must implement.
pub trait ConfigPanel {
    /// Title text for the right panel border.
    fn title(&self) -> String;

    /// Render content into the given area (inside the border).
    fn render(&self, frame: &mut Frame, area: Rect, palette: &Palette);

    /// Handle a key event. Returns PanelEvent.
    fn handle_key(&mut self, key: KeyEvent) -> PanelEvent;

    /// Whether the panel is capturing all input (editing, picking, confirming).
    fn is_capturing(&self) -> bool;

    /// Receive async data from App. Default no-op.
    fn receive_data(&mut self, _data: PanelData) {}
}

/// Communication from a panel up to ConfigView/App.
pub enum PanelEvent {
    /// Key was consumed, no further action needed.
    Consumed,
    /// Key was not handled — parent should route further.
    Ignored,
    /// Panel requests an action from the parent.
    Action(PanelAction),
}

/// Actions a panel can request from ConfigView/App.
pub enum PanelAction {
    /// Request async git ref fetch.
    FetchGitRefs { repo_url: String },
    /// Request SSH credential probe.
    ProbeSsh { host: String, user: String },
    /// Request Proxmox config validation.
    ValidateProxmox { host: String, user: String },
    /// Request ISO download to Proxmox host.
    DownloadIso {
        host: String,
        user: String,
        iso_storage_path: String,
        filename: String,
    },
    /// Panel deleted its config — parent should clean up.
    Deleted { name: String },
    /// Display an error.
    Error(String),
}

/// Async data delivered from App to a panel.
pub enum PanelData {
    GitRefs(RepoRefs),
    SshProbeResult(crate::ssh::probe::SshProbeStatus),
    ProxmoxValidation(crate::ops::hypervisor_proxmox_validate::ProxmoxValidation),
}

// ── Detail line types ──────────────────────────────────────────────────

/// A single line in a panel's detail view.
#[derive(Clone)]
pub struct DetailLine {
    pub text: String,
    pub style: DetailStyle,
    /// If set, this line is an editable field.
    pub field: Option<FieldKey>,
    /// The raw value (without label prefix) for populating edit buffer.
    pub raw_value: Option<String>,
    /// If set, this field uses a picker instead of free-text editing.
    pub picker: Option<PickerKind>,
    /// If set, this line is a clickable action (e.g. delete).
    pub action: Option<LineAction>,
    /// Optional foreground color override (for status indicators).
    pub fg_override: Option<Color>,
    /// Optional suffix appended after the main text with its own color (e.g. "●" status dot).
    pub suffix: Option<(String, Color)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DetailStyle {
    SectionHeader,
    /// Read-only field.
    Field,
    /// Editable via text input.
    EditableField,
    /// Destructive action (rendered in red).
    DangerAction,
}

/// Identifies a field's location in TOML files for editing.
#[derive(Debug, Clone)]
pub struct FieldKey {
    /// Which TOML file: "deployment", "build", "monitoring", "hypervisor"
    pub file: &'static str,
    /// Dotted path into the TOML, e.g. "network.gateway"
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerKind {
    GitRef { repo_url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineAction {
    DeleteHypervisor,
    DownloadIso,
}

// ── Builder helpers ────────────────────────────────────────────────────

pub fn push_header(lines: &mut Vec<DetailLine>, title: &str) {
    lines.push(DetailLine {
        text: String::new(),
        style: DetailStyle::SectionHeader,
        field: None,
        raw_value: None,
        picker: None,
        action: None,
        fg_override: None,
        suffix: None,
    });
    lines.push(DetailLine {
        text: format!("  {title}"),
        style: DetailStyle::SectionHeader,
        field: None,
        raw_value: None,
        picker: None,
        action: None,
        fg_override: None,
        suffix: None,
    });
}

pub fn push_field(lines: &mut Vec<DetailLine>, label: &str, value: &str) {
    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: DetailStyle::Field,
        field: None,
        raw_value: None,
        picker: None,
        action: None,
        fg_override: None,
        suffix: None,
    });
}

pub fn push_editable(
    lines: &mut Vec<DetailLine>,
    label: &str,
    value: &str,
    file: &'static str,
    path: &str,
) {
    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: DetailStyle::EditableField,
        field: Some(FieldKey {
            file,
            path: path.to_string(),
        }),
        raw_value: Some(value.to_string()),
        picker: None,
        action: None,
        fg_override: None,
        suffix: None,
    });
}

pub fn push_pickable(
    lines: &mut Vec<DetailLine>,
    label: &str,
    value: &str,
    file: &'static str,
    path: &str,
    kind: PickerKind,
) {
    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: DetailStyle::EditableField,
        field: Some(FieldKey {
            file,
            path: path.to_string(),
        }),
        raw_value: Some(value.to_string()),
        picker: Some(kind),
        action: None,
        fg_override: None,
        suffix: None,
    });
}

pub fn push_danger_action(lines: &mut Vec<DetailLine>, text: &str, action: LineAction) {
    lines.push(DetailLine {
        text: String::new(),
        style: DetailStyle::SectionHeader,
        field: None,
        raw_value: None,
        picker: None,
        action: None,
        fg_override: None,
        suffix: None,
    });
    lines.push(DetailLine {
        text: format!("  {text}"),
        style: DetailStyle::DangerAction,
        field: None,
        raw_value: None,
        picker: None,
        action: Some(action),
        fg_override: None,
        suffix: None,
    });
}

// ── Shared navigation ──────────────────────────────────────────────────

pub fn next_editable_line(lines: &[DetailLine], from: usize) -> Option<usize> {
    (from + 1..lines.len()).find(|&i| lines[i].field.is_some() || lines[i].action.is_some())
}

pub fn prev_editable_line(lines: &[DetailLine], from: usize) -> Option<usize> {
    (0..from)
        .rev()
        .find(|&i| lines[i].field.is_some() || lines[i].action.is_some())
}

pub fn first_editable_line(lines: &[DetailLine]) -> usize {
    lines
        .iter()
        .position(|l| l.field.is_some() || l.action.is_some())
        .unwrap_or(0)
}

pub fn ensure_visible(selected: usize, scroll_offset: &mut usize, visible_height: usize) {
    if selected < *scroll_offset {
        *scroll_offset = selected;
    }
    if visible_height > 0 && selected >= *scroll_offset + visible_height {
        *scroll_offset = selected - visible_height + 1;
    }
}

/// Render a tui_input::Input value with a visible cursor as a Line.
/// `label` is rendered in `label_style`, the input text + cursor in bright/hover palette colors.
pub fn render_input_line<'a>(input: &Input, label: &str, palette: &Palette) -> Line<'a> {
    let p = palette;
    let value = input.value();
    let cursor = input.visual_cursor();
    let (before, rest) = value.split_at(
        value
            .char_indices()
            .nth(cursor)
            .map(|(b, _)| b)
            .unwrap_or(value.len()),
    );
    let cursor_str: String = rest
        .chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".to_string());
    let after_start = cursor_str.len().min(rest.len());
    let after = &rest[after_start..];

    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().fg(p.green_primary),
        ),
        Span::styled(
            before.to_string(),
            Style::default().fg(p.text_bright).bg(p.bg_hover),
        ),
        Span::styled(
            cursor_str,
            Style::default().fg(p.bg_base).bg(p.text_bright),
        ),
        Span::styled(
            after.to_string(),
            Style::default().fg(p.text_bright).bg(p.bg_hover),
        ),
    ])
}

// ── Shared rendering ───────────────────────────────────────────────────

/// Render a slice of DetailLines with selection highlight, scroll, and optional
/// inline editing. Returns the visible height of the content area for scroll
/// calculations.
pub fn render_detail_lines(
    frame: &mut Frame,
    area: Rect,
    lines: &[DetailLine],
    selected: usize,
    scroll_offset: usize,
    is_focused: bool,
    edit_input: Option<&Input>,
    palette: &Palette,
) -> usize {
    let p = palette;

    let rendered: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(i, dl)| {
            let is_selected = i == selected && is_focused;
            let is_editing = is_selected && edit_input.is_some();

            if is_editing {
                if let Some(input) = edit_input {
                    let label = dl.text.split(':').next().unwrap_or("    ?");
                    return render_input_line(input, label, p);
                }
            }

            let mut base_style = match dl.style {
                DetailStyle::SectionHeader => Style::default()
                    .fg(p.text_bright)
                    .add_modifier(Modifier::BOLD),
                DetailStyle::Field => Style::default().fg(p.text_tertiary),
                DetailStyle::EditableField => Style::default().fg(p.text_default),
                DetailStyle::DangerAction => Style::default().fg(p.red_error),
            };
            if let Some(color) = dl.fg_override {
                base_style = base_style.fg(color);
            }

            let style = if is_selected && (dl.field.is_some() || dl.action.is_some()) {
                base_style.bg(p.bg_hover)
            } else {
                base_style
            };

            if let Some((ref suffix_text, suffix_color)) = dl.suffix {
                Line::from(vec![
                    Span::styled(&dl.text, style),
                    Span::styled(format!(" {suffix_text}"), Style::default().fg(suffix_color)),
                ])
            } else {
                Line::from(Span::styled(&dl.text, style))
            }
        })
        .collect();

    let total_lines = rendered.len();
    let visible_height = area.height as usize;
    frame.render_widget(
        Paragraph::new(rendered)
            .style(Style::default().bg(p.bg_panel))
            .scroll((scroll_offset as u16, 0)),
        area,
    );

    if total_lines > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border_default));
        let mut scrollbar_state =
            ScrollbarState::new(total_lines.saturating_sub(visible_height)).position(scroll_offset);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }

    visible_height
}
