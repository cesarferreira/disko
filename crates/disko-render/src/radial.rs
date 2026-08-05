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
    /// One id that is placed however thin it is. The wedge under the cursor
    /// has to exist even when it is a sliver, or there is nothing for the
    /// highlight to point at.
    pub pinned: Option<usize>,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            rings: 3,
            min_span: 0.0015,
            pinned: None,
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

        if span >= options.min_span || options.pinned == Some(node.id) {
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

/// How much lighter the selected wedge is drawn, and how much darker every
/// other one goes to get out of its way.
const SELECTED_SHADE: f32 = 1.45;
const DIMMED_SHADE: f32 = 0.55;

/// The narrowest a highlight is ever drawn, in pixels of arc. Most wedges in a
/// large directory are thinner than a pixel, and a highlight nobody can see is
/// the same as no highlight at all.
const MIN_HIGHLIGHT_PIXELS: f64 = 3.0;

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

/// What the rasteriser was able to say about the selection at the size it drew.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Drawn {
    /// Set when the selected wedge is thinner than a pixel, so its highlight
    /// stands for the run of too-small wedges around it rather than for that
    /// one entry. Say so on screen: the chart cannot tell them apart, and
    /// pretending otherwise is what makes the cursor look broken.
    pub selection_is_a_run: bool,
}

/// Where the highlight goes: one slice of one ring.
struct Highlight {
    depth: usize,
    middle: f64,
    half_span: f64,
    color: Rgb,
    /// Whether the band stands for more than the selected wedge alone.
    is_a_run: bool,
}

impl Highlight {
    /// The band for the selected wedge.
    ///
    /// A wedge wide enough to see gets exactly its own arc. One that is not —
    /// which in a large directory is most of them — gets the whole contiguous
    /// run of siblings that are also too small, so the highlight says "somewhere
    /// in here" instead of pointing a three-pixel finger at a random neighbour.
    fn resolve(segments: &[Segment], geometry: &Geometry, selected: Option<usize>) -> Option<Self> {
        let id = selected?;
        let selected = segments.iter().find(|segment| segment.id == id)?;
        let radius = geometry.radius_of_ring(selected.depth);
        let visible = |segment: &Segment| segment.span() * TAU * radius >= MIN_HIGHLIGHT_PIXELS;

        // The wedges sharing this ring, in the order they were placed around it.
        let ring: Vec<&Segment> = segments
            .iter()
            .filter(|segment| segment.depth == selected.depth)
            .collect();
        let at = ring.iter().position(|segment| segment.id == id)?;

        let (mut first, mut last) = (at, at);
        let is_a_run = !visible(selected);
        if is_a_run {
            while first > 0 && !visible(ring[first - 1]) {
                first -= 1;
            }
            while last + 1 < ring.len() && !visible(ring[last + 1]) {
                last += 1;
            }
        }

        // Even a run can come to less than a pixel, so the floor still applies.
        let span = ring[last].end - ring[first].start;
        let floor = MIN_HIGHLIGHT_PIXELS / (TAU * radius);
        Some(Self {
            depth: selected.depth,
            middle: (ring[first].start + ring[last].end) / 2.0,
            half_span: span.max(floor) / 2.0,
            color: palette::shade(selected.color, SELECTED_SHADE),
            is_a_run,
        })
    }

    /// Whether a pixel is inside the highlighted slice.
    ///
    /// The slice runs outward: the selected wedge's arc *and* everything drawn
    /// beyond it in the same arc, which is what it contains. Lighting only the
    /// one ring leaves the rest of the slice dimmed, and the chart then looks
    /// like it is ignoring most of the cursor's moves.
    ///
    /// Turns wrap at twelve o'clock, so the distance between two of them is
    /// whichever way round the circle is shorter.
    fn covers(&self, depth: usize, turn: f64) -> bool {
        let gap = (turn - self.middle).abs();
        depth >= self.depth && gap.min(1.0 - gap) <= self.half_span
    }
}

/// Paint `segments` into `canvas`. Pixels outside the disc are left alone, so
/// a caller can compose the sunburst over something else.
pub fn render(segments: &[Segment], canvas: &mut Canvas, options: &RenderOptions) -> Drawn {
    let geometry = Geometry::new(canvas.width(), canvas.height(), options);
    if geometry.ring_thickness <= 0.0 {
        return Drawn::default();
    }

    let highlight = Highlight::resolve(segments, &geometry, options.selected);
    let drawn = Drawn {
        selection_is_a_run: highlight
            .as_ref()
            .is_some_and(|highlight| highlight.is_a_run),
    };

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
            let inside = highlight
                .as_ref()
                .is_some_and(|highlight| highlight.covers(depth, turn));

            // The selected wedge's own ring is painted straight over the
            // dividers: one widened to three pixels has nothing to spare for a
            // gap either side of it. Its contents, further out, keep their
            // dividers — that structure is the point of drawing them.
            if let Some(highlight) = &highlight
                && inside
                && depth == highlight.depth
            {
                canvas.set(x, y, highlight.color);
                continue;
            }

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

            canvas.set(x, y, shade_for(segment, inside, options));
        }
    }

    drawn
}

/// Deeper rings are already lighter shades of their parent, so brightening them
/// again would wash them into each other. Inside the slice they keep their true
/// colour, and the contrast comes from everything else stepping back.
fn shade_for(segment: &Segment, inside_the_slice: bool, options: &RenderOptions) -> Rgb {
    match options.selected {
        None => segment.color,
        Some(id) if id == segment.id => palette::shade(segment.color, SELECTED_SHADE),
        Some(_) if inside_the_slice => segment.color,
        Some(_) => palette::shade(segment.color, DIMMED_SHADE),
    }
}

/// Draw ring arcs and wedge dividers as dots. The monochrome fallback for
/// terminals without truecolor, and a cleaner look on light backgrounds.
pub fn render_outline(
    segments: &[Segment],
    canvas: &mut BrailleCanvas,
    options: &RenderOptions,
) -> Drawn {
    let geometry = Geometry::new(canvas.width(), canvas.height(), options);
    if geometry.ring_thickness <= 0.0 {
        return Drawn::default();
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

    // Dots carry no colour, so there is no highlight here — but the selection
    // is just as unplaceable, and the caller says so the same way.
    Drawn {
        selection_is_a_run: Highlight::resolve(segments, &geometry, options.selected)
            .is_some_and(|highlight| highlight.is_a_run),
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
    fn a_pinned_sliver_is_placed_anyway() {
        let root = node(0, 10_000, vec![node(1, 9_999, vec![]), node(2, 1, vec![])]);
        let segments = layout(
            &root,
            &LayoutOptions {
                pinned: Some(2),
                ..Default::default()
            },
        );

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].id, 2);
    }

    /// The reported bug: selecting anything past the biggest few entries dimmed
    /// the whole chart and lit nothing up, because the wedge it named was
    /// thinner than a pixel.
    #[test]
    fn selecting_a_sliver_still_lights_pixels_up() {
        let root = node(0, 10_000, vec![node(1, 9_999, vec![]), node(2, 1, vec![])]);
        let segments = layout(
            &root,
            &LayoutOptions {
                pinned: Some(2),
                ..Default::default()
            },
        );
        let mut canvas = Canvas::new(40, 20);
        render(
            &segments,
            &mut canvas,
            &RenderOptions {
                selected: Some(2),
                ..Default::default()
            },
        );

        let wanted = palette::shade(segments[1].color, SELECTED_SHADE);
        let lit = (0..canvas.height())
            .flat_map(|y| (0..canvas.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.get(x, y) == Some(wanted))
            .count();
        assert!(lit > 0, "the selected sliver should still be visible");
    }

    /// Entries under about a percent share one pixel of arc, so a highlight on
    /// one of them can only honestly mean "somewhere in this run". The band
    /// covers the run, and the caller is told so it can say as much.
    #[test]
    fn a_run_of_too_small_wedges_is_highlighted_together() {
        let tail: Vec<RadialNode> = (1..=6).map(|index| node(index, 20, vec![])).collect();
        let mut children = vec![node(100, 9_880, vec![])];
        children.extend(tail);
        let root = node(0, 10_000, children);
        let segments = layout(&root, &LayoutOptions::default());

        // Pixels painted in the selected wedge's own highlight colour.
        let painted = |selected: usize| {
            let mut canvas = Canvas::new(40, 20);
            let drawn = render(
                &segments,
                &mut canvas,
                &RenderOptions {
                    selected: Some(selected),
                    ..Default::default()
                },
            );
            let wanted = palette::shade(
                segments.iter().find(|s| s.id == selected).unwrap().color,
                SELECTED_SHADE,
            );
            let lit = (0..canvas.height())
                .flat_map(|y| (0..canvas.width()).map(move |x| (x, y)))
                .filter(|&(x, y)| canvas.get(x, y) == Some(wanted))
                .count();
            (drawn, lit)
        };

        let (big, _) = painted(100);
        assert!(
            !big.selection_is_a_run,
            "a wedge filling the circle is placeable on its own"
        );

        let (small, lit) = painted(3);
        assert!(small.selection_is_a_run, "a 0.2% wedge is not placeable");
        // One of these wedges is a fraction of a pixel of arc; what got painted
        // is the run of six around it.
        assert!(lit >= 3, "expected a visible band, painted {lit} pixels");
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

        let selected = shade_for(&segments[0], true, &options);
        let other = shade_for(&segments[1], false, &options);
        assert_ne!(selected, segments[0].color);
        assert!(other.r < segments[1].color.r.max(1));

        // What a selected wedge contains is part of the same slice: dimming it
        // would leave the cursor lighting up one band out of three.
        let contained = Segment {
            id: 99,
            depth: 2,
            ..segments[0].clone()
        };
        assert_eq!(shade_for(&contained, true, &options), contained.color);
        assert_ne!(shade_for(&contained, false, &options), contained.color);
    }

    /// What the legend's cursor picks out is a slice of the cheese, not one arc
    /// of it: the wedge and everything drawn beyond it in the same arc.
    #[test]
    fn the_selected_slice_lights_up_all_the_way_out() {
        let root = node(
            0,
            100,
            vec![
                node(1, 50, vec![node(2, 50, vec![node(3, 50, vec![])])]),
                node(4, 50, vec![node(5, 50, vec![])]),
            ],
        );
        let segments = layout(&root, &LayoutOptions::default());
        let mut canvas = Canvas::new(60, 30);
        render(
            &segments,
            &mut canvas,
            &RenderOptions {
                selected: Some(1),
                ..Default::default()
            },
        );

        // Every ring inside the selected arc keeps a true colour; the arc next
        // to it is dimmed at every ring.
        for depth in 1..=3 {
            let segment = segments.iter().find(|s| s.depth == depth).unwrap();
            let lit = painted_with(&canvas, segment.color) > 0
                || painted_with(&canvas, palette::shade(segment.color, SELECTED_SHADE)) > 0;
            assert!(lit, "ring {depth} of the selected slice stayed dark");
        }
        for id in [4, 5] {
            let segment = segments.iter().find(|s| s.id == id).unwrap();
            assert_eq!(
                painted_with(&canvas, segment.color),
                0,
                "wedge {id} is outside the slice and should be dimmed"
            );
            assert!(
                painted_with(&canvas, palette::shade(segment.color, DIMMED_SHADE)) > 0,
                "wedge {id} should still be drawn, just dimmed"
            );
        }
    }

    fn painted_with(canvas: &Canvas, color: Rgb) -> usize {
        (0..canvas.height())
            .flat_map(|y| (0..canvas.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.get(x, y) == Some(color))
            .count()
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
