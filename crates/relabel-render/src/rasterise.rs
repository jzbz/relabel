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

use crate::artwork::{self, ArtworkError};
use crate::barcode::{self, BarcodeError, MIN_MODULE_DOTS, Symbology};
use crate::canvas::Canvas as Surface;
use crate::code2d::{self, CodeError, ModuleGrid};
use crate::coverage::Coverage;
use crate::document::{
    BarcodeEl, Common, Element, ImageEl, ImageFit, ImageHalftone, Ink, Label, LineEl, RectDots,
    RectEl, Rot,
};
use crate::error::RenderError;
use crate::greymap::GreyMap;
use crate::halftone::{BinariseOpts, Zone, binarise};
use crate::target::Target;
use crate::text::{self, TextClip, TextEngine, TextError, TextStyle};
use relabel_proto::PrintDirection;

/// Zone ids are a byte and 0 is the page. Elements beyond the 254th share the
/// last id - on a label a few hundred dots long that is not a realistic count,
/// and sharing only matters where one of the sharers is a photograph.
const LAST_ZONE: u8 = 254;

/// An image drawn at a size other than the one it was imported for: its id,
/// the stored size, and the size drawn.
pub type ImageResampled = (u32, (u32, u32), (u32, u32));

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
    /// Text drawn, but not all of it: cut by its own frame. Drawn but not as
    /// asked, the same kind of entry as `rotation_approximated`.
    pub text_clipped: Vec<(u32, TextClip)>,
    /// Artwork drawn at a size other than the one it was imported for, by id,
    /// with the stored size and the size drawn: its frame has changed, or the
    /// label is drawn for a printer whose head runs the other way. Importing
    /// it again for the frame puts it right.
    pub image_resampled: Vec<ImageResampled>,
    /// Artwork placed one pixel to one dot that is larger than its frame, and
    /// cropped.
    pub image_clipped: Vec<u32>,
    /// Images whose own halftone or threshold went unused: the label has more
    /// elements than zone ids, and they share the last one.
    pub zone_shared: Vec<u32>,
}

impl RenderReport {
    /// Nothing was skipped, clipped or approximated.
    pub fn is_clean(&self) -> bool {
        self.unimplemented.is_empty()
            && self.unrecognised.is_empty()
            && self.off_canvas.is_empty()
            && self.rotation_approximated.is_empty()
            && self.failed.is_empty()
            && self.text_clipped.is_empty()
            && self.image_resampled.is_empty()
            && self.image_clipped.is_empty()
            && self.zone_shared.is_empty()
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

    // Built on the first text element, and only then: a few microseconds, and
    // no state outlives the render.
    let mut engine: Option<TextEngine> = None;
    report.zone_shared = zone_plan(label, Zone::PAGE).1;
    for (zone, el) in zone_ids(label) {
        let c = el.common();
        // The frame as drawn: a quarter turn swaps its sides, and checking the
        // stored frame instead let a turned symbol run off the canvas with a
        // clean report.
        if !fits(&rotated(&c.frame, c.rot), &label.canvas) {
            report.off_canvas.push(c.id);
        }
        if !c.rot.is_exact() {
            report.rotation_approximated.push(c.id);
        }
        match el {
            Element::Rect(e) => draw_rect(&mut surface, target, e, zone),
            Element::Line(e) => draw_line(&mut surface, target, e, zone),
            Element::Barcode(e) => {
                if let Err(err) = draw_barcode(&mut surface, target, e, zone) {
                    report.failed.push((c.id, err.to_string()));
                }
            }
            Element::Qr(e) => {
                let r = code2d::check_module(e.module_dots, e.out_of_spec)
                    .and_then(|()| code2d::qr(&e.data, e.ec, e.version))
                    .and_then(|(g, _)| draw_grid(&mut surface, target, c, &g, e.module_dots, zone));
                if let Err(err) = r {
                    report.failed.push((c.id, err.to_string()));
                }
            }
            Element::DataMatrix(e) => {
                // The same test draw_grid makes, so the size chosen is one it
                // will accept.
                let f = rotated(&c.frame, c.rot);
                let fits =
                    |g: &ModuleGrid| grid_fits(&oriented(g, c.rot, target), &f, e.module_dots);
                let r = code2d::check_module(e.module_dots, e.out_of_spec)
                    .and_then(|()| code2d::data_matrix(&e.data, &e.size, fits))
                    .and_then(|(g, _)| draw_grid(&mut surface, target, c, &g, e.module_dots, zone));
                if let Err(err) = r {
                    report.failed.push((c.id, err.to_string()));
                }
            }
            Element::Text(e) => match draw_text(&mut surface, target, e, zone, &mut engine) {
                Ok(clip) if clip.is_clipped() => report.text_clipped.push((c.id, clip)),
                Ok(_) => {}
                Err(err) => report.failed.push((c.id, err.to_string())),
            },
            Element::Image(e) => {
                if let Err(err) = draw_image(&mut surface, target, e, zone, &mut report) {
                    report.failed.push((c.id, err.to_string()));
                }
            }
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

/// The ink a symbol's quiet zone is knocked back to: paper under black ink,
/// and black under a reversed symbol.
fn paper_under(ink: Ink) -> Ink {
    match ink {
        Ink::Black => Ink::White,
        Ink::White => Ink::Black,
    }
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

/// Draw a label and reduce it to one bit, each image in its own zone with
/// its own halftone and threshold, and the page thresholded at 128. For the
/// common case of not caring about the report.
pub fn rasterise_plane(
    label: &Label,
    target: &Target,
) -> Result<relabel_proto::BitPlane, RenderError> {
    let (grey, _) = rasterise(label, target)?;
    let zones = zone_table(label, Zone::PAGE);
    Ok(binarise(&grey, &BinariseOpts::with_zones(&zones)))
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
    // Saturating, because the frame's position is a number from a file: a bar
    // offset added to a frame near `i32::MAX` would otherwise overflow. A
    // saturated position is far off the canvas, which is already reported.
    let place = |off: i32, w: i32| {
        if along_head {
            rect(b.feed, b.head.saturating_add(off), depth, w)
        } else {
            rect(b.feed.saturating_add(off), b.head, w, depth)
        }
    };
    blit(s, t, place(0, length), paper_under(e.common.ink), zone);
    for &(start, width) in &modules.bars {
        let (off, w) = (start as i32 * x, width as i32 * x);
        let off = if reversed { length - off - w } else { off };
        blit(s, t, place(off, w), e.common.ink, zone);
    }
    Ok(())
}

/// A 2D symbol's grid as it must be laid out in document coordinates to read
/// correctly on this target.
///
/// Turned by the element's rotation, then transposed on a `Top` target.
/// [`Target::place`] swaps the axes for `Top` - the feed runs down the canvas
/// and the head across it - and a swap is a reflection: a symbol laid out along
/// the document's axes would print mirrored, which a linear barcode survives
/// and a 2D symbol does not. Transposing first cancels it, and turning before
/// transposing keeps a quarter turn clockwise on the canvas.
fn oriented(grid: &ModuleGrid, rot: Rot, t: &Target) -> ModuleGrid {
    let g = grid.turned(rot);
    match t.direction {
        PrintDirection::Top => g.transposed(),
        PrintDirection::Left => g,
    }
}

/// Whether an oriented grid and its quiet zone fit a frame, as drawn, at a
/// module size. The one test both the DataMatrix size choice and the drawing
/// make, so they cannot disagree.
fn grid_fits(g: &ModuleGrid, f: &RectDots, module_dots: u16) -> bool {
    let (w, h) = g.footprint(module_dots);
    i64::from(w) <= i64::from(f.feed_len) && i64::from(h) <= i64::from(f.head_len)
}

/// Lay a 2D symbol into the top-left corner of its frame.
///
/// The same rules as a linear barcode: every module an exact square of whole
/// dots, the quiet zone knocked back to paper, and a symbol that does not fit
/// refused rather than clipped or shrunk. The frame and the grid are both
/// turned by the element's rotation, so a quarter-turned QR code is the same
/// symbol on its side; see [`oriented`] for the rest.
fn draw_grid(
    s: &mut Surface,
    t: &Target,
    c: &Common,
    grid: &ModuleGrid,
    module_dots: u16,
    zone: u8,
) -> Result<(), CodeError> {
    let f = rotated(&c.frame, c.rot);
    let g = oriented(grid, c.rot, t);
    let (w, h) = g.footprint(module_dots);
    if !grid_fits(&g, &f, module_dots) {
        return Err(CodeError::TooLarge {
            needed: (w, h),
            available: (f.feed_len.max(0) as u32, f.head_len.max(0) as u32),
        });
    }
    // The footprint fits a frame whose sides are `i32`, so these do too.
    let (w, h, m, q) = (w as i32, h as i32, i32::from(module_dots), g.quiet as i32);
    blit(s, t, rect(f.feed, f.head, w, h), paper_under(c.ink), zone);
    // One rectangle per run of dark modules rather than one per module: a
    // version 5 QR code is 1,369 modules and a few hundred runs.
    for y in 0..g.h {
        let mut x = 0;
        while x < g.w {
            if !g.is_dark(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < g.w && g.is_dark(x, y) {
                x += 1;
            }
            let r = rect(
                f.feed.saturating_add((q + start as i32) * m),
                f.head.saturating_add((q + y as i32) * m),
                (x - start) as i32 * m,
                m,
            );
            blit(s, t, r, c.ink, zone);
        }
    }
    Ok(())
}

/// Lay a text element out upright in its own frame, then turn it onto the
/// canvas.
///
/// The local frame is the frame as drawn, measured on the canvas and turned
/// back upright: its width is the reading width, whatever the rotation and
/// whichever way the printer's head runs. The canvas is the label in reading
/// orientation on every target, so the upright raster is turned by the
/// element's rotation and placed at the frame's corner - on a `Top` target,
/// the transpose its axis swap would otherwise need cancels against that same
/// swap. See the text module.
fn draw_text(
    s: &mut Surface,
    t: &Target,
    e: &crate::document::TextEl,
    zone: u8,
    engine: &mut Option<TextEngine>,
) -> Result<TextClip, TextError> {
    let c = &e.common;
    let Some((w, h)) = local_size(c, t) else {
        // Nothing is lost when there is nothing to draw; otherwise the text
        // would vanish with a clean report.
        return if e.text.chars().all(char::is_whitespace) {
            Ok(TextClip::default())
        } else {
            Err(TextError::BadParam("the text frame has no area"))
        };
    };
    let eng = match engine {
        Some(eng) => eng,
        None => engine.insert(TextEngine::new()?),
    };
    let style = TextStyle {
        family: &e.family,
        ppem: e.size,
        line_height: e.line_height,
        weight: e.weight.clone(),
        align: e.align.clone(),
        raster: e.raster.clone(),
    };
    let local = eng.render_local(&e.text, &style, w, h, local_limit(t))?;
    let f = rotated(&c.frame, c.rot);
    let (x, y) = t.place(f.feed, f.head);
    s.blend_coverage(x, y, &local.cov.turned(c.rot), c.ink, zone);
    Ok(local.clip)
}

/// The frame an element is drawn in, as its upright content sees it: measured
/// on the canvas, then turned back by the element's rotation, so its width is
/// the reading width whatever the turn and whichever way the printer's head
/// runs. `None` when it has no area.
///
/// Public because import sizes artwork for exactly this frame, and the editor
/// has to ask for it the way the rasteriser does.
pub fn local_size(c: &Common, t: &Target) -> Option<(u32, u32)> {
    let f = rotated(&c.frame, c.rot);
    let (cw, ch) = t.extent(f.feed_len, f.head_len);
    if cw <= 0 || ch <= 0 {
        return None;
    }
    let quarter = matches!(c.rot.quantised(), Rot::D90 | Rot::D270);
    let (w, h) = if quarter { (ch, cw) } else { (cw, ch) };
    Some((w as u32, h as u32))
}

/// The most dots a local raster may hold: as many as the canvas it lands on,
/// and never fewer than text's floor. A frame that fits the label is never
/// refused for its area, while one billions of dots on a side - it came from a
/// file - is. Sides are capped separately, at [`text::MAX_SIDE`].
fn local_limit(t: &Target) -> u64 {
    (u64::from(t.width_px) * u64::from(t.height_px)).max(text::MAX_LOCAL_DOTS)
}

/// Unpack an image element's embedded artwork and lay it into its frame.
///
/// The artwork was sized for this frame when it was imported, so this is
/// normally a straight copy. When the frame has changed since - or the label
/// is drawn for a printer whose head runs the other way - it is resampled
/// nearest-neighbour, in integers, and reported, because the fix is to import
/// it again. Placed as text is: upright in a local buffer, turned by index,
/// then blended, so its white leaves whatever lies beneath alone. Its zone is
/// its whole box, less the dots another element already holds where the
/// artwork is white or nearly: see [`Surface::blend_artwork`].
fn draw_image(
    s: &mut Surface,
    t: &Target,
    e: &ImageEl,
    zone: u8,
    report: &mut RenderReport,
) -> Result<(), ArtworkError> {
    let c = &e.common;
    if let ImageFit::Unknown(v) = &e.fit {
        return Err(ArtworkError::UnknownValue {
            what: "the fit",
            value: v.clone(),
        });
    }
    if let Some(ImageHalftone::Unknown(v)) = &e.halftone {
        return Err(ArtworkError::UnknownValue {
            what: "the halftone",
            value: v.clone(),
        });
    }
    if e.png.is_empty() {
        return Err(ArtworkError::NoArtwork {
            path: e.path.clone(),
        });
    }
    let (w, h) = local_size(c, t).ok_or(ArtworkError::EmptyFrame)?;
    let limit = local_limit(t);
    if w > text::MAX_SIDE || h > text::MAX_SIDE || u64::from(w) * u64::from(h) > limit {
        return Err(ArtworkError::FrameTooLarge { w, h });
    }
    let art = artwork::from_stored(&e.png, limit)?;
    let p = artwork::fit_box((art.w, art.h), (w, h), &e.fit)
        .ok_or(ArtworkError::FrameTooLarge { w, h })?;
    if (p.bw, p.bh) != (art.w, art.h) {
        report
            .image_resampled
            .push((c.id, (art.w, art.h), (p.bw, p.bh)));
    }
    if e.fit == ImageFit::Dots && (art.w > w || art.h > h) {
        report.image_clipped.push(c.id);
    }

    // Each dot of the frame inside the box takes the stored pixel under its
    // centre. Stored sides are at most 8191 and box sides under 2^32, so the
    // products stay far inside u64.
    let mut cov = Coverage::new(w, h);
    let mut claim = Coverage::new(w, h);
    let (sw, sh) = (u64::from(art.w), u64::from(art.h));
    let (bw, bh) = (i64::from(p.bw), i64::from(p.bh));
    for y in 0..h {
        let by = i64::from(y) - p.oy;
        if by < 0 || by >= bh {
            continue;
        }
        let sy = ((2 * by as u64 + 1) * sh) / (2 * bh as u64);
        for x in 0..w {
            let bx = i64::from(x) - p.ox;
            if bx < 0 || bx >= bw {
                continue;
            }
            let sx = ((2 * bx as u64 + 1) * sw) / (2 * bw as u64);
            let i = y as usize * w as usize + x as usize;
            cov.data[i] = 255 - art.px[(sy * sw + sx) as usize];
            claim.data[i] = 255;
        }
    }
    let f = rotated(&c.frame, c.rot);
    let (x, y) = t.place(f.feed, f.head);
    s.blend_artwork(x, y, &cov.turned(c.rot), &claim.turned(c.rot), c.ink, zone);
    Ok(())
}

/// Each drawable element with the zone id it paints, in z-order: 1 upwards,
/// the elements past the 254th all sharing the last id.
fn zone_ids(label: &Label) -> impl Iterator<Item = (u8, &Element)> {
    label.drawable().enumerate().map(|(i, el)| {
        let id = i.saturating_add(1).min(usize::from(LAST_ZONE));
        (id as u8, el)
    })
}

/// The binariser's settings for every zone a label paints: entry 0 is `page`,
/// and each image with its own halftone or threshold has them in its entry.
///
/// For [`binarise`], with the grey map [`rasterise`] made from the same label,
/// as [`rasterise_plane`] does.
pub fn zone_table(label: &Label, page: Zone) -> Vec<Zone> {
    zone_plan(label, page).0
}

/// The zone table, and the images whose override could not be honoured
/// because they share the last zone id with other elements - which then gets
/// the page's settings, since it cannot have all of theirs.
fn zone_plan(label: &Label, page: Zone) -> (Vec<Zone>, Vec<u32>) {
    let mut zones = vec![page];
    let (mut on_last, mut overridden) = (0usize, Vec::new());
    for (id, el) in zone_ids(label) {
        let (zone, overrides) = match el {
            Element::Image(e) => (
                Zone {
                    halftone: e
                        .halftone
                        .as_ref()
                        .and_then(ImageHalftone::known)
                        .unwrap_or(page.halftone),
                    threshold: e.threshold.unwrap_or(page.threshold),
                    ..page
                },
                e.halftone.is_some() || e.threshold.is_some(),
            ),
            _ => (page, false),
        };
        if usize::from(id) == zones.len() {
            zones.push(zone);
        }
        if id == LAST_ZONE {
            on_last += 1;
            if overrides {
                overridden.push(el.common().id);
            }
        }
    }
    if on_last > 1 {
        zones[usize::from(LAST_ZONE)] = page;
        (zones, overridden)
    } else {
        (zones, Vec::new())
    }
}
