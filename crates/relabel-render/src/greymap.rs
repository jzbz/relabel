//! Ink coverage, and the reduction to one bit.

use relabel_proto::BitPlane;

/// One byte of ink coverage per dot: 0 fully inked, 255 bare paper.
///
/// Deliberately not a `tiny_skia::Pixmap`. Keeping the rasteriser out of the
/// public API means the halftoning code, the tests and any future caller work
/// on a plain buffer, and the crate can change how it draws without changing
/// what it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreyMap {
    pub w: u16,
    pub h: u16,
    /// Row-major, `w * h` bytes.
    pub px: Vec<u8>,
}

impl GreyMap {
    pub fn new(w: u16, h: u16) -> GreyMap {
        GreyMap {
            w,
            h,
            px: vec![255; usize::from(w) * usize::from(h)],
        }
    }

    #[inline]
    pub fn at(&self, x: u16, y: u16) -> u8 {
        if x >= self.w || y >= self.h {
            return 255;
        }
        self.px[usize::from(y) * usize::from(self.w) + usize::from(x)]
    }

    /// Reduce to one bit with a hard threshold: anything at or below `t` is
    /// black.
    ///
    /// The right choice for line art, rules, barcodes and 2D modules, all of
    /// which are already exactly 0 or 255 and so come through bit-exact
    /// whatever the threshold is. Photographs want a diffusion kernel instead;
    /// that arrives with the halftoning module, along with the per-element
    /// isolation that stops diffusion error leaking out of a photograph and
    /// across a barcode.
    pub fn threshold(&self, t: u8) -> BitPlane {
        let mut plane = BitPlane::new(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.at(x, y) <= t {
                    plane.set(x, y, true);
                }
            }
        }
        plane
    }

    /// How much of the canvas is inked, as a fraction.
    ///
    /// Cheap, and worth showing: a label above about half coverage is slow to
    /// print and hard on the head.
    pub fn coverage(&self, t: u8) -> f32 {
        if self.px.is_empty() {
            return 0.0;
        }
        let inked = self.px.iter().filter(|p| **p <= t).count();
        inked as f32 / self.px.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_inclusive_at_the_boundary() {
        let mut g = GreyMap::new(4, 1);
        g.px = vec![0, 128, 129, 255];
        let p = g.threshold(128);
        assert!(p.get(0, 0));
        assert!(p.get(1, 0), "exactly at the threshold counts as ink");
        assert!(!p.get(2, 0));
        assert!(!p.get(3, 0));
    }

    #[test]
    fn reading_outside_the_map_is_paper_not_a_panic() {
        let g = GreyMap::new(2, 2);
        assert_eq!(g.at(99, 0), 255);
        assert_eq!(g.at(0, 99), 255);
    }

    #[test]
    fn coverage_counts_inked_dots() {
        let mut g = GreyMap::new(4, 1);
        g.px = vec![0, 0, 255, 255];
        assert_eq!(g.coverage(128), 0.5);
    }
}
