//! New elements: what each kind starts as, where it lands, and what the renderer
//! makes of an element whose frame has just changed.
//!
//! No toolkit type in any signature, for `geom`'s reason: every rule here is
//! reached by an ordinary unit test. Sizes come from the encoders and the renderer
//! rather than from constants, so a default is the size the symbol actually is,
//! and every default is pinned by a test that draws it.

use crate::doc::{describe, kind_name};
use crate::geom;
use relabel_proto::PrintDirection;
use relabel_render::barcode::{self, MIN_MODULE_DOTS, Symbology};
use relabel_render::code2d::{self, ModuleGrid};
use relabel_render::document::{
    BarcodeEl, Common, DataMatrixEl, DmSize, Element, ElementId, ElementOrRaw, GlyphRaster, Ink,
    Label, LineEl, QrEc, QrEl, RectDots, RectEl, Rot, TextAlign, TextEl, TextWeight,
};
use relabel_render::media::{self, Bounds};
use relabel_render::rasterise::{self, Failure};
use relabel_render::target::Target;
use relabel_render::text::{self, TextClip};

/// The kinds an operator can add from nothing. Image is not among them: an image
/// with no artwork draws nothing and preflight refuses it, so an image is only
/// ever made from artwork, by an import ([`crate::doc::Cmd::Import`]). Its tool is
/// the seventh button, token [`IMAGE_TOOL`], which no `Kind` answers to, so
/// [`crate::doc::Cmd::Create`] can never make an image without artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Barcode,
    Qr,
    DataMatrix,
    Rect,
    Line,
}

impl Kind {
    /// In toolbar order, which is also token order.
    pub const ALL: [Kind; 6] = [
        Kind::Text,
        Kind::Barcode,
        Kind::Qr,
        Kind::DataMatrix,
        Kind::Rect,
        Kind::Line,
    ];

    /// The integer the toolbar sends back, for `Field::token`'s reason.
    pub fn token(self) -> i32 {
        self as i32
    }

    pub fn from_token(t: i32) -> Option<Kind> {
        usize::try_from(t)
            .ok()
            .and_then(|i| Kind::ALL.get(i))
            .copied()
    }

    /// The letter held with Control and Shift. Q is not QR's: Command arrives as
    /// control on macOS, and Command+Shift+Q logs out there. U and E are left to
    /// input methods, which take Control+Shift with them for Unicode and emoji.
    fn letter(self) -> char {
        match self {
            Kind::Text => 'T',
            Kind::Barcode => 'B',
            Kind::Qr => 'R',
            Kind::DataMatrix => 'M',
            Kind::Rect => 'O',
            Kind::Line => 'L',
        }
    }

    /// The kind whose chord a key's text is, in either case: Shift is held, so the
    /// text arrives upper case on one layout and lower case on another.
    pub fn from_letter(s: &str) -> Option<Kind> {
        let mut chars = s.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return None;
        };
        Kind::ALL
            .into_iter()
            .find(|k| k.letter().eq_ignore_ascii_case(&c))
    }

    /// The button's face.
    pub fn caption(self) -> &'static str {
        match self {
            Kind::Text => "Text",
            Kind::Barcode => "Barcode",
            Kind::Qr => "QR",
            Kind::DataMatrix => "DataMatrix",
            Kind::Rect => "Rectangle",
            Kind::Line => "Line",
        }
    }

    /// The button's accessible name: what it does, not what it shows.
    pub fn button_name(self) -> &'static str {
        match self {
            Kind::Text => "Add text",
            Kind::Barcode => "Add barcode",
            Kind::Qr => "Add QR code",
            Kind::DataMatrix => "Add DataMatrix",
            Kind::Rect => "Add rectangle",
            Kind::Line => "Add line",
        }
    }

    /// The chord, spelled the way the button's accessible description announces it.
    pub fn keys(self) -> String {
        format!("Control+Shift+{}", self.letter())
    }
}

/// The edit bar's seventh tool, which adds an image by importing artwork rather
/// than making an element from nothing: the token after every [`Kind`]'s.
pub const IMAGE_TOOL: i32 = 6;
/// The image tool's face: it opens a dialog, so it ends in an ellipsis.
pub const IMAGE_CAPTION: &str = "Image\u{2026}";
/// The image tool's accessible name, for [`Kind::button_name`]'s reason.
pub const IMAGE_BUTTON_NAME: &str = "Add image";
/// The image tool's chord, spelled as [`Kind::keys`] spells the others'. I is no
/// input method's and no platform's with Control and Shift that this build knows of.
pub const IMAGE_KEYS: &str = "Control+Shift+I";

/// The tool a key's text asks for with Control and Shift held: a [`Kind`]'s token,
/// [`IMAGE_TOOL`] for I in either case, and -1 for any other text.
pub fn tool_of_letter(s: &str) -> i32 {
    if s.eq_ignore_ascii_case("i") {
        return IMAGE_TOOL;
    }
    Kind::from_letter(s).map_or(-1, Kind::token)
}

/// The module size a 2D symbol starts at when it fits, and the one a barcode
/// starts at on a printer whose head lies along the short axis.
const WIDE_MODULE: u16 = 3;

/// A new element of `kind`, with id `id`, sized and placed for this label, and
/// what the operator should hear about how it was sized or where it went.
///
/// `others` are where the elements already drawn put ink ([`geom::solid`]), which the
/// new one is kept a millimetre clear of.
///
/// Defaults are chosen in canvas terms, `w` across and `h` down, and stored
/// through [`Target::extent`], which is its own inverse, so a Top printer gets the
/// same picture as a Left one. Every kind starts unnamed, upright, black, shown
/// and unlocked.
pub fn fresh(
    kind: Kind,
    id: ElementId,
    label: &Label,
    target: &Target,
    direction: PrintDirection,
    bounds: &Bounds,
    others: &[geom::Solid],
) -> (Element, Option<String>) {
    let dpmm = label.canvas.dpmm;
    let mm = geom::millimetre(dpmm);
    let mms = |tenths: i32| dpmm.dots_from_tenths_mm(tenths).max(1);
    let [_, _, aw, ah] = area(label, bounds, target);
    let mut notes = Vec::new();
    // Placed at the origin for now: the lengths are what the placement needs.
    let common = |w: i32, h: i32| {
        let (feed_len, head_len) = target.extent(w, h);
        Common::new(id, frame(0, 0, feed_len, head_len))
    };
    let mut e = match kind {
        Kind::Text => {
            // Two and a half millimetres of em, which is twenty dots at 203 dpi -
            // measured clean in a 96 by 24 frame - and a frame one line tall: the
            // em and a fifth, rounded up, which is the line box the layout uses.
            let size = mms(25);
            let w = mms(120);
            let h = (size * 6 + 4) / 5;
            Element::Text(TextEl {
                common: common(w, h),
                text: "Text".into(),
                // Written, not left empty: the default family is the renderer's
                // choice today and a later build's choice after, and a new label
                // should say which face it was drawn with.
                family: text::SANS.into(),
                size: size as f32,
                line_height: 1.0,
                align: TextAlign::Start,
                weight: TextWeight::Regular,
                raster: GlyphRaster::Hinted,
            })
        }
        Kind::Barcode => {
            // Exactly as long as its symbol, because a barcode is refused rather
            // than clipped when its frame is one dot short, and a longer frame is
            // only blank paper.
            let data = "123456";
            let len = |m: u16| {
                barcode::width_dots(Symbology::Code128, data, m)
                    .map_or(0, |d| i32::try_from(d).unwrap_or(i32::MAX))
            };
            let mut m = barcode::default_module_dots(direction);
            let mut w = len(m);
            if w + 2 * mm > aw && m > MIN_MODULE_DOTS {
                let wanted = m;
                m = MIN_MODULE_DOTS;
                w = len(m);
                notes.push(format!(
                    "its module is {m} dots, because {wanted} would not fit"
                ));
            }
            let h = mms(50).min(ah);
            Element::Barcode(BarcodeEl {
                common: common(w, h),
                symbology: Symbology::Code128.name().into(),
                data: data.into(),
                module_dots: m,
                bar_dots: 0,
                out_of_spec: false,
            })
        }
        Kind::Qr => {
            let data = "relabel";
            let grid = code2d::qr(data, QrEc::M, None)
                .expect("a constant seven letters encode as a QR code")
                .0;
            let (m, (w, h)) = symbol(&grid, aw, ah);
            Element::Qr(QrEl {
                common: common(w, h),
                data: data.into(),
                module_dots: m,
                ec: QrEc::M,
                // Not pinned: a label with no data binding shows one value, and a
                // pinned version is a batch's concern.
                version: None,
                out_of_spec: false,
            })
        }
        Kind::DataMatrix => {
            let data = "12345";
            // The smallest square that holds the data, which is the one the
            // rasteriser chooses for a frame the symbol's own size.
            let grid = code2d::data_matrix(data, &DmSize::Square, |_| true)
                .expect("a constant five digits encode as a DataMatrix")
                .0;
            let (m, (w, h)) = symbol(&grid, aw, ah);
            Element::DataMatrix(DataMatrixEl {
                common: common(w, h),
                data: data.into(),
                module_dots: m,
                size: DmSize::Square,
                out_of_spec: false,
            })
        }
        Kind::Rect => Element::Rect(RectEl {
            common: common(mms(50), mms(30)),
            stroke_dots: 2,
            radius_dots: 0,
        }),
        Kind::Line => Element::Line(LineEl {
            // As thick as its frame, so the frame is the ink on either printer.
            common: common(mms(120), 2),
            thickness_dots: 2,
        }),
    };
    notes.extend(place(&mut e, label, target, bounds, others));
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    (e, note)
}

/// Put a new element, sized and at the origin, where [`fresh`] puts every new
/// element: a millimetre inside the area a new element may go, at the first place
/// clear of the ink in `others` ([`spot`]), kept reachable. Returns what the
/// operator should hear about where it went: that it overlaps another element, is
/// larger than the area, runs off the label or lands in a keep-out.
///
/// Shared with an imported image, which arrives already sized for its artwork.
pub fn place(
    e: &mut Element,
    label: &Label,
    target: &Target,
    bounds: &Bounds,
    others: &[geom::Solid],
) -> Vec<String> {
    let mm = geom::millimetre(label.canvas.dpmm);
    let a = area(label, bounds, target);
    let [_, _, aw, ah] = a;
    let mut notes = Vec::new();
    let c = crate::ins::common_of(e);
    let (w, h) = target.extent(c.frame.feed_len, c.frame.head_len);
    let at = |x: i32, y: i32| {
        let (feed, head) = geom::to_document(target, x, y);
        RectDots {
            feed,
            head,
            ..c.frame
        }
    };
    let clear = |x: i32, y: i32| {
        let f = at(x, y);
        geom::inside_canvas(&bounds.canvas, &f) && bounds.keepout(&f).is_none()
    };
    let (x, y, overlapped) = spot((w, h), a, mm, others, clear);
    if overlapped {
        notes.push("it overlaps another element".into());
    }
    if w > aw || h > ah {
        notes.push(format!(
            "it is {w} by {h} dots and the label has room for {aw} by {ah}"
        ));
    }
    let f = at(x, y);
    let (feed, head) = geom::keep_reachable(bounds, f, f.feed, f.head);
    c.frame = RectDots { feed, head, ..f };
    // Where no place was clear, what is wrong with the one taken is said as well as
    // tinted: the status line is all a screen reader user has after an add. Asked of
    // the frame as it landed, in `Doc::shown`'s order, since a frame off the label
    // reads to `keepout` as the tail.
    let drawn = geom::as_drawn(c.frame, c.rot);
    if !geom::inside_canvas(&bounds.canvas, &drawn) {
        notes.push("it runs off the label".into());
    } else if let Some(k) = bounds.keepout(&drawn) {
        let where_ = crate::a11y::keepout_phrase(k);
        notes.push(match k {
            media::Keepout::Repeated => where_.to_string(),
            _ => format!("it is {where_}"),
        });
    }
    notes
}

/// The most a new image may be, in canvas dots across and down: the area a new
/// element may go ([`area`]) less a millimetre each side, which is where
/// [`place`] can put it clear of the edges. An axis two millimetres long or less
/// keeps its whole length, since a millimetre either side would leave it none.
///
/// What [`relabel_render::artwork::import_within`] shrinks new artwork to fit,
/// never enlarging it.
pub fn import_box(label: &Label, bounds: &Bounds, target: &Target) -> (Option<u32>, Option<u32>) {
    let mm = geom::millimetre(label.canvas.dpmm);
    let [_, _, aw, ah] = area(label, bounds, target);
    let less = |len: i32| {
        let inner = if len > 2 * mm { len - 2 * mm } else { len };
        u32::try_from(inner).ok().filter(|&v| v > 0)
    };
    (less(aw), less(ah))
}

/// A 2D symbol's module and footprint: three dots a module when the symbol and its
/// quiet zone fit the area that way, and the two-dot floor when they do not.
fn symbol(grid: &ModuleGrid, aw: i32, ah: i32) -> (u16, (i32, i32)) {
    let dots = |m: u16| {
        let (w, h) = grid.footprint(m);
        (
            i32::try_from(w).unwrap_or(i32::MAX),
            i32::try_from(h).unwrap_or(i32::MAX),
        )
    };
    let (w, h) = dots(WIDE_MODULE);
    if w <= aw && h <= ah {
        (WIDE_MODULE, (w, h))
    } else {
        (MIN_MODULE_DOTS, dots(MIN_MODULE_DOTS))
    }
}

/// Where a new element may go, as a canvas `(x, y, w, h)`: the usable area, and of
/// that only the panel a repeat copies from, because an element anywhere else is
/// overwritten by the copy.
///
/// An axis the two leave no length on - an inset wider than the stock, a panel
/// wholly in the tail - uses the canvas's own, and the element is then placed where
/// preflight will say what is wrong with it, rather than nowhere.
pub fn area(label: &Label, bounds: &Bounds, target: &Target) -> [i32; 4] {
    let mut r = bounds.usable;
    if let Ok(Some(plan)) = media::plan(label) {
        r = intersect(r, plan.source);
    }
    let (x, y, w, h) = geom::on_canvas(target, r, Rot::D0);
    let (cx, cy, cw, ch) = geom::on_canvas(target, bounds.canvas, Rot::D0);
    let (x, w) = if w > 0 { (x, w) } else { (cx, cw) };
    let (y, h) = if h > 0 { (y, h) } else { (cy, ch) };
    [x, y, w, h]
}

/// The overlap of two rectangles, with no length where they do not meet.
fn intersect(a: RectDots, b: RectDots) -> RectDots {
    let axis = |a0: i32, al: i32, b0: i32, bl: i32| {
        let start = i64::from(a0).max(i64::from(b0));
        let end = (i64::from(a0) + i64::from(al)).min(i64::from(b0) + i64::from(bl));
        let clip = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        (clip(start), clip((end - start).max(0)))
    };
    let (feed, feed_len) = axis(a.feed, a.feed_len, b.feed, b.feed_len);
    let (head, head_len) = axis(a.head, a.head_len, b.head, b.head_len);
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

/// The canvas origin for a `(w, h)` element in `area`, and whether it lands on
/// another element's ink.
///
/// Scanned in reading order, a millimetre at a time, from a millimetre inside the
/// area to a millimetre short of its far edge: the first origin at which the
/// element, grown by a millimetre along every axis with room for a millimetre either
/// side, holds none of the ink in `others`, and `clear` accepts, is taken. An axis
/// too short for the margins centres the element and keeps nothing clear along it,
/// so an 87-dot QR centred across a 96-dot head needs only to miss other ink there;
/// one too short for the element starts it at the area's edge. When no origin
/// qualifies, the scan's first one is used.
///
/// Ink is [`geom::covers`]'s: a line counts as deep as its thickness wherever that
/// runs past its frame, a frame with no area holds none unless it is a line with
/// length, and neither does the paper inside a stroked rectangle, so the inside of a
/// border is room to place on.
/// Boxes are half-open, so an element may end on the dot another's margin starts.
pub fn spot(
    size: (i32, i32),
    area: [i32; 4],
    mm: i32,
    others: &[geom::Solid],
    clear: impl Fn(i32, i32) -> bool,
) -> (i32, i32, bool) {
    let [ax, ay, aw, ah] = area;
    let (w, h) = size;
    let xs = starts(ax, aw, w, mm);
    let ys = starts(ay, ah, h, mm);
    let pad = |margins: bool| if margins { i64::from(mm) } else { 0 };
    let (pad_x, pad_y) = (pad(has_margins(w, aw, mm)), pad(has_margins(h, ah, mm)));
    let meets = |x: i32, y: i32, px: i64, py: i64| {
        let (x, y) = (i64::from(x), i64::from(y));
        let b = [x - px, x + i64::from(w) + px, y - py, y + i64::from(h) + py];
        others.iter().any(|s| geom::covers(s, b))
    };
    for &y in &ys {
        for &x in &xs {
            if !meets(x, y, pad_x, pad_y) && clear(x, y) {
                return (x, y, false);
            }
        }
    }
    let (x, y) = (xs[0], ys[0]);
    // Said only when it is so: an origin refused for its keep-out alone is on no
    // other element, and the canvas tints what is wrong with it.
    (x, y, w > 0 && h > 0 && meets(x, y, 0, 0))
}

/// Whether an axis `alen` dots long has room for `len` and a millimetre either side.
fn has_margins(len: i32, alen: i32, mm: i32) -> bool {
    len.saturating_add(mm.saturating_mul(2)) <= alen
}

/// The origins one axis offers, a millimetre apart, always including the last.
fn starts(a: i32, alen: i32, len: i32, mm: i32) -> Vec<i32> {
    let (lo, hi) = if has_margins(len, alen, mm) {
        (a + mm, a + alen - mm - len)
    } else if len <= alen {
        let s = a + (alen - len) / 2;
        (s, s)
    } else {
        (a, a)
    };
    let step = usize::try_from(mm.max(1)).unwrap_or(1);
    let mut v: Vec<i32> = (lo..=hi).step_by(step).collect();
    if v.last() != Some(&hi) {
        v.push(hi);
    }
    v
}

/// What the renderer makes of an element whose frame has just changed from
/// `before` to `after`, when that differs from what it made of it before.
///
/// The frame is all a resize writes: a symbol never scales, text keeps its em size
/// and artwork is resampled rather than imported again. So what a resize did to the
/// content is the renderer's answer, and this asks it - drawing a label holding
/// only this element, shown, with no repeat, once each way - rather than restating
/// its rules and drifting from them. Said: a symbol that will not be drawn and one
/// that is drawn again, text cut by its frame and text whole again, artwork
/// resampled or cropped and artwork drawn as imported or whole again, and a line
/// whose longer side now runs the other way. A fit written in the inspector and a
/// re-import are asked the same question, since each changes what the frame does
/// to the artwork.
///
/// Two renders of one element per committed change, never per pointer event: with
/// embedded artwork each is a PNG decode.
pub fn render_note(
    label: &Label,
    target: &Target,
    before: &Element,
    after: &Element,
) -> Option<String> {
    frame_notes(label, target, before, after, false)
}

/// [`render_note`] for artwork just imported again into `after`. Artwork that is
/// resampled or cropped as it lands is said so whether or not the old artwork
/// was: the new artwork was meant for the frame, and when the frame changed while
/// the file was being read, that is the only word the operator gets of it.
pub fn import_note(
    label: &Label,
    target: &Target,
    before: &Element,
    after: &Element,
) -> Option<String> {
    frame_notes(label, target, before, after, true)
}

fn frame_notes(
    label: &Label,
    target: &Target,
    before: &Element,
    after: &Element,
    imported: bool,
) -> Option<String> {
    let c = after.common();
    let who = describe(c.id, &c.name);
    let kind = kind_name(after);
    let was = issues(label, target, before);
    let now = issues(label, target, after);
    let mut notes = Vec::new();
    match (&was.failed, &now.failed) {
        (_, Some(f)) if was.failed.as_ref() != Some(f) => {
            notes.push(format!("{kind} {who} will not be drawn: {f}"));
        }
        (Some(_), None) => notes.push(format!("{kind} {who} is drawn again")),
        _ => {}
    }
    match (was.clipped, now.clipped) {
        (_, Some(clip)) if was.clipped != Some(clip) => {
            notes.push(format!("{kind} {who} is cut by its frame{}", cut(clip)));
        }
        (Some(_), None) if now.failed.is_none() => {
            notes.push(format!("{kind} {who} is no longer cut by its frame"));
        }
        _ => {}
    }
    if now.resampled && (imported || !was.resampled) {
        notes.push(format!(
            "the artwork of {who} is resampled for its frame until it is imported again"
        ));
    }
    // Only for artwork that is drawn: artwork that stopped drawing is not drawn as
    // imported, and the failure is said above.
    if was.resampled && !now.resampled && now.failed.is_none() {
        notes.push(format!("the artwork of {who} is drawn as imported again"));
    }
    if now.cropped && (imported || !was.cropped) {
        notes.push(format!("the artwork of {who} is cropped by its frame"));
    }
    if was.cropped && !now.cropped && now.failed.is_none() {
        notes.push(format!(
            "the artwork of {who} is no longer cropped by its frame"
        ));
    }
    if let (Element::Line(_), Element::Line(_)) = (before, after) {
        let along = |e: &Element| {
            let d = geom::as_drawn(e.common().frame, e.common().rot);
            d.feed_len >= d.head_len
        };
        if along(before) != along(after) {
            notes.push(if along(after) {
                format!("{who} now runs along the feed")
            } else {
                format!("{who} now runs across the head")
            });
        }
    }
    (!notes.is_empty()).then(|| notes.join("; "))
}

/// What the renderer reported about one element drawn on its own.
#[derive(Default)]
struct Issues {
    failed: Option<Failure>,
    clipped: Option<TextClip>,
    /// Whether, not how: a frame changed while the artwork was already resampled
    /// is still the same sentence, and the operator has heard it.
    resampled: bool,
    /// What the renderer reports as clipped, and a cover's crop, which it does not
    /// ([`crate::ins::cropped_by_frame`]).
    cropped: bool,
}

/// A label holding only `e`, shown, on `label`'s canvas and stock.
fn alone(label: &Label, e: &Element) -> Label {
    let mut one = Label::new(label.canvas.feed, label.canvas.head, label.canvas.dpmm);
    one.canvas = label.canvas;
    one.media = label.media.clone();
    let mut e = e.clone();
    crate::ins::common_of(&mut e).hidden = false;
    one.elements = vec![ElementOrRaw::Known(e)];
    one
}

fn issues(label: &Label, target: &Target, e: &Element) -> Issues {
    let one = alone(label, e);
    let id = e.common().id;
    let Ok((_, report)) = rasterise::rasterise(&one, target) else {
        return Issues::default();
    };
    Issues {
        failed: report
            .failed
            .into_iter()
            .find(|(i, _)| *i == id)
            .map(|(_, f)| f),
        clipped: report
            .text_clipped
            .into_iter()
            .find(|(i, _)| *i == id)
            .map(|(_, c)| c),
        resampled: report.image_resampled.iter().any(|r| r.0 == id),
        cropped: report.image_clipped.contains(&id)
            || match e {
                Element::Image(i) => rasterise::local_size(&i.common, target)
                    .is_some_and(|u| crate::ins::cropped_by_frame(i, u)),
                _ => false,
            },
    }
}

/// How many dots element `e` inks drawn on its own, and how many dots its frame
/// holds: the one-bit plane the printer is sent, with the element's own halftone
/// and threshold, and no repeat, so each dot is counted once. `None` when it is not
/// drawn or its frame has no area.
///
/// Measured on the element alone, not blended with what lies beneath it: the
/// share is the element's, and says whether its artwork prints at all. A white
/// element is measured in black, since the dots it knocks back to paper are none
/// at all on bare paper: the count is then the dots it knocks out.
pub fn ink(label: &Label, target: &Target, e: &Element) -> Option<(u64, u64)> {
    let f = e.common().frame;
    let area = u64::try_from(f.feed_len).ok()? * u64::try_from(f.head_len).ok()?;
    if area == 0 {
        return None;
    }
    let mut black = e.clone();
    crate::ins::common_of(&mut black).ink = Ink::Black;
    let mut one = alone(label, &black);
    one.media.fold = None;
    let (_, plane, report) = rasterise::rasterise_preview(&one, target).ok()?;
    if report.failed.iter().any(|(i, _)| *i == e.common().id) {
        return None;
    }
    let inked = plane.bits().iter().map(|b| u64::from(b.count_ones())).sum();
    Some((inked, area))
}

/// A share of a frame: nothing, every dot, or a whole percentage kept off 0 and
/// 100, so a few dots are never read as none or all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Share {
    Nothing,
    Percent(u64),
    Every,
}

impl Share {
    fn of((inked, area): (u64, u64)) -> Share {
        if inked == 0 {
            Share::Nothing
        } else if inked >= area {
            Share::Every
        } else {
            Share::Percent(((inked * 100 + area / 2) / area).clamp(1, 99))
        }
    }

    /// As what the element prints: "34% of its frame".
    fn said(self) -> String {
        match self {
            Share::Nothing => "nothing".into(),
            Share::Percent(p) => format!("{p}% of its frame"),
            Share::Every => "every dot of its frame".into(),
        }
    }

    /// As what it printed before, in brackets after the new share: "34%".
    fn short(self) -> String {
        match self {
            Share::Nothing => "nothing".into(),
            Share::Percent(p) => format!("{p}%"),
            Share::Every => "every dot".into(),
        }
    }
}

/// What an element does to the dots it draws: prints them, or, in white ink,
/// knocks them out of whatever lies beneath.
fn verb(e: &Element) -> &'static str {
    match e.common().ink {
        Ink::Black => "prints",
        Ink::White => "knocks out",
    }
}

/// How much of its frame `e` prints, as a clause of the sentence an import ends
/// with: "it prints 34% of its frame", "it prints nothing", and for white ink "it
/// knocks out 34% of its frame". `None` when it is not drawn, which the sentence
/// says in its own words.
pub fn prints(label: &Label, target: &Target, e: &Element) -> Option<String> {
    ink(label, target, e).map(|i| format!("it {} {}", verb(e), Share::of(i).said()))
}

/// What a halftone or threshold change from `before` to `after` does to how much
/// of the frame prints, when that is said differently from before: "Image element
/// 1 now prints 41% of its frame (was 36%)".
///
/// Until the grey half of the print pane is built, this is how a change of
/// halftone or threshold is judged without looking at the canvas. Two one-element
/// renders per write: a committed value, or one step of the arrows in the
/// threshold or halftone field, which is a write of its own; never per typed
/// character.
pub fn ink_note(
    label: &Label,
    target: &Target,
    before: &Element,
    after: &Element,
) -> Option<String> {
    let now = Share::of(ink(label, target, after)?);
    let was = ink(label, target, before).map(Share::of);
    if was == Some(now) {
        return None;
    }
    let c = after.common();
    let mut note = format!(
        "{} {} now {} {}",
        kind_name(after),
        describe(c.id, &c.name),
        verb(after),
        now.said()
    );
    if let Some(w) = was {
        note.push_str(&format!(" (was {})", w.short()));
    }
    Some(note)
}

/// How text is cut, in the terms the operator can act on: whole lines first, then a
/// word wider than the frame, then ink alone.
fn cut(clip: TextClip) -> String {
    if clip.lines_whole < clip.lines {
        format!(": {} of {} lines whole", clip.lines_whole, clip.lines)
    } else if clip.overhang_dots > 0 {
        format!(": a word runs {} dots past its width", clip.overhang_dots)
    } else {
        format!(": {} dots of ink are cut", clip.ink_clipped_dots)
    }
}

fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Cmd, Doc};
    use crate::ins::Field;
    use relabel_render::document::{ImageEl, ImageFit, ImageHalftone, KnownShape, MediaShape};
    use relabel_render::media::Keepout;
    use relabel_render::target::Dpmm;

    fn open(l: Label, direction: PrintDirection) -> Doc {
        Doc::open(l, direction).expect("a printable label")
    }

    fn last(doc: &Doc) -> &Element {
        doc.label()
            .elements
            .iter()
            .rev()
            .find_map(ElementOrRaw::known)
            .expect("an element was added")
    }

    fn created(l: Label, direction: PrintDirection, kind: Kind) -> Doc {
        let mut doc = open(l, direction);
        doc.apply(Cmd::Create(kind));
        doc
    }

    fn commit(doc: &mut Doc, field: Field, text: &str) -> Option<String> {
        doc.apply(Cmd::StageField {
            field,
            text: text.into(),
        });
        doc.apply(Cmd::CommitField).note
    }

    /// Every default is drawn, on its own label, on a Left and a Top printer at
    /// 203 dpi and a Left one at 300: nothing refused, clipped or in a keep-out.
    #[test]
    fn every_new_kind_draws_clean_inside_the_usable_area_on_three_labels() {
        for (feed, head, dpmm, direction) in [
            (320, 96, Dpmm::DPI_203, PrintDirection::Left),
            (240, 384, Dpmm::DPI_203, PrintDirection::Top),
            (472, 142, Dpmm::DPI_300, PrintDirection::Left),
        ] {
            for kind in Kind::ALL {
                let doc = created(Label::new(feed, head, dpmm), direction, kind);
                let ctx = format!("{kind:?} on {feed}x{head} {direction:?}");
                let (_, report) = rasterise::rasterise(doc.label(), doc.target()).expect("drawn");
                assert!(report.is_clean(), "{ctx}: {report:?}");
                let f = last(&doc).common().frame;
                let usable = Bounds::of(doc.label()).usable;
                assert!(
                    geom::inside_canvas(&usable, &f),
                    "{ctx}: {f:?} is not inside {usable:?}"
                );
            }
        }
    }

    #[test]
    fn a_new_barcode_is_as_long_as_its_symbol() {
        let doc = created(
            Label::new(320, 96, Dpmm::DPI_203),
            PrintDirection::Left,
            Kind::Barcode,
        );
        let Element::Barcode(b) = last(&doc) else {
            panic!("a barcode");
        };
        assert_eq!(
            (b.module_dots, b.data.as_str(), b.bar_dots),
            (3, "123456", 0)
        );
        assert_eq!(
            (b.common.frame.feed_len, b.common.frame.head_len),
            (264, 40)
        );

        let doc = created(
            Label::new(240, 384, Dpmm::DPI_203),
            PrintDirection::Top,
            Kind::Barcode,
        );
        let Element::Barcode(b) = last(&doc) else {
            panic!("a barcode");
        };
        assert_eq!(b.module_dots, 2);
        assert_eq!(
            (b.common.frame.head_len, b.common.frame.feed_len),
            (176, 40),
            "a Top printer's head is across the canvas"
        );
    }

    #[test]
    fn a_barcode_on_a_thirty_millimetre_label_drops_to_two_dot_modules() {
        let mut doc = open(Label::new(240, 96, Dpmm::DPI_203), PrintDirection::Left);
        let out = doc.apply(Cmd::Create(Kind::Barcode));
        let Element::Barcode(b) = last(&doc) else {
            panic!("a barcode");
        };
        assert_eq!((b.module_dots, b.common.frame.feed_len), (2, 176));
        let note = out.note.expect("said");
        assert!(note.contains("module is 2 dots"), "{note}");
    }

    /// Eighty-seven dots of symbol and quiet zone on a 96-dot head leave no room for
    /// a millimetre either side, so the symbol keeps its module and gives up the
    /// margin.
    #[test]
    fn a_new_qr_is_centred_across_a_head_too_narrow_for_its_margins() {
        let doc = created(
            Label::new(320, 96, Dpmm::DPI_203),
            PrintDirection::Left,
            Kind::Qr,
        );
        let Element::Qr(q) = last(&doc) else {
            panic!("a QR code");
        };
        assert_eq!(q.common.frame, frame(8, 4, 87, 87));
        assert_eq!((q.module_dots, q.version, q.ec), (3, None, QrEc::M));
    }

    #[test]
    fn a_new_text_names_its_face_and_fits_one_line() {
        let doc = created(
            Label::new(320, 96, Dpmm::DPI_203),
            PrintDirection::Left,
            Kind::Text,
        );
        let Element::Text(t) = last(&doc) else {
            panic!("a text");
        };
        assert_eq!(t.family, text::SANS);
        assert_eq!((t.size, t.line_height), (20.0, 1.0));
        assert_eq!(t.raster, GlyphRaster::Hinted);
        assert_eq!(t.common.frame, frame(8, 8, 96, 24));
    }

    #[test]
    fn a_new_line_is_as_thick_as_its_frame_on_both_printers() {
        for (feed, head, direction) in [
            (320, 96, PrintDirection::Left),
            (240, 384, PrintDirection::Top),
        ] {
            let doc = created(Label::new(feed, head, Dpmm::DPI_203), direction, Kind::Line);
            let Element::Line(l) = last(&doc) else {
                panic!("a line");
            };
            assert_eq!(l.thickness_dots, 2);
            let f = l.common.frame;
            let stored = match direction {
                PrintDirection::Left => (f.feed_len, f.head_len),
                PrintDirection::Top => (f.head_len, f.feed_len),
            };
            assert_eq!(stored, (96, 2), "{direction:?}");
            let (_, _, w, h) = geom::on_canvas(doc.target(), f, l.common.rot);
            assert_eq!((w, h), (96, 2), "{direction:?}");
        }
    }

    #[test]
    fn a_new_element_stays_in_the_repeat_source_panel() {
        for json in [
            include_str!("../../relabel-render/tests/corpus/v1/fold-over.relabel"),
            include_str!("../../relabel-render/tests/corpus/v1/fold-head-axis.relabel"),
        ] {
            let mut l = Label::from_json(json).expect("a corpus label");
            l.elements.clear();
            let doc = created(l, PrintDirection::Left, Kind::Rect);
            let source = media::plan(doc.label())
                .expect("a plan")
                .expect("it repeats")
                .source;
            let f = last(&doc).common().frame;
            assert!(
                geom::inside_canvas(&source, &f),
                "{f:?} is not inside {source:?}"
            );
            assert_eq!(Bounds::of(doc.label()).keepout(&f), None, "{f:?}");
        }
    }

    #[test]
    fn a_new_element_that_cannot_land_clear_of_a_keep_out_says_where_it_is() {
        for direction in [PrintDirection::Left, PrintDirection::Top] {
            for kind in [Kind::Text, Kind::Qr, Kind::Line] {
                let mut l = Label::new(96, 96, Dpmm::DPI_203);
                l.media.shape = MediaShape::Known(KnownShape::Circle);
                let mut doc = open(l, direction);
                let note = doc.apply(Cmd::Create(kind)).note.unwrap_or_default();
                let f = last(&doc).common().frame;
                assert_eq!(
                    Bounds::of(doc.label()).keepout(&f),
                    Some(Keepout::Shape),
                    "{kind:?} on {direction:?}: the case under test"
                );
                assert!(
                    note.contains("outside the die-cut"),
                    "{kind:?} on {direction:?}: {note}"
                );
            }
        }
    }

    /// Ink across the whole frame.
    fn plain(x: i32, y: i32, w: i32, h: i32) -> geom::Solid {
        geom::Solid {
            frame: (x, y, w, h),
            ink: (x, y, w, h),
            hollow: None,
        }
    }

    #[test]
    fn a_new_element_lands_inside_a_border_a_millimetre_clear_of_its_stroke() {
        let area = [0, 0, 320, 96];
        let anywhere = |_: i32, _: i32| true;
        let border = geom::Solid {
            hollow: Some(2),
            ..plain(0, 0, 320, 96)
        };
        let three = [
            plain(8, 8, 40, 24),
            plain(120, 8, 40, 24),
            plain(240, 40, 40, 24),
        ];
        assert_eq!(
            spot((40, 24), area, 8, &[border], anywhere),
            (16, 16, false)
        );
        let bordered: Vec<geom::Solid> = std::iter::once(border).chain(three).collect();
        assert_eq!(
            spot((40, 24), area, 8, &bordered, anywhere),
            (56, 16, false)
        );
        let filled: Vec<geom::Solid> = std::iter::once(plain(0, 0, 320, 96)).chain(three).collect();
        assert_eq!(spot((40, 24), area, 8, &filled, anywhere), (8, 8, true));
        let inset = geom::Solid {
            hollow: Some(2),
            ..plain(8, 8, 304, 80)
        };
        assert_eq!(spot((40, 24), area, 8, &[inset], anywhere), (24, 24, false));
        assert_eq!(spot((87, 87), area, 8, &[border], anywhere), (16, 4, false));
        assert_eq!(
            spot(
                (87, 87),
                area,
                8,
                &[border, plain(16, 16, 96, 24)],
                anywhere
            ),
            (120, 4, false)
        );
    }

    #[test]
    fn a_millimetre_is_kept_only_along_an_axis_with_room_for_one_either_side() {
        let area = [0, 0, 320, 96];
        let anywhere = |_: i32, _: i32| true;
        let rule = [plain(0, 0, 60, 2)];
        // A millimetre kept down the head as well gave (72, 4).
        assert_eq!(spot((87, 87), area, 8, &rule, anywhere), (8, 4, false));
        assert_eq!(spot((40, 24), area, 8, &rule, anywhere), (72, 8, false));
    }

    #[test]
    fn the_spot_is_a_millimetre_inside_and_a_millimetre_clear() {
        let area = [0, 0, 320, 96];
        let anywhere = |_: i32, _: i32| true;
        assert_eq!(spot((40, 24), area, 8, &[], anywhere), (8, 8, false));
        let three = [
            plain(8, 8, 40, 24),
            plain(120, 8, 40, 24),
            plain(240, 40, 40, 24),
        ];
        assert_eq!(spot((40, 24), area, 8, &three, anywhere), (56, 8, false));
        assert_eq!(
            spot((40, 24), area, 8, &[plain(0, 0, 320, 96)], anywhere),
            (8, 8, true)
        );
        // A frame with no area is nothing to keep clear of.
        assert_eq!(
            spot((40, 24), area, 8, &[plain(8, 8, 0, 24)], anywhere),
            (8, 8, false)
        );
        // The scan's last origin on an axis is offered even off the millimetre grid.
        assert_eq!(
            spot((40, 24), [0, 0, 61, 96], 8, &[], |x, _| x == 13),
            (13, 8, false)
        );
        // Refused only by `clear`, the fallback is on no other element.
        assert_eq!(spot((40, 24), area, 8, &[], |_, _| false), (8, 8, false));
    }

    #[test]
    fn a_rectangle_added_to_a_bordered_label_is_placed_as_if_the_border_were_paper() {
        let mut l = Label::new(320, 96, Dpmm::DPI_203);
        l.elements = vec![ElementOrRaw::Known(Element::Rect(RectEl {
            common: Common::new(1, frame(0, 0, 320, 96)),
            stroke_dots: 2,
            radius_dots: 0,
        }))];
        let mut doc = open(l, PrintDirection::Left);
        let out = doc.apply(Cmd::Create(Kind::Rect));
        assert_eq!(last(&doc).common().frame, frame(16, 16, 40, 24));
        assert_eq!(
            out.note.as_deref(),
            Some("added Rectangle element 2 at feed 16, head 16, 40 by 24 dots")
        );
        let out = doc.apply(Cmd::Create(Kind::Rect));
        assert_eq!(last(&doc).common().frame, frame(64, 16, 40, 24));
        let note = out.note.unwrap_or_default();
        assert!(!note.contains("overlaps"), "{note}");
    }

    /// Every element of the corpus label but its border is ink across its frame, and
    /// they leave no clear place, so the rule hides no real overlap.
    #[test]
    fn the_kitchen_sink_label_is_still_full_apart_from_its_border() {
        let l = Label::from_json(include_str!(
            "../../relabel-render/tests/corpus/v1/kitchen-sink.relabel"
        ))
        .expect("a corpus label");
        let mut doc = open(l, PrintDirection::Left);
        let note = doc.apply(Cmd::Create(Kind::Rect)).note.unwrap_or_default();
        assert_eq!(last(&doc).common().frame, frame(8, 8, 40, 24));
        assert!(note.contains("it overlaps another element"), "{note}");
    }

    #[test]
    fn the_kind_letters_are_the_chords_the_buttons_announce() {
        assert_eq!(Kind::ALL.map(Kind::token), [0, 1, 2, 3, 4, 5]);
        for k in Kind::ALL {
            assert_eq!(Kind::from_token(k.token()), Some(k));
            let letter = k.letter();
            assert_eq!(
                k.keys(),
                format!("Control+Shift+{}", letter.to_ascii_uppercase())
            );
            assert_eq!(Kind::from_letter(&letter.to_string()), Some(k));
        }
        assert_eq!(Kind::from_token(6), None);
        assert_eq!(Kind::from_token(-1), None);
        assert_eq!(Kind::from_letter("t"), Some(Kind::Text));
        assert_eq!(Kind::from_letter("T"), Some(Kind::Text));
        assert_eq!(Kind::from_letter("r"), Some(Kind::Qr));
        assert_eq!(Kind::from_letter("o"), Some(Kind::Rect));
        assert_eq!(Kind::from_letter("q"), None);
        assert_eq!(Kind::from_letter("z"), None);
        assert_eq!(Kind::from_letter(""), None);
        assert_eq!(Kind::from_letter("tt"), None);
    }

    #[test]
    fn shrinking_a_symbol_below_its_footprint_is_said_and_growing_it_back_too() {
        let mut doc = created(
            Label::new(320, 96, Dpmm::DPI_203),
            PrintDirection::Left,
            Kind::Qr,
        );
        let note = commit(&mut doc, Field::HeadLen, "80").expect("said");
        assert_eq!(last(&doc).common().frame.head_len, 80);
        assert!(note.contains("will not be drawn"), "{note}");
        assert!(note.contains("87"), "{note}");
        let note = commit(&mut doc, Field::HeadLen, "87").expect("said");
        assert!(note.contains("is drawn again"), "{note}");
        // And a size change the symbol does not notice says nothing.
        assert_eq!(commit(&mut doc, Field::HeadLen, "90"), None);
    }

    #[test]
    fn a_line_that_turns_axis_says_so() {
        let mut doc = created(
            Label::new(320, 96, Dpmm::DPI_203),
            PrintDirection::Left,
            Kind::Line,
        );
        let note = commit(&mut doc, Field::FeedLen, "1").expect("said");
        assert!(note.contains("now runs across the head"), "{note}");
        let note = commit(&mut doc, Field::FeedLen, "96").expect("said");
        assert!(note.contains("now runs along the feed"), "{note}");
    }

    #[test]
    fn a_text_frame_cut_short_is_said() {
        let mut doc = created(
            Label::new(320, 96, Dpmm::DPI_203),
            PrintDirection::Left,
            Kind::Text,
        );
        let note = commit(&mut doc, Field::HeadLen, "16").expect("said");
        assert!(note.contains("cut by its frame"), "{note}");
        let note = commit(&mut doc, Field::HeadLen, "24").expect("said");
        assert!(note.contains("no longer cut"), "{note}");
    }

    #[test]
    fn resizing_embedded_artwork_says_it_is_resampled() {
        let l = Label::from_json(include_str!(
            "../../relabel-render/tests/corpus/v1/images.relabel"
        ))
        .expect("a corpus label");
        let mut doc = open(l, PrintDirection::Left);
        doc.apply(Cmd::Select(Some(1)));
        let note = commit(&mut doc, Field::FeedLen, "16").expect("said");
        assert!(note.contains("resampled"), "{note}");
    }

    /// A line inks its thickness from the frame's origin whatever the frame's depth,
    /// so a thick line on a two-dot frame is ink forty dots deep, reached here through
    /// the inspector alone.
    #[test]
    fn a_new_element_keeps_clear_of_a_lines_ink_where_it_runs_past_the_lines_frame() {
        let mut doc = created(
            Label::new(320, 96, Dpmm::DPI_203),
            PrintDirection::Left,
            Kind::Line,
        );
        assert_eq!(last(&doc).common().frame, frame(8, 8, 96, 2));
        commit(&mut doc, Field::FeedLen, "304");
        commit(&mut doc, Field::ThicknessDots, "40");
        assert_eq!(last(&doc).common().frame, frame(8, 8, 304, 2));
        let out = doc.apply(Cmd::Create(Kind::Text));
        assert_eq!(last(&doc).common().frame, frame(8, 56, 96, 24));
        assert_eq!(
            out.note.as_deref(),
            Some("added Text element 2 at feed 8, head 56, 96 by 24 dots")
        );
    }

    #[test]
    fn the_image_tool_is_token_six_and_letter_i() {
        assert_eq!(IMAGE_TOOL, 6);
        assert_eq!(tool_of_letter("i"), IMAGE_TOOL);
        assert_eq!(tool_of_letter("I"), IMAGE_TOOL);
        assert_eq!(tool_of_letter("t"), Kind::Text.token());
        assert_eq!(tool_of_letter("L"), Kind::Line.token());
        assert_eq!(tool_of_letter("q"), -1);
        assert_eq!(tool_of_letter("ii"), -1);
        assert_eq!(tool_of_letter(""), -1);
        assert_eq!(Kind::from_token(IMAGE_TOOL), None);
        assert_eq!(Kind::ALL.len(), 6);
        assert_eq!(
            Kind::from_letter("i"),
            None,
            "no kind takes the image's letter"
        );
        assert_eq!(
            (IMAGE_CAPTION, IMAGE_BUTTON_NAME, IMAGE_KEYS),
            ("Image\u{2026}", "Add image", "Control+Shift+I")
        );
    }

    #[test]
    fn the_import_box_is_the_area_less_a_millimetre() {
        let box_of = |feed: i32, head: i32| {
            let doc = open(Label::new(feed, head, Dpmm::DPI_203), PrintDirection::Left);
            import_box(doc.label(), &Bounds::of(doc.label()), doc.target())
        };
        assert_eq!(box_of(320, 96), (Some(304), Some(80)));
        assert_eq!(box_of(240, 96), (Some(224), Some(80)));
        // Two millimetres or less: a millimetre either side would leave nothing.
        assert_eq!(box_of(16, 96), (Some(16), Some(80)));
        assert_eq!(box_of(320, 12), (Some(304), Some(12)));
        // A Top printer's head runs across the canvas: the box is in canvas terms.
        let doc = open(Label::new(240, 384, Dpmm::DPI_203), PrintDirection::Top);
        assert_eq!(
            import_box(doc.label(), &Bounds::of(doc.label()), doc.target()),
            (Some(368), Some(224))
        );
    }

    /// An image element holding exactly `art`.
    fn image(art: relabel_render::artwork::Artwork, f: RectDots, fit: ImageFit) -> Element {
        Element::Image(ImageEl {
            common: Common::new(1, f),
            path: "art.png".into(),
            png: crate::import::fixtures::stored(art),
            fit,
            halftone: Some(ImageHalftone::Threshold),
            threshold: Some(128),
        })
    }

    fn with_fit(e: &Element, fit: ImageFit) -> Element {
        let mut e = e.clone();
        if let Element::Image(i) = &mut e {
            i.fit = fit;
        }
        e
    }

    fn with_frame(e: &Element, f: RectDots) -> Element {
        let mut e = e.clone();
        crate::ins::common_of(&mut e).frame = f;
        e
    }

    fn with_threshold(e: &Element, t: u8) -> Element {
        let mut e = e.clone();
        if let Element::Image(i) = &mut e {
            i.threshold = Some(t);
        }
        e
    }

    #[test]
    fn render_note_says_when_artwork_is_drawn_as_imported_again() {
        let doc = open(Label::new(320, 96, Dpmm::DPI_203), PrintDirection::Left);
        let (l, t) = (doc.label(), doc.target());
        let art = crate::import::fixtures::art(48, 32, |x, _| if x < 24 { 0 } else { 255 });
        let stretched = image(art.clone(), frame(8, 8, 64, 32), ImageFit::Stretch);
        let contained = with_fit(&stretched, ImageFit::Contain);
        let note = render_note(l, t, &contained, &stretched).expect("said");
        assert!(note.contains("resampled for its frame"), "{note}");
        let note = render_note(l, t, &stretched, &contained).expect("said");
        assert_eq!(note, "the artwork of element 1 is drawn as imported again");

        let cropped = image(art, frame(8, 8, 40, 32), ImageFit::Dots);
        let whole = with_frame(&cropped, frame(8, 8, 48, 32));
        let note = render_note(l, t, &whole, &cropped).expect("said");
        assert_eq!(note, "the artwork of element 1 is cropped by its frame");
        let note = render_note(l, t, &cropped, &whole).expect("said");
        assert_eq!(
            note,
            "the artwork of element 1 is no longer cropped by its frame"
        );
        // A cover made shallower than the frame it was imported for resamples
        // nothing, and the renderer reports no clip, but most of it is cut away.
        let square = crate::import::fixtures::art(32, 32, |x, _| if x < 16 { 0 } else { 255 });
        let covered = image(square, frame(8, 8, 32, 32), ImageFit::Cover);
        let shallow = with_frame(&covered, frame(8, 8, 32, 8));
        let note = render_note(l, t, &covered, &shallow).expect("said");
        assert_eq!(note, "the artwork of element 1 is cropped by its frame");
        let note = render_note(l, t, &shallow, &covered).expect("said");
        assert_eq!(
            note,
            "the artwork of element 1 is no longer cropped by its frame"
        );
        // Artwork that stops drawing is said to, and not to be drawn as imported.
        let tiled = with_fit(&stretched, ImageFit::Unknown("tile".into()));
        let note = render_note(l, t, &stretched, &tiled).expect("said");
        assert!(note.contains("will not be drawn"), "{note}");
        assert!(!note.contains("as imported"), "{note}");
        // Freshly imported artwork that is resampled is said to be, even when the
        // artwork it replaced was too.
        assert_eq!(render_note(l, t, &stretched, &stretched), None);
        let note = import_note(l, t, &stretched, &stretched).expect("said");
        assert!(note.contains("resampled for its frame"), "{note}");
    }

    #[test]
    fn ink_note_says_how_much_of_the_frame_prints() {
        let doc = open(Label::new(320, 96, Dpmm::DPI_203), PrintDirection::Left);
        let (l, t) = (doc.label(), doc.target());
        let ramp = crate::import::fixtures::art(64, 32, |x, _| (x * 4) as u8);
        let at_128 = image(ramp, frame(8, 8, 64, 32), ImageFit::Contain);
        let at_180 = with_threshold(&at_128, 180);
        let note = ink_note(l, t, &at_128, &at_180).expect("said");
        let pct = |s: &str| -> u64 {
            s.split('%')
                .next()
                .unwrap()
                .rsplit(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        let (p, q) = (pct(&note), pct(note.split("(was ").nth(1).expect("was")));
        assert!(
            note.starts_with("Image element 1 now prints ") && note.ends_with("%)") && p > q,
            "{note}"
        );
        assert_eq!(
            prints(l, t, &at_128).as_deref(),
            Some(format!("it prints {q}% of its frame").as_str())
        );
        // An unchanged phrase is not said again.
        assert_eq!(ink_note(l, t, &at_128, &with_threshold(&at_128, 129)), None);

        let white = image(
            crate::import::fixtures::art(32, 32, |_, _| 255),
            frame(8, 8, 32, 32),
            ImageFit::Contain,
        );
        let note = ink_note(l, t, &with_threshold(&white, 255), &white).expect("said");
        assert_eq!(note, "Image element 1 now prints nothing (was every dot)");
        assert_eq!(prints(l, t, &white).as_deref(), Some("it prints nothing"));

        let black = image(
            crate::import::fixtures::art(32, 32, |_, _| 0),
            frame(8, 8, 64, 32),
            ImageFit::Contain,
        );
        let filled = with_fit(&black, ImageFit::Stretch);
        let note = ink_note(l, t, &black, &filled).expect("said");
        assert_eq!(
            note,
            "Image element 1 now prints every dot of its frame (was 50%)"
        );
        // Not drawn is not a share.
        let unknown = {
            let mut e = filled.clone();
            if let Element::Image(i) = &mut e {
                i.halftone = Some(ImageHalftone::Unknown("blue_noise".into()));
            }
            e
        };
        assert_eq!(prints(l, t, &unknown), None);
        assert_eq!(ink_note(l, t, &filled, &unknown), None);
        let note = ink_note(l, t, &unknown, &filled).expect("said");
        assert_eq!(note, "Image element 1 now prints every dot of its frame");
    }

    /// White ink knocks dots back to paper, which drawn alone on paper is no dot at
    /// all: the share is said as what it knocks out, and a change to it is heard.
    #[test]
    fn a_white_image_is_said_by_what_it_knocks_out() {
        let doc = open(Label::new(320, 96, Dpmm::DPI_203), PrintDirection::Left);
        let (l, t) = (doc.label(), doc.target());
        let white = |e: Element| {
            let mut e = e;
            crate::ins::common_of(&mut e).ink = relabel_render::document::Ink::White;
            e
        };
        let half = white(image(
            crate::import::fixtures::art(64, 32, |x, _| if x < 32 { 0 } else { 255 }),
            frame(8, 8, 64, 32),
            ImageFit::Contain,
        ));
        assert_eq!(
            prints(l, t, &half).as_deref(),
            Some("it knocks out 50% of its frame")
        );
        let ramp = white(image(
            crate::import::fixtures::art(64, 32, |x, _| (x * 4) as u8),
            frame(8, 8, 64, 32),
            ImageFit::Contain,
        ));
        let note = ink_note(l, t, &ramp, &with_threshold(&ramp, 250)).expect("said");
        assert!(
            note.starts_with("Image element 1 now knocks out ") && note.ends_with("%)"),
            "{note}"
        );
    }
}
