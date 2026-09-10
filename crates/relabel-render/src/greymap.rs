//! Ink coverage, and the zone each dot belongs to.

use crate::halftone::{BinariseOpts, Zone, binarise};
use relabel_proto::BitPlane;

/// One byte of ink coverage per dot, 0 fully inked and 255 bare paper, and one
/// byte of zone id per dot.
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
    /// Row-major, `w * h` zone ids. 0 is the page; each element is painted into
    /// its own id so that halftoning error never travels between elements. See
    /// [`crate::halftone`].
    pub zone: Vec<u8>,
}

impl GreyMap {
    pub fn new(w: u16, h: u16) -> GreyMap {
        let n = usize::from(w) * usize::from(h);
        GreyMap {
            w,
            h,
            px: vec![255; n],
            zone: vec![0; n],
        }
    }

    /// Wrap existing coverage, all in the page zone.
    ///
    /// A buffer of the wrong length is padded with paper or cut short rather
    /// than refused, so an image decoded to a slightly different size cannot
    /// turn into a panic here.
    pub fn from_px(w: u16, h: u16, mut px: Vec<u8>) -> GreyMap {
        let n = usize::from(w) * usize::from(h);
        px.resize(n, 255);
        GreyMap {
            w,
            h,
            px,
            zone: vec![0; n],
        }
    }

    #[inline]
    pub fn at(&self, x: u16, y: u16) -> u8 {
        if x >= self.w || y >= self.h {
            return 255;
        }
        self.px
            .get(usize::from(y) * usize::from(self.w) + usize::from(x))
            .copied()
            .unwrap_or(255)
    }

    /// Reduce to one bit with a hard threshold: anything at or below `t` is
    /// black.
    ///
    /// The same rule the halftoning module uses - this is that module with a
    /// single thresholding zone - so there is exactly one definition of "is
    /// this dot ink" in the crate.
    pub fn threshold(&self, t: u8) -> BitPlane {
        let zones = [Zone {
            threshold: t,
            ..Zone::PAGE
        }];
        binarise(self, &BinariseOpts::with_zones(&zones))
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

    #[test]
    fn a_wrong_length_buffer_is_normalised() {
        let g = GreyMap::from_px(4, 2, vec![0, 0, 0]);
        assert_eq!(g.px.len(), 8);
        assert_eq!(g.at(3, 1), 255, "padded with paper");
        assert_eq!(g.zone.len(), 8);
    }
}
