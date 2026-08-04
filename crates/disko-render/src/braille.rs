//! A monochrome 2×4-per-cell dot canvas.
//!
//! Braille packs four times the vertical resolution of half-blocks, so curves
//! come out noticeably smoother — at the cost of a single colour per cell.
//! That trade is right for outlines, which is where disko uses it: the radial
//! view falls back to braille arcs when colour is off.

/// Which bit in the braille pattern each (column, row) dot sets. The bottom
/// row is 0x40/0x80 because braille was 2×3 before the 8-dot extension.
const DOT_BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

const BRAILLE_BASE: u32 = 0x2800;

#[derive(Clone, Debug)]
pub struct BrailleCanvas {
    width_cells: usize,
    height_cells: usize,
    cells: Vec<u8>,
}

impl BrailleCanvas {
    /// Sized in terminal cells; the dot grid is `width * 2 × height * 4`.
    pub fn new(width_cells: usize, height_cells: usize) -> Self {
        Self {
            width_cells,
            height_cells,
            cells: vec![0; width_cells * height_cells],
        }
    }

    pub fn width(&self) -> usize {
        self.width_cells * 2
    }

    pub fn height(&self) -> usize {
        self.height_cells * 4
    }

    pub fn set(&mut self, x: usize, y: usize) {
        if x >= self.width() || y >= self.height() {
            return;
        }
        let index = (y / 4) * self.width_cells + (x / 2);
        self.cells[index] |= DOT_BITS[x % 2][y % 4];
    }

    pub fn is_set(&self, x: usize, y: usize) -> bool {
        if x >= self.width() || y >= self.height() {
            return false;
        }
        let index = (y / 4) * self.width_cells + (x / 2);
        self.cells[index] & DOT_BITS[x % 2][y % 4] != 0
    }

    pub fn clear(&mut self) {
        self.cells.fill(0);
    }

    /// One string per cell row. Blank cells become spaces rather than the
    /// empty braille pattern, which some fonts render at the wrong width.
    pub fn lines(&self) -> Vec<String> {
        self.cells
            .chunks(self.width_cells)
            .map(|row| {
                row.iter()
                    .map(|&bits| {
                        if bits == 0 {
                            ' '
                        } else {
                            char::from_u32(BRAILLE_BASE + bits as u32).unwrap_or(' ')
                        }
                    })
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_resolution_is_two_by_four_per_cell() {
        let canvas = BrailleCanvas::new(3, 2);
        assert_eq!(canvas.width(), 6);
        assert_eq!(canvas.height(), 8);
        assert_eq!(canvas.lines().len(), 2);
    }

    #[test]
    fn a_single_dot_renders_as_the_top_left_pattern() {
        let mut canvas = BrailleCanvas::new(1, 1);
        canvas.set(0, 0);
        assert_eq!(canvas.lines()[0], "⠁");
    }

    #[test]
    fn all_eight_dots_render_as_a_full_cell() {
        let mut canvas = BrailleCanvas::new(1, 1);
        for x in 0..2 {
            for y in 0..4 {
                canvas.set(x, y);
            }
        }
        assert_eq!(canvas.lines()[0], "⣿");
    }

    #[test]
    fn empty_cells_are_spaces() {
        let canvas = BrailleCanvas::new(2, 1);
        assert_eq!(canvas.lines()[0], "  ");
    }

    #[test]
    fn out_of_bounds_dots_are_ignored() {
        let mut canvas = BrailleCanvas::new(1, 1);
        canvas.set(9, 9);
        assert!(!canvas.is_set(9, 9));
        assert_eq!(canvas.lines()[0], " ");
    }
}
