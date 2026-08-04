//! The details panel — everything `duf` shows by default and disko does not.

use disko_core::size::{format, format_percent_whole};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::model;
use crate::tui::app::App;
use crate::tui::theme;

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

    if let Some(entry) = app.current_entry() {
        rows.push((String::new(), String::new()));
        rows.push(("Folder".into(), model::display_path(&entry.path)));
        rows.push((
            "Size".into(),
            format(entry.size(app.settings.size_kind), unit),
        ));
        rows.push(("Items".into(), entry.items.to_string()));
        rows.push((
            "Counting".into(),
            match app.settings.size_kind {
                disko_core::SizeKind::Allocated => "blocks used on disk".into(),
                disko_core::SizeKind::Apparent => "apparent file sizes".into(),
            },
        ));
    }

    if rows.is_empty() {
        return;
    }

    let popup = centered(area, 62, rows.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(
            rows.into_iter()
                .map(|(label, value)| {
                    Line::from(vec![
                        Span::styled(format!(" {label:<11}"), theme::muted()),
                        Span::raw(value),
                    ])
                })
                .collect::<Vec<_>>(),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Details — d to close "),
        ),
        popup,
    );
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
