//! A colour raster that fits inside terminal cells.
//!
//! Each cell holds two stacked pixels, drawn as `▀` with the top pixel as the
//! foreground and the bottom as the background. That doubles vertical
//! resolution *and* keeps a colour per pixel — braille would give 2×4 pixels
//! but only one colour per cell, which a sunburst cannot use.
//!
//! Terminal cells are about twice as tall as they are wide, so these
//! half-height pixels come out roughly square and circles look like circles.

use crate::palette::Rgb;

/// One terminal cell: two vertically stacked pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct HalfCell {
    pub top: Option<Rgb>,
    pub bottom: Option<Rgb>,
}

impl HalfCell {
    pub fn is_empty(&self) -> bool {
        self.top.is_none() && self.bottom.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<Option<Rgb>>,
}

impl Canvas {
    /// Sized in terminal cells; the pixel grid is `width × height * 2`.
    pub fn new(width_cells: usize, height_cells: usize) -> Self {
        let width = width_cells;
        let height = height_cells * 2;
        Self {
            width,
            height,
            pixels: vec![None; width * height],
        }
    }

    /// Pixels across, which equals the cell width.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Pixels down, which is twice the cell height.
    pub fn height(&self) -> usize {
        self.height
    }

    pub fn set(&mut self, x: usize, y: usize, color: Rgb) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = Some(color);
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Option<Rgb> {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x]
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.pixels.fill(None);
    }

    /// One row of cells per two rows of pixels, ready to hand to a terminal.
    pub fn cell_rows(&self) -> Vec<Vec<HalfCell>> {
        (0..self.height / 2)
            .map(|row| {
                (0..self.width)
                    .map(|x| HalfCell {
                        top: self.get(x, row * 2),
                        bottom: self.get(x, row * 2 + 1),
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
    fn a_canvas_is_twice_as_tall_in_pixels_as_in_cells() {
        let canvas = Canvas::new(10, 4);
        assert_eq!(canvas.width(), 10);
        assert_eq!(canvas.height(), 8);
        assert_eq!(canvas.cell_rows().len(), 4);
    }

    #[test]
    fn pixels_pair_up_into_cells() {
        let mut canvas = Canvas::new(2, 1);
        let red = Rgb::new(255, 0, 0);
        let blue = Rgb::new(0, 0, 255);
        canvas.set(0, 0, red);
        canvas.set(0, 1, blue);

        let rows = canvas.cell_rows();
        assert_eq!(
            rows[0][0],
            HalfCell {
                top: Some(red),
                bottom: Some(blue)
            }
        );
        assert!(rows[0][1].is_empty());
    }

    #[test]
    fn out_of_bounds_writes_are_ignored() {
        let mut canvas = Canvas::new(2, 1);
        canvas.set(99, 99, Rgb::new(1, 2, 3));
        assert!(canvas.cell_rows()[0].iter().all(HalfCell::is_empty));
    }
}
