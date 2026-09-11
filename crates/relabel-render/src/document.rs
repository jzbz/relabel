//! The label document: what an operator draws, and what gets saved.
//!
//! # Geometry is whole dots
//!
//! Every position, size and margin is an `i32` count of printer dots.
//! Millimetres are an input convention and a readout, never the storage unit,
//! for three reasons. A millimetre is a promise the hardware cannot keep - at
//! 300 dpi, 40 mm is 472.44 dots and something must round, so it is better that
//! it rounds once at authoring where the operator can see it. On a 96-dot head
//! one dot is 1 % of the label, a barcode module is two or three dots and a
//! text stem is one or two, so sub-dot geometry describes something that cannot
//! be printed. And integers round-trip through JSON exactly, where floats need
//! a non-default `serde_json` feature to promise as much.
//!
//! The single deliberate exception is font size, which is an em size in dots
//! and stays `f32` because shrink-to-fit needs somewhere to move.
//!
//! # Forward compatibility is a feature, not an accident
//!
//! A build that opens a file written by a later version must give it back
//! unharmed. Two mechanisms do that, and both are load-bearing: unknown
//! top-level keys are captured by a flattened map rather than rejected, and
//! unknown *element types* survive as [`ElementOrRaw::Raw`]. The obvious
//! `#[serde(other)]` cannot do the second job - it is only allowed on a unit
//! variant, so it discards the payload it was supposed to preserve.

use crate::target::Dpmm;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The file's own name for itself, checked before anything else is parsed.
pub const FORMAT_TAG: &str = "relabel-label";
/// Bumped only by a change that an older build could not read correctly.
/// Additive fields do not bump it; they get a default instead.
pub const FORMAT_VERSION: u32 = 1;

/// A rectangle in dots, feed axis first.
///
/// Signed on purpose: an element may hang off the edge deliberately, and
/// reporting that is preflight's job rather than something the type forbids.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RectDots {
    pub feed: i32,
    pub head: i32,
    pub feed_len: i32,
    pub head_len: i32,
}

/// Quarter turns. Stored as the degree so a free angle could be added later
/// without breaking the schema.
/// Rotation, stored as a degree.
///
/// [`Other`](Rot::Other) is what keeps the promise the storage format makes. A
/// build that quantised every unknown angle to zero would round-trip a
/// 45-degree element as an unrotated one and write the zero back - destroying
/// the user's work to protect a schema that was designed to accommodate it.
/// Instead the degree survives, and the rasteriser quantises to the nearest
/// quarter turn and says so in its report.
///
/// Deserialised from a signed value so `-90` means 270 rather than failing.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(from = "i32", into = "i32")]
pub enum Rot {
    #[default]
    D0,
    D90,
    D180,
    D270,
    Other(u16),
}

impl From<i32> for Rot {
    fn from(v: i32) -> Rot {
        match v.rem_euclid(360) {
            0 => Rot::D0,
            90 => Rot::D90,
            180 => Rot::D180,
            270 => Rot::D270,
            other => Rot::Other(other as u16),
        }
    }
}

impl From<Rot> for i32 {
    fn from(r: Rot) -> i32 {
        match r {
            Rot::D0 => 0,
            Rot::D90 => 90,
            Rot::D180 => 180,
            Rot::D270 => 270,
            Rot::Other(d) => i32::from(d),
        }
    }
}

impl Rot {
    /// The nearest quarter turn, for a rasteriser that can only do four.
    pub fn quantised(self) -> Rot {
        match self {
            Rot::Other(d) => Rot::from(((i32::from(d) + 45) / 90 * 90) % 360),
            other => other,
        }
    }

    pub fn is_exact(self) -> bool {
        !matches!(self, Rot::Other(_))
    }
}

/// Unprintable margin per edge, in dots.
///
/// The manufacturer's registry carries this per model **and per label type** -
/// a D110_M is 1 mm on its leading edge for both its papers, a plain D110 is
/// 0.5 mm on gap stock and 1.5 mm on transparent, and a B3S is 6 mm on black
/// mark. It is not a per-model constant and must not be flattened into one.
///
/// **Which of the four is which is not confirmed.** The naming here follows the
/// one observation available: a 3-dot bar drawn at the very start of the feed
/// on a D110_M, whose registry says 1 mm on the first field, did not appear on
/// the label. That is consistent with the first field being the leading edge
/// and is not proof of it. Until a print settles it, the safe reading is that
/// *some* edge is unprintable by that much.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Insets {
    pub feed_start: i32,
    pub feed_end: i32,
    pub head_start: i32,
    pub head_end: i32,
}

impl Insets {
    pub fn is_zero(&self) -> bool {
        *self == Insets::default()
    }
}

/// The drawing area, in dots.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Canvas {
    /// Along the feed axis. Deliberately not called "width": which axis is
    /// horizontal depends on the printer, and the document does not know one.
    pub feed: i32,
    /// Across the print head axis.
    pub head: i32,
    pub dpmm: Dpmm,
    #[serde(default, skip_serializing_if = "Insets::is_zero")]
    pub inset: Insets,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnownShape {
    #[default]
    Rect,
    RoundedRect {
        radius: i32,
    },
    Circle,
}

/// The die-cut outline, or one this build has never heard of.
///
/// Adding a shape is an additive change that does not bump the format version,
/// so an older build meets `"oval"` sooner or later. Refusing the whole file
/// over a *guide* would be absurd, and silently defaulting it to a rectangle
/// would lose it on the next save, so the unknown value is carried through
/// verbatim and drawn as a rectangle.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum MediaShape {
    Known(KnownShape),
    Unknown(Value),
}

impl Default for MediaShape {
    fn default() -> MediaShape {
        MediaShape::Known(KnownShape::default())
    }
}

impl MediaShape {
    /// Omitted from a saved file when it is a plain rectangle. For a format
    /// whose point is that labels diff in git, a line of noise in every file is
    /// not free.
    pub fn is_default(&self) -> bool {
        matches!(self, MediaShape::Known(KnownShape::Rect))
    }
}

/// Cable-flag stock: paper at one end that carries no content.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tail {
    pub at_start: bool,
    pub len_dots: i32,
}

/// Fold-over stock.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub at_dots: i32,
    pub parts: u8,
}

/// What happens to the rendered region across a fold.
///
/// A transform of the *rendered* result rather than a clone of each element,
/// which is how the incumbent tool does it. Applying it after rasterisation is
/// undo-safe, composes correctly with halftoning, and gets upside-down text
/// right without a second code path.
/// What happens to the rendered region across a fold.
///
/// [`Unknown`](Repeat::Unknown) exists for the same reason as
/// [`MediaShape::Unknown`], and matters more: mistaking `Copy` for `None`
/// prints half a fold-over label, so an unrecognised value has to be reported
/// rather than quietly treated as nothing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    None,
    Copy,
    Flip,
    #[serde(untagged)]
    Unknown(String),
}

/// The physical stock. Display and preset matching only - [`Canvas`] is the
/// authority on geometry.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Media {
    pub feed_tenths_mm: i32,
    pub head_tenths_mm: i32,
    #[serde(default, skip_serializing_if = "MediaShape::is_default")]
    pub shape: MediaShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<Tail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fold: Option<Fold>,
}

/// What the label was drawn for. Advisory: it lets the app say "this was drawn
/// for a 96-dot head and yours is 192" instead of silently re-fitting.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuthoredFor {
    /// Owned rather than borrowed: this is deserialised from a file, so it
    /// cannot be a `&'static str` however tempting the model registry makes it.
    #[serde(default)]
    pub model: Option<String>,
    pub head_px: u16,
    pub dpi: u16,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authored_for: Option<AuthoredFor>,
}

/// Whether an element lays ink down or knocks it out.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Ink {
    #[default]
    Black,
    /// Knocks back to paper, which is how reversed text is done - and reversed
    /// text is one of the few tricks that stays legible at sixteen dots.
    White,
}

pub type ElementId = u32;

/// Every `type` tag this build understands.
///
/// Kept beside [`Element::kind`], which must agree with it - the test at the
/// bottom of this module fails if they drift.
pub const KNOWN_KINDS: &[&str] = &[
    "text",
    "barcode",
    "qr",
    "data_matrix",
    "image",
    "rect",
    "line",
];

/// Properties every element has.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Common {
    pub id: ElementId,
    #[serde(default)]
    pub name: String,
    pub frame: RectDots,
    #[serde(default)]
    pub rot: Rot,
    #[serde(default)]
    pub ink: Ink,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub locked: bool,
    /// Fields a later version added to *any* element type.
    ///
    /// Exactly one catch-all per element, and it lives here rather than on each
    /// element struct. Two flattened maps in one struct is not merely redundant:
    /// the outer one absorbs the inner one's fields as well, and the resulting
    /// save emits duplicate keys that this same build then refuses to load.
    #[serde(flatten)]
    pub unknown: Map<String, Value>,
}

impl Common {
    pub fn new(id: ElementId, frame: RectDots) -> Common {
        Common {
            id,
            name: String::new(),
            frame,
            rot: Rot::default(),
            ink: Ink::default(),
            hidden: false,
            locked: false,
            unknown: Map::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RectEl {
    #[serde(flatten)]
    pub common: Common,
    /// Zero means filled; any positive value is the outline thickness in dots.
    ///
    /// Unsigned, so a negative value cannot silently mean "filled" - that would
    /// contradict this comment and hide an arithmetic mistake upstream.
    #[serde(default)]
    pub stroke_dots: u16,
    #[serde(default)]
    pub radius_dots: u16,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LineEl {
    #[serde(flatten)]
    pub common: Common,
    #[serde(default = "one_dot")]
    pub thickness_dots: u16,
}

fn one_dot() -> u16 {
    1
}

/// Placeholders for the element types whose renderers land in later steps.
///
/// Defined now rather than later because the *schema* has to be stable from the
/// first saved file, and adding a variant to a tagged enum later would make
/// every older build treat those labels as unknown.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TextEl {
    #[serde(flatten)]
    pub common: Common,
    /// The template, not the resolved string, so a placeholder survives a save.
    pub text: String,
    #[serde(default)]
    pub family: String,
    /// Em size in dots. The one deliberate float; see the module note.
    ///
    /// Validated on the way in and on the way out. `serde_json` writes a
    /// non-finite float as `null`, so a size that became NaN through a
    /// shrink-to-fit division would save without complaint and then fail to
    /// load - putting the operator's text in the file but not on the label.
    pub size: f32,
    #[serde(default = "one_f32")]
    pub line_height: f32,
    /// Where each line sits across the frame's reading width.
    #[serde(default)]
    pub align: TextAlign,
    #[serde(default)]
    pub weight: TextWeight,
    /// How outlines become dots. Always written, like QR error correction: a
    /// later build choosing a different default must not change what an
    /// existing label prints.
    #[serde(default)]
    pub raster: GlyphRaster,
}

/// Where each line of a text element sits across its frame.
///
/// The three style value sets below are open, like [`Repeat`]: a value a
/// later build adds survives a round trip here, and the element is reported
/// as not drawn rather than the whole label refused.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextWeight {
    #[default]
    Regular,
    SemiBold,
    #[serde(untagged)]
    Unknown(String),
}

/// How glyph outlines become dots.
///
/// Which reads better on paper is the P6 print test's question; the default
/// is the one that measured cleanest on the grid.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum GlyphRaster {
    /// Grid-fitted by skrifa's autohinter for a monochrome target, and filled
    /// with hard edges. Every stem of a glyph comes out the same whole number
    /// of dots.
    #[default]
    Hinted,
    /// Unhinted and anti-aliased, left to the binariser's threshold.
    Smooth,
    #[serde(untagged)]
    Unknown(String),
}

fn one_f32() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BarcodeEl {
    #[serde(flatten)]
    pub common: Common,
    pub symbology: String,
    pub data: String,
    /// Dots, never millimetres. Every bar is an exact integer multiple of this
    /// and the rasteriser never scales a barcode: a module width derived from
    /// millimetres rounds inconsistently across one symbol, and a scanner
    /// cannot recover the narrow-element width from that.
    pub module_dots: u16,
    /// Bar height in dots. Zero means the full depth of the element's frame.
    pub bar_dots: u16,
    /// Print below the standards' floors: a module narrower than two dots, or a
    /// retail symbol at a size its specification does not allow.
    ///
    /// Off by default, because the output may not scan. It exists so the
    /// module-width print test can be run at all, and so a label read only by
    /// one known scanner can be made smaller than the standards would like.
    #[serde(default, skip_serializing_if = "is_false")]
    pub out_of_spec: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// QR error correction: the share of the symbol that can be lost and still
/// read, roughly 7, 15, 25 and 30 percent.
///
/// Always written out, never skipped as a default. A label's meaning must not
/// change if a later build picks a different default, and for a QR code the
/// level decides the size.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QrEc {
    L,
    /// The common default. Direct thermal scuffs, so the extra recovery over L
    /// is worth the capacity.
    #[default]
    M,
    Q,
    H,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct QrEl {
    #[serde(flatten)]
    pub common: Common,
    pub data: String,
    /// Dots per module, and every module is exactly this many dots square.
    pub module_dots: u16,
    #[serde(default)]
    pub ec: QrEc,
    /// A pinned version, 1 to 40. Absent means the smallest that holds the
    /// data - fine for a one-off label, wrong for a batch, where the symbol
    /// would change size from row to row and shift the layout, so the editor
    /// pins it.
    ///
    /// There is no way to ask for Micro QR, deliberately; see the
    /// [`code2d`](crate::code2d) module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    /// Permit a one-dot module. Off by default, because it will not scan.
    #[serde(default, skip_serializing_if = "is_false")]
    pub out_of_spec: bool,
}

/// Which DataMatrix symbol to use.
///
/// For the three automatic choices, the symbol drawn is the first in the
/// encoder's own order - fewest data codewords, then the shorter diagonal -
/// that holds the data and fits the element's frame at its module size. When
/// none fits the frame, the smallest that holds the data is chosen, and drawing
/// it reports the shortfall.
///
/// Stored as a string so that a value this build does not know - or a size it
/// has no table entry for - survives a round trip rather than failing the load.
/// The rasteriser reports it instead.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(from = "String", into = "String")]
pub enum DmSize {
    /// Squares only: `"square"`.
    #[default]
    Square,
    /// Rectangles only, DMRE included: `"rectangle"`. Usually the right answer
    /// on a long, narrow label.
    Rectangle,
    /// Either: `"any"`.
    Any,
    /// Exactly this size, rows by columns: `"16x48"`. What the editor pins, for
    /// the same reason as a QR version.
    Fixed {
        rows: u8,
        cols: u8,
    },
    Unknown(String),
}

impl From<String> for DmSize {
    fn from(s: String) -> DmSize {
        match s.as_str() {
            "square" => return DmSize::Square,
            "rectangle" => return DmSize::Rectangle,
            "any" => return DmSize::Any,
            _ => {}
        }
        // Only the canonical spelling parses, so "016x48" is preserved as
        // written rather than rewritten on the next save.
        if let Some((r, c)) = s.split_once('x')
            && let (Ok(rows), Ok(cols)) = (r.parse::<u8>(), c.parse::<u8>())
            && format!("{rows}x{cols}") == s
        {
            return DmSize::Fixed { rows, cols };
        }
        DmSize::Unknown(s)
    }
}

impl From<DmSize> for String {
    fn from(s: DmSize) -> String {
        match s {
            DmSize::Square => "square".into(),
            DmSize::Rectangle => "rectangle".into(),
            DmSize::Any => "any".into(),
            DmSize::Fixed { rows, cols } => format!("{rows}x{cols}"),
            DmSize::Unknown(s) => s,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DataMatrixEl {
    #[serde(flatten)]
    pub common: Common,
    pub data: String,
    /// Dots per module, and every module is exactly this many dots square.
    pub module_dots: u16,
    /// Always written out, for the same reason as [`QrEl::ec`].
    #[serde(default)]
    pub size: DmSize,
    /// Permit a one-dot module. Off by default, because it will not scan.
    #[serde(default, skip_serializing_if = "is_false")]
    pub out_of_spec: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ImageEl {
    #[serde(flatten)]
    pub common: Common,
    /// Relative to the document where possible, so a label and its artwork move
    /// together.
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Element {
    Text(TextEl),
    Barcode(BarcodeEl),
    Qr(QrEl),
    DataMatrix(DataMatrixEl),
    Image(ImageEl),
    Rect(RectEl),
    Line(LineEl),
}

impl Element {
    pub fn common(&self) -> &Common {
        match self {
            Element::Text(e) => &e.common,
            Element::Barcode(e) => &e.common,
            Element::Qr(e) => &e.common,
            Element::DataMatrix(e) => &e.common,
            Element::Image(e) => &e.common,
            Element::Rect(e) => &e.common,
            Element::Line(e) => &e.common,
        }
    }

    /// The keys an element of this kind writes besides the common ones. A key
    /// with one of these names in the element's unknown-field map would be
    /// written twice, and the reload would keep only one of them.
    pub fn own_keys(&self) -> &'static [&'static str] {
        match self {
            Element::Text(_) => &[
                "text",
                "family",
                "size",
                "line_height",
                "align",
                "weight",
                "raster",
            ],
            Element::Barcode(_) => &[
                "symbology",
                "data",
                "module_dots",
                "bar_dots",
                "out_of_spec",
            ],
            Element::Qr(_) => &["data", "module_dots", "ec", "version", "out_of_spec"],
            Element::DataMatrix(_) => &["data", "module_dots", "size", "out_of_spec"],
            Element::Image(_) => &["path"],
            Element::Rect(_) => &["stroke_dots", "radius_dots"],
            Element::Line(_) => &["thickness_dots"],
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Element::Text(_) => "text",
            Element::Barcode(_) => "barcode",
            Element::Qr(_) => "qr",
            Element::DataMatrix(_) => "data_matrix",
            Element::Image(_) => "image",
            Element::Rect(_) => "rect",
            Element::Line(_) => "line",
        }
    }
}

/// An element, or one this build does not recognise.
///
/// The second case is the whole point. A label written by a later version, with
/// an element type that did not exist when this build was compiled, must come
/// back out of a round-trip byte for byte - so it is kept as raw JSON, drawn as
/// a placeholder, and written back unchanged.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum ElementOrRaw {
    Known(Element),
    Raw(Value),
}

impl ElementOrRaw {
    /// The `type` tag, whether or not this build understands it.
    pub fn kind(&self) -> Option<&str> {
        match self {
            ElementOrRaw::Known(e) => Some(e.kind()),
            // Untagged enums give unhelpful errors on genuinely corrupt input,
            // so an unknown element is required to at least look like one.
            ElementOrRaw::Raw(v) => v.get("type").and_then(Value::as_str),
        }
    }

    pub fn known(&self) -> Option<&Element> {
        match self {
            ElementOrRaw::Known(e) => Some(e),
            ElementOrRaw::Raw(_) => None,
        }
    }
}

/// A whole label.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Label {
    /// Checked before anything else, so foreign JSON fails at the first field
    /// rather than after a confusing partial parse.
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub meta: Meta,
    pub media: Media,
    pub canvas: Canvas,
    pub elements: Vec<ElementOrRaw>,
    #[serde(default)]
    pub repeat: Repeat,
    /// Everything a later version wrote that this build does not know about.
    /// Never `deny_unknown_fields`: silently deleting a stranger's data is
    /// worse than not understanding it.
    #[serde(flatten)]
    pub unknown: Map<String, Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not a relabel label: the format tag is {0:?}")]
    NotALabel(String),
    /// A file from a future major version, which by definition this build
    /// cannot read correctly. Additive changes do not land here - they arrive
    /// as unknown fields, which are preserved.
    #[error(
        "this label needs a newer relabel: it is version {found}, this build reads {supported}"
    )]
    TooNew { found: u32, supported: u32 },
    #[error("an element has no type tag")]
    UntypedElement,
    /// An element whose `type` this build *does* know, but which failed to
    /// parse.
    ///
    /// Without this the untagged fallback would quietly demote it to an opaque
    /// blob: the file would load, the element would vanish from the label, and
    /// the report would blame a future version for a type this build
    /// understands perfectly well. Worse, re-saving preserves the blob, so the
    /// element is dead permanently.
    #[error("the {kind} element {id} is malformed: {reason}")]
    MalformedElement {
        id: String,
        kind: String,
        reason: String,
    },
    /// A key written into an escape-hatch map that shadows a real field.
    #[error("{0:?} is a reserved key and cannot be stored as an unknown field")]
    ReservedKey(String),
    /// A version that is absent, zero, or not a number.
    #[error("the format version is missing or not a whole number of at least 1")]
    BadVersion,
    /// Two elements claiming the same id.
    #[error("element id {0} appears more than once")]
    DuplicateId(ElementId),
    /// A float that would be written as `null` and fail to load back.
    #[error("element {id} has a {what} of {value}, which cannot be saved")]
    BadFloat {
        what: &'static str,
        id: ElementId,
        value: f32,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Keys that already mean something at the top level of a label.
const RESERVED_LABEL_KEYS: &[&str] = &[
    "format", "version", "meta", "media", "canvas", "elements", "repeat",
];

/// Keys that already mean something on an element.
const RESERVED_ELEMENT_KEYS: &[&str] = &[
    "type", "id", "name", "frame", "rot", "ink", "hidden", "locked",
];

/// Bring an older document up to the current shape.
///
/// One arm today, and that is the point: the chain has to exist before the
/// first file ships, or the first real version bump has nowhere to go. The
/// fixtures under `tests/corpus/` are what actually keep old files working;
/// this function is only the code that reads them.
fn migrate(version: u32, raw: Value) -> Result<Value, LoadError> {
    match version {
        1 => Ok(raw),
        other => Err(LoadError::TooNew {
            found: other,
            supported: FORMAT_VERSION,
        }),
    }
}

impl Label {
    /// A blank label sized for a canvas.
    pub fn new(feed: i32, head: i32, dpmm: Dpmm) -> Label {
        Label {
            format: FORMAT_TAG.to_string(),
            version: FORMAT_VERSION,
            meta: Meta::default(),
            media: Media {
                feed_tenths_mm: dpmm.tenths_mm_from_dots(feed),
                head_tenths_mm: dpmm.tenths_mm_from_dots(head),
                shape: MediaShape::default(),
                tail: None,
                fold: None,
            },
            canvas: Canvas {
                feed,
                head,
                dpmm,
                inset: Insets::default(),
            },
            elements: Vec::new(),
            repeat: Repeat::None,
            unknown: Map::new(),
        }
    }

    /// Load a label.
    ///
    /// Parsed in two stages, and the order matters. Deserialising straight into
    /// `Label` would run the format and version gates only *after* the whole
    /// body had parsed - so a genuine future version would be reported as
    /// `missing field \`media\`` rather than "you need a newer relabel", and a
    /// foreign file would give whichever structural complaint came first. The
    /// gates are only useful to the files that actually need them, which are
    /// exactly the files that will not deserialise.
    pub fn from_json(s: &str) -> Result<Label, LoadError> {
        let raw: Value = serde_json::from_str(s)?;

        match raw.get("format").and_then(Value::as_str) {
            Some(FORMAT_TAG) => {}
            Some(other) => return Err(LoadError::NotALabel(other.to_string())),
            None => return Err(LoadError::NotALabel("(absent)".into())),
        }
        let version = raw
            .get("version")
            .and_then(Value::as_u64)
            .ok_or(LoadError::BadVersion)?;
        if version == 0 {
            // Not merely odd: a zero would skip the migration chain entirely
            // the moment there is one.
            return Err(LoadError::BadVersion);
        }
        if version > u64::from(FORMAT_VERSION) {
            return Err(LoadError::TooNew {
                found: version as u32,
                supported: FORMAT_VERSION,
            });
        }

        let raw = migrate(version as u32, raw)?;
        let label: Label = serde_json::from_value(raw)?;
        // An untagged enum will happily accept any object as `Raw`, so a
        // corrupt element would otherwise become a silent placeholder.
        if label.elements.iter().any(|e| e.kind().is_none()) {
            return Err(LoadError::UntypedElement);
        }
        // `Raw` must mean "a type tag this build has never heard of" and
        // nothing else. An untagged enum falls through on *any* deserialisation
        // error, so a known type with one bad field lands in `Raw` too - which
        // has to be reported, not absorbed.
        for el in &label.elements {
            let ElementOrRaw::Raw(v) = el else { continue };
            let Some(kind) = v.get("type").and_then(Value::as_str) else {
                continue;
            };
            if !KNOWN_KINDS.contains(&kind) {
                continue;
            }
            let reason = match serde_json::from_value::<Element>(v.clone()) {
                Ok(_) => "it round-trips alone but not in place".to_string(),
                Err(e) => e.to_string(),
            };
            return Err(LoadError::MalformedElement {
                id: v
                    .get("id")
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "?".into()),
                kind: kind.to_string(),
                reason,
            });
        }
        // Ids are the handle everything else uses - the report names elements
        // by id, and an editor selects by it. Duplicates are silent chaos.
        let mut seen = std::collections::BTreeSet::new();
        for el in label.elements.iter().filter_map(ElementOrRaw::known) {
            if !seen.insert(el.common().id) {
                return Err(LoadError::DuplicateId(el.common().id));
            }
        }
        Ok(label)
    }

    /// Pretty-printed with a trailing newline, so labels diff in git.
    ///
    /// Refuses rather than writing a file this same build could not read back.
    /// That is the worst outcome the format has, so it is checked here even
    /// though it means `to_json` can fail for reasons other than I/O.
    pub fn to_json(&self) -> Result<String, LoadError> {
        self.check_writable()?;
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }

    /// Everything that would produce an unloadable file.
    fn check_writable(&self) -> Result<(), LoadError> {
        // A key in an escape-hatch map that collides with a real field emits
        // that key twice, and the reload fails on the duplicate. Not reachable
        // by deserialising - serde consumes known names first - but entirely
        // reachable by code writing into these public maps.
        for k in self.unknown.keys() {
            if RESERVED_LABEL_KEYS.contains(&k.as_str()) {
                return Err(LoadError::ReservedKey(k.clone()));
            }
        }
        for el in self.elements.iter().filter_map(ElementOrRaw::known) {
            let c = el.common();
            for k in c.unknown.keys() {
                if RESERVED_ELEMENT_KEYS.contains(&k.as_str())
                    || el.own_keys().contains(&k.as_str())
                {
                    return Err(LoadError::ReservedKey(k.clone()));
                }
            }
            if let Element::Text(t) = el {
                for (what, v) in [("size", t.size), ("line_height", t.line_height)] {
                    if !v.is_finite() || v <= 0.0 {
                        return Err(LoadError::BadFloat {
                            what,
                            id: c.id,
                            value: v,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Elements this build can actually draw, in z-order, skipping hidden ones.
    pub fn drawable(&self) -> impl Iterator<Item = &Element> {
        self.elements
            .iter()
            .filter_map(ElementOrRaw::known)
            .filter(|e| !e.common().hidden)
    }

    /// Elements written by a later version, which are preserved but not drawn.
    pub fn unrecognised(&self) -> impl Iterator<Item = &str> {
        self.elements.iter().filter_map(|e| match e {
            ElementOrRaw::Raw(v) => v.get("type").and_then(Value::as_str),
            ElementOrRaw::Known(_) => None,
        })
    }
}
