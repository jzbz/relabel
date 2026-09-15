//! The inspector's projection: one element as a fixed run of named, ranged,
//! already-formatted slots, and the one place a per-kind field is written.
//!
//! Here rather than in `.slint` because which fields a kind has is
//! [`Element::own_keys`]'s answer, what a value means is the document's, and a
//! pane that held a second copy of either would be the copy that went stale. No
//! toolkit type appears in any signature of the projection, which is what lets
//! every rule below be reached by an ordinary unit test rather than through a
//! simulated gesture. The toolkit appears only in [`wire`], which is the
//! inspector's half of what `canvas::wire` is for the canvas.

use crate::canvas::{Canvas, PanePush};
use crate::doc::{Cmd, Doc};
use crate::doc::{describe, kind_name};
use crate::geom;
use crate::rows::{InsHost, InsRow};
use relabel_render::barcode::{MIN_MODULE_DOTS, Symbology};
use relabel_render::code2d::DM_SIZES;
use relabel_render::document::{
    Common, DmSize, Element, ElementId, GlyphRaster, ImageFit, ImageHalftone, Ink, LoadError, QrEc,
    Rot, TextAlign, TextWeight,
};
use relabel_render::text;
use slint::{Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::LazyLock;

/// Which document field a slot is showing.
///
/// A Rust enum rather than a spelled key on the wire: the pane hands back the
/// token this module minted and never reads it, so a field this build does not
/// have cannot arrive as a typo, and the writer below can be an exhaustive match
/// per element type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    // Every kind.
    Name,
    Feed,
    Head,
    FeedLen,
    HeadLen,
    Rot,
    Ink,
    State,
    // Text.
    Text,
    Family,
    Size,
    LineHeight,
    Align,
    Weight,
    Raster,
    // Barcode.
    Symbology,
    Data,
    ModuleDots,
    BarDots,
    Spec,
    // QR.
    Ec,
    Version,
    // DataMatrix.
    DmSize,
    // Image.
    Path,
    Artwork,
    Fit,
    Halftone,
    Threshold,
    // Rectangle and line.
    StrokeDots,
    RadiusDots,
    ThicknessDots,
}

/// The eight fields every kind has, in pane and Tab order: the lengths beside the
/// placement they grow from.
const COMMON: [Field; 8] = [
    Field::Name,
    Field::Feed,
    Field::Head,
    Field::FeedLen,
    Field::HeadLen,
    Field::Rot,
    Field::Ink,
    Field::State,
];

/// The symbologies this build draws, spelled as the file spells them. Built from
/// `Symbology::ALL` rather than written out, so a symbology added to the render
/// crate is drawable here with no second list to update.
static SYMBOLOGIES: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| Symbology::ALL.iter().map(|s| s.name()).collect());

static SYMBOLOGY_UNIT: LazyLock<String> = LazyLock::new(|| spoken_list(&SYMBOLOGIES));

static FAMILY_UNIT: LazyLock<String> =
    LazyLock::new(|| format!("{} or {}", text::SANS, text::MONO));

impl Field {
    /// Every field, in one order, which is also the token order.
    pub const ALL: [Field; 31] = [
        Field::Name,
        Field::Feed,
        Field::Head,
        Field::FeedLen,
        Field::HeadLen,
        Field::Rot,
        Field::Ink,
        Field::State,
        Field::Text,
        Field::Family,
        Field::Size,
        Field::LineHeight,
        Field::Align,
        Field::Weight,
        Field::Raster,
        Field::Symbology,
        Field::Data,
        Field::ModuleDots,
        Field::BarDots,
        Field::Spec,
        Field::Ec,
        Field::Version,
        Field::DmSize,
        Field::Path,
        Field::Artwork,
        Field::Fit,
        Field::Halftone,
        Field::Threshold,
        Field::StrokeDots,
        Field::RadiusDots,
        Field::ThicknessDots,
    ];

    /// The opaque token the pane carries. The index into [`Field::ALL`], so the
    /// round trip is an array lookup rather than a second table to keep in step.
    pub fn token(self) -> i32 {
        self as i32
    }

    pub fn from_token(t: i32) -> Option<Field> {
        usize::try_from(t)
            .ok()
            .and_then(|i| Field::ALL.get(i).copied())
    }

    /// The word a reader hears first and a test addresses the field by.
    ///
    /// Feed and head, never x and y: `a11y`'s note names the inspector as one of
    /// the places the document's own axes are spoken, and a reader who types
    /// "feed 12" has to find the same 12 in the report that refuses it.
    pub fn label(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::Feed => "feed",
            Field::Head => "head",
            Field::FeedLen => "feed length",
            Field::HeadLen => "head length",
            Field::Rot => "rotation",
            Field::Ink => "ink",
            Field::State => "state",
            Field::Text => "text",
            Field::Family => "family",
            Field::Size => "em size",
            Field::LineHeight => "line height",
            Field::Align => "alignment",
            Field::Weight => "weight",
            Field::Raster => "rasterisation",
            Field::Symbology => "symbology",
            Field::Data => "data",
            Field::ModuleDots => "module",
            Field::BarDots => "bar height",
            Field::Spec => "specification",
            Field::Ec => "error correction",
            Field::Version => "version",
            Field::DmSize => "size",
            Field::Path => "source",
            Field::Artwork => "artwork",
            Field::Fit => "fit",
            Field::Halftone => "halftone",
            Field::Threshold => "threshold",
            Field::StrokeDots => "stroke",
            Field::RadiusDots => "corner radius",
            Field::ThicknessDots => "thickness",
        }
    }

    /// The unit, drawn after the field and announced as its description. For a
    /// word field it is what the word may be, which is the only place an
    /// operator can learn that the spelling is `floyd_steinberg`.
    ///
    /// Short, because it is drawn in one line no wider than the field it follows
    /// and anything longer is elided on screen; what an empty field means is the
    /// hint's to say, and why a value cannot be drawn is the note's.
    pub fn unit(self) -> &'static str {
        match self {
            Field::Name => "any words",
            Field::Feed | Field::Head | Field::FeedLen | Field::HeadLen => "dots",
            Field::Rot => "degrees",
            Field::Ink => "black or white",
            Field::State => "flags",
            Field::Text | Field::Data => "a template",
            Field::Family => FAMILY_UNIT.as_str(),
            Field::Size => "dots, over 0",
            Field::LineHeight => "ems, over 0",
            Field::Align => "start, center or end",
            Field::Weight => "regular or semibold",
            Field::Raster => "hinted or smooth",
            Field::Symbology => SYMBOLOGY_UNIT.as_str(),
            Field::ModuleDots => "dots per module",
            Field::BarDots => "dots; 0 is the frame",
            Field::Spec => "a flag",
            Field::Ec => "l, m, q or h",
            Field::Version => "1 to 40",
            Field::DmSize => "square, rectangle, any, or like 16x48",
            Field::Path => "imports nothing",
            Field::Artwork => "embedded PNG",
            Field::Fit => "contain, cover, stretch or dots",
            Field::Halftone => "threshold, floyd_steinberg, atkinson, sierra3 or bayer8",
            Field::Threshold => "grey level",
            Field::StrokeDots => "dots; 0 fills",
            Field::RadiusDots => "dots",
            Field::ThicknessDots => "dots",
        }
    }

    /// What an empty field means, shown as placeholder text. Empty when an empty
    /// field means nothing at all.
    pub fn hint(self) -> &'static str {
        match self {
            Field::Name => "no name",
            Field::Text => "no text",
            Field::Family => "the default face",
            Field::Symbology => "no symbology",
            Field::Data => "no data",
            Field::Version => "the smallest that fits",
            Field::DmSize => "no size",
            Field::Path => "not recorded",
            Field::Halftone | Field::Threshold => "the page's",
            _ => "",
        }
    }

    /// This field's key in the file, for a message that has to name something
    /// the operator can find in it. `None` for `State`, the row that is a sentence
    /// rather than a field, and for `Artwork`, which summarises `png`.
    pub fn key(self) -> Option<&'static str> {
        Some(match self {
            Field::Name => "name",
            Field::Feed => "frame.feed",
            Field::Head => "frame.head",
            Field::FeedLen => "frame.feed_len",
            Field::HeadLen => "frame.head_len",
            Field::Rot => "rot",
            Field::Ink => "ink",
            Field::State | Field::Artwork => return None,
            Field::Text => "text",
            Field::Family => "family",
            Field::Size | Field::DmSize => "size",
            Field::LineHeight => "line_height",
            Field::Align => "align",
            Field::Weight => "weight",
            Field::Raster => "raster",
            Field::Symbology => "symbology",
            Field::Data => "data",
            Field::ModuleDots => "module_dots",
            Field::BarDots => "bar_dots",
            Field::Spec => "out_of_spec",
            Field::Ec => "ec",
            Field::Version => "version",
            Field::Path => "path",
            Field::Fit => "fit",
            Field::Halftone => "halftone",
            Field::Threshold => "threshold",
            Field::StrokeDots => "stroke_dots",
            Field::RadiusDots => "radius_dots",
            Field::ThicknessDots => "thickness_dots",
        })
    }

    /// The spellings this build can draw, for a field whose value set is open:
    /// typing something else is kept and warned about rather than refused. The
    /// two closed sets list theirs too, for the refusal to quote. Empty for a
    /// field that has no such set.
    ///
    /// The symbologies and the two faces come from the render crate; the rest
    /// have no public list there, and
    /// `every_spelling_this_build_claims_to_draw_is_one_the_document_type_knows`
    /// is what keeps these from drifting.
    pub fn drawable_values(self) -> &'static [&'static str] {
        match self {
            Field::Ink => &["black", "white"],
            Field::Family => &[text::SANS, text::MONO],
            Field::Align => &["start", "center", "end"],
            Field::Weight => &["regular", "semibold"],
            Field::Raster => &["hinted", "smooth"],
            Field::Symbology => SYMBOLOGIES.as_slice(),
            Field::Ec => &["l", "m", "q", "h"],
            Field::DmSize => &["square", "rectangle", "any"],
            Field::Fit => &["contain", "cover", "stretch", "dots"],
            Field::Halftone => &[
                "threshold",
                "floyd_steinberg",
                "atkinson",
                "sierra3",
                "bayer8",
            ],
            _ => &[],
        }
    }

    /// Which control this field is. A property of the field alone: whether the
    /// element is locked is the slot's business, not the field's.
    pub fn shape(self) -> Shape {
        match self {
            Field::Feed
            | Field::Head
            | Field::FeedLen
            | Field::HeadLen
            | Field::Rot
            | Field::Size
            | Field::LineHeight
            | Field::ModuleDots
            | Field::BarDots
            | Field::Version
            | Field::Threshold
            | Field::StrokeDots
            | Field::ThicknessDots => Shape::Number,
            Field::State | Field::Spec | Field::Artwork | Field::RadiusDots => Shape::Read,
            _ => Shape::Word,
        }
    }

    /// The interval and the increment announced for a number field that is not a
    /// placement or a length. Their intervals depend on the element and the canvas,
    /// so [`common_slots`] is handed them.
    fn range(self) -> (f32, f32, f32) {
        match self {
            // A quarter turn is the only rotation this build draws exactly.
            Field::Rot => (0.0, 359.0, 90.0),
            // The document's bound is "more than 0", which an inclusive minimum
            // cannot say; the unit says it instead.
            Field::Size => (0.0, text::MAX_PPEM, 1.0),
            Field::LineHeight => (0.0, text::MAX_LINE_HEIGHT, 0.1),
            Field::ModuleDots | Field::BarDots | Field::StrokeDots | Field::ThicknessDots => {
                (0.0, f32::from(u16::MAX), 1.0)
            }
            Field::Version => (1.0, 40.0, 1.0),
            Field::Threshold => (0.0, 255.0, 1.0),
            Field::Feed | Field::Head | Field::FeedLen | Field::HeadLen => (0.0, 0.0, 1.0),
            _ => (0.0, 0.0, 0.0),
        }
    }
}

/// Which of the three controls a slot is, or none at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Unused,
    Number,
    Word,
    Read,
}

impl Shape {
    /// The integer `InsSlot.shape` carries. Not a Slint enum, for `ElRow.keepout`'s
    /// reason: a shape the pane does not know draws as nothing.
    pub fn ordinal(self) -> i32 {
        match self {
            Shape::Unused => 0,
            Shape::Number => 1,
            Shape::Word => 2,
            Shape::Read => 3,
        }
    }
}

/// One slot: already named, already formatted, already ranged.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub field: Field,
    pub shape: Shape,
    pub value: String,
    /// Announced to a reader, enforced by nobody here. The enforcing is
    /// [`write`]'s and [`crate::geom::keep_reachable`]'s.
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub read_only: bool,
}

impl Slot {
    /// An unused slot: no shape, no height, nothing in the accessibility tree.
    pub fn unused() -> Slot {
        Slot {
            field: Field::Name,
            shape: Shape::Unused,
            value: String::new(),
            min: 0.0,
            max: 0.0,
            step: 0.0,
            read_only: true,
        }
    }
}

/// How many slots the pane has.
///
/// Fifteen is what the widest kind needs - eight common rows plus a Text
/// element's seven - and one spare means the next field added is a change to this
/// file alone. Fixed, because a model whose length changed would emit a reset,
/// and a reset drops the instance holding the keyboard focus.
pub const SLOTS: usize = 16;

fn slot(e: &Element, f: Field, (min, max, step): (f32, f32, f32)) -> Slot {
    let shape = f.shape();
    Slot {
        field: f,
        shape,
        value: read(e, f),
        min,
        max,
        step,
        // A locked element announces every field read-only; the refusal itself
        // is `Doc::write_field`'s, because no widget's own set-value action
        // checks the flag.
        read_only: shape == Shape::Read || e.common().locked,
    }
}

/// The eight rows every kind has, in pane and Tab order.
///
/// `reach` is the interval a placement may land in on each axis, from
/// [`crate::geom::reach`] - the same call the editor clamps with, so the range a
/// reader is told and the range that is enforced cannot drift. `lens` is the same
/// for the feed length and the head length, from [`crate::geom::len_reach`].
pub fn common_slots(
    e: &Element,
    reach: ((i32, i32), (i32, i32)),
    lens: ((i32, i32), (i32, i32)),
) -> Vec<Slot> {
    let ((feed_lo, feed_hi), (head_lo, head_hi)) = reach;
    let ((feed_len_lo, feed_len_hi), (head_len_lo, head_len_hi)) = lens;
    COMMON
        .into_iter()
        .map(|f| {
            let range = match f {
                Field::Feed => (feed_lo as f32, feed_hi as f32, 1.0),
                Field::Head => (head_lo as f32, head_hi as f32, 1.0),
                Field::FeedLen => (feed_len_lo as f32, feed_len_hi as f32, 1.0),
                Field::HeadLen => (head_len_lo as f32, head_len_hi as f32, 1.0),
                other => other.range(),
            };
            slot(e, f, range)
        })
        .collect()
}

/// The fields that belong to this kind, in pane and Tab order.
///
/// Exhaustive over the element type, for `Doc::common_mut`'s reason: an element
/// type added to the document model must fail to compile here rather than quietly
/// arrive with no fields at all.
fn kind_fields(e: &Element) -> &'static [Field] {
    match e {
        Element::Text(_) => &[
            Field::Text,
            Field::Family,
            Field::Size,
            Field::LineHeight,
            Field::Align,
            Field::Weight,
            Field::Raster,
        ],
        Element::Barcode(_) => &[
            Field::Symbology,
            Field::Data,
            Field::ModuleDots,
            Field::BarDots,
            Field::Spec,
        ],
        Element::Qr(_) => &[
            Field::Data,
            Field::ModuleDots,
            Field::Ec,
            Field::Version,
            Field::Spec,
        ],
        Element::DataMatrix(_) => &[Field::Data, Field::ModuleDots, Field::DmSize, Field::Spec],
        Element::Image(_) => &[
            Field::Path,
            Field::Artwork,
            Field::Fit,
            Field::Halftone,
            Field::Threshold,
        ],
        Element::Rect(_) => &[Field::StrokeDots, Field::RadiusDots],
        Element::Line(_) => &[Field::ThicknessDots],
    }
}

/// The rows that belong to this kind, in pane and Tab order.
pub fn kind_slots(e: &Element) -> Vec<Slot> {
    kind_fields(e)
        .iter()
        .map(|&f| slot(e, f, f.range()))
        .collect()
}

/// The heading: what the canvas calls this element.
pub fn heading(e: &Element) -> String {
    let c = e.common();
    format!("{} {}", kind_name(e), describe(c.id, &c.name))
}

/// What one field shows when the document is the only thing talking. Empty for
/// a field this kind does not have.
pub fn read(e: &Element, f: Field) -> String {
    let c = e.common();
    match f {
        Field::Name => return c.name.clone(),
        Field::Feed => return c.frame.feed.to_string(),
        Field::Head => return c.frame.head.to_string(),
        Field::FeedLen => return c.frame.feed_len.to_string(),
        Field::HeadLen => return c.frame.head_len.to_string(),
        Field::Rot => return i32::from(c.rot).to_string(),
        Field::Ink => return ink_spelling(c.ink).to_string(),
        Field::State => return state(c),
        _ => {}
    }
    match (e, f) {
        (Element::Text(t), Field::Text) => t.text.clone(),
        (Element::Text(t), Field::Family) => t.family.clone(),
        (Element::Text(t), Field::Size) => t.size.to_string(),
        (Element::Text(t), Field::LineHeight) => t.line_height.to_string(),
        (Element::Text(t), Field::Align) => align_spelling(&t.align).to_string(),
        (Element::Text(t), Field::Weight) => weight_spelling(&t.weight).to_string(),
        (Element::Text(t), Field::Raster) => raster_spelling(&t.raster).to_string(),
        (Element::Barcode(b), Field::Symbology) => b.symbology.clone(),
        (Element::Barcode(b), Field::Data) => b.data.clone(),
        (Element::Barcode(b), Field::ModuleDots) => b.module_dots.to_string(),
        (Element::Barcode(b), Field::BarDots) => b.bar_dots.to_string(),
        (Element::Barcode(b), Field::Spec) => spec(b.out_of_spec),
        (Element::Qr(q), Field::Data) => q.data.clone(),
        (Element::Qr(q), Field::ModuleDots) => q.module_dots.to_string(),
        (Element::Qr(q), Field::Ec) => ec_spelling(q.ec).to_string(),
        (Element::Qr(q), Field::Version) => q.version.map(|v| v.to_string()).unwrap_or_default(),
        (Element::Qr(q), Field::Spec) => spec(q.out_of_spec),
        (Element::DataMatrix(d), Field::Data) => d.data.clone(),
        (Element::DataMatrix(d), Field::ModuleDots) => d.module_dots.to_string(),
        (Element::DataMatrix(d), Field::DmSize) => String::from(d.size.clone()),
        (Element::DataMatrix(d), Field::Spec) => spec(d.out_of_spec),
        (Element::Image(i), Field::Path) => i.path.clone(),
        (Element::Image(i), Field::Artwork) => artwork(&i.png),
        (Element::Image(i), Field::Fit) => fit_spelling(&i.fit).to_string(),
        (Element::Image(i), Field::Halftone) => i
            .halftone
            .as_ref()
            .map(|h| halftone_spelling(h).to_string())
            .unwrap_or_default(),
        (Element::Image(i), Field::Threshold) => {
            i.threshold.map(|t| t.to_string()).unwrap_or_default()
        }
        (Element::Rect(r), Field::StrokeDots) => r.stroke_dots.to_string(),
        (Element::Rect(r), Field::RadiusDots) => radius(r.radius_dots),
        (Element::Line(l), Field::ThicknessDots) => l.thickness_dots.to_string(),
        _ => String::new(),
    }
}

/// What happened to a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wrote {
    /// The value was already what was typed.
    Unchanged,
    /// Written. The note is `Some` when the document kept something this build
    /// cannot draw - an unknown font family, an alignment spelled another way -
    /// because a stored value that makes the element vanish has to be said out
    /// loud somewhere, and the renderer's own report has no pane yet.
    Changed(Option<String>),
}

/// Write one field of one element. `Err` is a refusal, already phrased.
///
/// The only place a per-kind field is written, the way `Doc::write_frame` is the
/// only place a frame is written. Matching on the element first, with no
/// top-level wildcard, so an eighth element type fails to compile here; the
/// wildcard inside each arm is a mis-routed field, which is a test failure rather
/// than a compile error because the field table is data.
///
/// Exactly two classes are refused: text the field's own type cannot hold, and a
/// value the file could not be written and read back from. Everything else is
/// stored as typed and reported by the renderer and by preflight, because the
/// format keeps what it does not understand on purpose and a pane that refused an
/// unknown symbology would be deciding that for it.
///
/// A placement or a length is stored as typed here; clamping either is `Doc`'s,
/// which is what owns the canvas it has to stay reachable on.
pub fn write(e: &mut Element, f: Field, text: &str) -> Result<Wrote, String> {
    let kind = kind_name(e);
    if !COMMON.contains(&f) && !kind_fields(e).contains(&f) {
        return Err(format!("a {kind} has no {}", f.label()));
    }
    if f.shape() == Shape::Read {
        return Err(format!("{} is shown here and not edited", f.label()));
    }
    let (id, who) = {
        let c = e.common();
        (c.id, describe(c.id, &c.name))
    };
    match f {
        Field::Name => return Ok(set(&mut common_of(e).name, text.to_string(), None)),
        Field::Feed => {
            let v = number(f, text)?;
            return Ok(set(&mut common_of(e).frame.feed, v, None));
        }
        Field::Head => {
            let v = number(f, text)?;
            return Ok(set(&mut common_of(e).frame.head, v, None));
        }
        Field::FeedLen => {
            let v = number(f, text)?;
            return Ok(set(&mut common_of(e).frame.feed_len, v, None));
        }
        Field::HeadLen => {
            let v = number(f, text)?;
            return Ok(set(&mut common_of(e).frame.head_len, v, None));
        }
        Field::Rot => {
            let v = Rot::from(number(f, text)?);
            let note = (!v.is_exact()).then(|| {
                format!(
                    "{who} keeps a rotation of {} degrees, which this build draws at the \
                     nearest quarter turn, {}",
                    i32::from(v),
                    i32::from(v.quantised())
                )
            });
            return Ok(set(&mut common_of(e).rot, v, note));
        }
        Field::Ink => {
            let v = match text {
                "black" => Ink::Black,
                "white" => Ink::White,
                _ => return Err(closed(f, text)),
            };
            return Ok(set(&mut common_of(e).ink, v, None));
        }
        _ => {}
    }
    match e {
        Element::Text(t) => match f {
            Field::Text => {
                let n = text.chars().count();
                let note = (n > text::MAX_TEXT_CHARS).then(|| {
                    format!(
                        "{who} keeps {n} characters of text, which this build cannot draw: \
                         the most it draws is {}",
                        text::MAX_TEXT_CHARS
                    )
                });
                Ok(set(&mut t.text, text.to_string(), note))
            }
            Field::Family => {
                let note = if !text.is_empty() && !f.drawable_values().contains(&text) {
                    Some(undrawable(&who, f, text))
                } else if text == text::MONO && t.weight == TextWeight::SemiBold {
                    Some(no_mono_semibold(&who))
                } else {
                    None
                };
                Ok(set(&mut t.family, text.to_string(), note))
            }
            Field::Size => {
                let v = decimal(f, id, text)?;
                let note = (v > text::MAX_PPEM).then(|| {
                    format!(
                        "{who} keeps an em size of {v} dots, which this build cannot draw: \
                         the largest it draws is {}",
                        text::MAX_PPEM
                    )
                });
                Ok(set(&mut t.size, v, note))
            }
            Field::LineHeight => {
                let v = decimal(f, id, text)?;
                let note = (v > text::MAX_LINE_HEIGHT).then(|| {
                    format!(
                        "{who} keeps a line height of {v}, which this build cannot draw: \
                         the largest it draws is {}",
                        text::MAX_LINE_HEIGHT
                    )
                });
                Ok(set(&mut t.line_height, v, note))
            }
            Field::Align => {
                let v = match text {
                    "start" => TextAlign::Start,
                    "center" => TextAlign::Center,
                    "end" => TextAlign::End,
                    _ => TextAlign::Unknown(text.to_string()),
                };
                let note = open_note(&who, f, text);
                Ok(set(&mut t.align, v, note))
            }
            Field::Weight => {
                let v = match text {
                    "regular" => TextWeight::Regular,
                    "semibold" => TextWeight::SemiBold,
                    _ => TextWeight::Unknown(text.to_string()),
                };
                // The pair is legal in the file and has no face to draw with, so
                // it is kept and named rather than refused.
                let note = open_note(&who, f, text).or_else(|| {
                    (v == TextWeight::SemiBold && t.family == text::MONO)
                        .then(|| no_mono_semibold(&who))
                });
                Ok(set(&mut t.weight, v, note))
            }
            Field::Raster => {
                let v = match text {
                    "hinted" => GlyphRaster::Hinted,
                    "smooth" => GlyphRaster::Smooth,
                    _ => GlyphRaster::Unknown(text.to_string()),
                };
                let note = open_note(&who, f, text);
                Ok(set(&mut t.raster, v, note))
            }
            _ => Err(format!("a {kind} has no {}", f.label())),
        },
        Element::Barcode(b) => match f {
            Field::Symbology => {
                let note = Symbology::from_name(text)
                    .is_none()
                    .then(|| undrawable(&who, f, text));
                Ok(set(&mut b.symbology, text.to_string(), note))
            }
            Field::Data => Ok(set(&mut b.data, text.to_string(), None)),
            Field::ModuleDots => {
                let v = count(f, text)?;
                let note = module_note(&who, v, b.out_of_spec);
                Ok(set(&mut b.module_dots, v, note))
            }
            Field::BarDots => Ok(set(&mut b.bar_dots, count(f, text)?, None)),
            _ => Err(format!("a {kind} has no {}", f.label())),
        },
        Element::Qr(q) => match f {
            Field::Data => Ok(set(&mut q.data, text.to_string(), None)),
            Field::ModuleDots => {
                let v = count(f, text)?;
                let note = module_note(&who, v, q.out_of_spec);
                Ok(set(&mut q.module_dots, v, note))
            }
            Field::Ec => {
                let v = match text {
                    "l" => QrEc::L,
                    "m" => QrEc::M,
                    "q" => QrEc::Q,
                    "h" => QrEc::H,
                    _ => return Err(closed(f, text)),
                };
                Ok(set(&mut q.ec, v, None))
            }
            Field::Version => {
                let v = maybe_u8(f, text)?;
                let note = v.filter(|v| !(1..=40).contains(v)).map(|v| {
                    format!(
                        "{who} keeps version {v}, which this build cannot draw: a QR version \
                         is 1 to 40"
                    )
                });
                Ok(set(&mut q.version, v, note))
            }
            _ => Err(format!("a {kind} has no {}", f.label())),
        },
        Element::DataMatrix(d) => match f {
            Field::Data => Ok(set(&mut d.data, text.to_string(), None)),
            Field::ModuleDots => {
                let v = count(f, text)?;
                let note = module_note(&who, v, d.out_of_spec);
                Ok(set(&mut d.module_dots, v, note))
            }
            Field::DmSize => {
                let v = DmSize::from(text.to_string());
                let drawable = match v {
                    DmSize::Square | DmSize::Rectangle | DmSize::Any => true,
                    DmSize::Fixed { rows, cols } => {
                        DM_SIZES.iter().any(|&(_, r, c)| (r, c) == (rows, cols))
                    }
                    DmSize::Unknown(_) => false,
                };
                let note = (!drawable).then(|| {
                    format!(
                        "{who} keeps size {text:?}, which this build cannot draw: it draws {}, \
                         or rows by columns of one of the {} sizes a DataMatrix comes in, \
                         written like 16x48",
                        spoken_list(f.drawable_values()),
                        DM_SIZES.len()
                    )
                });
                Ok(set(&mut d.size, v, note))
            }
            _ => Err(format!("a {kind} has no {}", f.label())),
        },
        Element::Image(i) => match f {
            Field::Path => Ok(set(&mut i.path, text.to_string(), None)),
            Field::Fit => {
                let v = match text {
                    "contain" => ImageFit::Contain,
                    "cover" => ImageFit::Cover,
                    "stretch" => ImageFit::Stretch,
                    "dots" => ImageFit::Dots,
                    _ => ImageFit::Unknown(text.to_string()),
                };
                let note = open_note(&who, f, text);
                Ok(set(&mut i.fit, v, note))
            }
            Field::Halftone => {
                if text.is_empty() {
                    return Ok(set(&mut i.halftone, None, None));
                }
                let v = match text {
                    "threshold" => ImageHalftone::Threshold,
                    "floyd_steinberg" => ImageHalftone::FloydSteinberg,
                    "atkinson" => ImageHalftone::Atkinson,
                    "sierra3" => ImageHalftone::Sierra3,
                    "bayer8" => ImageHalftone::Bayer8,
                    _ => ImageHalftone::Unknown(text.to_string()),
                };
                let note = open_note(&who, f, text);
                Ok(set(&mut i.halftone, Some(v), note))
            }
            Field::Threshold => Ok(set(&mut i.threshold, maybe_u8(f, text)?, None)),
            _ => Err(format!("a {kind} has no {}", f.label())),
        },
        Element::Rect(r) => match f {
            Field::StrokeDots => Ok(set(&mut r.stroke_dots, count(f, text)?, None)),
            _ => Err(format!("a {kind} has no {}", f.label())),
        },
        Element::Line(l) => match f {
            Field::ThicknessDots => Ok(set(&mut l.thickness_dots, count(f, text)?, None)),
            _ => Err(format!("a {kind} has no {}", f.label())),
        },
    }
}

/// The text one increment would carry, in this field's own unit.
///
/// A step is a commit of a computed string, so there is still one writer. A
/// fractional field steps on the decimal the field is showing rather than on the
/// float behind it, so ten steps of a tenth read 2 rather than 2.0000002. A whole
/// number stops at the edge of the range it announces rather than stepping into a
/// refusal, and never moves against the key when the file already holds a value
/// outside that range. An empty optional field stays empty: what it stands for
/// is the page's, and not this pane's to know.
pub fn stepped(e: &Element, f: Field, by: i32) -> String {
    let now = read(e, f);
    let by = i64::from(by);
    match f {
        Field::Feed | Field::Head => whole_step(&now, by, i64::from(i32::MIN), i64::from(i32::MAX)),
        // From one dot to the longest a canvas is. The floor a negative origin
        // raises is `Doc`'s to hold and to say, as a placement's clamp is.
        Field::FeedLen | Field::HeadLen => whole_step(&now, by, 1, i64::from(geom::MAX_LEN)),
        // A quarter turn, unbounded: the document normalises any degree, so 270
        // and a quarter turn more is 0 rather than a refusal.
        Field::Rot => whole_step(&now, 90 * by, i64::MIN / 2, i64::MAX / 2),
        Field::Size | Field::LineHeight => decimal_step(&now, by, f.range().2),
        Field::ModuleDots | Field::BarDots | Field::StrokeDots | Field::ThicknessDots => {
            whole_step(&now, by, 0, i64::from(u16::MAX))
        }
        Field::Version if !now.is_empty() => whole_step(&now, by, 1, 40),
        Field::Threshold if !now.is_empty() => whole_step(&now, by, 0, 255),
        _ => now,
    }
}

/// One step of a whole number, held inside `lo..=hi` - or, for a value already
/// outside it, held where it is rather than moved against the key.
fn whole_step(now: &str, by: i64, lo: i64, hi: i64) -> String {
    match now.trim().parse::<i64>() {
        Ok(n) => (n + by).clamp(n.min(lo), n.max(hi)).to_string(),
        Err(_) => now.to_string(),
    }
}

/// The mutable half of [`Element::common`], which `relabel-render` does not have.
///
/// Exhaustive on purpose: a new element type in the document model fails to
/// compile here rather than quietly becoming unmovable. `Doc::common_mut` is the
/// only caller besides [`write`], so the match exists once.
pub fn common_of(e: &mut Element) -> &mut Common {
    match e {
        Element::Text(x) => &mut x.common,
        Element::Barcode(x) => &mut x.common,
        Element::Qr(x) => &mut x.common,
        Element::DataMatrix(x) => &mut x.common,
        Element::Image(x) => &mut x.common,
        Element::Rect(x) => &mut x.common,
        Element::Line(x) => &mut x.common,
    }
}

fn set<T: PartialEq>(slot: &mut T, v: T, note: Option<String>) -> Wrote {
    if *slot == v {
        Wrote::Unchanged
    } else {
        *slot = v;
        Wrote::Changed(note)
    }
}

/// A whole number the file holds as `i32`: a placement or a rotation.
pub(crate) fn number(f: Field, text: &str) -> Result<i32, String> {
    let t = text.trim();
    t.parse::<i32>().map_err(|_| match t.parse::<i128>() {
        Ok(_) => format!(
            "{} takes a whole number from {} to {}, and {t} is outside that",
            f.label(),
            i32::MIN,
            i32::MAX
        ),
        Err(_) => not_whole(f, text),
    })
}

/// A whole number from 0 to `hi`. Refused outside it, never wrapped: a module of
/// 65 536 stored as 0 would be a different barcode.
fn whole_to(f: Field, text: &str, hi: i128) -> Result<i128, String> {
    let t = text.trim();
    let n: i128 = t.parse().map_err(|_| not_whole(f, text))?;
    if !(0..=hi).contains(&n) {
        return Err(format!(
            "{} takes a whole number from 0 to {hi}, and {t} is outside that",
            f.label()
        ));
    }
    Ok(n)
}

fn count(f: Field, text: &str) -> Result<u16, String> {
    whole_to(f, text, i128::from(u16::MAX)).map(|n| n as u16)
}

fn maybe_u8(f: Field, text: &str) -> Result<Option<u8>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    whole_to(f, text, i128::from(u8::MAX)).map(|n| Some(n as u8))
}

/// One of the document's two floats. A value the file could not be written and
/// read back from is refused in the words `Label::to_json` would have used, so
/// the operator hears the same sentence the save would have said.
fn decimal(f: Field, id: ElementId, text: &str) -> Result<f32, String> {
    let t = text.trim();
    let v: f32 = t
        .parse()
        .map_err(|_| format!("{} takes a number, and {text:?} is not one", f.label()))?;
    if !v.is_finite() || v <= 0.0 {
        return Err(LoadError::BadFloat {
            what: f.key().unwrap_or(f.label()),
            id,
            value: v,
        }
        .to_string());
    }
    Ok(v)
}

/// Step a decimal on the digits it is showing, so no binary fraction is ever
/// added to it.
fn decimal_step(now: &str, by: i64, step: f32) -> String {
    let places = |s: &str| s.split_once('.').map_or(0, |(_, frac)| frac.len());
    let Ok(v) = now.trim().parse::<f64>() else {
        return now.to_string();
    };
    let d = places(now.trim()).max(places(&step.to_string())).min(9);
    let scale = 10f64.powi(d as i32);
    let scaled = (v * scale).round() + (by as f64) * (f64::from(step) * scale).round();
    let mut s = format!("{:.*}", d, scaled / scale);
    if s.contains('.') {
        s.truncate(s.trim_end_matches('0').trim_end_matches('.').len());
    }
    s
}

fn not_whole(f: Field, text: &str) -> String {
    format!(
        "{} takes a whole number, and {text:?} is not one",
        f.label()
    )
}

/// A refusal for a value set with no `Unknown` arm: the file could not hold it.
fn closed(f: Field, text: &str) -> String {
    format!(
        "{} is {}, and {text:?} is not one of those",
        f.label(),
        spoken_list(f.drawable_values())
    )
}

fn open_note(who: &str, f: Field, text: &str) -> Option<String> {
    (!f.drawable_values().contains(&text)).then(|| undrawable(who, f, text))
}

fn undrawable(who: &str, f: Field, text: &str) -> String {
    format!(
        "{who} keeps {} {text:?}, which this build cannot draw: it draws {}",
        f.label(),
        spoken_list(f.drawable_values())
    )
}

fn no_mono_semibold(who: &str) -> String {
    format!(
        "{who} keeps weight semibold in {}, which this build cannot draw: that family has \
         no semibold face",
        text::MONO
    )
}

/// The two module widths that draw nothing unless asked. Everything else a
/// module can be wrong about - too wide for its frame, outside a retail
/// symbol's specification - needs the symbol, and is the renderer's to say.
fn module_note(who: &str, v: u16, out_of_spec: bool) -> Option<String> {
    if v == 0 {
        Some(format!(
            "{who} keeps a module of 0 dots, which draws nothing"
        ))
    } else if v < MIN_MODULE_DOTS && !out_of_spec {
        Some(format!(
            "{who} keeps a module of {v} dot, which this build draws only when printing out \
             of spec: the narrowest it draws otherwise is {MIN_MODULE_DOTS}"
        ))
    } else {
        None
    }
}

fn spoken_list(v: &[&str]) -> String {
    match v {
        [] => String::new(),
        [one] => (*one).to_string(),
        [init @ .., last] => format!("{} or {last}", init.join(", ")),
    }
}

fn ink_spelling(i: Ink) -> &'static str {
    match i {
        Ink::Black => "black",
        Ink::White => "white",
    }
}

fn ec_spelling(e: QrEc) -> &'static str {
    match e {
        QrEc::L => "l",
        QrEc::M => "m",
        QrEc::Q => "q",
        QrEc::H => "h",
    }
}

fn align_spelling(a: &TextAlign) -> &str {
    match a {
        TextAlign::Start => "start",
        TextAlign::Center => "center",
        TextAlign::End => "end",
        TextAlign::Unknown(s) => s,
    }
}

fn weight_spelling(w: &TextWeight) -> &str {
    match w {
        TextWeight::Regular => "regular",
        TextWeight::SemiBold => "semibold",
        TextWeight::Unknown(s) => s,
    }
}

fn raster_spelling(r: &GlyphRaster) -> &str {
    match r {
        GlyphRaster::Hinted => "hinted",
        GlyphRaster::Smooth => "smooth",
        GlyphRaster::Unknown(s) => s,
    }
}

fn fit_spelling(f: &ImageFit) -> &str {
    match f {
        ImageFit::Contain => "contain",
        ImageFit::Cover => "cover",
        ImageFit::Stretch => "stretch",
        ImageFit::Dots => "dots",
        ImageFit::Unknown(s) => s,
    }
}

fn halftone_spelling(h: &ImageHalftone) -> &str {
    match h {
        ImageHalftone::Threshold => "threshold",
        ImageHalftone::FloydSteinberg => "floyd_steinberg",
        ImageHalftone::Atkinson => "atkinson",
        ImageHalftone::Sierra3 => "sierra3",
        ImageHalftone::Bayer8 => "bayer8",
        ImageHalftone::Unknown(s) => s,
    }
}

fn spec(out_of_spec: bool) -> String {
    if out_of_spec {
        "printed out of spec".into()
    } else {
        "held to the specification".into()
    }
}

fn state(c: &Common) -> String {
    let mut parts = Vec::new();
    if c.locked {
        parts.push("locked".to_string());
    }
    match c.unknown.len() {
        0 => {}
        1 => parts.push("1 field this build does not know is kept".into()),
        n => parts.push(format!("{n} fields this build does not know are kept")),
    }
    if parts.is_empty() {
        "\u{2014}".into()
    } else {
        parts.join("; ")
    }
}

/// The artwork's size, counted from the length of its base64 and never decoded:
/// this row is rebuilt on every pointer move of a drag, and a PNG decode per
/// moved dot is the cost the row must not have.
fn artwork(png: &str) -> String {
    if png.is_empty() {
        return "no artwork".into();
    }
    let pad = png.bytes().rev().take_while(|&b| b == b'=').count();
    let bytes = (png.len() / 4 * 3).saturating_sub(pad);
    format!("embedded, {bytes} bytes")
}

fn radius(r: u16) -> String {
    if r == 0 {
        "0 dots".into()
    } else {
        format!("{r} dots, which this build draws square")
    }
}

/// Wire one inspector to the document the canvas already has.
///
/// Called after [`crate::canvas::wire`], and the pane is pushed once here for
/// whatever the document opened with - `Canvas::publish` does not push it,
/// because the `Everything` arm of `refresh` calls `publish` and the pane would
/// then be pushed twice for one outcome.
pub fn wire<H: InsHost>(host: &H, canvas: &Rc<RefCell<Canvas<H>>>) {
    let slots: Rc<VecModel<H::Slot>> =
        Rc::new(VecModel::from(vec![H::Slot::of(&Slot::unused()); SLOTS]));
    host.set_ins_slots(ModelRc::from(slots.clone()));
    // Every caption any kind shows, once, so the pane measures its caption column
    // from all of them and keeps one width whatever is selected.
    host.set_ins_captions(
        Field::ALL
            .iter()
            .map(|f| f.label())
            .collect::<Vec<_>>()
            .join("\n")
            .as_str()
            .into(),
    );

    // Written one row at a time for `Canvas::publish`'s reason: `set_vec` emits a
    // model reset. The length never changes, so it never has to.
    let push: PanePush<H> = {
        let slots = slots.clone();
        Rc::new(move |host: &H, doc: &Doc| {
            let Some(rows) = doc.inspected() else {
                host.set_ins_shown(false);
                return;
            };
            for (i, s) in rows.iter().enumerate() {
                slots.set_row_data(i, H::Slot::of(s));
            }
            host.set_ins_heading(doc.inspected_heading().as_str().into());
            host.set_ins_shown(true);
            // After the rows, never before: the number has to land in the field
            // the row describes rather than in the last selection's.
            host.invoke_show_fields();
        })
    };
    canvas.borrow_mut().tell_pane(push.clone());
    push(host, canvas.borrow().doc());

    // The canvas's `on!` macro is not used for these three: a token this build
    // has no field for must send no command at all, and the macro has to compose
    // one unconditionally. The shape is otherwise identical - upgrade the weak
    // handle or return, apply one `Cmd`, refresh what the outcome says changed.
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_field_edited(move |token, text| {
            let (Some(host), Some(field)) = (weak.upgrade(), Field::from_token(token)) else {
                return;
            };
            let mut c = canvas.borrow_mut();
            let out = c.doc_mut().apply(Cmd::StageField {
                field,
                text: text.to_string(),
            });
            // Typing is a new attempt, so what the last refusal said comes off the
            // status line. Otherwise the same mistake made twice leaves the line
            // holding what it already held, `Property::set` marks nothing dirty,
            // and the polite live region - the only channel a refusal has - is
            // silent the second time. Cleared here and not just before the next
            // note, because the node is rebuilt once per dispatch and would see
            // the same text on either side of a clear and a set.
            if out.note.is_none() {
                host.set_status("".into());
            }
            c.refresh(&host, out);
        });
    }
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_field_commit(move || {
            let Some(host) = weak.upgrade() else {
                return;
            };
            let mut c = canvas.borrow_mut();
            let out = c.doc_mut().apply(Cmd::CommitField);
            c.refresh(&host, out);
        });
    }
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_field_stepped(move |token, by| {
            let (Some(host), Some(field)) = (weak.upgrade(), Field::from_token(token)) else {
                return;
            };
            let mut c = canvas.borrow_mut();
            let out = c.doc_mut().apply(Cmd::StepField { field, by });
            // A step is a new attempt too, for the same reason as typing.
            if out.note.is_none() {
                host.set_status("".into());
            }
            c.refresh(&host, out);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_render::Label;
    use relabel_render::document::ElementOrRaw;

    fn el(json: &str) -> Element {
        serde_json::from_str(json).expect("an element this build knows")
    }

    const FRAME: &str = r#""frame": {"feed": 8, "head": 4, "feed_len": 96, "head_len": 24}"#;

    /// One element of each kind, with ordinary values.
    fn every_kind() -> Vec<Element> {
        [
            r#""type": "text", "id": 1, "name": "SKU", "text": "{sku}", "family": "Noto Sans", "size": 20.0"#,
            r#""type": "barcode", "id": 2, "symbology": "code128", "data": "123", "module_dots": 2, "bar_dots": 0"#,
            r#""type": "qr", "id": 3, "data": "x", "module_dots": 3"#,
            r#""type": "data_matrix", "id": 4, "data": "x", "module_dots": 2"#,
            r#""type": "image", "id": 5, "path": "logo.png", "png": "iVBORw0KGgo=""#,
            r#""type": "rect", "id": 6, "stroke_dots": 2, "radius_dots": 3"#,
            r#""type": "line", "id": 7"#,
        ]
        .into_iter()
        .map(|body| el(&format!("{{{body}, {FRAME}}}")))
        .collect()
    }

    /// The same seven with every string empty and every option absent: the
    /// values a field is least able to show.
    fn blank_kinds() -> Vec<Element> {
        [
            r#""type": "text", "id": 1, "text": "", "size": 20.0"#,
            r#""type": "barcode", "id": 2, "symbology": "", "data": "", "module_dots": 2, "bar_dots": 0"#,
            r#""type": "qr", "id": 3, "data": "", "module_dots": 3"#,
            r#""type": "data_matrix", "id": 4, "data": "", "module_dots": 2, "size": """#,
            r#""type": "image", "id": 5"#,
            r#""type": "rect", "id": 6"#,
            r#""type": "line", "id": 7"#,
        ]
        .into_iter()
        .map(|body| el(&format!("{{{body}, {FRAME}}}")))
        .collect()
    }

    /// Values a later build or a hand-edited file might hold.
    fn unusual_kinds() -> Vec<Element> {
        [
            r#""type": "text", "id": 1, "rot": 45, "ink": "white", "text": "a", "family": "Comic Sans", "size": 12.5, "line_height": 1.3, "align": "wobbly", "weight": "black", "raster": "sdf""#,
            r#""type": "barcode", "id": 2, "symbology": "code93", "data": "{sku}", "module_dots": 1, "bar_dots": 900, "out_of_spec": true"#,
            r#""type": "qr", "id": 3, "data": "x", "module_dots": 0, "ec": "h", "version": 41"#,
            r#""type": "data_matrix", "id": 4, "data": "x", "module_dots": 2, "size": "016x48""#,
            r#""type": "image", "id": 5, "fit": "tile", "halftone": "blue_noise", "threshold": 0"#,
            r#""type": "rect", "id": 6, "rot": -90"#,
            r#""type": "line", "id": 7, "thickness_dots": 0"#,
        ]
        .into_iter()
        .map(|body| el(&format!("{{{body}, {FRAME}}}")))
        .collect()
    }

    const REACH: ((i32, i32), (i32, i32)) = ((-95, 319), (-23, 95));

    const LENS: ((i32, i32), (i32, i32)) = ((1, 65535), (1, 65535));

    fn all_slots(e: &Element) -> Vec<Slot> {
        let mut s = common_slots(e, REACH, LENS);
        s.extend(kind_slots(e));
        s
    }

    fn text_of(kinds: Vec<Element>) -> Element {
        kinds
            .into_iter()
            .find(|e| matches!(e, Element::Text(_)))
            .expect("a text element")
    }

    fn of_kind(kinds: Vec<Element>, kind: &str) -> Element {
        kinds
            .into_iter()
            .find(|e| e.kind() == kind)
            .expect("that kind")
    }

    /// Keys a kind writes that no slot edits or names, each with its reason in
    /// the module's field table. A key added to `own_keys` later fails the test
    /// below until someone has decided whether it is a field.
    const SHOWN_NOT_EDITED: &[(&str, &str)] = &[
        // Summarised by the artwork row, never offered as text to type.
        ("image", "png"),
    ];

    #[test]
    fn the_token_of_every_field_is_its_own_index() {
        for (i, f) in Field::ALL.iter().enumerate() {
            assert_eq!(f.token(), i as i32, "{f:?}");
            assert_eq!(Field::from_token(i as i32), Some(*f));
        }
        assert_eq!(Field::from_token(-1), None);
        assert_eq!(Field::from_token(Field::ALL.len() as i32), None);
    }

    #[test]
    fn the_fields_each_kind_shows_are_exactly_the_keys_that_kind_writes() {
        for e in every_kind() {
            let mut shown: Vec<&str> = kind_slots(&e)
                .iter()
                .filter_map(|s| s.field.key())
                .collect();
            shown.extend(
                SHOWN_NOT_EDITED
                    .iter()
                    .filter(|(kind, _)| *kind == e.kind())
                    .map(|(_, key)| *key),
            );
            shown.sort_unstable();
            let mut own = e.own_keys().to_vec();
            own.sort_unstable();
            assert_eq!(shown, own, "the {} element", e.kind());
        }
        let common: Vec<_> = COMMON.iter().filter_map(|f| f.key()).collect();
        assert_eq!(
            common,
            [
                "name",
                "frame.feed",
                "frame.head",
                "frame.feed_len",
                "frame.head_len",
                "rot",
                "ink"
            ]
        );
    }

    #[test]
    fn a_length_is_a_number_of_whole_dots() {
        assert_eq!(Field::ALL.len(), 31);
        let f = Field::FeedLen;
        assert_eq!(
            (f.label(), f.unit(), f.key(), f.shape()),
            ("feed length", "dots", Some("frame.feed_len"), Shape::Number)
        );
        let f = Field::HeadLen;
        assert_eq!(
            (f.label(), f.unit(), f.key(), f.shape()),
            ("head length", "dots", Some("frame.head_len"), Shape::Number)
        );
        let rect = el(
            r#"{"type": "rect", "id": 6, "frame": {"feed": 8, "head": 8, "feed_len": 40, "head_len": 24}}"#,
        );
        assert_eq!(read(&rect, Field::FeedLen), "40");
        assert_eq!(read(&rect, Field::HeadLen), "24");
        assert_eq!(stepped(&rect, Field::FeedLen, -100), "1");
        assert_eq!(stepped(&rect, Field::HeadLen, 1), "25");
        assert_eq!(stepped(&rect, Field::HeadLen, 70_000), "65535");
    }

    #[test]
    fn every_one_of_the_seven_kinds_fits_the_run_of_slots() {
        let widest = every_kind()
            .iter()
            .map(|e| all_slots(e).len())
            .max()
            .expect("seven kinds");
        assert!(widest <= SLOTS, "{widest} rows for {SLOTS} slots");
        assert_eq!(widest, 15, "the text element is the widest kind");
    }

    #[test]
    fn no_kind_leaves_a_gap_in_the_run_of_slots() {
        for e in every_kind() {
            let slots = all_slots(&e);
            assert!(
                slots.iter().all(|s| s.shape != Shape::Unused),
                "{}",
                e.kind()
            );
            for (i, s) in slots.iter().enumerate() {
                assert!(
                    slots[..i].iter().all(|t| t.field != s.field),
                    "{:?} twice in the {}",
                    s.field,
                    e.kind()
                );
            }
            assert_eq!(slots[0].field, Field::Name, "the keyboard lands on a name");
        }
    }

    #[test]
    fn every_slot_of_every_kind_has_a_name_and_a_unit() {
        for e in every_kind() {
            for s in all_slots(&e) {
                assert!(!s.field.label().is_empty(), "{:?}", s.field);
                assert!(!s.field.unit().is_empty(), "{:?}", s.field);
            }
        }
    }

    /// An empty field has to mean something, and the placeholder is what says
    /// it; a blank box with no placeholder is a field nobody can read.
    #[test]
    fn every_field_of_every_kind_is_shown_with_something_rather_than_an_empty_string() {
        for e in blank_kinds().into_iter().chain(every_kind()) {
            for s in all_slots(&e) {
                assert!(
                    !s.value.is_empty() || !s.field.hint().is_empty(),
                    "{:?} of the {} shows nothing and hints nothing",
                    s.field,
                    e.kind()
                );
            }
        }
    }

    #[test]
    fn reading_a_field_and_writing_it_straight_back_changes_nothing() {
        for e in every_kind()
            .into_iter()
            .chain(blank_kinds())
            .chain(unusual_kinds())
        {
            for s in all_slots(&e).into_iter().filter(|s| s.shape != Shape::Read) {
                let mut again = e.clone();
                assert_eq!(
                    write(&mut again, s.field, &read(&e, s.field)),
                    Ok(Wrote::Unchanged),
                    "{:?} of the {}",
                    s.field,
                    e.kind()
                );
                assert_eq!(again, e);
            }
        }
    }

    #[test]
    fn a_rotation_of_forty_five_degrees_survives_being_shown_and_typed_back() {
        let mut e = of_kind(every_kind(), "rect");
        let wrote = write(&mut e, Field::Rot, "45").expect("any degree is accepted");
        assert!(
            matches!(&wrote, Wrote::Changed(Some(note)) if note.contains("nearest quarter turn, 90")),
            "{wrote:?}"
        );
        assert_eq!(e.common().rot, Rot::Other(45));
        assert_eq!(read(&e, Field::Rot), "45");
        assert_eq!(write(&mut e, Field::Rot, "45"), Ok(Wrote::Unchanged));
        write(&mut e, Field::Rot, "-90").expect("a negative degree");
        assert_eq!(read(&e, Field::Rot), "270");
    }

    #[test]
    fn an_em_size_of_twelve_and_a_half_dots_survives_being_shown_and_typed_back() {
        let mut e = text_of(every_kind());
        assert_eq!(write(&mut e, Field::Size, "12.5"), Ok(Wrote::Changed(None)));
        assert_eq!(read(&e, Field::Size), "12.5");
        assert_eq!(write(&mut e, Field::Size, "12.5"), Ok(Wrote::Unchanged));
        let json = serde_json::to_string(&e).expect("serialisable");
        assert_eq!(el(&json), e);
    }

    #[test]
    fn an_em_size_of_zero_is_refused_because_the_label_could_not_be_reloaded() {
        let mut e = text_of(every_kind());
        let before = e.clone();
        let refused = write(&mut e, Field::Size, "0").expect_err("zero is refused");
        assert_eq!(e, before);
        // What the save itself would have said about the same number.
        let mut label = Label::new(320, 96, relabel_render::Dpmm::DPI_203);
        if let Element::Text(t) = &mut e {
            t.size = 0.0;
        }
        label.elements = vec![ElementOrRaw::Known(e)];
        let save = label.to_json().expect_err("a zero size cannot be saved");
        assert_eq!(refused, save.to_string());
    }

    #[test]
    fn an_em_size_that_is_not_a_number_is_refused_in_the_words_the_save_would_have_used() {
        for typed in ["NaN", "inf", "-2"] {
            let mut e = text_of(every_kind());
            let refused = write(&mut e, Field::Size, typed).expect_err("refused");
            let value: f32 = typed.parse().expect("a float spelling");
            if let Element::Text(t) = &mut e {
                t.size = value;
            }
            let mut label = Label::new(320, 96, relabel_render::Dpmm::DPI_203);
            label.elements = vec![ElementOrRaw::Known(e)];
            let save = label.to_json().expect_err("unsaveable");
            assert_eq!(refused, save.to_string(), "{typed}");
        }
        let mut e = text_of(every_kind());
        let refused = write(&mut e, Field::Size, "twelve").expect_err("refused");
        assert!(refused.contains("\"twelve\""), "{refused}");
        assert_eq!(read(&e, Field::Size), "20");
    }

    #[test]
    fn a_module_wider_than_the_format_holds_is_refused_rather_than_wrapped() {
        let mut e = of_kind(every_kind(), "barcode");
        let refused = write(&mut e, Field::ModuleDots, "65536").expect_err("refused");
        assert!(refused.contains("65535"), "{refused}");
        assert_eq!(
            read(&e, Field::ModuleDots),
            "2",
            "not 0, which is 65536 wrapped"
        );
        assert!(write(&mut e, Field::ModuleDots, "-1").is_err());
        assert_eq!(
            write(&mut e, Field::ModuleDots, "65535"),
            Ok(Wrote::Changed(None))
        );
    }

    #[test]
    fn a_threshold_outside_a_single_byte_is_refused() {
        let mut e = of_kind(every_kind(), "image");
        for typed in ["256", "-1", "1e3", "grey"] {
            assert!(write(&mut e, Field::Threshold, typed).is_err(), "{typed}");
        }
        assert_eq!(read(&e, Field::Threshold), "");
        assert_eq!(
            write(&mut e, Field::Threshold, "255"),
            Ok(Wrote::Changed(None))
        );
    }

    #[test]
    fn an_empty_version_field_is_the_smallest_symbol_that_holds_the_data() {
        let mut e = of_kind(every_kind(), "qr");
        write(&mut e, Field::Version, "5").expect("a version");
        assert!(matches!(&e, Element::Qr(q) if q.version == Some(5)));
        assert_eq!(write(&mut e, Field::Version, ""), Ok(Wrote::Changed(None)));
        assert!(matches!(&e, Element::Qr(q) if q.version.is_none()));
        assert_eq!(read(&e, Field::Version), "");
    }

    #[test]
    fn an_empty_threshold_field_is_the_pages_threshold() {
        let mut e = of_kind(unusual_kinds(), "image");
        assert_eq!(read(&e, Field::Threshold), "0");
        assert_eq!(
            write(&mut e, Field::Threshold, ""),
            Ok(Wrote::Changed(None))
        );
        assert!(matches!(&e, Element::Image(i) if i.threshold.is_none()));
    }

    #[test]
    fn an_empty_halftone_field_is_the_pages_halftone() {
        let mut e = of_kind(every_kind(), "image");
        write(&mut e, Field::Halftone, "atkinson").expect("a halftone");
        assert!(matches!(&e, Element::Image(i) if i.halftone == Some(ImageHalftone::Atkinson)));
        assert_eq!(write(&mut e, Field::Halftone, ""), Ok(Wrote::Changed(None)));
        assert!(matches!(&e, Element::Image(i) if i.halftone.is_none()));
    }

    #[test]
    fn a_closed_value_set_refuses_a_spelling_the_file_could_not_hold() {
        let mut text = text_of(every_kind());
        let before = text.clone();
        let refused = write(&mut text, Field::Ink, "grey").expect_err("no such ink");
        assert!(refused.contains("black or white"), "{refused}");
        assert_eq!(text, before);
        let mut qr = of_kind(every_kind(), "qr");
        let refused = write(&mut qr, Field::Ec, "M").expect_err("the file spells it m");
        assert!(refused.contains("l, m, q or h"), "{refused}");
        assert_eq!(read(&qr, Field::Ec), "m");
    }

    #[test]
    fn an_open_value_set_keeps_a_spelling_this_build_does_not_know() {
        let mut text = text_of(every_kind());
        assert!(matches!(
            write(&mut text, Field::Align, "wobbly"),
            Ok(Wrote::Changed(Some(_)))
        ));
        assert!(
            matches!(&text, Element::Text(t) if t.align == TextAlign::Unknown("wobbly".into()))
        );
        assert_eq!(read(&text, Field::Align), "wobbly");
        let mut dm = of_kind(every_kind(), "data_matrix");
        write(&mut dm, Field::DmSize, "016x48").expect("kept");
        assert_eq!(read(&dm, Field::DmSize), "016x48", "not rewritten as 16x48");
        let mut barcode = of_kind(every_kind(), "barcode");
        write(&mut barcode, Field::Symbology, "code93").expect("kept");
        assert_eq!(read(&barcode, Field::Symbology), "code93");
    }

    #[test]
    fn a_value_this_build_cannot_draw_is_kept_and_the_note_names_what_it_can_draw() {
        let cases = [
            ("text", Field::Family, "Arial"),
            ("text", Field::Align, "justify"),
            ("text", Field::Weight, "bold"),
            ("text", Field::Raster, "sdf"),
            ("barcode", Field::Symbology, "code93"),
            ("data_matrix", Field::DmSize, "circle"),
            ("image", Field::Fit, "tile"),
            ("image", Field::Halftone, "blue_noise"),
        ];
        for (kind, f, typed) in cases {
            let mut e = of_kind(every_kind(), kind);
            let Ok(Wrote::Changed(Some(note))) = write(&mut e, f, typed) else {
                panic!("{f:?} {typed:?} was not kept with a note");
            };
            assert_eq!(read(&e, f), typed);
            assert!(note.contains(&format!("{typed:?}")), "{note}");
            for spelling in f.drawable_values() {
                assert!(note.contains(spelling), "{note} does not name {spelling}");
            }
        }
        // And a value it can draw is kept with nothing to say.
        let mut e = text_of(every_kind());
        assert_eq!(
            write(&mut e, Field::Align, "center"),
            Ok(Wrote::Changed(None))
        );
    }

    #[test]
    fn a_font_family_this_build_cannot_draw_is_stored_exactly_as_it_was_typed() {
        let mut e = text_of(every_kind());
        let typed = " Comic Sans MS ";
        write(&mut e, Field::Family, typed).expect("kept");
        let json = serde_json::to_string(&e).expect("serialisable");
        assert!(matches!(el(&json), Element::Text(t) if t.family == typed));
    }

    #[test]
    fn every_spelling_this_build_claims_to_draw_is_one_the_document_type_knows() {
        use serde_json::{from_value, json};
        for s in Field::Ink.drawable_values() {
            from_value::<Ink>(json!(s)).expect("an ink");
        }
        for s in Field::Ec.drawable_values() {
            from_value::<QrEc>(json!(s)).expect("an error correction level");
        }
        for s in Field::Align.drawable_values() {
            let v: TextAlign = from_value(json!(s)).expect("json");
            assert!(!matches!(v, TextAlign::Unknown(_)), "{s}");
        }
        for s in Field::Weight.drawable_values() {
            let v: TextWeight = from_value(json!(s)).expect("json");
            assert!(!matches!(v, TextWeight::Unknown(_)), "{s}");
        }
        for s in Field::Raster.drawable_values() {
            let v: GlyphRaster = from_value(json!(s)).expect("json");
            assert!(!matches!(v, GlyphRaster::Unknown(_)), "{s}");
        }
        for s in Field::Fit.drawable_values() {
            let v: ImageFit = from_value(json!(s)).expect("json");
            assert!(!matches!(v, ImageFit::Unknown(_)), "{s}");
        }
        for s in Field::Halftone.drawable_values() {
            let v: ImageHalftone = from_value(json!(s)).expect("json");
            assert!(v.known().is_some(), "{s}");
        }
        for s in Field::DmSize.drawable_values() {
            assert!(
                !matches!(DmSize::from(s.to_string()), DmSize::Unknown(_)),
                "{s}"
            );
        }
        for s in Field::Symbology.drawable_values() {
            assert!(Symbology::from_name(s).is_some(), "{s}");
        }
        assert_eq!(
            Field::Symbology.drawable_values().len(),
            Symbology::ALL.len()
        );
        assert_eq!(Field::Family.drawable_values(), [text::SANS, text::MONO]);
        // The unit is where an operator reads the spellings, so it must carry
        // every one of them.
        for f in Field::ALL {
            for s in f.drawable_values() {
                assert!(f.unit().contains(s), "{:?}'s unit omits {s}", f);
            }
        }
    }

    #[test]
    fn a_field_that_belongs_to_another_kind_writes_nothing() {
        let mut rect = of_kind(every_kind(), "rect");
        let before = rect.clone();
        let refused = write(&mut rect, Field::Size, "12").expect_err("a rectangle has no em size");
        assert_eq!(refused, "a Rectangle has no em size");
        assert_eq!(rect, before);
        let mut text = text_of(every_kind());
        let before = text.clone();
        assert!(write(&mut text, Field::StrokeDots, "2").is_err());
        assert_eq!(text, before);
        assert_eq!(read(&text, Field::StrokeDots), "");
    }

    #[test]
    fn a_read_only_field_refuses_rather_than_writing_quietly() {
        for e in every_kind() {
            for s in all_slots(&e).into_iter().filter(|s| s.shape == Shape::Read) {
                let mut again = e.clone();
                let refused = write(&mut again, s.field, "1").expect_err("a read-only row refuses");
                assert!(refused.contains("not edited"), "{refused}");
                assert_eq!(again, e);
                assert!(s.read_only, "{:?} announces itself read-only", s.field);
            }
        }
    }

    #[test]
    fn a_step_is_one_unit_of_the_field_and_a_rotation_step_is_a_quarter_turn() {
        let rect = of_kind(every_kind(), "rect");
        assert_eq!(stepped(&rect, Field::Feed, 1), "9");
        assert_eq!(stepped(&rect, Field::Head, -1), "3");
        assert_eq!(stepped(&rect, Field::Rot, 1), "90");
        assert_eq!(stepped(&rect, Field::Rot, -1), "-90");
        assert_eq!(stepped(&rect, Field::StrokeDots, 1), "3");
        let free = of_kind(unusual_kinds(), "text");
        assert_eq!(
            stepped(&free, Field::Rot, 1),
            "135",
            "a free angle stays free"
        );
        let text = text_of(every_kind());
        assert_eq!(stepped(&text, Field::Size, 1), "21");
        let line = of_kind(unusual_kinds(), "line");
        assert_eq!(
            stepped(&line, Field::ThicknessDots, -1),
            "0",
            "held at the floor"
        );
        let qr = of_kind(unusual_kinds(), "qr");
        assert_eq!(
            stepped(&qr, Field::Version, 1),
            "41",
            "not moved against the key"
        );
        assert_eq!(stepped(&qr, Field::Version, -1), "40");
        let image = of_kind(every_kind(), "image");
        assert_eq!(
            stepped(&image, Field::Threshold, 1),
            "",
            "the page's stays the page's"
        );
    }

    #[test]
    fn ten_steps_of_a_tenth_read_two_rather_than_a_rounding_error() {
        let mut e = text_of(every_kind());
        assert_eq!(read(&e, Field::LineHeight), "1");
        for _ in 0..10 {
            let next = stepped(&e, Field::LineHeight, 1);
            write(&mut e, Field::LineHeight, &next).expect("a step is accepted");
        }
        assert_eq!(read(&e, Field::LineHeight), "2");
        assert!(matches!(&e, Element::Text(t) if t.line_height == 2.0));
    }
}
