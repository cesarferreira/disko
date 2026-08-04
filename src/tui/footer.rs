//! The one-line key hint / search input / status bar at the bottom.

use disko_core::size::format;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, View};
use crate::tui::text::{truncate, width};
use crate::tui::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if app.search_active {
        draw_search(frame, area, app);
        return;
    }

    // Narrow terminals get the short hint list rather than a sentence cut off
    // mid-word.
    let roomy = area.width >= 76;
    let keys = match (app.view, roomy) {
        (View::Overview, true) => {
            "Enter explore   → open   ← up   / search   s sort   d details   q quit"
        }
        (View::Overview, false) => "Enter explore   / search   s sort   q quit",
        (View::Explorer, true) => {
            "↑↓ select   Enter open   Backspace up   Space mark   Esc overview   q quit"
        }
        (View::Explorer, false) => "↑↓ select   Enter open   Esc back   q quit",
        (View::Picker, _) => "↑↓ select   Enter scan   q quit",
        (View::Scanning, _) => "Esc stop scan   q quit",
    };

    // The right-hand slot shows the most useful thing available: what you
    // marked, then anything the app wants to tell you, then the sort order.
    let right = if !app.marks.is_empty() {
        format!(
            "{} marked · {}",
            app.marks.len(),
            format(app.marked_total(), app.settings.unit)
        )
    } else if let Some(status) = &app.status {
        status.clone()
    } else if app.view == View::Overview {
        format!("sorted by {}", app.sort.label())
    } else {
        String::new()
    };

    // Better to drop the status entirely than to render a lone ellipsis.
    let total = area.width as usize;
    let room = total.saturating_sub(width(keys) + 2);
    let right = if room >= 6 {
        truncate(&right, room)
    } else {
        String::new()
    };
    let gap = total.saturating_sub(width(keys) + width(&right));

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(keys, theme::muted()),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, theme::secondary()),
        ])),
        area,
    );
}

fn draw_search(frame: &mut Frame, area: Rect, app: &App) {
    let query = app.search.clone().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/", theme::accent()),
            Span::raw(query),
            Span::styled("▏", theme::accent()),
            Span::raw("   "),
            Span::styled("Enter keep   Esc clear", theme::muted()),
        ])),
        area,
    );
}
