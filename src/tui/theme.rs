//! Oxide visual theme — color constants, palettes, and style helpers.
//!
//! The active palette is **Design System** — derived from OKLCH values in
//! `oxidecomputer/design-system` (`styles/main.css` + `dark.css`).
//!
//! A **Website** palette (hex values from oxide.computer homepage JS/SVGs) is
//! preserved but deprecated. The two diverge in greens and text grays — see
//! `OXIDE-VISUAL-STYLE.md` for the full comparison. The website palette may
//! be useful in future for matching specific marketing-site visuals.
//!
//! Not yet wired into the module tree — add `pub mod theme;` to
//! `src/tui/mod.rs` when ready to integrate.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding};

// ── Palette ─────────────────────────────────────────────────────

/// Complete color palette for the TUI.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Palette {
    // Backgrounds (darkest → lightest)
    pub bg_base: Color,
    pub bg_panel: Color,
    pub bg_card: Color,
    pub bg_hover: Color,

    // Borders
    pub border_default: Color,
    pub border_focus: Color,
    pub border_input: Color,

    // Text (dimmest → brightest)
    pub text_disabled: Color,
    pub text_tertiary: Color,
    pub text_secondary: Color,
    pub text_default: Color,
    pub text_raised: Color,
    pub text_bright: Color,

    // Green scale
    pub green_primary: Color,
    pub green_secondary: Color,
    pub green_border: Color,
    pub green_accent_border: Color,
    pub green_border_dark: Color,
    pub green_bg: Color,
    pub green_bg_active: Color,

    // Semantic
    pub yellow_warn: Color,
    pub red_error: Color,
    pub blue_info: Color,

    // ASCII art diagram colors
    pub ascii_active: Color,
    pub ascii_structural: Color,
}

impl Palette {
    /// Default palette — Design System OKLCH dark theme.
    /// Source: oxidecomputer/design-system styles/main.css + dark.css
    pub const fn default() -> Self {
        Self::design_system()
    }

    /// Design system palette — from oxidecomputer/design-system OKLCH dark theme.
    /// Pure cyan-greens (R=0), brighter text, more saturated semantics.
    pub const fn design_system() -> Self {
        Self {
            // Backgrounds — neutral scale
            bg_base: Color::Rgb(11, 13, 18), // #0B0D12 neutral-0 / surface-default
            bg_panel: Color::Rgb(18, 21, 25), // #121519 neutral-50 / surface-raise
            bg_card: Color::Rgb(23, 25, 29), // #17191D neutral-100 / surface-secondary
            bg_hover: Color::Rgb(30, 33, 36), // #1E2124 neutral-200 / surface-hover

            // Borders — neutral scale
            border_default: Color::Rgb(48, 49, 52), // #303134 neutral-300 / stroke-default
            border_focus: Color::Rgb(67, 68, 71),   // #434447 neutral-400 / stroke-raise
            border_input: Color::Rgb(48, 49, 52),   // #303134 neutral-300

            // Text — neutral scale
            text_disabled: Color::Rgb(93, 94, 96), // #5D5E60 neutral-500 / content-quaternary
            text_tertiary: Color::Rgb(128, 129, 131), // #808183 neutral-600 / content-tertiary
            text_secondary: Color::Rgb(162, 163, 164), // #A2A3A4 neutral-700 / content-secondary
            text_default: Color::Rgb(185, 186, 187), // #B9BABB neutral-800 / content-default
            text_raised: Color::Rgb(221, 221, 221), // #DDDDDD neutral-900 / content-raise
            text_bright: Color::Rgb(238, 238, 238), // #EEEEEE neutral-1100

            // Green scale
            green_primary: Color::Rgb(0, 216, 145), // #00D891 green-800 / content-accent
            green_secondary: Color::Rgb(0, 182, 124), // #00B67C green-700 / content-accent-secondary
            green_border: Color::Rgb(0, 147, 102),    // #009366 green-600 / content-accent-tertiary
            green_accent_border: Color::Rgb(0, 84, 64), // #005440 green-400 / stroke-accent-tertiary
            green_border_dark: Color::Rgb(0, 61, 49), // #003D31 green-300 / stroke-accent-quaternary
            green_bg: Color::Rgb(0, 41, 34),          // #002922 green-200 / surface-accent
            green_bg_active: Color::Rgb(0, 41, 34),   // #002922 green-200

            // Semantic
            yellow_warn: Color::Rgb(254, 187, 85), // #FEBB55 yellow-800 / content-notice
            red_error: Color::Rgb(254, 103, 132),  // #FE6784 red-800 / content-error
            blue_info: Color::Rgb(129, 153, 254),  // #8199FE blue-800 / content-info

            // ASCII art — monochromatic green pair
            ascii_active: Color::Rgb(0, 216, 145), // #00D891 green-800
            ascii_structural: Color::Rgb(0, 147, 102), // #009366 green-600
        }
    }
}

// ── Helper Functions ────────────────────────────────────────────

/// Render a ▊/⎕ progress bar as a `Line`.
pub fn render_bar(ratio: f64, width: u16, fg: Color, empty_fg: Color) -> Line<'static> {
    let ratio = ratio.clamp(0.0, 1.0);
    let total = width as usize;
    let filled = (ratio * total as f64).round() as usize;
    let empty = total.saturating_sub(filled);

    Line::from(vec![
        Span::styled("\u{258A}".repeat(filled), Style::default().fg(fg)),
        Span::styled("\u{2395}".repeat(empty), Style::default().fg(empty_fg)),
    ])
}

/// Create a panel block with focus-aware styling.
pub fn panel_block(title: &str, focused: bool, p: &Palette) -> Block<'static> {
    let border_color = if focused {
        p.border_focus
    } else {
        p.border_default
    };
    let title_color = if focused {
        p.text_raised
    } else {
        p.text_tertiary
    };

    Block::default()
        .title(format!(" {} ", title.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title_style(Style::default().fg(title_color))
        .style(Style::default().bg(p.bg_panel))
        .padding(Padding::new(2, 2, 1, 1))
}

/// Create a green-accented panel block (e.g., for recovery view).
pub fn panel_block_accent(title: &str, p: &Palette) -> Block<'static> {
    Block::default()
        .title(format!(" {} ", title.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.green_border))
        .title_style(Style::default().fg(p.green_primary))
        .style(Style::default().bg(p.bg_panel))
        .padding(Padding::new(2, 2, 1, 1))
}

/// Render a section header: UPPERCASE, BOLD, bright text.
pub fn section_header(text: &str, p: &Palette) -> Line<'static> {
    Line::from(Span::styled(
        text.to_uppercase(),
        Style::default()
            .fg(p.text_bright)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Map a percentage to a threshold color.
pub fn threshold_color(pct: u8, warning: u8, critical: u8, p: &Palette) -> Color {
    if pct >= critical {
        p.red_error
    } else if pct >= warning {
        p.yellow_warn
    } else {
        p.green_primary
    }
}

/// Format a Duration as a human-readable string (e.g., "1h 02m 45s").
pub fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!(
            "{}h {:02}m {:02}s",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}
