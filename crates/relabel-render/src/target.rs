//! Where the label is going, and the exact arithmetic for getting there.

use crate::error::RenderError;
use relabel_proto::PrintDirection;

/// Dots per millimetre, exactly.
///
/// A rational rather than a float because the conversion runs on every
/// geometry value and the two resolutions in the whole catalogue are exactly
/// 8 dots/mm at 203 dpi and 3000/254 at 300 dpi. `relabel-proto`'s own helper
/// uses a truncated decimal constant, which happens never to change an answer
/// for any model in the vendor table - but a document format should not depend
/// on a happens-never, so conversions in this crate go through here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dpmm {
    pub num: u32,
    pub den: u32,
}

impl Dpmm {
    pub const DPI_203: Dpmm = Dpmm { num: 8, den: 1 };
    pub const DPI_300: Dpmm = Dpmm {
        num: 3000,
        den: 254,
    };

    pub const fn for_dpi(dpi: u16) -> Dpmm {
        if dpi >= 300 {
            Dpmm::DPI_300
        } else {
            Dpmm::DPI_203
        }
    }

    /// Tenths of a millimetre to whole dots, rounding half away from zero.
    ///
    /// Tenths rather than millimetres because label dimensions are quoted in
    /// halves and tenths, and an integer input keeps the whole path exact.
    /// `i64` intermediates: 3000 times a length in tenths overflows `i32` at
    /// about 71 cm, which a continuous roll can exceed.
    pub fn dots_from_tenths_mm(self, tenths: i32) -> i32 {
        let n = i64::from(tenths) * i64::from(self.num);
        let d = i64::from(self.den) * 10;
        let r = if n >= 0 {
            (n + d / 2) / d
        } else {
            (n - d / 2) / d
        };
        r as i32
    }

    /// Whole dots back to tenths of a millimetre, for display only.
    pub fn tenths_mm_from_dots(self, dots: i32) -> i32 {
        let n = i64::from(dots) * i64::from(self.den) * 10;
        let d = i64::from(self.num);
        let r = if n >= 0 {
            (n + d / 2) / d
        } else {
            (n - d / 2) / d
        };
        r as i32
    }

    /// Millimetres, for a readout. Never for storage.
    pub fn mm_from_dots(self, dots: i32) -> f32 {
        f64::from(dots) as f32 * self.den as f32 / self.num as f32
    }
}

/// Everything the rasteriser is allowed to know about the destination.
///
/// Sized in **canvas orientation**: the way the finished label reads, which is
/// how the operator lays it out. That is not the same as the way it goes down
/// the wire. A printer whose head lies along the short axis of the media - the
/// whole D11/D110/D101 family - takes a landscape strip whose *height* is the
/// head, and the encoder rotates it on the way out.
///
/// So `width_px` is not "the head" and `height_px` is not "the feed". Which is
/// which depends on the model's print direction, and
/// [`for_label`](Self::for_label) is the only thing that should be deciding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub width_px: u16,
    pub height_px: u16,
    pub dpi: u16,
}

impl Target {
    pub fn new(width_px: u16, height_px: u16, dpi: u16) -> Result<Target, RenderError> {
        if width_px == 0 || height_px == 0 {
            return Err(RenderError::EmptyTarget {
                w: width_px,
                h: height_px,
            });
        }
        Ok(Target {
            width_px,
            height_px,
            dpi,
        })
    }

    /// The canvas for a label of `length_px` along the feed, on a print head of
    /// `head_px` dots, for a printer with the given direction.
    ///
    /// This is where the axis swap lives, once.
    pub fn for_label(
        head_px: u16,
        length_px: u16,
        dpi: u16,
        direction: PrintDirection,
    ) -> Result<Target, RenderError> {
        match direction {
            // The head runs across the top of the canvas.
            PrintDirection::Top => Target::new(head_px, length_px, dpi),
            // The head runs down the side, so the label reads landscape.
            PrintDirection::Left => Target::new(length_px, head_px, dpi),
        }
    }

    pub fn dpmm(&self) -> Dpmm {
        Dpmm::for_dpi(self.dpi)
    }

    /// Which axis will be handed to the print head once the encoder has
    /// rotated the plane.
    pub fn head_axis(&self, direction: PrintDirection) -> u16 {
        match direction {
            PrintDirection::Top => self.width_px,
            PrintDirection::Left => self.height_px,
        }
    }

    /// The other axis: how far the paper travels.
    pub fn feed_axis(&self, direction: PrintDirection) -> u16 {
        match direction {
            PrintDirection::Top => self.height_px,
            PrintDirection::Left => self.width_px,
        }
    }

    /// Check the canvas exactly matches a print head.
    ///
    /// Equality rather than a bound, and the reason is specific to the rotated
    /// case: the encoder rotates before sending, so a canvas *narrower* than
    /// the head does not print inset with even margins - the padding all ends
    /// up on one side and the label prints shifted. See
    /// [`RenderError::HeadAxisMismatch`].
    pub fn require_head(
        &self,
        printhead_px: u16,
        direction: PrintDirection,
    ) -> Result<(), RenderError> {
        let got = self.head_axis(direction);
        if got != printhead_px {
            return Err(RenderError::HeadAxisMismatch {
                got,
                want: printhead_px,
            });
        }
        Ok(())
    }

    /// The label's extent along the feed, in tenths of a millimetre.
    pub fn feed_tenths_mm(&self, direction: PrintDirection) -> i32 {
        self.dpmm()
            .tenths_mm_from_dots(i32::from(self.feed_axis(direction)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_label_is_exact_at_203_dpi() {
        let d = Dpmm::for_dpi(203);
        assert_eq!(d, Dpmm::DPI_203);
        assert_eq!(d.dots_from_tenths_mm(400), 320, "40 mm");
        assert_eq!(d.dots_from_tenths_mm(120), 96, "12 mm");
        assert_eq!(d.tenths_mm_from_dots(320), 400);
        assert_eq!(d.tenths_mm_from_dots(96), 120);
    }

    /// 300 dpi is where a float constant would start to drift, so the rational
    /// is checked against values computed by hand.
    #[test]
    fn three_hundred_dpi_is_exact_too() {
        let d = Dpmm::for_dpi(300);
        // 40 mm * 3000/254 = 472.44 -> 472
        assert_eq!(d.dots_from_tenths_mm(400), 472);
        // 50 mm -> 590.55 -> 591, which is the published B1 Pro head width.
        assert_eq!(d.dots_from_tenths_mm(500), 591);
        // 25.4 mm is exactly 300 dots, with no rounding at all.
        assert_eq!(d.dots_from_tenths_mm(254), 300);
    }

    #[test]
    fn rounding_is_half_away_from_zero_in_both_directions() {
        let d = Dpmm::DPI_203;
        // 0.0625 mm is exactly half a dot.
        assert_eq!(d.dots_from_tenths_mm(1), 1, "0.1 mm rounds up to 1 dot");
        assert_eq!(d.dots_from_tenths_mm(-1), -1);
        assert_eq!(d.dots_from_tenths_mm(0), 0);
    }

    /// A long continuous roll must not overflow the intermediate.
    #[test]
    fn a_metre_of_paper_does_not_overflow() {
        let d = Dpmm::DPI_300;
        assert_eq!(d.dots_from_tenths_mm(10_000), 11_811, "1 m at 300 dpi");
    }

    #[test]
    fn a_target_must_have_area() {
        assert!(Target::new(0, 320, 203).is_err());
        assert!(Target::new(96, 0, 203).is_err());
        assert!(Target::new(320, 96, 203).is_ok());
    }

    /// The axis swap, which is the whole reason `for_label` exists. A D110-class
    /// printer takes a landscape strip; a B1-class one does not.
    #[test]
    fn the_canvas_is_laid_out_the_way_the_label_reads() {
        let d110 = Target::for_label(96, 320, 203, PrintDirection::Left).unwrap();
        assert_eq!(
            (d110.width_px, d110.height_px),
            (320, 96),
            "landscape strip"
        );
        assert_eq!(d110.head_axis(PrintDirection::Left), 96);
        assert_eq!(d110.feed_axis(PrintDirection::Left), 320);

        let b1 = Target::for_label(384, 320, 203, PrintDirection::Top).unwrap();
        assert_eq!((b1.width_px, b1.height_px), (384, 320));
        assert_eq!(b1.head_axis(PrintDirection::Top), 384);
    }

    /// A canvas that does not match the head does not print inset, it prints
    /// shifted, because the encoder rotates before sending.
    #[test]
    fn the_head_axis_must_match_exactly() {
        let t = Target::new(320, 92, 203).unwrap();
        assert_eq!(
            t.require_head(96, PrintDirection::Left),
            Err(RenderError::HeadAxisMismatch { got: 92, want: 96 })
        );
        let ok = Target::for_label(96, 320, 203, PrintDirection::Left).unwrap();
        assert!(ok.require_head(96, PrintDirection::Left).is_ok());
    }

    #[test]
    fn feed_length_is_reported_in_tenths_of_a_millimetre() {
        let t = Target::for_label(96, 320, 203, PrintDirection::Left).unwrap();
        assert_eq!(t.feed_tenths_mm(PrintDirection::Left), 400, "40.0 mm");
    }
}
