//! Document to ink coverage.
//!
//! Elements are drawn in z-order, which is document order. Hidden elements are
//! skipped; elements this build does not recognise are skipped too, and
//! reported, because a label that quietly loses an element is worse than one
//! that says it could not draw it.
//!
//! Each element is also painted into its own halftoning zone as it is drawn,
//! so that when a photograph is dithered its error cannot travel into the
//! element beside it. See [`crate::halftone`].

use crate::barcode::{self, BarcodeError, MIN_MODULE_DOTS, Symbology};
use crate::canvas::Canvas as Surface;
use crate::document::{BarcodeEl, Element, Ink, Label, LineEl, RectDots, RectEl, Rot};
use crate::error::RenderError;
use crate::greymap::GreyMap;
use crate::target::Target;

/// Zone ids are a byte and 0 is the page. Elements beyond the 254th share the
/// last id - on a label a few hundred dots long that is not a realistic count,
/// and sharing only matters where one of the sharers is a photograph.
const LAST_ZONE: u8 = 254;

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
    /// Elements that could not be drawn at all, by id, with the reason.
    ///
    /// Refusal is deliberate for symbologies: a barcode that does not fit is
    /// not clipped, because a clipped Code 128 scans as a different value.
    pub failed: Vec<(u32, String)>,
}

impl RenderReport {
    /// Nothing was skipped, clipped or approximated.
    pub fn is_clean(&self) -> bool {
        self.unimplemented.is_empty()
            && self.unrecognised.is_empty()
            && self.off_canvas.is_empty()
            && self.rotation_approximated.is_empty()
            && self.failed.is_empty()
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

    let mut next_zone: u8 = 1;
    for el in label.drawable() {
        let c = el.common();
        if !fits(&c.frame, &label.canvas) {
            report.off_canvas.push(c.id);
        }
        if !c.rot.is_exact() {
            report.rotation_approximated.push(c.id);
        }
        let zone = next_zone;
        next_zone = next_zone.saturating_add(1).min(LAST_ZONE);
        match el {
            Element::Rect(e) => draw_rect(&mut surface, target, e, zone),
            Element::Line(e) => draw_line(&mut surface, target, e, zone),
            Element::Barcode(e) => {
                if let Err(err) = draw_barcode(&mut surface, target, e, zone) {
                    report.failed.push((c.id, err.to_string()));
                }
            }
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

/// Lay down one rectangle given in document coordinates, and claim it for a
/// zone.
///
/// Every placement goes through [`Target::place`] and [`Target::extent`], which
/// is what keeps a `Top`-direction printer from coming out transposed.
fn blit(s: &mut Surface, t: &Target, r: RectDots, ink: Ink, zone: u8) {
    let (x, y) = t.place(r.feed, r.head);
    let (w, h) = t.extent(r.feed_len, r.head_len);
    s.fill_rect_dots(x, y, w, h, ink);
    s.paint_zone(x, y, w, h, zone);
}

fn rect(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

fn draw_rect(s: &mut Surface, t: &Target, e: &RectEl, zone: u8) {
    let f = rotated(&e.common.frame, e.common.rot);
    let ink = e.common.ink;
    if e.stroke_dots == 0 {
        blit(s, t, f, ink, zone);
        return;
    }
    // An outline is four rectangles rather than a stroked path: a stroke is
    // centred on its coordinate, so a one-dot outline would straddle two rows
    // and light neither fully - a grey line where a black one was asked for.
    let w = i32::from(e.stroke_dots).min(f.feed_len.min(f.head_len));
    blit(s, t, rect(f.feed, f.head, f.feed_len, w), ink, zone);
    blit(
        s,
        t,
        rect(f.feed, f.head + f.head_len - w, f.feed_len, w),
        ink,
        zone,
    );
    blit(s, t, rect(f.feed, f.head, w, f.head_len), ink, zone);
    blit(
        s,
        t,
        rect(f.feed + f.feed_len - w, f.head, w, f.head_len),
        ink,
        zone,
    );
}

fn draw_line(s: &mut Surface, t: &Target, e: &LineEl, zone: u8) {
    let f = rotated(&e.common.frame, e.common.rot);
    let w = i32::from(e.thickness_dots).max(1);
    // Only axis-aligned rules in this step. Which axis is decided by the
    // longer side, so a frame describes the line without a separate direction.
    let r = if f.feed_len >= f.head_len {
        rect(f.feed, f.head, f.feed_len, w)
    } else {
        rect(f.feed, f.head, w, f.head_len)
    };
    blit(s, t, r, e.common.ink, zone);
}

/// Swap the frame's axes for a quarter turn.
///
/// Only the extent is rotated here, not the content: a rotated *rectangle* is
/// still a rectangle. Elements with internal orientation - text, barcodes -
/// rotate their own content when they arrive. The origin stays where it was
/// rather than the element turning about its centre: a renderer has to pick
/// one, and the editor is the right place to keep an element visually in
/// place while it turns.
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

/// Lay a linear barcode into its frame.
///
/// Every bar is placed at an exact integer multiple of the module width: the
/// rasteriser never scales a symbol. The whole footprint, quiet zones included,
/// is knocked back to paper first, because a symbol without clear quiet zones
/// may not scan - so a barcode placed over a border cuts the border, which is
/// the right outcome.
///
/// For a quarter turn the symbol runs along the head axis instead of the feed,
/// and the half turns lay the modules in reverse, which scanners read either
/// way.
fn draw_barcode(s: &mut Surface, t: &Target, e: &BarcodeEl, zone: u8) -> Result<(), BarcodeError> {
    let sym = Symbology::from_name(&e.symbology)
        .ok_or_else(|| BarcodeError::UnknownSymbology(e.symbology.clone()))?;
    if e.module_dots == 0 {
        return Err(BarcodeError::ZeroModule);
    }
    if e.module_dots < MIN_MODULE_DOTS && !e.out_of_spec {
        return Err(BarcodeError::TooNarrow(e.module_dots));
    }
    let modules = barcode::encode(sym, &e.data)?;

    let b = rotated(&e.common.frame, e.common.rot);
    let turn = e.common.rot.quantised();
    let along_head = matches!(turn, Rot::D90 | Rot::D270);
    let reversed = matches!(turn, Rot::D180 | Rot::D270);

    let depth = if e.bar_dots > 0 {
        i32::from(e.bar_dots)
    } else if along_head {
        b.feed_len
    } else {
        b.head_len
    };
    // Compliance before length, so a retail symbol on a small label is refused
    // for the reason that holds at every size rather than whichever trips first.
    if !e.out_of_spec {
        barcode::check_retail(sym, e.module_dots, depth.max(0) as u32, t.dpmm())?;
    }
    let available = if along_head { b.head_len } else { b.feed_len };
    let length = modules.width_dots(e.module_dots);
    if i64::from(length) > i64::from(available) {
        return Err(BarcodeError::TooWide {
            needed: length,
            available: available.max(0) as u32,
        });
    }

    let length = length as i32;
    let x = i32::from(e.module_dots);
    let place = |off: i32, w: i32| {
        if along_head {
            rect(b.feed, b.head + off, depth, w)
        } else {
            rect(b.feed + off, b.head, w, depth)
        }
    };
    let paper = match e.common.ink {
        Ink::Black => Ink::White,
        Ink::White => Ink::Black,
    };
    blit(s, t, place(0, length), paper, zone);
    for &(start, width) in &modules.bars {
        let (off, w) = (start as i32 * x, width as i32 * x);
        let off = if reversed { length - off - w } else { off };
        blit(s, t, place(off, w), e.common.ink, zone);
    }
    Ok(())
}
