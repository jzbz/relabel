//! The rendered label, as one Slint image.
//!
//! Thin on purpose: `relabel-render`'s own `preview` module owns the pixels and
//! the scaling, and everything here is about handing them over correctly once.

use crate::doc::Doc;
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
