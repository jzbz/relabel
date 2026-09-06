//! Document to ink coverage.
//!
//! Elements are drawn in z-order, which is document order. Hidden elements are
//! skipped; elements this build does not recognise are skipped too, and
//! reported, because a label that quietly loses an element is worse than one
//! that says it could not draw it.

use crate::canvas::Canvas as Surface;
use crate::document::{Element, Ink, Label, LineEl, RectDots, RectEl, Rot};
use crate::error::RenderError;
use crate::greymap::GreyMap;
use crate::target::Target;

/// What happened while drawing, beyond the pixels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderReport {
    /// Element kinds this build cannot draw yet, in document order.
    pub unimplemented: Vec<&'static str>,
    /// Element types written by a later version. Preserved in the file,
    /// impossible to draw.
    pub unrecognised: Vec<String>,
    /// Ids of elements that fall wholly or partly outside the canvas.
    pub off_canvas: Vec<u32>,
    /// Ids of elements whose rotation was approximated to a quarter turn,
    /// because that is all this renderer can do. The document keeps the angle
    /// the user asked for.
    pub rotation_approximated: Vec<u32>,
}

impl RenderReport {
    pub fn is_clean(&self) -> bool {
        self.unimplemented.is_empty() && self.unrecognised.is_empty() && self.off_canvas.is_empty()
    }
}

/// Draw a label at a target size.
///
/// The target must match the label's own canvas: re-fitting a document drawn
/// for a different head is a decision for the caller to make explicitly, not
/// something to do silently on the way to the printer.
pub fn rasterise(label: &Label, target: &Target) -> Result<(GreyMap, RenderReport), RenderError> {
    let mut surface = Surface::new(target)?;
    let mut report = RenderReport::default();

    for kind in label.unrecognised() {
        report.unrecognised.push(kind.to_string());
    }

    for el in label.drawable() {
        let c = el.common();
        if !fits(&c.frame, &label.canvas) {
            report.off_canvas.push(c.id);
        }
        if !c.rot.is_exact() {
            report.rotation_approximated.push(c.id);
        }
        match el {
            Element::Rect(e) => draw_rect(&mut surface, target, e),
            Element::Line(e) => draw_line(&mut surface, target, e),
            other => report.unimplemented.push(other.kind()),
        }
    }

    Ok((surface.finish(), report))
}

/// Whether a frame lies entirely within the document's own canvas.
///
/// Bounded against the canvas rather than the target, and per axis rather than
/// against whichever happens to be larger - checking both axes against the long
/// one lets an element hang a long way off the short axis unreported.
///
/// Arithmetic in `i64`: these are signed values straight out of a file, and
/// `feed + feed_len` overflows for a frame near `i32::MAX`, which panics in a
/// debug build and silently wraps in a release one.
///
/// Not an error when it does not fit: a bleed off the edge can be deliberate,
/// and preflight is where that conversation belongs.
fn fits(f: &RectDots, canvas: &crate::document::Canvas) -> bool {
    let (feed, head) = (i64::from(f.feed), i64::from(f.head));
    let (fl, hl) = (i64::from(f.feed_len), i64::from(f.head_len));
    feed >= 0
        && head >= 0
        && feed + fl <= i64::from(canvas.feed)
        && head + hl <= i64::from(canvas.head)
}

/// Lay down one rectangle given in document coordinates.
///
/// Every placement goes through [`Target::place`] and [`Target::extent`], which
/// is what keeps a `Top`-direction printer from coming out transposed.
fn blit(s: &mut Surface, t: &Target, feed: i32, head: i32, fl: i32, hl: i32, ink: Ink) {
    let (x, y) = t.place(feed, head);
    let (w, h) = t.extent(fl, hl);
    s.fill_rect_dots(x, y, w, h, ink);
}

fn draw_rect(s: &mut Surface, t: &Target, e: &RectEl) {
    let f = rotated(&e.common.frame, e.common.rot);
    let ink = e.common.ink;
    if e.stroke_dots == 0 {
        blit(s, t, f.feed, f.head, f.feed_len, f.head_len, ink);
        return;
    }
    // An outline is four rectangles rather than a stroked path: a stroke is
    // centred on its coordinate, so a one-dot outline would straddle two rows
    // and light neither fully - a grey line where a black one was asked for.
    let w = i32::from(e.stroke_dots).min(f.feed_len.min(f.head_len));
    blit(s, t, f.feed, f.head, f.feed_len, w, ink);
    blit(s, t, f.feed, f.head + f.head_len - w, f.feed_len, w, ink);
    blit(s, t, f.feed, f.head, w, f.head_len, ink);
    blit(s, t, f.feed + f.feed_len - w, f.head, w, f.head_len, ink);
}

fn draw_line(s: &mut Surface, t: &Target, e: &LineEl) {
    let f = rotated(&e.common.frame, e.common.rot);
    let w = i32::from(e.thickness_dots).max(1);
    // Only axis-aligned rules in this step. Which axis is decided by the
    // longer side, so a frame describes the line without a separate direction.
    if f.feed_len >= f.head_len {
        blit(s, t, f.feed, f.head, f.feed_len, w, e.common.ink);
    } else {
        blit(s, t, f.feed, f.head, w, f.head_len, e.common.ink);
    }
}

/// Swap the frame's axes for a quarter turn.
///
/// Only the extent is rotated here, not the content: a rotated *rectangle* is
/// still a rectangle. Elements with internal orientation - text, barcodes -
/// rotate their own content when they arrive.
fn rotated(f: &RectDots, rot: Rot) -> RectDots {
    // An angle this renderer cannot draw is approximated to the nearest quarter
    // turn *here*, at the point of drawing. The document keeps what the user
    // asked for, and the report says the drawing is an approximation.
    match rot.quantised() {
        Rot::D90 | Rot::D270 => RectDots {
            feed_len: f.head_len,
            head_len: f.feed_len,
            ..*f
        },
        _ => *f,
    }
}

/// Convenience for the common case of not caring about the report.
pub fn rasterise_plane(
    label: &Label,
    target: &Target,
) -> Result<relabel_proto::BitPlane, RenderError> {
    let (grey, _) = rasterise(label, target)?;
    Ok(grey.threshold(128))
}
