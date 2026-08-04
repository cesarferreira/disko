//! The default screen: what is full, what is using the space, where to look
//! next — and nothing else.

use disko_core::size::{format, format_percent, format_percent_whole};
use disko_core::{DiskEntry, ScanState};
use disko_render::{bar, palette};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::model::{self, Row};
use crate::report::signed;
use crate::tui::app::App;
use crate::tui::text::{fit, width};
use crate::tui::theme;

/// Below this the percentage column is the first thing to go: the bar and the
/// absolute size already carry the message.
const PERCENT_MIN_WIDTH: u16 = 76;

const BAR_MAX_WIDTH: usize = 32;

/// Four is enough to point somewhere without becoming a second ranked list.
const LARGEST_ITEMS: usize = 4;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, list_state: &mut ListState) {
    let rows = app.rows();
    let (list_height, largest_height) = split_body(app, area, rows.len());

    let chunks = Layout::vertical([
        Constraint::Length(1), // volume and capacity
        Constraint::Length(1), // capacity bar
        Constraint::Length(1),
        Constraint::Length(1), // current folder
        Constraint::Length(1),
        Constraint::Length(list_height), // ranked entries
        Constraint::Length(largest_height),
        Constraint::Min(0),    // slack, so both blocks sit under the header
        Constraint::Length(1), // footer
    ])
    .horizontal_margin(1)
    .split(area);

    draw_capacity(frame, chunks[0], chunks[1], app);
    draw_current_folder(frame, chunks[3], app);
    draw_rows(frame, chunks[5], app, &rows, list_state);
    if largest_height > 0 {
        draw_largest(frame, chunks[6], app);
    }
    crate::tui::footer::draw(frame, chunks[8], app);
}

/// Divide the space between the header and the footer.
///
/// The ranked list is the answer and gets what it needs first; "Largest items"
/// is the follow-up question and only appears if there is room left for a
/// heading and at least two entries.
fn split_body(app: &App, area: Rect, row_count: usize) -> (u16, u16) {
    const HEADER: usize = 5;
    const FOOTER: usize = 1;
    const LARGEST_MIN: usize = 4; // blank, heading, two entries

    let body = (area.height as usize).saturating_sub(HEADER + FOOTER);
    if body == 0 {
        return (0, 0);
    }

    let wanted = row_count.max(1);
    let largest_available = body.saturating_sub(wanted);
    let largest = if largest_available >= LARGEST_MIN
        && app.current_entry().is_some()
        && !app.showing_growth()
    {
        largest_available.min(LARGEST_ITEMS + 2)
    } else {
        0
    };

    let list = wanted.min(body - largest);
    (list as u16, largest as u16)
}

/// The header always describes the *filesystem*, never the directory being
/// explored. Conflating the two is the single most confusing thing a disk tool
/// can do.
fn draw_capacity(frame: &mut Frame, name_area: Rect, bar_area: Rect, app: &App) {
    let unit = app.settings.unit;

    let (name, right) = match &app.filesystem {
        Some(fs) => (
            fs.name.clone(),
            format!(
                "{} used of {}",
                format(fs.used, unit),
                format(fs.total, unit)
            ),
        ),
        None => (model::display_path(&app.root), String::new()),
    };

    let total = name_area.width as usize;
    let gap = total.saturating_sub(width(&name) + width(&right));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(name, theme::heading()),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, theme::secondary()),
        ])),
        name_area,
    );

    let Some(fs) = &app.filesystem else { return };
    let fraction = fs.used_fraction();
    let percent = format_percent_whole(fraction);
    let gauge_width = (bar_area.width as usize)
        .saturating_sub(width(&percent) + 2)
        .min(64);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                bar::gauge(fraction, gauge_width),
                Style::default().fg(theme::color(palette::capacity_color(fraction))),
            ),
            Span::raw("  "),
            Span::styled(percent, theme::secondary()),
        ])),
        bar_area,
    );
}

/// And this line always describes the directory, so the two numbers can never
/// be mistaken for each other.
fn draw_current_folder(frame: &mut Frame, area: Rect, app: &App) {
    let Some(entry) = app.current_entry() else {
        return;
    };
    let unit = app.settings.unit;

    let mut spans = vec![
        Span::styled("Current folder: ", theme::muted()),
        Span::raw(model::display_path(&entry.path)),
        Span::raw("    "),
        Span::styled(
            format(entry.size(app.settings.size_kind), unit),
            theme::accent(),
        ),
    ];

    // When growth is on screen, say how much and over what period — a column
    // of "+2 GB" means nothing without knowing whether that was an hour or a
    // month.
    if let (true, Some(diff)) = (app.showing_growth(), &app.diff) {
        spans.push(Span::raw("    "));
        spans.push(Span::styled(
            format!(
                "{} in {}",
                signed(diff.total_delta(), unit),
                crate::timefmt::humanize(diff.elapsed())
            ),
            Style::default().fg(theme::color(palette::growth_color(
                diff.total_delta(),
                diff.total_delta().abs().max(1),
            ))),
        ));
    }

    // Numbers from a snapshot are real, just old. Saying so is the whole
    // difference between showing them and misleading with them.
    if let Some(taken_at) = app.provisional {
        let age = disko_core::history::now().saturating_sub(taken_at);
        spans.push(Span::raw("    "));
        spans.push(Span::styled(
            format!("as of {} · rescanning…", crate::timefmt::ago(age)),
            theme::warning(),
        ));
    } else if let Some(note) = incomplete_note(entry) {
        spans.push(Span::raw("    "));
        spans.push(Span::styled(note, theme::warning()));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Says so out loud when the numbers are a floor rather than a total.
fn incomplete_note(entry: &DiskEntry) -> Option<String> {
    match entry.scan_state {
        ScanState::Complete => None,
        ScanState::Denied => Some("unreadable".to_string()),
        ScanState::Cancelled => Some("scan cancelled — sizes incomplete".to_string()),
        ScanState::Skipped => Some("not scanned — network mount".to_string()),
        ScanState::Partial => Some("some folders skipped or unreadable".to_string()),
    }
}

fn draw_rows(frame: &mut Frame, area: Rect, app: &App, rows: &[Row], list_state: &mut ListState) {
    if rows.is_empty() {
        let message = if app.search.is_some() {
            "nothing matches that search"
        } else if app.showing_growth() {
            "nothing here changed"
        } else {
            "this folder is empty"
        };
        frame.render_widget(Paragraph::new(Span::styled(message, theme::muted())), area);
        return;
    }

    let unit = app.settings.unit;
    let growth = app.showing_growth();
    let show_percent = area.width >= PERCENT_MIN_WIDTH && !growth;

    // In growth mode the leading column is the signed change, which is what
    // the whole view exists to show; the current size moves to the right.
    let amount = |row: &Row| -> String {
        if growth {
            signed(row.delta.unwrap_or(0), unit)
        } else {
            format(row.size, unit)
        }
    };

    let size_width = rows
        .iter()
        .map(|row| width(&amount(row)))
        .max()
        .unwrap_or(8);
    let name_width = rows
        .iter()
        .map(|row| width(&row.name))
        .max()
        .unwrap_or(12)
        .clamp(10, 28);
    let percent_width = if show_percent {
        7
    } else if growth {
        11
    } else {
        0
    };
    let scale = rows
        .iter()
        .map(|row| row.delta.unwrap_or(0).abs())
        .max()
        .unwrap_or(1)
        .max(1);

    // Marker, size, name, then whatever is left goes to the bar.
    let fixed = 2 + size_width + 2 + name_width + 2 + percent_width;
    let bar_width = (area.width as usize)
        .saturating_sub(fixed + 1)
        .min(BAR_MAX_WIDTH);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let color = if growth {
                palette::growth_color(row.delta.unwrap_or(0), scale)
            } else if row.is_other() {
                palette::CATEGORICAL[7]
            } else {
                palette::categorical(row.color_index)
            };
            let marked = row
                .path
                .as_ref()
                .is_some_and(|path| app.marks.contains(path));

            let mut spans = vec![
                Span::styled(if marked { "✓ " } else { "  " }, theme::accent()),
                Span::styled(format!("{:>size_width$}", amount(row)), theme::heading()),
                Span::raw("  "),
                Span::raw(fit(&row.name, name_width)),
                Span::raw("  "),
                Span::styled(
                    fit(&bar::bar(row.fraction, bar_width), bar_width),
                    Style::default().fg(theme::color(color)),
                ),
            ];
            if show_percent {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("{:>5}", format_percent(row.fraction)),
                    theme::muted(),
                ));
            } else if growth {
                // What it is now, so a "+2 GB" has something to be 2 GB of.
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("{:>9}", format(row.size, unit)),
                    theme::muted(),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    list_state.select(Some(app.selection.min(rows.len() - 1)));
    frame.render_stateful_widget(
        List::new(items).highlight_style(theme::selected()),
        area,
        list_state,
    );
}

fn draw_largest(frame: &mut Frame, area: Rect, app: &App) {
    // In growth mode the ranked list already answers "where should I look
    // next"; a second list of merely-large things would compete with it.
    if app.showing_growth() {
        return;
    }
    let Some(entry) = app.current_entry() else {
        return;
    };
    // One line goes to the blank separator and one to the heading.
    let count = (area.height as usize).saturating_sub(2);
    let items = model::largest_items(entry, count, app.settings.size_kind);
    if items.is_empty() {
        return;
    }

    let unit = app.settings.unit;
    let size_width = items
        .iter()
        .map(|item| width(&format(item.size(app.settings.size_kind), unit)))
        .max()
        .unwrap_or(8);

    let mut lines = vec![
        Line::default(),
        Line::from(Span::styled("Largest items", theme::muted())),
    ];
    lines.extend(items.iter().map(|item| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "{:>size_width$}",
                    format(item.size(app.settings.size_kind), unit)
                ),
                theme::heading(),
            ),
            Span::raw("   "),
            Span::styled(model::display_path(&item.path), theme::secondary()),
        ])
    }));

    frame.render_widget(Paragraph::new(lines), area);
}
