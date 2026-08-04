//! Sunburst layout and rasterisation.
//!
//! Layout is pure geometry over sizes, deliberately knowing nothing about
//! files: callers hand in [`RadialNode`]s carrying their own ids, and get back
//! [`Segment`]s they can map straight back to whatever those ids meant.

use std::f64::consts::TAU;

use crate::braille::BrailleCanvas;
use crate::canvas::Canvas;
use crate::palette::{self, Rgb};

/// Input to [`layout`]. `id` is opaque — the caller decides what it points at.
#[derive(Clone, Debug)]
pub struct RadialNode {
    pub id: usize,
    pub label: String,
    pub size: u64,
    /// Overrides the palette for this wedge. The growth view uses it to colour
    /// by what changed instead of by which child it is.
    pub color: Option<Rgb>,
    pub children: Vec<RadialNode>,
}

impl RadialNode {
    pub fn leaf(id: usize, label: impl Into<String>, size: u64) -> Self {
        Self {
            id,
            label: label.into(),
            size,
            color: None,
            children: Vec::new(),
        }
    }
}

/// A placed wedge. Angles are in turns: `0.0` is twelve o'clock and they run
/// clockwise, so `0.25` is three o'clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub id: usize,
    /// Ring number, 1 for the innermost.
    pub depth: usize,
    pub start: f64,
    pub end: f64,
    pub color: Rgb,
    pub label: String,
    pub size: u64,
}

impl Segment {
    pub fn span(&self) -> f64 {
        self.end - self.start
    }

    pub fn midpoint(&self) -> f64 {
        (self.start + self.end) / 2.0
    }

    pub fn contains(&self, turn: f64) -> bool {
        turn >= self.start && turn < self.end
    }
}

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    /// How many levels deep to draw.
    pub rings: usize,
    /// Wedges thinner than this many turns are dropped: below roughly a
    /// thousandth of a circle they are narrower than a pixel and only add
    /// speckle.
    pub min_span: f64,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            rings: 3,
            min_span: 0.0015,
        }
    }
}

/// Place `root`'s descendants around the circle. `root` itself is the hole in
/// the middle and gets no segment.
pub fn layout(root: &RadialNode, options: &LayoutOptions) -> Vec<Segment> {
    let mut segments = Vec::new();
    place(
        &root.children,
        0.0,
        1.0,
        root.size,
        1,
        None,
        options,
        &mut segments,
    );
    segments
}

#[allow(clippy::too_many_arguments)]
fn place(
    nodes: &[RadialNode],
    parent_start: f64,
    parent_span: f64,
    parent_size: u64,
    depth: usize,
    parent_color: Option<Rgb>,
    options: &LayoutOptions,
    out: &mut Vec<Segment>,
) {
    if depth > options.rings || parent_size == 0 || parent_span <= 0.0 {
        return;
    }

    let mut cursor = parent_start;
    for (index, node) in nodes.iter().enumerate() {
        let span = (node.size as f64 / parent_size as f64) * parent_span;

        // Top-level children each get their own hue; deeper rings are lighter
        // shades of their parent, alternating slightly so neighbouring
        // siblings do not melt into one another.
        let color = match (node.color, parent_color) {
            (Some(explicit), _) => explicit,
            (None, None) => palette::categorical(index),
            (None, Some(parent)) => {
                palette::shade(parent, 1.18 + if index % 2 == 0 { 0.0 } else { 0.10 })
            }
        };

        if span >= options.min_span {
            out.push(Segment {
                id: node.id,
                depth,
                start: cursor,
                end: cursor + span,
                color,
                label: node.label.clone(),
                size: node.size,
            });
        }

        place(
            &node.children,
            cursor,
            span,
            node.size,
            depth + 1,
            Some(color),
            options,
            out,
        );
        cursor += span;
    }
}

#[derive(Clone, Debug)]
pub struct RenderOptions {
    /// Radius of the hole in the middle, as a fraction of the outer radius.
    /// The hole carries the current directory's total.
    pub inner_radius: f64,
    pub rings: usize,
    /// Segment id to highlight; everything else dims.
    pub selected: Option<usize>,
    /// Blank pixels left between rings.
    pub ring_gap: f64,
    /// Blank pixels left between neighbouring wedges.
    pub segment_gap: f64,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            inner_radius: 0.34,
            rings: 3,
            selected: None,
            ring_gap: 0.6,
            segment_gap: 0.8,
        }
    }
}

/// Geometry shared by the rasteriser and the label placement, so a label
/// always lands on the wedge it names.
struct Geometry {
    center_x: f64,
    center_y: f64,
    outer: f64,
    inner: f64,
    ring_thickness: f64,
}

impl Geometry {
    fn new(width: usize, height: usize, options: &RenderOptions) -> Self {
        let center_x = width as f64 / 2.0;
        let center_y = height as f64 / 2.0;
        let outer = (center_x.min(center_y) - 1.0).max(1.0);
        let inner = outer * options.inner_radius;
        let rings = options.rings.max(1) as f64;
        Self {
            center_x,
            center_y,
            outer,
            inner,
            ring_thickness: (outer - inner) / rings,
        }
    }

    fn radius_of_ring(&self, depth: usize) -> f64 {
        self.inner + (depth as f64 - 0.5) * self.ring_thickness
    }
}

/// Angle of a point in turns, clockwise from twelve o'clock.
fn turn_of(dx: f64, dy: f64) -> f64 {
    (dx.atan2(-dy).rem_euclid(TAU)) / TAU
}

/// Paint `segments` into `canvas`. Pixels outside the disc are left alone, so
/// a caller can compose the sunburst over something else.
pub fn render(segments: &[Segment], canvas: &mut Canvas, options: &RenderOptions) {
    let geometry = Geometry::new(canvas.width(), canvas.height(), options);
    if geometry.ring_thickness <= 0.0 {
        return;
    }

    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            let dx = x as f64 + 0.5 - geometry.center_x;
            let dy = y as f64 + 0.5 - geometry.center_y;
            let radius = (dx * dx + dy * dy).sqrt();

            if radius < geometry.inner || radius > geometry.outer {
                continue;
            }

            let depth = ((radius - geometry.inner) / geometry.ring_thickness).floor() as usize + 1;
            if depth > options.rings {
                continue;
            }

            // Leave the outer sliver of each ring blank so rings read as
            // separate bands instead of one gradient.
            let into_ring =
                (radius - geometry.inner) - (depth - 1) as f64 * geometry.ring_thickness;
            if into_ring > geometry.ring_thickness - options.ring_gap {
                continue;
            }

            let turn = turn_of(dx, dy);
            let Some(segment) = segments
                .iter()
                .find(|s| s.depth == depth && s.contains(turn))
            else {
                continue;
            };

            // A fixed pixel gap is a shrinking angle as the radius grows, so
            // the divider stays one pixel wide on every ring.
            let padding = options.segment_gap / (TAU * radius);
            if turn - segment.start < padding || segment.end - turn < padding {
                continue;
            }

            canvas.set(x, y, shade_for(segment, options));
        }
    }
}

fn shade_for(segment: &Segment, options: &RenderOptions) -> Rgb {
    match options.selected {
        Some(id) if id == segment.id => palette::shade(segment.color, 1.45),
        Some(_) => palette::shade(segment.color, 0.55),
        None => segment.color,
    }
}

/// Draw ring arcs and wedge dividers as dots. The monochrome fallback for
/// terminals without truecolor, and a cleaner look on light backgrounds.
pub fn render_outline(segments: &[Segment], canvas: &mut BrailleCanvas, options: &RenderOptions) {
    let geometry = Geometry::new(canvas.width(), canvas.height(), options);
    if geometry.ring_thickness <= 0.0 {
        return;
    }

    let plot = |radius: f64, turn: f64, canvas: &mut BrailleCanvas| {
        let angle = turn * TAU;
        let x = geometry.center_x + radius * angle.sin();
        let y = geometry.center_y - radius * angle.cos();
        if x >= 0.0 && y >= 0.0 {
            canvas.set(x as usize, y as usize);
        }
    };

    for segment in segments {
        let outer = geometry.inner + segment.depth as f64 * geometry.ring_thickness;
        let inner = outer - geometry.ring_thickness;

        // One step per dot along the arc, so the curve has no gaps.
        let arc_length = segment.span() * TAU * outer;
        let steps = (arc_length.ceil() as usize).max(1);
        for step in 0..=steps {
            let turn = segment.start + segment.span() * step as f64 / steps as f64;
            plot(outer, turn, canvas);
        }

        for step in 0..=(geometry.ring_thickness.ceil() as usize) {
            let radius = inner + step as f64;
            plot(radius.min(outer), segment.start, canvas);
        }
    }
}

/// Where a segment's label belongs, in canvas pixels — the middle of its arc.
/// `None` when the wedge is too narrow for text to fit.
pub fn label_anchor(
    segment: &Segment,
    canvas: &Canvas,
    options: &RenderOptions,
    label_width: usize,
) -> Option<(usize, usize)> {
    let geometry = Geometry::new(canvas.width(), canvas.height(), options);
    let radius = geometry.radius_of_ring(segment.depth);

    // Arc length at this radius, in cells. Pixels are one cell wide.
    let arc_cells = segment.span() * TAU * radius;
    if arc_cells < label_width as f64 {
        return None;
    }

    let angle = segment.midpoint() * TAU;
    let x = geometry.center_x + radius * angle.sin();
    let y = geometry.center_y - radius * angle.cos();
    if x < 0.0 || y < 0.0 || x >= canvas.width() as f64 || y >= canvas.height() as f64 {
        return None;
    }
    Some((x as usize, y as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: usize, size: u64, children: Vec<RadialNode>) -> RadialNode {
        RadialNode {
            id,
            label: format!("n{id}"),
            size,
            color: None,
            children,
        }
    }

    #[test]
    fn top_level_wedges_tile_the_whole_circle() {
        let root = node(0, 100, vec![node(1, 60, vec![]), node(2, 40, vec![])]);
        let segments = layout(&root, &LayoutOptions::default());

        assert_eq!(segments.len(), 2);
        assert!((segments[0].start - 0.0).abs() < 1e-9);
        assert!((segments[0].end - 0.6).abs() < 1e-9);
        assert!((segments[1].start - 0.6).abs() < 1e-9);
        assert!((segments[1].end - 1.0).abs() < 1e-9);
    }

    #[test]
    fn children_stay_inside_their_parents_wedge() {
        let root = node(0, 100, vec![node(1, 50, vec![node(2, 25, vec![])])]);
        let segments = layout(&root, &LayoutOptions::default());

        let parent = &segments[0];
        let child = segments.iter().find(|s| s.id == 2).unwrap();
        assert_eq!(child.depth, 2);
        assert!(child.start >= parent.start && child.end <= parent.end + 1e-9);
        // Half of a half is a quarter of the circle.
        assert!((child.span() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn an_explicit_colour_overrides_the_palette() {
        let mut root = node(0, 100, vec![node(1, 100, vec![])]);
        let chosen = Rgb::new(1, 2, 3);
        root.children[0].color = Some(chosen);

        let segments = layout(&root, &LayoutOptions::default());
        assert_eq!(segments[0].color, chosen);
    }

    #[test]
    fn rings_beyond_the_limit_are_not_placed() {
        let root = node(
            0,
            100,
            vec![node(1, 100, vec![node(2, 100, vec![node(3, 100, vec![])])])],
        );
        let segments = layout(
            &root,
            &LayoutOptions {
                rings: 2,
                ..Default::default()
            },
        );

        assert_eq!(segments.len(), 2);
        assert!(segments.iter().all(|s| s.depth <= 2));
    }

    #[test]
    fn slivers_are_dropped_but_still_shift_their_siblings() {
        let root = node(0, 10_000, vec![node(1, 9_999, vec![]), node(2, 1, vec![])]);
        let segments = layout(&root, &LayoutOptions::default());

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].id, 1);
    }

    #[test]
    fn an_empty_parent_places_nothing() {
        let root = node(0, 0, vec![node(1, 0, vec![])]);
        assert!(layout(&root, &LayoutOptions::default()).is_empty());
    }

    #[test]
    fn twelve_oclock_is_zero_and_angles_run_clockwise() {
        assert!((turn_of(0.0, -1.0) - 0.0).abs() < 1e-9);
        assert!((turn_of(1.0, 0.0) - 0.25).abs() < 1e-9);
        assert!((turn_of(0.0, 1.0) - 0.5).abs() < 1e-9);
        assert!((turn_of(-1.0, 0.0) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn rendering_fills_the_ring_and_leaves_the_hole_empty() {
        let root = node(0, 100, vec![node(1, 100, vec![])]);
        let segments = layout(&root, &LayoutOptions::default());
        let mut canvas = Canvas::new(40, 20);
        render(&segments, &mut canvas, &RenderOptions::default());

        let center = (canvas.width() / 2, canvas.height() / 2);
        assert!(
            canvas.get(center.0, center.1).is_none(),
            "the hole should stay empty"
        );

        let painted = (0..canvas.height())
            .flat_map(|y| (0..canvas.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.get(x, y).is_some())
            .count();
        assert!(
            painted > 100,
            "expected a visible ring, painted {painted} pixels"
        );
    }

    #[test]
    fn selection_dims_everything_else() {
        let root = node(0, 100, vec![node(1, 50, vec![]), node(2, 50, vec![])]);
        let segments = layout(&root, &LayoutOptions::default());
        let options = RenderOptions {
            selected: Some(1),
            ..Default::default()
        };

        let selected = shade_for(&segments[0], &options);
        let other = shade_for(&segments[1], &options);
        assert_ne!(selected, segments[0].color);
        assert!(other.r < segments[1].color.r.max(1));
    }

    #[test]
    fn narrow_wedges_get_no_label() {
        let canvas = Canvas::new(40, 20);
        let wide = Segment {
            id: 1,
            depth: 1,
            start: 0.0,
            end: 0.9,
            color: Rgb::new(1, 2, 3),
            label: "wide".into(),
            size: 90,
        };
        let narrow = Segment {
            end: 0.02,
            ..wide.clone()
        };
        let options = RenderOptions::default();

        assert!(label_anchor(&wide, &canvas, &options, 4).is_some());
        assert!(label_anchor(&narrow, &canvas, &options, 4).is_none());
    }

    #[test]
    fn outlines_draw_dots_without_panicking() {
        let root = node(
            0,
            100,
            vec![node(1, 70, vec![node(2, 30, vec![])]), node(3, 30, vec![])],
        );
        let segments = layout(&root, &LayoutOptions::default());
        let mut canvas = BrailleCanvas::new(40, 10);
        render_outline(&segments, &mut canvas, &RenderOptions::default());

        let dots = canvas
            .lines()
            .join("")
            .chars()
            .filter(|c| *c != ' ')
            .count();
        assert!(dots > 10, "expected outline dots, got {dots}");
    }
}
