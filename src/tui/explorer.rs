//! The radial explorer: a sunburst of the current directory, with a legend
//! tying every wedge to a name and a size.

use disko_core::size::format;
use disko_render::radial::{self, LayoutOptions, RenderOptions, Segment};
use disko_render::{BrailleCanvas, Canvas, palette};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::model::Row;
use crate::tui::app::App;
use crate::tui::text::{fit, width};
use crate::tui::theme;

/// Three rings is the sweet spot: enough to see structure two levels down,
/// few enough that the outer ring is still thick enough to read.
const RINGS: usize = 3;

const LEGEND_WIDTH: u16 = 34;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // breadcrumb
        Constraint::Length(1),
        Constraint::Min(6), // chart and legend
        Constraint::Length(1),
    ])
    .horizontal_margin(1)
    .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled(app.breadcrumb(), theme::heading())),
        chunks[0],
    );

    let legend_width = LEGEND_WIDTH.min(chunks[2].width.saturating_sub(20));
    let body = Layout::horizontal([Constraint::Min(20), Constraint::Length(legend_width)])
        .split(chunks[2]);

    let rows = app.rows();
    draw_chart(frame, body[0], app, &rows);
    if legend_width > 0 {
        draw_legend(frame, body[1], app, &rows);
    }

    crate::tui::footer::draw(frame, chunks[3], app);
}

fn draw_chart(frame: &mut Frame, area: Rect, app: &App, rows: &[Row]) {
    if area.width < 8 || area.height < 4 {
        return;
    }

    let (root, ids) = app.radial_tree(RINGS);
    let segments = radial::layout(
        &root,
        &LayoutOptions {
            rings: RINGS,
            ..Default::default()
        },
    );

    // The selected row and the highlighted wedge are the same thing seen two
    // ways, so they are looked up from one source: the selected path.
    let selected_id = rows
        .get(app.selection)
        .and_then(|row| row.path.as_ref())
        .and_then(|path| ids.iter().position(|candidate| candidate == path));

    let options = RenderOptions {
        rings: RINGS,
        selected: selected_id,
        ..Default::default()
    };

    let lines = if theme::colors_disabled() {
        outline_lines(&segments, area, &options)
    } else {
        color_lines(&segments, area, &options)
    };

    frame.render_widget(Paragraph::new(lines), area);
    draw_hole(frame, area, app);
}

fn color_lines(segments: &[Segment], area: Rect, options: &RenderOptions) -> Vec<Line<'static>> {
    let mut canvas = Canvas::new(area.width as usize, area.height as usize);
    radial::render(segments, &mut canvas, options);

    canvas
        .cell_rows()
        .into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|cell| match (cell.top, cell.bottom) {
                        (None, None) => Span::raw(" "),
                        (Some(top), None) => {
                            Span::styled("▀", Style::default().fg(theme::color(top)))
                        }
                        (None, Some(bottom)) => {
                            Span::styled("▄", Style::default().fg(theme::color(bottom)))
                        }
                        (Some(top), Some(bottom)) => Span::styled(
                            "▀",
                            Style::default()
                                .fg(theme::color(top))
                                .bg(theme::color(bottom)),
                        ),
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn outline_lines(segments: &[Segment], area: Rect, options: &RenderOptions) -> Vec<Line<'static>> {
    let mut canvas = BrailleCanvas::new(area.width as usize, area.height as usize);
    radial::render_outline(segments, &mut canvas, options);
    canvas.lines().into_iter().map(Line::from).collect()
}

/// The hole in the middle carries the total for the directory you are in —
/// the number the whole chart is a breakdown of.
fn draw_hole(frame: &mut Frame, area: Rect, app: &App) {
    let Some(entry) = app.current_entry() else {
        return;
    };

    let name = entry.name().to_string();
    let size = format(entry.size(app.settings.size_kind), app.settings.unit);
    let box_width = (width(&name).max(width(&size)) as u16 + 2).min(area.width);
    if box_width == 0 || area.height < 4 {
        return;
    }

    let hole = Rect {
        x: area.x + area.width.saturating_sub(box_width) / 2,
        y: area.y + area.height / 2 - 1,
        width: box_width,
        height: 2,
    };

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(size, theme::heading())),
            Line::from(Span::styled(name, theme::muted())),
        ])
        .alignment(Alignment::Center),
        hole,
    );
}

fn draw_legend(frame: &mut Frame, area: Rect, app: &App, rows: &[Row]) {
    let unit = app.settings.unit;
    let capacity = area.height as usize;
    if capacity == 0 || rows.is_empty() {
        return;
    }

    // Keep the selection on screen when the legend is shorter than the list.
    let offset = app.selection.saturating_sub(capacity.saturating_sub(1));
    let visible = rows.iter().skip(offset).take(capacity);

    let size_width = rows
        .iter()
        .map(|row| width(&format(row.size, unit)))
        .max()
        .unwrap_or(8);
    let name_width = (area.width as usize).saturating_sub(size_width + 5);

    let lines: Vec<Line> = visible
        .enumerate()
        .map(|(index, row)| {
            let color = palette::categorical(row.color_index);
            let is_selected = offset + index == app.selection;
            let marked = row
                .path
                .as_ref()
                .is_some_and(|path| app.marks.contains(path));

            let name_style = if is_selected {
                theme::selected()
            } else {
                Style::default()
            };

            Line::from(vec![
                Span::styled("■ ", Style::default().fg(theme::color(color))),
                Span::styled(fit(&row.name, name_width), name_style),
                Span::raw(" "),
                Span::styled(
                    format!("{:>size_width$}", format(row.size, unit)),
                    theme::secondary(),
                ),
                Span::styled(if marked { " ✓" } else { "" }, theme::accent()),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}
