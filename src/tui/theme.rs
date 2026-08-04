//! Bridges `disko-render`'s colours to ratatui, plus the handful of styles the
//! screens share.

use disko_render::Rgb;
use ratatui::style::{Color, Modifier, Style};

pub fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

pub fn heading() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

pub fn muted() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn secondary() -> Style {
    Style::default().fg(Color::Gray)
}

pub fn selected() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Rgb(200, 200, 205))
        .add_modifier(Modifier::BOLD)
}

/// Keyboard shortcuts in the footer. Bright enough to pick out of a dim row
/// at a glance without competing with the data above it.
pub fn key() -> Style {
    Style::default()
        .fg(Color::Rgb(122, 190, 245))
        .add_modifier(Modifier::BOLD)
}

pub fn accent() -> Style {
    Style::default().fg(Color::Cyan)
}

pub fn warning() -> Style {
    Style::default().fg(Color::Rgb(213, 94, 0))
}

/// Terminals that set `NO_COLOR` get the braille outline instead of a filled
/// colour sunburst. https://no-color.org
pub fn colors_disabled() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}
