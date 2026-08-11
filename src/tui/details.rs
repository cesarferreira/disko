//! The details panel — everything `duf` shows by default and disko does not.

use disko_core::categories;
use disko_core::size::{format, format_percent_whole};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::deletion::Outcome;
use crate::model;
use crate::tui::app::App;
use crate::tui::{text, theme};

/// How wide the panel would like to be, before the terminal has its say.
const PANEL_WIDTH: u16 = 66;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let mut rows: Vec<(String, String)> = Vec::new();
    let unit = app.settings.unit;

    if let Some(fs) = &app.filesystem {
        rows.push(("Volume".into(), fs.name.clone()));
        rows.push(("Mount".into(), fs.mount_point.display().to_string()));
        rows.push(("Device".into(), fs.device.clone()));
        rows.push(("Filesystem".into(), fs.fs_type.clone()));
        rows.push(("Read-only".into(), yes_no(fs.read_only)));
        rows.push(("Removable".into(), yes_no(fs.removable)));
        rows.push(("Kind".into(), fs.kind.clone()));
        rows.push((
            "Capacity".into(),
            format!(
                "{} used of {} ({})",
                format(fs.used, unit),
                format(fs.total, unit),
                format_percent_whole(fs.used_fraction())
            ),
        ));
        rows.push(("Available".into(), format(fs.available, unit)));
        if let Some(inodes) = &fs.inodes {
            rows.push((
                "Inodes".into(),
                format!(
                    "{} used of {} ({})",
                    inodes.used,
                    inodes.total,
                    format_percent_whole(inodes.used_fraction())
                ),
            ));
        }
    }

    // The panel describes whatever is selected, falling back to the folder
    // being explored: "what is this and what has it been doing".
    let focus = app
        .selected_row()
        .and_then(|row| row.path)
        .or_else(|| app.current_entry().map(|_| app.cwd.clone()));

    if let Some(path) = &focus {
        rows.push((String::new(), String::new()));
        rows.push(("Path".into(), model::display_path(path)));

        if let Some(entry) = app.tree.as_ref().and_then(|tree| tree.resolve(path)) {
            rows.push((
                "Current size".into(),
                format(entry.size(app.settings.size_kind), unit),
            ));
            rows.push(("Items".into(), entry.items.to_string()));
            if entry.modified > 0 {
                rows.push((
                    "Last used".into(),
                    crate::timefmt::ago(disko_core::history::now().saturating_sub(entry.modified)),
                ));
            }
        }

        if let Some(diff) = &app.diff
            && let Some(change) = diff.change_for(path)
        {
            rows.push((
                format!("Change ({})", crate::timefmt::humanize(diff.elapsed())),
                crate::report::signed(change.delta, unit),
            ));
            rows.push((
                "Was".into(),
                if change.before_is_bound {
                    format!("under {}", format(diff_floor(diff), unit))
                } else {
                    format(change.before, unit)
                },
            ));
        }

        // Naming what produced something is most of the value in deciding
        // whether to delete it.
        if let Some(rule) = categories::classify(path) {
            rows.push(("Category".into(), rule.label.to_string()));
            rows.push(("Reclaiming".into(), rule.safety.label().to_string()));
            if let Some(command) = rule.regenerate {
                rows.push(("Regenerate".into(), command.to_string()));
            }
        }

        rows.push((
            "Counting".into(),
            match app.settings.size_kind {
                disko_core::SizeKind::Allocated => "blocks used on disk".into(),
                disko_core::SizeKind::Apparent => "apparent file sizes".into(),
            },
        ));
    }

    if rows.is_empty() && app.outcomes.is_empty() {
        return;
    }

    // The report of the last deletion sits above the description, separated by
    // a blank line, so both are counted when sizing the popup.
    let leading = if app.outcomes.is_empty() {
        0
    } else {
        app.outcomes.len() + 1
    };
    let popup = centered(area, PANEL_WIDTH, (leading + rows.len()) as u16 + 2);
    frame.render_widget(Clear, popup);

    let mut lines = outcome_lines(&app.outcomes, popup.width.saturating_sub(2) as usize);
    if leading > 0 {
        lines.push(Line::default());
    }
    lines.extend(rows.into_iter().map(|(label, value)| {
        Line::from(vec![
            Span::styled(format!(" {label:<11} "), theme::muted()),
            Span::raw(value),
        ])
    }));

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(
            if app.outcomes.is_empty() {
                " Details — d to close "
            } else {
                " Details — last deletion — d to close "
            },
        )),
        popup,
    );
}

/// One line per target: what happened to it, and to which path.
///
/// The reason is the whole point of the report — a refusal nobody can read is
/// no better than a silent one — so it claims its width first and the paths
/// share what is left, losing their middles if they must.
fn outcome_lines(outcomes: &[Outcome], inner: usize) -> Vec<Line<'static>> {
    if outcomes.is_empty() {
        return Vec::new();
    }
    // A leading space, and one between the two columns.
    let room = inner.saturating_sub(2);
    // Even the longest reason leaves the path something to work with.
    let reasons: Vec<String> = outcomes
        .iter()
        .map(|outcome| text::truncate(&outcome.detail(), room.saturating_sub(12).max(1)))
        .collect();
    let widest = reasons
        .iter()
        .map(|reason| text::width(reason))
        .max()
        .unwrap_or(0);
    let paths = room.saturating_sub(widest).max(1);

    outcomes
        .iter()
        .zip(reasons)
        .map(|(outcome, reason)| {
            let path = text::shorten_path(&model::display_path(outcome.path()), paths);
            let style = if outcome.succeeded() {
                theme::muted()
            } else {
                theme::warning()
            };
            Line::from(vec![
                Span::styled(format!(" {} ", text::pad(&path, paths)), theme::muted()),
                Span::styled(reason, style),
            ])
        })
        .collect()
}

/// The snapshot noise floor, recovered from any entry the diff knows was
/// bounded by it.
fn diff_floor(diff: &disko_core::Diff) -> u64 {
    // Snapshots keep entries down to a ten-thousandth of the root.
    (diff.before_total / 10_000).max(1_000_000)
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}
