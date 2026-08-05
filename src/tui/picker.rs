//! The disk chooser shown when `disko` is run with no path.

use disko_core::size::{format, format_percent_whole};
use disko_render::{bar, palette};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::tui::app::App;
use crate::tui::text::{fit, width};
use crate::tui::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, list_state: &mut ListState) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .horizontal_margin(1)
    .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled("Disks", theme::heading())),
        chunks[0],
    );

    if app.filesystems.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no mounted filesystems found — try disko --all",
                theme::muted(),
            )),
            chunks[2],
        );
        crate::tui::footer::draw(frame, chunks[3], app, None);
        return;
    }

    let unit = app.settings.unit;
    let name_width = app
        .filesystems
        .iter()
        .map(|fs| width(&fs.name))
        .max()
        .unwrap_or(12)
        .clamp(8, 24);
    let gauge_width = (chunks[2].width as usize)
        .saturating_sub(name_width + 34)
        .clamp(8, 28);

    let items: Vec<ListItem> = app
        .filesystems
        .iter()
        .map(|fs| {
            let fraction = fs.used_fraction();
            ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(fit(&fs.name, name_width), theme::heading()),
                Span::raw("  "),
                Span::styled(
                    bar::gauge(fraction, gauge_width),
                    Style::default().fg(theme::color(palette::capacity_color(fraction))),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:>4}", format_percent_whole(fraction)),
                    theme::secondary(),
                ),
                Span::raw("   "),
                Span::styled(
                    format!(
                        "{} free of {}",
                        format(fs.available, unit),
                        format(fs.total, unit)
                    ),
                    theme::muted(),
                ),
            ]))
        })
        .collect();

    list_state.select(Some(app.picker_index.min(app.filesystems.len() - 1)));
    frame.render_stateful_widget(
        List::new(items).highlight_style(theme::selected()),
        chunks[2],
        list_state,
    );

    crate::tui::footer::draw(frame, chunks[3], app, None);
}
