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
        Constraint::Length(2),
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
        let elapsed = app
            .scan_elapsed()
            .map(|elapsed| format!(" · {}s", elapsed.as_secs()))
            .unwrap_or_default();
        lines.push(Line::from(Span::styled(
            format!(
                "{} items · {}{}",
                progress.entries(),
                format(progress.bytes(), app.settings.unit),
                elapsed
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
    draw_found(frame, chunks[2], app);
    crate::tui::footer::draw(frame, chunks[3], app, None);
}

/// The biggest things counted so far.
///
/// These are finished directories, not running estimates: each number is final
/// for that folder. The list is incomplete, never wrong.
fn draw_found(frame: &mut Frame, area: Rect, app: &App) {
    if app.streamed.is_empty() || area.height < 3 {
        return;
    }

    let unit = app.settings.unit;
    let rows = (area.height as usize)
        .saturating_sub(2)
        .min(app.streamed.len());
    let size_width = app
        .streamed
        .iter()
        .take(rows)
        .map(|done| format(done.allocated, unit).chars().count())
        .max()
        .unwrap_or(8);
    let path_width = (area.width as usize).saturating_sub(size_width + 6);

    let mut lines = vec![
        Line::default(),
        Line::from(Span::styled("  Biggest so far", theme::muted())),
    ];
    lines.extend(app.streamed.iter().take(rows).map(|done| {
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("{:>size_width$}", format(done.allocated, unit)),
                theme::heading(),
            ),
            Span::raw("   "),
            Span::styled(
                shorten_path(&model::display_path(&done.path), path_width),
                theme::secondary(),
            ),
        ])
    }));

    frame.render_widget(Paragraph::new(lines), area);
}
