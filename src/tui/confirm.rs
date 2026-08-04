//! The "are you sure" panel.
//!
//! It lists every path by name and the total that will go, because the whole
//! point is that the user reads it before typing the word.

use disko_core::size::format;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::model::display_path;
use crate::tui::app::{App, CONFIRM_WORD, Confirm};
use crate::tui::text::truncate;
use crate::tui::theme;

/// Enough paths to see what you are doing, not so many the total scrolls away.
const MAX_LISTED: usize = 10;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, confirm: &Confirm) {
    let unit = app.settings.unit;
    let width = 68u16.min(area.width);
    let listed = confirm.targets.len().min(MAX_LISTED);
    // Header, blank, paths, overflow note, blank, prompt, and the border.
    let height = (listed as u16 + 7).min(area.height);

    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);

    let danger = Style::default()
        .fg(Color::Rgb(213, 94, 0))
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Permanently delete ", danger),
            Span::styled(
                format!("{} ", confirm.targets.len()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(if confirm.targets.len() == 1 {
                "item, "
            } else {
                "items, "
            }),
            Span::styled(format(confirm.total(), unit), danger),
        ]),
        Line::default(),
    ];

    let inner = width.saturating_sub(6) as usize;
    for target in confirm.targets.iter().take(MAX_LISTED) {
        // A trailing slash is the difference between losing one file and
        // losing everything under a directory.
        let name = match target.is_dir {
            true => format!("{}/", display_path(&target.path)),
            false => display_path(&target.path),
        };
        lines.push(Line::from(vec![
            Span::styled("  • ", theme::muted()),
            Span::raw(truncate(&name, inner)),
            Span::styled(format!("  {}", format(target.size, unit)), theme::muted()),
        ]));
    }
    if confirm.targets.len() > MAX_LISTED {
        lines.push(Line::from(Span::styled(
            format!("  … and {} more", confirm.targets.len() - MAX_LISTED),
            theme::muted(),
        )));
    }

    lines.push(Line::default());
    lines.push(prompt_line(confirm, danger));

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(danger)
                .title(" This cannot be undone "),
        ),
        popup,
    );
}

fn prompt_line(confirm: &Confirm, danger: Style) -> Line<'static> {
    // Typing the word is the point: it makes the action deliberate rather than
    // one keystroke away from a cursor that happened to be in the wrong place.
    let mut spans = vec![
        Span::styled(" Type ", theme::muted()),
        Span::styled(CONFIRM_WORD, danger),
        Span::styled(" to confirm: ", theme::muted()),
        Span::raw(confirm.typed.clone()),
        Span::styled("▏", theme::accent()),
    ];

    if confirm.is_armed() {
        spans.push(Span::styled("  Enter to delete", danger));
    } else if confirm.nagged {
        spans.push(Span::styled(
            format!("  type {CONFIRM_WORD} first"),
            theme::warning(),
        ));
    } else {
        spans.push(Span::styled("  Esc to cancel", theme::muted()));
    }
    Line::from(spans)
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}
