//! The rendered label, as one Slint image: the canvas's, at the view's zoom, and
//! the Preview pane's, fitted to a box.
//!
//! Thin on purpose: `relabel-render`'s own `preview` module owns the pixels and
//! the scaling, and everything here is about handing them over correctly once.

use crate::doc::Doc;
use relabel_proto::BitPlane;
use relabel_render::preview::{Extent, Scale, plane_stamp, plane_to_rgb8};
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

/// A pane's worth of dots, and the two numbers the element must be sized with.
pub struct Painted {
    pub image: Image,
    /// Device pixels. Stated to the toolkit in `phx`, so the blit is 1:1 and no
    /// filter runs whatever the display scale.
    pub px_w: i32,
    pub px_h: i32,
}

/// What the canvas last drew, so that it can decline to draw it again.
#[derive(Default)]
pub struct Pane {
    stamp: Option<u64>,
}

/// Why there are no dots to show. The frames are still drawn, so the operator
/// can move elements on a label whose render failed - which is the label most
/// in need of editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoDots {
    /// The render refused. Carries what it said.
    Refused(String),
    /// The expanded plane is larger than `preview::MAX_BYTES`.
    TooLarge,
    /// The rendered plane is not the size the document's canvas says it is.
    /// Never seen; checked because the failure it prevents - a pane of the
    /// right length and the wrong shape - is invisible in a buffer of bytes.
    Disagrees {
        plane: (u16, u16),
        canvas: (i32, i32),
    },
}

impl Pane {
    /// Render, and hand over a fresh buffer - or `Ok(None)` when the dots are
    /// the ones already on screen.
    ///
    /// A fresh buffer every time, never a kept one: `SharedVector::detach` is a
    /// no-op only at refcount one, so a buffer already given to an `Image` deep
    /// copies on the next write. The saving comes from not calling this at all,
    /// which is what the stamp is for - Slint compares image buffers by data
    /// pointer, so a freshly allocated buffer of identical pixels always looks
    /// new and forces a texture upload.
    pub fn draw(&mut self, doc: &Doc, zoom: u32) -> Result<Option<Painted>, NoDots> {
        let scale = Scale::up(zoom);
        let plane = relabel_render::rasterise_plane(doc.label(), doc.target())
            .map_err(|e| NoDots::Refused(e.to_string()))?;
        let (dots_x, dots_y) = doc.canvas_dots();
        if i32::from(plane.width()) != dots_x || i32::from(plane.height()) != dots_y {
            return Err(NoDots::Disagrees {
                plane: (plane.width(), plane.height()),
                canvas: (dots_x, dots_y),
            });
        }
        let extent = Extent::for_plane(&plane, scale).ok_or(NoDots::TooLarge)?;
        let stamp = plane_stamp(&plane, scale);
        if self.stamp == Some(stamp) {
            return Ok(None);
        }
        // All three numbers out of the one Extent: deriving the width and the
        // height separately from the length is how a square-ish label becomes a
        // sheared picture that no byte count can detect.
        let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(extent.px_w, extent.px_h);
        plane_to_rgb8(&plane, scale, buf.make_mut_bytes());
        self.stamp = Some(stamp);
        Ok(Some(Painted {
            image: Image::from_rgb8(buf),
            px_w: extent.px_w as i32,
            px_h: extent.px_h as i32,
        }))
    }

    /// Forget what is on screen, so the next [`draw`](Self::draw) paints
    /// whatever the stamp says - for a zoom change, where the pixels are
    /// different even though the dots are not.
    pub fn invalidate(&mut self) {
        self.stamp = None;
    }
}

/// A plane at the largest scale that fits a box of `box_w` by `box_h` physical
/// pixels, and that scale: the Preview pane's image, which shows a plane the
/// caller already drew rather than drawing one.
///
/// A fresh buffer every time, for [`Pane::draw`]'s reason. `None` only when the
/// plane has no extent at any scale, which [`Scale::for_pane`] says cannot
/// happen for a plane a label can make.
pub fn paint_fit(plane: &BitPlane, box_w: u32, box_h: u32) -> Option<(Painted, Scale)> {
    let scale = Scale::for_pane(plane.width(), plane.height(), box_w, box_h);
    let extent = Extent::for_plane(plane, scale)?;
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(extent.px_w, extent.px_h);
    plane_to_rgb8(plane, scale, buf.make_mut_bytes());
    let painted = Painted {
        image: Image::from_rgb8(buf),
        px_w: i32::try_from(extent.px_w).ok()?,
        px_h: i32::try_from(extent.px_h).ok()?,
    };
    Some((painted, scale))
}

impl std::fmt::Display for NoDots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoDots::Refused(why) => write!(f, "the label could not be drawn: {why}"),
            NoDots::TooLarge => write!(
                f,
                "this label has more pixels at this magnification than any pane can show"
            ),
            NoDots::Disagrees { plane, canvas } => write!(
                f,
                "the rendered label is {} by {} dots and its canvas is {} by {}",
                plane.0, plane.1, canvas.0, canvas.1
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The author's 40 by 12 mm label at scale 1: two pixels to the dot in the
    /// pane's 640 by 200 box, the plane in canvas orientation, never turned.
    #[test]
    fn a_40_by_12_mm_plane_fits_the_pane_at_two_pixels_a_dot() {
        let plane = BitPlane::new(320, 96);
        let (painted, scale) = paint_fit(&plane, 640, 200).expect("a plane fits");
        assert_eq!(scale, Scale::up(2));
        assert_eq!((painted.px_w, painted.px_h), (640, 192));
        let size = painted.image.size();
        assert_eq!((size.width, size.height), (640, 192));
    }

    #[test]
    fn a_plane_larger_than_the_box_is_shown_reduced() {
        let plane = BitPlane::new(800, 96);
        let (painted, scale) = paint_fit(&plane, 640, 200).expect("a plane fits");
        assert_eq!(scale, Scale::down(2));
        assert_eq!((painted.px_w, painted.px_h), (400, 48));
    }

    /// A plane no symmetry or shift leaves unchanged: a steep diagonal, one
    /// whole head line and one whole feed line.
    fn marked(w: u16, h: u16) -> BitPlane {
        let mut plane = BitPlane::new(w, h);
        for x in 0..w {
            plane.set(x, ((u32::from(x) * 7) % u32::from(h)) as u16, true);
        }
        for y in 0..h {
            plane.set(w / 2, y, true);
        }
        for x in 0..w {
            plane.set(x, h / 3, true);
        }
        plane
    }

    /// What the pane shows is the plane it was given, dot for dot: read back
    /// from the image's own pixels at two pixels a dot, and equal to the
    /// render crate's drawing of that plane when reduced.
    #[test]
    fn the_fitted_image_is_the_plane_dot_for_dot() {
        let plane = marked(320, 96);
        let (painted, scale) = paint_fit(&plane, 640, 200).expect("a plane fits");
        assert_eq!(scale, Scale::up(2));
        let px = painted.image.to_rgb8().expect("an RGB image");
        let (w, bytes) = (px.width() as usize, px.as_bytes());
        for y in 0..plane.height() {
            for x in 0..plane.width() {
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let at = ((usize::from(y) * 2 + dy) * w + usize::from(x) * 2 + dx) * 3;
                    let ink = bytes[at] < 128;
                    assert_eq!(ink, plane.get(x, y), "dot ({x}, {y})");
                }
            }
        }

        let wide = marked(800, 96);
        let (painted, scale) = paint_fit(&wide, 640, 200).expect("a plane fits");
        assert_eq!(scale, Scale::down(2));
        let px = painted.image.to_rgb8().expect("an RGB image");
        let mut want = vec![0; px.as_bytes().len()];
        plane_to_rgb8(&wide, scale, &mut want);
        assert_eq!(px.as_bytes(), want.as_slice());
        assert!(want.iter().any(|&b| b < 128), "something is inked");
    }
}
