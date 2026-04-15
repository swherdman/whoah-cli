use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::git::RepoRefs;
use crate::tui::theme::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefMode {
    Branch,
    Tag,
    Commit,
}

impl RefMode {
    const ALL: [RefMode; 3] = [RefMode::Branch, RefMode::Tag, RefMode::Commit];

    fn label(self) -> &'static str {
        match self {
            RefMode::Branch => "Branch",
            RefMode::Tag => "Tag",
            RefMode::Commit => "Commit",
        }
    }

    fn next(self) -> Self {
        match self {
            RefMode::Branch => RefMode::Tag,
            RefMode::Tag => RefMode::Commit,
            RefMode::Commit => RefMode::Branch,
        }
    }

    fn prev(self) -> Self {
        match self {
            RefMode::Branch => RefMode::Commit,
            RefMode::Tag => RefMode::Branch,
            RefMode::Commit => RefMode::Tag,
        }
    }
}

/// Action returned from handle_key to the caller.
pub enum SelectorAction {
    /// Keep the selector open.
    Continue,
    /// User confirmed — persist this value as git_ref.
    Confirm(String),
    /// User cancelled.
    Cancel,
}

pub struct GitRefSelector {
    mode: RefMode,
    branch_input: Input,
    tag_input: Input,
    commit_input: Input,
    cache: RepoRefs,
    /// Filtered candidates for the active mode's autocomplete.
    filtered: Vec<String>,
    /// Which filtered item is highlighted (-1 equivalent = nothing selected).
    selected_idx: Option<usize>,
}

impl GitRefSelector {
    /// Create a new selector, pre-populated from the current git_ref value and cache.
    pub fn new(current_git_ref: Option<&str>, cache: RepoRefs) -> Self {
        let current = current_git_ref.unwrap_or("");

        // Determine initial mode by checking what the current ref matches
        let (mode, branch_val, tag_val, commit_val) = classify_ref(current, &cache);

        let mut sel = Self {
            mode,
            branch_input: Input::new(branch_val),
            tag_input: Input::new(tag_val),
            commit_input: Input::new(commit_val),
            cache,
            filtered: Vec::new(),
            selected_idx: None,
        };
        sel.update_filtered();
        sel
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SelectorAction {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Mode switching: Tab, Shift+Left, Shift+Right
        if key.code == KeyCode::Tab || (shift && key.code == KeyCode::Right) {
            self.mode = self.mode.next();
            self.selected_idx = None;
            self.update_filtered();
            return SelectorAction::Continue;
        }
        if shift && key.code == KeyCode::Left {
            self.mode = self.mode.prev();
            self.selected_idx = None;
            self.update_filtered();
            return SelectorAction::Continue;
        }

        match key.code {
            KeyCode::Esc => SelectorAction::Cancel,
            KeyCode::Enter => {
                // If a dropdown item is selected, accept it into the input first
                if let Some(idx) = self.selected_idx {
                    if let Some(item) = self.filtered.get(idx).cloned() {
                        self.accept_selection(item);
                        self.selected_idx = None;
                        self.update_filtered();
                        return SelectorAction::Continue;
                    }
                }
                // Confirm — only if we can resolve to a SHA
                match self.resolved_git_ref() {
                    Some(value) => SelectorAction::Confirm(value),
                    None => SelectorAction::Continue, // can't resolve, stay open
                }
            }
            KeyCode::Down => {
                if !self.filtered.is_empty() {
                    self.selected_idx = Some(match self.selected_idx {
                        None => 0,
                        Some(i) => (i + 1).min(self.filtered.len() - 1),
                    });
                }
                SelectorAction::Continue
            }
            KeyCode::Up => {
                if let Some(i) = self.selected_idx {
                    self.selected_idx = if i == 0 { None } else { Some(i - 1) };
                }
                SelectorAction::Continue
            }
            _ => {
                // Forward to active input
                let event = CtEvent::Key(key);
                match self.mode {
                    RefMode::Branch => self.branch_input.handle_event(&event),
                    RefMode::Tag => self.tag_input.handle_event(&event),
                    RefMode::Commit => self.commit_input.handle_event(&event),
                };
                self.selected_idx = None;
                self.update_filtered();
                SelectorAction::Continue
            }
        }
    }

    fn accept_selection(&mut self, display: String) {
        match self.mode {
            RefMode::Branch => {
                // Strip " (default)" suffix if present
                let name = display
                    .strip_suffix(" (default)")
                    .unwrap_or(&display)
                    .to_string();
                self.branch_input = Input::new(name.clone());
                self.tag_input = Input::new(String::new());
                if let Some(entry) = self.cache.branches.iter().find(|b| b.name == name) {
                    self.commit_input = Input::new(short_sha(&entry.sha));
                }
            }
            RefMode::Tag => {
                self.tag_input = Input::new(display.clone());
                if let Some(entry) = self.cache.tags.iter().find(|t| t.name == display) {
                    self.commit_input = Input::new(short_sha(&entry.sha));
                }
            }
            RefMode::Commit => {
                // Display format is "abc123def456 commit message" — extract just the SHA
                let sha = display
                    .split_whitespace()
                    .next()
                    .unwrap_or(&display)
                    .to_string();
                self.commit_input = Input::new(sha);
            }
        }
    }

    /// The value to persist as git_ref — always a commit SHA when known.
    /// Returns None if the ref can't be resolved to a SHA (unknown branch/tag).
    fn resolved_git_ref(&self) -> Option<String> {
        match self.mode {
            RefMode::Branch => {
                let name = self.branch_input.value();
                if name.is_empty() || name == self.cache.default_branch {
                    return Some(String::new()); // empty = HEAD/default
                }
                self.cache
                    .branches
                    .iter()
                    .find(|b| b.name == name)
                    .map(|b| b.sha.clone())
            }
            RefMode::Tag => {
                let name = self.tag_input.value();
                if name.is_empty() {
                    return None;
                }
                self.cache
                    .tags
                    .iter()
                    .find(|t| t.name == name)
                    .map(|t| t.sha.clone())
            }
            RefMode::Commit => {
                let v = self.commit_input.value();
                if v.is_empty() {
                    None
                } else {
                    Some(v.to_string())
                }
            }
        }
    }

    fn update_filtered(&mut self) {
        match self.mode {
            RefMode::Branch => {
                let query = self.branch_input.value().to_lowercase();
                self.filtered = self
                    .cache
                    .branches
                    .iter()
                    .filter(|b| query.is_empty() || b.name.to_lowercase().contains(&query))
                    .take(15)
                    .map(|b| {
                        if b.name == self.cache.default_branch {
                            format!("{} (default)", b.name)
                        } else {
                            b.name.clone()
                        }
                    })
                    .collect();
            }
            RefMode::Tag => {
                let query = self.tag_input.value().to_lowercase();
                self.filtered = self
                    .cache
                    .tags
                    .iter()
                    .filter(|t| query.is_empty() || t.name.to_lowercase().contains(&query))
                    .take(15)
                    .map(|t| t.name.clone())
                    .collect();
            }
            RefMode::Commit => {
                let query = self.commit_input.value().to_lowercase();
                self.filtered = self
                    .cache
                    .commits
                    .iter()
                    .filter(|c| {
                        query.is_empty()
                            || c.sha.to_lowercase().contains(&query)
                            || c.message.to_lowercase().contains(&query)
                    })
                    .take(10)
                    .map(|c| format!("{} {}", short_sha(&c.sha), c.message))
                    .collect();
            }
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let dropdown_height = self.filtered.len().min(8) as u16;
        let popup_h = (5 + dropdown_height).min(area.height.saturating_sub(2));
        let popup_w = area.width.saturating_sub(4).min(55);
        let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
        let y = area.y + 1;
        let popup_area = Rect::new(x, y, popup_w, popup_h);

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(" Select git_ref ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.green_border))
            .title_style(Style::default().fg(p.text_bright));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        if inner.height < 3 {
            return;
        }

        let chunks = Layout::vertical([
            Constraint::Length(1), // mode tabs
            Constraint::Length(1), // input line
            Constraint::Length(1), // context line (commit / branch info)
            Constraint::Min(0),    // dropdown
        ])
        .split(inner);

        // Mode tabs
        self.render_mode_tabs(frame, chunks[0], p);

        // Active input
        self.render_input(frame, chunks[1], p);

        // Context line
        self.render_context(frame, chunks[2], p);

        // Dropdown
        if !self.filtered.is_empty() {
            self.render_dropdown(frame, chunks[3], p);
        }
    }

    fn render_mode_tabs(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        for (i, mode) in RefMode::ALL.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(p.border_default)));
            }
            if *mode == self.mode {
                spans.push(Span::styled(
                    format!("[{}]", mode.label()),
                    Style::default()
                        .fg(p.green_primary)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    mode.label(),
                    Style::default().fg(p.text_disabled),
                ));
            }
        }
        spans.push(Span::styled(
            "  Shift + ←→:mode",
            Style::default().fg(p.text_tertiary),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let (label, input, editable) = match self.mode {
            RefMode::Branch => ("branch", &self.branch_input, true),
            RefMode::Tag => ("tag", &self.tag_input, true),
            RefMode::Commit => ("commit", &self.commit_input, true),
        };

        let value = input.value();
        let cursor = input.visual_cursor();

        if editable {
            // Render with cursor
            let (before, rest) = value.split_at(
                value
                    .char_indices()
                    .nth(cursor)
                    .map(|(b, _)| b)
                    .unwrap_or(value.len()),
            );
            let cursor_char: String = rest
                .chars()
                .next()
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".to_string());
            let after_start = cursor_char.len().min(rest.len());
            let after = &rest[after_start..];

            let line = Line::from(vec![
                Span::styled(format!(" {label}: "), Style::default().fg(p.text_tertiary)),
                Span::styled(before.to_string(), Style::default().fg(p.text_bright)),
                Span::styled(
                    cursor_char,
                    Style::default().fg(p.bg_base).bg(p.text_bright),
                ),
                Span::styled(after.to_string(), Style::default().fg(p.text_bright)),
            ]);
            frame.render_widget(Paragraph::new(line), area);
        } else {
            frame.render_widget(
                Paragraph::new(format!(" {label}: {value}"))
                    .style(Style::default().fg(p.text_disabled)),
                area,
            );
        }
    }

    fn render_context(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let (text, color) = match self.mode {
            RefMode::Branch => {
                let branch = self.branch_input.value();
                if let Some(entry) = self.cache.branches.iter().find(|b| b.name == branch) {
                    (
                        format!(" saves: {} ({})", short_sha(&entry.sha), branch),
                        p.green_primary,
                    )
                } else if branch.is_empty() {
                    (
                        format!(" saves: HEAD ({})", self.cache.default_branch),
                        p.text_tertiary,
                    )
                } else {
                    (
                        " unknown branch — select from list".to_string(),
                        p.yellow_warn,
                    )
                }
            }
            RefMode::Tag => {
                let tag = self.tag_input.value();
                if let Some(entry) = self.cache.tags.iter().find(|t| t.name == tag) {
                    (
                        format!(" saves: {} (tag {})", short_sha(&entry.sha), tag),
                        p.green_primary,
                    )
                } else if tag.is_empty() {
                    (" select a tag from the list".to_string(), p.text_tertiary)
                } else {
                    (" unknown tag — select from list".to_string(), p.yellow_warn)
                }
            }
            RefMode::Commit => {
                let v = self.commit_input.value();
                if v.is_empty() {
                    (" type or select a commit SHA".to_string(), p.text_tertiary)
                } else {
                    (format!(" saves: {v}"), p.green_primary)
                }
            }
        };
        frame.render_widget(Paragraph::new(text).style(Style::default().fg(color)), area);
    }

    fn render_dropdown(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        if area.height == 0 {
            return;
        }
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .map(|s| ListItem::new(format!(" {s}")).style(Style::default().fg(p.text_default)))
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(p.green_primary)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">");
        let mut state = ListState::default();
        state.select(self.selected_idx);
        frame.render_stateful_widget(list, area, &mut state);
    }
}

/// Classify a git_ref string into a mode + initial field values.
fn classify_ref(current: &str, cache: &RepoRefs) -> (RefMode, String, String, String) {
    if current.is_empty() {
        // Default: branch mode with default branch
        let sha = cache
            .branches
            .iter()
            .find(|b| b.name == cache.default_branch)
            .map(|b| short_sha(&b.sha))
            .unwrap_or_default();
        return (
            RefMode::Branch,
            cache.default_branch.clone(),
            String::new(),
            sha,
        );
    }

    // Check if it matches a branch
    if let Some(entry) = cache.branches.iter().find(|b| b.name == current) {
        return (
            RefMode::Branch,
            current.to_string(),
            String::new(),
            short_sha(&entry.sha),
        );
    }

    // Check if it matches a tag
    if let Some(entry) = cache.tags.iter().find(|t| t.name == current) {
        return (
            RefMode::Tag,
            String::new(),
            current.to_string(),
            short_sha(&entry.sha),
        );
    }

    // Assume it's a commit SHA
    (
        RefMode::Commit,
        String::new(),
        String::new(),
        current.to_string(),
    )
}

fn short_sha(sha: &str) -> String {
    sha.get(..12).unwrap_or(sha).to_string()
}
