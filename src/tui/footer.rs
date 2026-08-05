//! The one-line key hint / search input / status bar at the bottom.

use disko_core::size::format;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, View};
use crate::tui::text::{truncate, width};
use crate::tui::theme;

/// A key and what it does. Kept apart so the key can be drawn in a colour that
/// separates it from its description — one dim run of text makes the reader
/// parse "Enter open ← back" character by character looking for the keys.
type Hint = (&'static str, &'static str);

/// Always shown, however narrow the terminal gets.
const QUIT: Hint = ("q", "quit");

const GAP: usize = 3;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if app.search_active {
        draw_search(frame, area, app);
        return;
    }

    let total = area.width as usize;

    // The status earns its place only if there is room left after the keys.
    let room_for_status = total.saturating_sub(minimum_width(app) + GAP);
    let right = if room_for_status >= 8 {
        truncate(&status(app), room_for_status)
    } else {
        String::new()
    };

    let budget = total.saturating_sub(if right.is_empty() {
        0
    } else {
        width(&right) + GAP
    });

    let mut spans = hint_spans(app, budget);
    let used: usize = spans.iter().map(|span| width(&span.content)).sum();
    let gap = total.saturating_sub(used + width(&right));
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(right, theme::secondary()));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Fit as many hints as the width allows, dropping the least important first
/// but never the one that says how to get out.
fn hint_spans(app: &App, budget: usize) -> Vec<Span<'static>> {
    let quit = quit_hint(app);
    let quit_width = hint_width(quit);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0;

    for hint in hints_for(app.view) {
        let needed = hint_width(*hint) + if spans.is_empty() { 0 } else { GAP };
        // Leave room for the escape hatch, always.
        if used + needed + GAP + quit_width > budget {
            break;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" ".repeat(GAP)));
            used += GAP;
        }
        spans.extend(render(*hint));
        used += hint_width(*hint);
    }

    if !spans.is_empty() {
        spans.push(Span::raw(" ".repeat(GAP)));
    }
    spans.extend(render(quit));
    spans
}

fn render(hint: Hint) -> [Span<'static>; 2] {
    let (key, label) = hint;
    [
        Span::styled(key, theme::key()),
        Span::styled(format!(" {label}"), theme::muted()),
    ]
}

fn hint_width(hint: Hint) -> usize {
    width(hint.0) + 1 + width(hint.1)
}

/// The narrowest the keys can get: one hint plus the way out.
fn minimum_width(app: &App) -> usize {
    let quit = hint_width(quit_hint(app));
    match hints_for(app.view).first() {
        Some(first) => hint_width(*first) + GAP + quit,
        None => quit,
    }
}

fn quit_hint(app: &App) -> Hint {
    match app.view {
        // Escape steps back to the overview rather than leaving.
        View::Explorer => ("Esc", "back"),
        View::Scanning => ("Esc", "stop"),
        _ => QUIT,
    }
}

/// Most useful first: the tail is what gets dropped on a narrow terminal.
fn hints_for(view: View) -> &'static [Hint] {
    match view {
        View::Overview => &[
            ("Enter", "explore"),
            ("→", "open"),
            ("←", "back"),
            ("t", "growth"),
            ("Space", "mark"),
            ("x", "delete"),
            ("o", "reveal"),
            ("y", "copy"),
            ("/", "search"),
            ("d", "details"),
        ],
        View::Explorer => &[
            ("↑↓", "select"),
            ("Enter", "open"),
            ("←", "back"),
            ("t", "growth"),
            ("Space", "mark"),
            ("x", "delete"),
            ("o", "reveal"),
            ("y", "copy"),
        ],
        View::Picker => &[("↑↓", "select"), ("Enter", "scan")],
        View::Scanning => &[],
    }
}

/// The right-hand slot shows the most useful thing available.
fn status(app: &App) -> String {
    if !app.marks.is_empty() {
        format!(
            "{} marked · {}",
            app.marks.len(),
            format(app.marked_total(), app.settings.unit)
        )
    } else if let Some(status) = &app.status {
        status.clone()
    } else if let Some(watch) = &app.watch {
        format!(
            "watching · every {}s · {}",
            watch.every,
            crate::timefmt::humanize(watch.elapsed())
        )
    } else if app.showing_growth() {
        "showing growth".to_string()
    } else if app.view == View::Overview {
        format!("sorted by {}", app.sort.label())
    } else {
        String::new()
    }
}

fn draw_search(frame: &mut Frame, area: Rect, app: &App) {
    let query = app.search.clone().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/", theme::accent()),
            Span::styled(query, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("▏", theme::accent()),
            Span::raw("   "),
            Span::styled("Enter", theme::key()),
            Span::styled(" keep", theme::muted()),
            Span::raw("   "),
            Span::styled("Esc", theme::key()),
            Span::styled(" clear", theme::muted()),
        ])),
        area,
    );
}
