//! The drawing surface, and the two paints allowed on it.
//!
//! # Why a pixmap and not a mask
//!
//! `tiny_skia::Mask` is the obvious choice for a monochrome renderer - one byte
//! per pixel, no colour to throw away - and it is the wrong one. It has no
//! `fill_rect` and no `stroke_path`, so every rectangle would become a path,
//! and paths take a different route through the rasteriser with different
//! degenerate-bounds behaviour. A 32-bit buffer for a 320x96 label is 120 KB;
//! the correctness is worth more than the memory.
//!
//! # Why every draw goes through a wrapper
//!
//! Three properties of the rasteriser matter here, and all three are lost if a
//! call site talks to it directly:
//!
//! - **`fill_rect` silently degrades.** Given any transform that is not the
//!   identity it converts the rectangle to a path and fills that instead, which
//!   changes the snapping. So the transform is always the identity and geometry
//!   is transformed by us, in whole dots, beforehand.
//! - **Anti-aliasing defaults to on.** For a barcode module or a rule that is
//!   wrong: a half-lit module is a scan failure. There are exactly two paints,
//!   [`hard`](Canvas::hard) and [`soft`](Canvas::soft), and only glyph outlines
//!   get the soft one.
//! - **Rounding is a floor, not a round.** A caller passing a fractional dot has
//!   usually made an arithmetic mistake, so this crate refuses fractions rather
//!   than quietly flooring them.

use crate::coverage::Coverage;
use crate::document::Ink;
use crate::error::RenderError;
use crate::greymap::GreyMap;
use crate::target::Target;
use tiny_skia::{Color, Paint, Pixmap, Rect, Shader, Transform};

pub struct Canvas {
    pixmap: Pixmap,
    /// Zone id per dot, painted alongside the ink. See [`crate::halftone`].
    zone: Vec<u8>,
    w: u16,
    h: u16,
}

impl Canvas {
    /// A blank canvas the size of the target, white all over.
    ///
    /// White rather than transparent because the ground has to be opaque: with
    /// an opaque ground and opaque ink every pixel ends up at alpha 255, so the
    /// premultiplied red byte *is* the luminance and reading it back needs no
    /// division and no colour conversion.
    pub fn new(target: &Target) -> Result<Canvas, RenderError> {
        let mut pixmap = Pixmap::new(u32::from(target.width_px), u32::from(target.height_px))
            .ok_or(RenderError::EmptyTarget {
                w: target.width_px,
                h: target.height_px,
            })?;
        pixmap.fill(Color::WHITE);
        Ok(Canvas {
            pixmap,
            zone: vec![0; usize::from(target.width_px) * usize::from(target.height_px)],
            w: target.width_px,
            h: target.height_px,
        })
    }

    pub fn width(&self) -> u16 {
        self.w
    }
    pub fn height(&self) -> u16 {
        self.h
    }

    /// The paint for anything with an edge that must land on a dot boundary:
    /// barcode bars, 2D modules, rules, rectangles, image blits.
    pub fn hard(ink: Ink) -> Paint<'static> {
        let mut p = Paint {
            anti_alias: false,
            ..Paint::default()
        };
        p.shader = Shader::SolidColor(match ink {
            Ink::Black => Color::BLACK,
            Ink::White => Color::WHITE,
        });
        p
    }

    /// The paint for glyph outlines, which are thresholded afterwards.
    ///
    /// The only thing that gets anti-aliasing. Everything else is
    /// [`hard`](Self::hard).
    pub fn soft(ink: Ink) -> Paint<'static> {
        Paint {
            anti_alias: true,
            ..Canvas::hard(ink)
        }
    }

    /// Fill a rectangle given in whole dots.
    ///
    /// Clipped to the canvas rather than refused: an element hanging off the
    /// edge is a layout question for preflight to report, not a reason for the
    /// rasteriser to fail.
    pub fn fill_rect_dots(&mut self, x: i32, y: i32, w: i32, h: i32, ink: Ink) {
        if w <= 0 || h <= 0 {
            return;
        }
        // Clipped in i64. These coordinates come from a document, which came
        // from a file: a frame near `i32::MAX` overflows the addition, which
        // panics in a debug build and silently wraps in a release one - turning
        // an out-of-bounds element into one that draws somewhere plausible.
        let (x0, y0) = (i64::from(x).max(0), i64::from(y).max(0));
        let x1 = (i64::from(x) + i64::from(w)).min(i64::from(self.w));
        let y1 = (i64::from(y) + i64::from(h)).min(i64::from(self.h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (x0, y0, x1, y1) = (x0 as i32, y0 as i32, x1 as i32, y1 as i32);
        let Some(rect) = Rect::from_ltrb(x0 as f32, y0 as f32, x1 as f32, y1 as f32) else {
            return;
        };
        // Identity, always. See the module note.
        self.pixmap
            .fill_rect(rect, &Canvas::hard(ink), Transform::identity(), None);
    }

    /// Mark a rectangle of dots as belonging to a zone.
    ///
    /// Called with the same rectangle as the ink it accompanies, so an element's
    /// zone is exactly where it draws - an outline claims its four bars, not
    /// the empty box inside them. A bounding box would be simpler to explain
    /// and fails badly in one ordinary case: a border drawn last would claim the
    /// whole label and put every element back into one zone.
    ///
    /// Clipped in `i64` for the same reason as [`fill_rect_dots`](Self::fill_rect_dots).
    pub fn paint_zone(&mut self, x: i32, y: i32, w: i32, h: i32, id: u8) {
        if w <= 0 || h <= 0 {
            return;
        }
        let (x0, y0) = (i64::from(x).max(0), i64::from(y).max(0));
        let x1 = (i64::from(x) + i64::from(w)).min(i64::from(self.w));
        let y1 = (i64::from(y) + i64::from(h)).min(i64::from(self.h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let stride = usize::from(self.w);
        for yy in y0 as usize..y1 as usize {
            let row = yy * stride;
            self.zone[row + x0 as usize..row + x1 as usize].fill(id);
        }
    }

    /// A horizontal rule `thickness` dots deep.
    ///
    /// Expressed as a rectangle rather than a stroked path on purpose: a
    /// one-dot stroke centred on an integer coordinate straddles two rows and
    /// lights neither fully, which on a thermal head is a grey line where a
    /// black one was asked for.
    pub fn hline_dots(&mut self, x: i32, y: i32, len: i32, thickness: i32, ink: Ink) {
        self.fill_rect_dots(x, y, len, thickness.max(1), ink);
    }

    /// A vertical rule `thickness` dots wide.
    pub fn vline_dots(&mut self, x: i32, y: i32, len: i32, thickness: i32, ink: Ink) {
        self.fill_rect_dots(x, y, thickness.max(1), len, ink);
    }

    /// Reject a coordinate that is not a whole dot.
    ///
    /// Called by anything taking geometry from a document, so the failure is
    /// reported where the number came from rather than silently floored.
    pub fn require_whole(what: &'static str, value: f32) -> Result<i32, RenderError> {
        if value.fract() != 0.0 || !value.is_finite() {
            return Err(RenderError::NotOnGrid { what, value });
        }
        Ok(value as i32)
    }

    /// Blend an 8-bit coverage buffer onto the canvas, and claim for `zone`
    /// exactly the dots it touches.
    ///
    /// `cov` is already in canvas orientation, its top-left dot at `(x, y)`.
    /// Clipped in `i64` for the same reason as
    /// [`fill_rect_dots`](Self::fill_rect_dots). A dot with coverage 0 is left
    /// alone - ink and zone both - so a text element's zone is its glyphs, not
    /// its frame.
    pub fn blend_coverage(&mut self, x: i32, y: i32, cov: &Coverage, ink: Ink, zone: u8) {
        self.blend(x, y, cov, None, ink, zone);
    }

    /// Blend artwork onto the canvas: [`blend_coverage`](Self::blend_coverage)'s
    /// arithmetic, with the zone decided differently.
    ///
    /// `claim`, the same size as `cov`, marks the dots of the artwork's placed
    /// box. An image's own halftone and threshold must reach all of them, white
    /// included - otherwise a threshold of 255 leaves its paper white, and a
    /// darkened Bayer matrix draws a contour round every pure-white area - so a
    /// box dot joins the image's zone wherever no other element has claimed it.
    /// Where one has, the image takes the dot only where it is visibly inked,
    /// with more than [`NEAR_PAPER`] of coverage: the faint surround of a
    /// scanned or JPEG logo must not re-threshold the anti-aliased text beneath
    /// it.
    pub fn blend_artwork(
        &mut self,
        x: i32,
        y: i32,
        cov: &Coverage,
        claim: &Coverage,
        ink: Ink,
        zone: u8,
    ) {
        if (claim.w, claim.h) != (cov.w, cov.h) {
            return;
        }
        self.blend(x, y, cov, Some(claim), ink, zone);
    }

    fn blend(
        &mut self,
        x: i32,
        y: i32,
        cov: &Coverage,
        claim: Option<&Coverage>,
        ink: Ink,
        zone: u8,
    ) {
        let (cw, ch) = (cov.w, cov.h);
        let len = cw as usize * ch as usize;
        if cw == 0 || ch == 0 || cov.data.len() < len || claim.is_some_and(|m| m.data.len() < len) {
            return;
        }
        let (x0, y0) = (i64::from(x).max(0), i64::from(y).max(0));
        let x1 = (i64::from(x) + i64::from(cw)).min(i64::from(self.w));
        let y1 = (i64::from(y) + i64::from(ch)).min(i64::from(self.h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let stride = usize::from(self.w);
        let data = self.pixmap.data_mut();
        for cy in y0..y1 {
            let sy = (cy - i64::from(y)) as usize;
            for cx in x0..x1 {
                let s = sy * cw as usize + (cx - i64::from(x)) as usize;
                let c = cov.data[s];
                let i = cy as usize * stride + cx as usize;
                let claims = match claim {
                    None => c > 0,
                    Some(m) if m.data[s] == 0 => continue,
                    Some(_) => self.zone[i] == 0 || c > NEAR_PAPER,
                };
                if c > 0 {
                    let l = blend(data[i * 4], c, ink);
                    data[i * 4..i * 4 + 4].copy_from_slice(&[l, l, l, 255]);
                }
                if claims {
                    self.zone[i] = zone;
                }
            }
        }
    }

    /// Take the ink coverage, 0 meaning fully inked and 255 meaning bare paper.
    ///
    /// The pixmap is opaque throughout, so every pixel has alpha 255 and the
    /// premultiplied red byte is already the luminance. No division, no
    /// weighted sum, and nothing to get wrong about which luminance
    /// coefficients apply - that question only arises when a colour image is
    /// blitted in, and it is settled there.
    pub fn finish(self) -> GreyMap {
        let src = self.pixmap.data();
        let mut px = Vec::with_capacity(usize::from(self.w) * usize::from(self.h));
        for chunk in src.as_chunks::<4>().0 {
            px.push(chunk[0]);
        }
        GreyMap {
            w: self.w,
            h: self.h,
            px,
            zone: self.zone,
        }
    }
}

/// Artwork coverage at or below this is paper as far as zones go: see
/// [`Canvas::blend_artwork`]. Two levels of 255, which a JPEG's ringing or a
/// scan's paper tone easily reaches and no eye sees.
pub const NEAR_PAPER: u8 = 2;

/// One dot of luminance `l` under coverage `c` of `ink`.
///
/// tiny-skia's own arithmetic, not a fresh one: for an opaque solid paint it
/// reduces source-over to source, and source with coverage is a lerp,
/// `div255(d * (255 - c) + s * c)` with `div255(v) = (v + 255) >> 8`. So a
/// blended buffer matches filling the outline straight into the canvas, dot
/// for dot. `div255` is not a true division by 255 - it is off by one for
/// about a quarter of all (L, c) pairs - and matching it anyway is the point.
#[inline]
pub fn blend(l: u8, c: u8, ink: Ink) -> u8 {
    let t: u32 = match ink {
        Ink::Black => 0,
        Ink::White => 255,
    };
    let (l, c) = (u32::from(l), u32::from(c));
    ((l * (255 - c) + t * c + 255) >> 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u16, h: u16) -> Canvas {
        Canvas::new(&Target::new(w, h, 203).unwrap()).unwrap()
    }

    #[test]
    fn a_new_canvas_is_bare_paper() {
        let g = canvas(16, 4).finish();
        assert!(g.px.iter().all(|p| *p == 255));
        assert_eq!(g.px.len(), 64);
    }

    /// The trap this whole wrapper exists for: a one-dot rule must light
    /// exactly one row, fully, and leave its neighbours untouched.
    #[test]
    fn a_one_dot_rule_lights_exactly_one_row() {
        let mut c = canvas(8, 5);
        c.hline_dots(0, 2, 8, 1, Ink::Black);
        let g = c.finish();
        for x in 0..8 {
            assert_eq!(g.at(x, 1), 255, "row above stays clear");
            assert_eq!(g.at(x, 2), 0, "the rule is fully inked");
            assert_eq!(g.at(x, 3), 255, "row below stays clear");
        }
    }

    #[test]
    fn a_one_dot_column_lights_exactly_one_column() {
        let mut c = canvas(5, 8);
        c.vline_dots(3, 0, 8, 1, Ink::Black);
        let g = c.finish();
        for y in 0..8 {
            assert_eq!(g.at(2, y), 255);
            assert_eq!(g.at(3, y), 0);
            assert_eq!(g.at(4, y), 255);
        }
    }

    #[test]
    fn hard_paint_has_no_partial_coverage() {
        // Anti-aliasing on a rect edge would show up as a value between the two
        // extremes somewhere. With the hard paint there is nothing in between.
        let mut c = canvas(10, 10);
        c.fill_rect_dots(2, 2, 5, 5, Ink::Black);
        let g = c.finish();
        assert!(
            g.px.iter().all(|p| *p == 0 || *p == 255),
            "every dot is fully on or fully off"
        );
    }

    #[test]
    fn drawing_off_the_edge_clips_rather_than_panicking() {
        let mut c = canvas(8, 8);
        c.fill_rect_dots(-4, -4, 6, 6, Ink::Black);
        c.fill_rect_dots(6, 6, 100, 100, Ink::Black);
        let g = c.finish();
        assert_eq!(g.at(0, 0), 0, "the part that is on the canvas is drawn");
        assert_eq!(g.at(7, 7), 0);
        assert_eq!(g.at(3, 3), 255, "and the rest is not");
    }

    /// Coordinates arrive from a file, so the clip arithmetic must survive the
    /// extremes of the type rather than panicking in debug and wrapping in
    /// release.
    #[test]
    fn extreme_coordinates_clip_without_overflowing() {
        let mut c = canvas(8, 8);
        // Starts far left and is wide enough to reach the canvas: draws.
        c.fill_rect_dots(-100, 0, i32::MAX, 4, Ink::Black);
        // Starts past the right edge: draws nothing.
        c.fill_rect_dots(i32::MAX, 4, i32::MAX, 4, Ink::Black);
        // Ends at -1, entirely off to the left: draws nothing.
        c.fill_rect_dots(i32::MIN, 4, i32::MAX, 4, Ink::Black);
        let g = c.finish();
        assert_eq!(g.at(0, 0), 0, "the visible part is drawn");
        assert_eq!(g.at(7, 0), 0);
        assert_eq!(g.at(0, 4), 255, "and the two off-canvas ones are not");
        assert_eq!(g.at(7, 4), 255);
    }

    #[test]
    fn degenerate_rectangles_draw_nothing() {
        let mut c = canvas(8, 8);
        c.fill_rect_dots(2, 2, 0, 5, Ink::Black);
        c.fill_rect_dots(2, 2, 5, -1, Ink::Black);
        let g = c.finish();
        assert!(g.px.iter().all(|p| *p == 255));
    }

    #[test]
    fn white_ink_clears_back_to_paper() {
        let mut c = canvas(8, 8);
        c.fill_rect_dots(0, 0, 8, 8, Ink::Black);
        c.fill_rect_dots(2, 2, 4, 4, Ink::White);
        let g = c.finish();
        assert_eq!(g.at(0, 0), 0);
        assert_eq!(g.at(3, 3), 255, "knocked back out");
    }

    fn solid(w: u32, h: u32) -> Coverage {
        Coverage {
            w,
            h,
            data: vec![255; (w * h) as usize],
        }
    }

    /// A coverage blit is clipped at every edge of the canvas - ink and zone
    /// both - and never wraps into the next row.
    #[test]
    fn a_coverage_blit_clips_at_every_edge() {
        for (x, y) in [(6, 2), (2, 6), (-3, 2), (2, -2), (6, 6), (-3, -2)] {
            let mut c = canvas(8, 8);
            c.blend_coverage(x, y, &solid(5, 3), Ink::Black, 7);
            let g = c.finish();
            for yy in 0..8u16 {
                for xx in 0..8u16 {
                    let inside =
                        (x..x + 5).contains(&i32::from(xx)) && (y..y + 3).contains(&i32::from(yy));
                    let i = usize::from(yy) * 8 + usize::from(xx);
                    assert_eq!(
                        g.px[i],
                        if inside { 0 } else { 255 },
                        "({x}, {y}) at ({xx}, {yy})"
                    );
                    assert_eq!(
                        g.zone[i],
                        if inside { 7 } else { 0 },
                        "({x}, {y}) zone at ({xx}, {yy})"
                    );
                }
            }
        }
        let mut c = canvas(8, 8);
        c.blend_coverage(i32::MAX, i32::MIN, &solid(5, 3), Ink::Black, 7);
        assert!(
            c.finish().px.iter().all(|&p| p == 255),
            "far off the canvas"
        );
    }

    /// Partial coverage blends with what is already on the canvas, not with
    /// bare paper; zero coverage leaves a dot alone entirely.
    #[test]
    fn partial_coverage_blends_with_the_ground_beneath() {
        let mut c = canvas(4, 1);
        c.fill_rect_dots(0, 0, 2, 1, Ink::Black);
        let cov = Coverage {
            w: 4,
            h: 1,
            data: vec![128, 0, 128, 0],
        };
        c.blend_coverage(0, 0, &cov, Ink::Black, 3);
        let g = c.finish();
        assert_eq!(g.px, vec![0, 0, 127, 255]);
        assert_eq!(g.zone, vec![3, 0, 3, 0]);
        assert_eq!(blend(0, 128, Ink::White), 128);
        assert_eq!(blend(255, 255, Ink::Black), 0);
        assert_eq!(blend(77, 0, Ink::White), 77);
    }

    #[test]
    fn fractional_geometry_is_refused_not_floored() {
        assert!(Canvas::require_whole("x", 10.0).is_ok());
        assert_eq!(Canvas::require_whole("x", 10.0).unwrap(), 10);
        assert!(matches!(
            Canvas::require_whole("x", 10.875),
            Err(RenderError::NotOnGrid { .. })
        ));
        assert!(Canvas::require_whole("x", f32::NAN).is_err());
    }
}
