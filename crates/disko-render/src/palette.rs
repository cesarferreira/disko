//! Colours for the radial view and the capacity bar.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// The Okabe–Ito qualitative palette: eight hues chosen to stay distinct for
/// the common forms of colour blindness, which matters more here than usual
/// because a segment's colour is the only thing tying it to its legend row.
pub const CATEGORICAL: [Rgb; 8] = [
    Rgb::new(0, 114, 178),   // blue
    Rgb::new(230, 159, 0),   // orange
    Rgb::new(0, 158, 115),   // bluish green
    Rgb::new(204, 121, 167), // reddish purple
    Rgb::new(86, 180, 233),  // sky blue
    Rgb::new(213, 94, 0),    // vermillion
    Rgb::new(240, 228, 66),  // yellow
    Rgb::new(140, 140, 148), // neutral grey
];

/// Wraps, so a directory with more than eight children still gets a colour.
pub fn categorical(index: usize) -> Rgb {
    CATEGORICAL[index % CATEGORICAL.len()]
}

/// `factor < 1.0` darkens toward black, `> 1.0` lightens toward white.
///
/// Deeper rings are lighter shades of their parent, which is what makes a
/// sunburst readable as a hierarchy rather than a pile of unrelated wedges.
pub fn shade(color: Rgb, factor: f32) -> Rgb {
    let blend = |channel: u8| -> u8 {
        let value = channel as f32;
        let shaded = if factor <= 1.0 {
            value * factor
        } else {
            value + (255.0 - value) * (factor - 1.0).min(1.0)
        };
        shaded.clamp(0.0, 255.0) as u8
    };
    Rgb::new(blend(color.r), blend(color.g), blend(color.b))
}

pub const CAPACITY_OK: Rgb = Rgb::new(0, 158, 115);
pub const CAPACITY_WARN: Rgb = Rgb::new(230, 159, 0);
pub const CAPACITY_CRITICAL: Rgb = Rgb::new(213, 94, 0);

/// Green until the disk is worth thinking about, amber at 75%, red at 90%.
pub fn capacity_color(used_fraction: f64) -> Rgb {
    if used_fraction >= 0.90 {
        CAPACITY_CRITICAL
    } else if used_fraction >= 0.75 {
        CAPACITY_WARN
    } else {
        CAPACITY_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorical_colours_wrap() {
        assert_eq!(categorical(0), categorical(8));
        assert_eq!(categorical(3), CATEGORICAL[3]);
    }

    #[test]
    fn shading_moves_toward_black_and_white() {
        let base = Rgb::new(100, 100, 100);
        assert_eq!(shade(base, 0.5), Rgb::new(50, 50, 50));
        assert_eq!(shade(base, 1.0), base);
        let lighter = shade(base, 1.5);
        assert!(lighter.r > base.r && lighter.r < 255);
        assert_eq!(shade(base, 2.0), Rgb::new(255, 255, 255));
    }

    #[test]
    fn capacity_colour_escalates() {
        assert_eq!(capacity_color(0.10), CAPACITY_OK);
        assert_eq!(capacity_color(0.80), CAPACITY_WARN);
        assert_eq!(capacity_color(0.95), CAPACITY_CRITICAL);
    }
}
