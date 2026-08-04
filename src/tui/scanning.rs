//! The progress screen, shown while a scan runs.

use disko_core::size::format;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::model;
use crate::tui::app::App;
use crate::tui::text::shorten_path;
use crate::tui::theme;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn draw(frame: &mut Frame, area: Rect, app: &App, tick: usize) {
    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(4),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .horizontal_margin(1)
    .split(area);

    let spinner = SPINNER[tick % SPINNER.len()];
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{spinner} "), theme::accent()),
        Span::styled("Scanning ", theme::heading()),
        Span::raw(model::display_path(&app.root)),
    ])];

    if let Some(progress) = app.progress() {
        lines.push(Line::from(Span::styled(
            format!(
                "{} items · {}",
                progress.entries(),
                format(progress.bytes(), app.settings.unit)
            ),
            theme::secondary(),
        )));

        let current = progress.current();
        if !current.as_os_str().is_empty() {
            let width = chunks[1].width.saturating_sub(4) as usize;
            lines.push(Line::from(Span::styled(
                shorten_path(&model::display_path(&current), width),
                theme::muted(),
            )));
        }

        // Permission errors are normal on a full-disk scan; say how many
        // rather than pretending the total is exact.
        if progress.errors() > 0 {
            lines.push(Line::from(Span::styled(
                format!("{} folders unreadable", progress.errors()),
                theme::warning(),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        chunks[1],
    );
    crate::tui::footer::draw(frame, chunks[3], app);
}
