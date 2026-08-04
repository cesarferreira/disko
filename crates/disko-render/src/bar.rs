//! Horizontal bars built from block characters.

/// One eighth through eight eighths of a cell.
const EIGHTHS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

const TRACK: char = '░';

/// A bare bar with no track, sub-cell accurate: `bar(0.187, 32)` fills just
/// under six cells and ends on a partial block.
///
/// Anything above zero renders at least a sliver — a row that has a size but
/// no bar at all reads as a bug.
pub fn bar(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let fraction = fraction.clamp(0.0, 1.0);
    let mut eighths = (fraction * width as f64 * 8.0).round() as usize;
    if eighths == 0 && fraction > 0.0 {
        eighths = 1;
    }
    eighths = eighths.min(width * 8);

    let full = eighths / 8;
    let remainder = eighths % 8;

    let mut out = String::with_capacity(width * 3);
    for _ in 0..full {
        out.push(EIGHTHS[7]);
    }
    if remainder > 0 {
        out.push(EIGHTHS[remainder - 1]);
    }
    out
}

/// A bar over a visible track, always exactly `width` cells wide. Used for the
/// capacity header, where the empty part is the point.
pub fn gauge(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let fraction = fraction.clamp(0.0, 1.0);
    let mut filled = (fraction * width as f64).round() as usize;
    if filled == 0 && fraction > 0.0 {
        filled = 1;
    }
    // A disk at 99.6% should not draw as completely full.
    if filled == width && fraction < 1.0 {
        filled = width - 1;
    }

    let mut out = String::with_capacity(width * 3);
    for _ in 0..filled {
        out.push(EIGHTHS[7]);
    }
    for _ in filled..width {
        out.push(TRACK);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(s: &str) -> usize {
        s.chars().count()
    }

    #[test]
    fn bars_never_exceed_their_width() {
        for percent in 0..=100 {
            let rendered = bar(percent as f64 / 100.0, 20);
            assert!(cells(&rendered) <= 20, "{percent}% overflowed: {rendered}");
        }
    }

    #[test]
    fn a_full_bar_is_solid() {
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(bar(0.5, 4), "██");
    }

    #[test]
    fn tiny_values_still_show_something() {
        assert_eq!(bar(0.0, 10), "");
        assert_eq!(bar(0.0001, 10), "▏");
    }

    #[test]
    fn sub_cell_precision_uses_partial_blocks() {
        // 0.25 of 3 cells is 0.75 of a cell: six eighths.
        assert_eq!(bar(0.25, 3), "▊");
    }

    #[test]
    fn gauges_always_fill_their_width() {
        for percent in 0..=100 {
            assert_eq!(cells(&gauge(percent as f64 / 100.0, 24)), 24);
        }
    }

    #[test]
    fn a_nearly_full_gauge_keeps_one_empty_cell() {
        assert!(gauge(0.999, 10).ends_with(TRACK));
        assert_eq!(gauge(1.0, 10), "██████████");
    }
}
