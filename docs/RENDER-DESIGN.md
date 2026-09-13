# `relabel-render` — Design

**The label document, and the rasteriser that turns it into a 1-bit plane.**

Status: specification. Written against `DESIGN.md` §3.2, §4, §5 and §6.3, which this document supersedes for everything inside the crate. Where a researcher's claim was corrected by reading the crate source, the correction is what appears here; where something is genuinely unknown it says so and names the defensive choice.

---

## 1. Scope and boundaries

`relabel-render` owns three things and nothing else:

1. **The label document** — its Rust types, its `.relabel` JSON serialisation, its migration chain, and the variable-data binding that turns one document plus one CSV row into one page, printed as many times as the row's `$times` asks.
2. **The rasteriser** — document → 8-bit ink coverage → 1-bit `BitPlane`, at a stated dot size, with per-element control over anti-aliasing and halftoning.
3. **Preflight** — measuring every element of every bound row *before* a single frame is sent, and reporting what will not fit.

It knows exactly one thing about the destination:

```rust
/// Everything the rasteriser is allowed to know about where the label is going.
///
/// No model name, no printhead constant, no print direction. `relabel-core`
/// resolves a `ModelSpec` into one of these; `relabel-proto::encode_page`
/// applies the model's rotation afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Dots across the printhead axis. For a D110_M this is 96.
    pub head_px: u16,
    /// Dots along the feed axis. 320 for a 40 mm label at 203 dpi.
    pub feed_px: u16,
    pub dpi: u16,
}
```

**What it must not know about.** No `btleplug`, no `tokio`, no `slint`, no `relabel_core` type. No `PrintDirection` — rotation for a `Left`-direction head is `relabel-proto`'s `BitPlane::oriented()`, applied at the encoder, and the canvas is *always* authored in the orientation the printed label reads. No transport, no session, no job. It has no `async fn` and does no I/O except reading image and font files the caller names by path — and even those go through a caller-supplied byte slice wherever a GUI would rather hold the bytes itself.

**What it depends on from below.** `relabel-proto`, for `BitPlane` and for `models::{mm_to_px, px_to_mm, fit_to_head}`. That direction is one-way and must stay so: `BitPlane` lives in proto precisely so that a headless print server can link proto without linking parley.

**Why the boundary is drawn here rather than folding the renderer into core.** The dependency tail. `relabel-proto` builds in about a second with one dependency and is the fuzz target; adding parley, skrifa, tiny-skia and `image` to it would multiply that by two orders of magnitude for code the protocol tests never call. Keeping the split means the accumulator fuzzer needs no fonts and the protocol table tests need no rasteriser.

**One boundary that is deliberately *not* drawn.** The 1-bit → RGB8 expansion for the Slint preview lives here, in `preview.rs`, not in `relabel-gui` and not in `relabel-proto`. It takes and returns plain slices with no toolkit type in the signature (§7), so the crate stays toolkit-agnostic, but the zoom-and-expand loop is a rendering concern and duplicating it in the GUI would put the dot-grid arithmetic in two places.

---

## 2. The document model

### 2.1 The unit decision: whole dots, and an exact rational pitch

Every position, size, stroke width, module width and quiet zone is an `i32` **dot**. Millimetres are an input convention and a display readout. They are never the storage unit.

The argument, in order of weight:

**A millimetre value is a promise the hardware cannot keep.** `vendor/devices.json` carries exactly two resolutions across all 79 models — 203 dpi on 56 of them, 300 dpi on 23 — and `crates/relabel-proto/build.rs:62-66` fixes the vendor's own convention as `dpmm = if dpi >= 300 { 11.811_024 } else { 8.0 }`. At 203 dpi a 40 × 12 mm label is exactly 320 × 96 dots and one dot is 0.125 mm. At 300 dpi, 40 mm is 472.44 dots. Something must round. If the document stores millimetres, that rounding happens invisibly on every render; if it stores dots, it happens once, at authoring, where the operator can see the result and nudge it.

**On this hardware a dot is not a rounding error, it is a unit of design.** One dot is 1.04 % of a 96-dot axis. A barcode module is two or three dots. A text stem is between one and two. There is no sub-dot output — the head fires an element or it does not — so a geometry model with more resolution than the device is modelling something that cannot be printed.

**Float geometry is not guaranteed to survive a save/load.** `serde_json`'s `float_roundtrip` feature exists precisely because the default parse is best-effort; its own manifest describes the feature as using "sufficient precision … to ensure that they maintain accuracy when round-tripped through JSON" at roughly twice the parse cost. A document format whose geometry is only stable if you remember a non-default feature flag is a bad format. Integers have no such caveat, and they make snap-to-dot, arrow-key nudge, undo deduplication and `git diff` all trivial.

**Fractions of the label were considered and rejected.** Resizing a canvas from 320 to 384 dots would rescale a 2-dot barcode module to 2.4, which is a scan failure, and text does not scale linearly at these sizes anyway.

The document therefore records the pitch it was authored at, as an exact rational rather than a float:

```rust
/// Dots per millimetre, exactly. 8/1 at 203 dpi, 3000/254 at 300 dpi.
///
/// `relabel-proto`'s `mm_to_px` uses a truncated float constant (11.811_024
/// where the true value is 11.811023622…). Evaluating every 300-dpi
/// `max_print_width_mm` in the vendor catalogue shows the truncation never
/// changes the result, so it is latent rather than live — but the document
/// model does its own conversion through this type so the class of error
/// cannot arise here, and `build.rs` should be moved onto it too.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dpmm { pub num: u32, pub den: u32 }

impl Dpmm {
    pub const DPI_203: Dpmm = Dpmm { num: 8,    den: 1   };
    pub const DPI_300: Dpmm = Dpmm { num: 3000, den: 254 };

    pub const fn for_dpi(dpi: u16) -> Dpmm {
        if dpi >= 300 { Dpmm::DPI_300 } else { Dpmm::DPI_203 }
    }

    /// Exact, then one round-half-away-from-zero at the end. i64 intermediates
    /// because 3000 * a label length in tenths of a mm must not overflow.
    pub fn dots_from_tenths_mm(self, tenths: i32) -> i32 {
        let n = i64::from(tenths) * i64::from(self.num);
        let d = i64::from(self.den) * 10;
        (if n >= 0 { (n + d / 2) / d } else { (n - d / 2) / d }) as i32
    }

    pub fn tenths_mm_from_dots(self, dots: i32) -> i32 {
        let n = i64::from(dots) * i64::from(self.den) * 10;
        let d = i64::from(self.num);
        (if n >= 0 { (n + d / 2) / d } else { (n - d / 2) / d }) as i32
    }
}
```

**The one exception is font size**, which is `f32` in dots — an em size, which is what parley wants — because quantising it to whole dots removes the range that shrink-to-fit needs to work in. It is *rendered* at an integer ppem (§4.3); the document stores the request, the renderer stores the compromise.

**Rotation is quantised** to `Rot { D0, D90, D180, D270 }` and serialised as the integer degree so an arbitrary angle can be added later without a schema break. §6.3 of `DESIGN.md` already restricts text to 90° steps; this extends that to every element, for the reason in §4.5.

Independent confirmation that dots are the right unit: niimblue, the incumbent tool, stores label size in dots. Its `DEFAULT_LABEL_PROPS` is `{ printDirection: "left", size: { width: 240, height: 96 } }`, and millimetres appear only in its `LabelPreset` type alongside an explicit `unit` and `dpmm`. Two designs arriving at the same answer separately is worth something.

### 2.2 The types

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Label {
    /// Literal "relabel-label". Rejects foreign JSON at the first field rather
    /// than after a confusing partial parse.
    pub format: FormatTag,
    /// Major only. Additive fields never bump it. See §2.5.
    pub version: u32,
    #[serde(default)] pub meta: Meta,
    pub media: Media,
    pub canvas: Canvas,
    pub elements: Vec<ElementOrRaw>,
    #[serde(default)] pub repeat: Repeat,
    #[serde(default)] pub data: Option<DataBinding>,
    /// Everything a future version wrote that this build does not know about.
    /// Never `deny_unknown_fields`; see §2.5.
    #[serde(flatten)] pub unknown: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Meta {
    #[serde(default)] pub title: String,
    #[serde(default)] pub created: Option<String>,    // RFC 3339
    #[serde(default)] pub modified: Option<String>,
    #[serde(default)] pub app: Option<String>,        // "relabel 0.1.0"
    /// Advisory only. Lets the app warn "this was drawn for a 96-dot head and
    /// you have a 192-dot one" instead of silently re-fitting.
    #[serde(default)] pub authored_for: Option<AuthoredFor>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct AuthoredFor { pub model: Option<&'static str>, pub head_px: u16, pub dpi: u16 }

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Canvas {
    /// Along the feed axis, in dots. Never called "width".
    pub feed: i32,
    /// Across the printhead axis, in dots. Never called "height".
    pub head: i32,
    pub dpmm: Dpmm,
    /// Unprintable margin, in dots, per edge. The vendor record carries a
    /// per-material `blindZone` ("1.0|0.0|0.0|0.0" for D11 gap paper) that
    /// relabel's normalised snapshot currently drops; when it is re-extracted
    /// this is where it lands. Which of the four edges is which is UNVERIFIED,
    /// so the default is zero and this is a manual field until a print settles it.
    #[serde(default)] pub inset: Insets,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Insets { pub feed_start: i32, pub feed_end: i32, pub head_start: i32, pub head_end: i32 }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Media {
    /// Tenths of a millimetre, integers. Display and preset matching only —
    /// the canvas is the authority on geometry.
    pub feed_tenths_mm: i32,
    pub head_tenths_mm: i32,
    #[serde(default)] pub shape: MediaShape,   // Rect | RoundedRect { radius } | Circle | Unknown(Value)
    /// Cable-flag stock: a length of paper at one end of one axis that carries
    /// no content.
    #[serde(default)] pub tail: Option<Tail>,     // { at_start, len_dots, axis }
    /// Fold-over stock: where the label creases, along which axis, and into how
    /// many panels. `at_dots` is the position of the first crease, measured from
    /// the canvas origin.
    #[serde(default)] pub fold: Option<Fold>,     // { at_dots, parts, axis }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default] None,
    /// Duplicate the rendered region across the fold, unchanged.
    Copy,
    /// Duplicate rotated 180°, for a fold-over flag read from both sides.
    Flip,
    /// A mode a later version wrote. Reported, never treated as `None`.
    #[serde(untagged)] Unknown(Value),
}
```

`shape`, `tail` and `fold` are **editing guides and layout constraints, not marks**. This is taken from niimblue, where `_renderBackground()` draws them behind a `customBackground` flag while `getLabelBounds()` genuinely subtracts `tailLength` from the usable canvas — so the tail is real paper that must be kept clear, and the shape is a rounded or circular die-cut the content must stay inside. relabel does **not** shorten the canvas the way `getLabelBounds()` does: the page handed to the printer has to cover the whole label, because the paper travels that far and `Target::require_head` demands the head axis match exactly, so a shortened canvas prints shifted rather than inset. `media::Bounds` carries three rectangles instead - `canvas`, `content` (less the tail) and `usable` (less `Canvas::inset`) - and the die-cut is resolved against the canvas rather than stored, since a circle's centre and diameter follow from it. They cost about thirty lines of guide drawing and they describe cable-flag and fold-over stock, which is a large part of why these printers get bought.

`mirror` is the one property of niimblue's that changes output, and relabel models it differently on purpose. niimblue's `createMirroredObjects()` clones each canvas object across the fold and rotates it (its own comment restricts flip to a half split). relabel's `Repeat` is a transform of the **rendered region**, applied after **binarisation** to the finished plane: undo-safe, and it gets flipped text right for free without a second code path. After binarisation is not an implementation detail. Diffusion runs in the canvas frame, so a region rotated before it is dithered disagrees with its own half turn in 6,209 of 15,360 dots under Floyd-Steinberg, and a grey copy sharing its source's zone id moves 3,794 dots of the panel that was already right. Applied to the dots, the turned panel is the half turn exactly, the first panel does not move, and the halftone zones need no copy at all.

**`data` names the label's rows.** It is written only when set, so a label without data saves byte for byte as before, and `"data"` is a reserved key. A binding of a kind this build does not know - `inline`, say - is kept exactly as written; one of a kind it does know that does not parse, one with no `kind`, or a bare string is a load error, never carried along unread. A binding cannot hold one of its own keys among its unknown fields, or it would write that key twice. §2.6 has the types.

**`printDirection` is deliberately absent from the document.** `encode_page` already begins `let plane = plane.oriented(head.direction)` using the model registry's own constant. A document that carried a print direction would be asserting a fact about a printer it may never meet.

### 2.3 Elements

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ElementOrRaw {
    Known(Element),
    /// An element type a future version wrote. Preserved verbatim, drawn as a
    /// hatched placeholder, never silently dropped. See §2.5 for why
    /// #[serde(other)] cannot do this job.
    Raw(serde_json::Value),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Element {
    Text(TextEl),
    Barcode(BarcodeEl),
    Qr(QrEl),
    DataMatrix(DmEl),
    Image(ImageEl),
    Rect(RectEl),
    Line(LineEl),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Common {
    pub id: ElementId,                                // u32, unique within the doc
    #[serde(default)] pub name: String,               // "SKU barcode" — appears in preflight
    /// Position and size in dots, feed axis first. Signed: an element may hang
    /// off the edge deliberately (a bleed), and preflight warns rather than the
    /// model forbidding it.
    pub frame: RectDots,
    #[serde(default)] pub rot: Rot,
    /// One bit is one bit. `White` knocks out — which is how reversed text is
    /// done, and reversed text is one of the few tricks that stays legible at
    /// 16 dots. There is no colour here and there never will be.
    #[serde(default)] pub ink: Ink,                   // Black | White
    #[serde(default)] pub hidden: bool,
    #[serde(default)] pub locked: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TextEl {
    #[serde(flatten)] pub common: Common,
    /// The template, not the resolved string. `{sku}` survives a save.
    pub text: String,
    pub font: FontRef,                                // { family, weight: u16, italic: bool }
    /// Em size in dots. f32 — see §2.1.
    pub size: f32,
    #[serde(default)] pub align: Align,               // Start | Center | End
    #[serde(default)] pub valign: VAlign,             // Top | Middle | Bottom
    /// Multiple of em. 1.0 default, which is tighter than Noto's natural 1.362
    /// and is the right default when four lines must fit 96 dots.
    #[serde(default = "one")] pub line_height: f32,
    /// Extra advance per cluster, in dots. Fractional, applied before the
    /// per-glyph dot snap. Genuinely useful for squeezing one more character.
    #[serde(default)] pub letter_spacing: f32,
    #[serde(default = "yes")] pub wrap: bool,
    #[serde(default)] pub max_lines: Option<u8>,
    #[serde(default)] pub fit: TextFit,               // §2.6
    /// None = decide from ppem (§4.3). Some(_) = the operator has printed both
    /// and knows which one this label wants.
    #[serde(default)] pub raster: Option<GlyphRaster>, // MonoHinted | Antialiased
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BarcodeEl {
    #[serde(flatten)] pub common: Common,
    pub symbology: Symbology,
    /// The user's plain data. Never the encoder's internal escapes — see §5.2.
    pub data: String,
    /// The single most important field on this type. Dots, never millimetres:
    /// TEC-IT documents printers whose driver assumes 203.2 dpi where the spec
    /// says 203.0, so a mm-derived module width rounds inconsistently across
    /// one symbol and the scanner cannot recover the narrow-element width.
    /// Every bar is an exact integer multiple of this. The rasteriser NEVER
    /// scales a barcode.
    pub module_dots: u16,
    /// Wide:narrow for Code 39 and ITF only. 2 or 3; the crate's own tables
    /// bake 2:1, so 3:1 is not available in v1 (§8).
    #[serde(default)] pub ratio: u8,
    pub bar_dots: u16,
    /// Dots. Defaults come from the symbology, not from zero.
    #[serde(default)] pub quiet_dots: Option<u16>,
    #[serde(default)] pub hri: Option<Hri>,           // { size: f32, pos: Below | Above }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct QrEl {
    #[serde(flatten)] pub common: Common,
    pub data: String,
    pub ec: QrEc,                                     // L | M | Q | H
    pub module_dots: u16,
    #[serde(default = "four")] pub quiet_modules: u8,
    /// Pinned, or `None` for automatic. Pinning matters more here than
    /// anywhere else: in a CSV run an automatic version silently changes size
    /// between rows and shifts everything around it. Pinning turns "row 312
    /// needs a bigger symbol" into a preflight error instead of a bad label.
    ///
    /// Only Normal(1..=40) is representable. Micro QR is NOT offered — see §8.
    #[serde(default)] pub version: Option<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DmEl {
    #[serde(flatten)] pub common: Common,
    pub data: String,
    pub module_dots: u16,
    /// `None` = smallest that fits the data. Named sizes include the DMRE
    /// rectangles, which are the right shape for a 40 × 12 mm label (§6.4).
    #[serde(default)] pub symbol: Option<DmSymbol>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImageEl {
    #[serde(flatten)] pub common: Common,
    /// A breadcrumb for re-import, never opened. Always written, even empty:
    /// a build from before artwork was embedded requires it.
    #[serde(default)] pub path: String,
    /// Base64 of an 8-bit grey PNG, composited over white and sized for the
    /// frame at import (§4.6): a few KB, not a 3 MP photo, which removes the
    /// only argument for a zip container. The key's name is the format.
    #[serde(default, skip_serializing_if = "String::is_empty")] pub png: String,
    #[serde(default)] pub fit: ImageFit,              // Contain | Cover | Stretch | Dots, open
    /// Per-element override of the page halftone, written by import: an
    /// imported photo gets Floyd–Steinberg, a logo a threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub halftone: Option<ImageHalftone>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub threshold: Option<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RectEl {
    #[serde(flatten)] pub common: Common,
    /// Dots. 0 with `fill: false` draws nothing and preflight says so.
    #[serde(default)] pub stroke_dots: u16,
    #[serde(default)] pub fill: bool,
    #[serde(default)] pub radius_dots: u16,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LineEl {
    #[serde(flatten)] pub common: Common,
    pub a: PointDots,
    pub b: PointDots,
    #[serde(default = "one_dot")] pub width_dots: u16,
}
```

**As built, step 7.** `ImageEl` above replaces this section's first design, an `ImageSource` enum and an `invert` flag. `path` keeps its v1 name, so the frozen corpus element `"path": "logo.png"` still loads - and is reported as having no artwork - and a build from before step 7 still loads a newer image, carrying `png`, `fit`, `halftone` and `threshold` through a save as unknown fields. `ink: white` does what `invert` would have, without turning a transparent surround into a black box.

There is no `DateTime` element and no `Counter` element. Both are *variables* (§2.6), which keeps the element list at the seven the UI has to draw inspectors for.

### 2.4 Where files live

`dirs` 6.0.0, read from source rather than from its docs, because two of its functions do not do what they are documented to do.

| What | Call | Linux | macOS | Windows |
|---|---|---|---|---|
| `config.json` | `dirs::config_dir()` | `~/.config/relabel` | `~/Library/Application Support/relabel` | `%APPDATA%\relabel` |
| Printers, presets, user fonts | `dirs::data_dir()` | `~/.local/share/relabel` | same as config | `%APPDATA%\relabel` |
| Thumbnails, plane cache | `dirs::cache_dir()` | `~/.cache/relabel` | `~/Library/Caches/relabel` | `%LOCALAPPDATA%\relabel` |
| `.relabel` documents | see below | `~/Documents/relabel` | same | same |

Two traps, both found by reading:

- **Never call `dirs::preference_dir()`.** Its doc table at `dirs-6.0.0/src/lib.rs:148-158` claims `{FOLDERID_RoamingAppData}` on Windows; `dirs-6.0.0/src/win.rs:13` calls `dirs_sys::known_folder_local_app_data()`. Use `config_dir()`.
- **`document_dir()` returns `None` on a Linux box without xdg-user-dirs.** `lin.rs:22` is `dirs_sys::user_dir("DOCUMENTS")`, which reads `$XDG_CONFIG_HOME/user-dirs.dirs` through `read_all(..).unwrap_or(Vec::new())` and then removes from an empty map. Containers and minimal installs hit this.

```rust
fn default_label_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("relabel")
}
```

and that is a *default offered in the file dialog*, never a mandatory location. Every `dirs` function returns `Option<PathBuf>` and `dirs-sys-0.5.0/src` contains no `unwrap`, `expect` or `panic!`.

Saves are atomic: temp file **in the destination directory**, then rename, so the rename stays on one filesystem. That promotes `tempfile` from a dev-dependency to a real one for the GUI. A label editor that corrupts a file on a crash mid-save is unforgivable and the fix is fifteen lines.

For `THIRD-PARTY.md`: `dirs` 6 and `dirs-sys` 0.5 are MIT OR Apache-2.0, but `dirs-sys` depends on **`option-ext` 0.2.0, which is MPL-2.0** — file-level copyleft, fine to link from MIT, and the same class as `serialport`. Both are listed in `THIRD-PARTY.md` against the day they enter the graph.

### 2.5 Serialisation and evolution

One pretty-printed JSON file, extension `.relabel`, LF endings, trailing newline, so labels diff in git. `serde_json`'s `Map` is a `BTreeMap` unless `preserve_order` is on, so key order is deterministic with the default features already in the workspace.

The evolution rules, in order of how much damage breaking them does:

1. **Never `deny_unknown_fields`.** A v1 build that round-trips a file written by a v1.3 build must not silently delete the fields it did not recognise. `#[serde(flatten)] unknown: Map<String, Value>` on `Label` is what preserves them.
2. **Unknown element types need a different mechanism, and `#[serde(other)]` is not it.** `serde_derive-1.0.229/src/internals/check.rs:187` errors with `"#[serde(other)] must be on a unit variant"`, `:178` with `"must be on the last variant"`, and `:169` with `"cannot appear on untagged enum"`. It can only discard the payload. The `#[serde(untagged)] ElementOrRaw` above is the only shape that carries a future element through a round-trip. Validate that a `Raw` at least has a string `"type"` key, because untagged enums otherwise give useless error messages on genuinely corrupt input.
3. **`version` is a major number.** Additive fields get `#[serde(default)]` and do not bump it. `fn migrate(version: u32, raw: Value) -> Result<Value, LoadError>` is a chain, and `tests/corpus/v1/*.relabel` fixtures are never deleted. That corpus is what actually keeps saved files working; the migration function is just the code that reads it.
4. **No `uuid`.** The file path is the identity. `uuid` 1.26.0 is already in `Cargo.lock` via btleplug but *without* `v4` (`v4 = ["rng"]`, `rng = ["dep:getrandom"]`), so document ids would drag `getrandom` in for nothing. niimblue needs an id because it keys browser `localStorage`; a file-based app does not. An imported niimblue `id` is kept as an opaque round-trip string.

### 2.6 Variable data

The placeholder grammar is niimblue's, from `src/utils/canvas_preprocess.ts`:

```
/{\s*(\$?\w+)\s*(?:\|\s*(.*?)\s*)?}/g
```

so `{sku}`, `{ sku }`, `{dt}`, `{dt|YYYY-MM-DD}`, with JavaScript's meaning for every class: `\s` is JavaScript's white space and line terminators, which include U+FEFF and not U+0085; `\w` is ASCII only; and `.` stops at LF, CR, U+2028 and U+2029, so a filter never spans lines. A placeholder ends at the first `}` that can close it. relabel matches the grammar with a hand-written scanner in one pass (`template.rs`), because the regex backtracks quadratically on a run of `{a|`, and tests the scanner against a transliteration of the regex on generated input.

A placeholder resolves in niimblue's order: a column of that name - its cell as it is, any filter ignored, an empty cell blanking the placeholder - then `dt`, the job's clock formatted by a port of dayjs's tokens (`YYYY-MM-DD HH:mm:ss` by default), and otherwise it is **left exactly as written**. A column called `dt` therefore wins over the clock. Substitution applies to text and to barcode, QR and DataMatrix data, and to nothing else. `$times` is the row-repeat column, surfaced in the UI as a Copies column. Matching the grammar is what makes an imported niimblue template keep working.

Three niimblue behaviours are not copied, because each prints a wrong label without a word. Only the table's own column names resolve: niimblue's lookup walks the JavaScript prototype chain, so `{constructor}` printed a function's source. A row must have exactly as many fields as the header: niimblue pads a short row and drops extra fields, and an extra field usually means a comma that should have been quoted. And `$times` is a whole number from 0 to 10 000, or empty for 1: niimblue's `parseInt` printed three labels for `3abc`, sixteen for `0x10` and none for `abc`. niimblue's `\n` cell escape is not applied; the niimblue importer converts it.

`DESIGN.md` §6.3 uses the same single-brace form.

```rust
#[serde(untagged)]
pub enum DataBinding {
    Known(KnownBinding),
    /// A kind a later build added, kept exactly as written.
    Unknown(serde_json::Value),
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnownBinding {
    /// Path and dialect only. relabel-render reads no file and parses no CSV.
    CsvFile(CsvFile),                     // "csv_file"
}

pub struct CsvFile {
    pub path: String,                     // beside the label, as written
    pub delimiter: char,                  // default ','
    pub has_header: bool,                 // default true
    #[serde(flatten)] pub unknown: serde_json::Map<String, serde_json::Value>,
}

/// Rows under a header, as the editor or the command line read them.
pub struct Table { pub columns: Vec<String>, pub rows: Vec<Vec<String>> }

pub struct Bindings<'a> {
    pub columns: &'a [String],
    pub row: &'a [String],
    /// Read once per job, so a preview matches the print. The crate reads no clock.
    pub now: CivilTime,
}

/// Pure. Returns a NEW document with every template resolved, so the editor's
/// document stays immutable and "preview row 37" is one call.
pub fn bind(label: &Label, b: &Bindings<'_>) -> Result<Label, BindError>;

pub enum BindError {
    RowLength { cells: usize, columns: usize },
    AmbiguousColumn(String),              // a name used twice, and asked for
    FieldTooLong { element: ElementId, limit: usize },   // 64 KiB once bound
}
```

Direct field binding needs no second mechanism: a barcode whose entire `data` is `{sku}` *is* a bound field.

**Overflow is where this earns its keep.** Barcodes, QR and DataMatrix fail hard, and that is not negotiable: a clipped Code 128 scans as a different, valid value, and a module shrunk below two dots scans as nothing. Overflow is the *common* case here, not the edge case: twenty characters of Code 128 at the two-dot minimum are 510 dots before the quiet zones and 550 with them, against a whole label of 320.

Text that overflows its frame is clipped and reported with its numbers (`TextClipped`, a warning). Shrink-to-fit, niimblue's `setAndShrinkText`, is deferred:

```rust
pub enum TextFit { Clip, Shrink { min: f32 }, Fail }
```

When it lands, a missing key must mean `Clip`, because that is how every existing label prints, and `min: 8.0` sits below the x-height floor of §6.6.

Which is why `preflight` (§9) exists and is called before a single frame is sent. A 500-label run that fails at label 300 has burned 300 labels of thermal stock.

---

## 3. The ink canvas

Before the pipeline, the one decision everything else hangs off.

### 3.1 `tiny_skia::Mask`, not `tiny_skia::Pixmap`

The drawing surface is an 8-bit coverage buffer:

```rust
/// One byte per dot, row-major, no padding. 0 = bare paper, 255 = full ink.
///
/// This is the OPPOSITE polarity to luminance and to the CLI's existing
/// `bitmap.rs`, which works in greys where 0 is black. Every conversion into
/// this type is `ink = 255 - luma`, and that subtraction is where a bug would
/// live, so it happens in exactly one function (`ink_from_luma`).
pub struct InkMap {
    w: u16,
    h: u16,
    mask: tiny_skia::Mask,   // data() is the coverage bytes, 1 B/px
    zone: Vec<u8>,           // §4.7
}
```

`tiny_skia::Mask::new(width, height) -> Option<Self>` (`mask.rs:48`) allocates `vec![0; w * h]`, so a fresh mask is bare paper. `Mask::data(&self) -> &[u8]` (`:126`) and `Mask::data_mut(&mut self) -> &mut [u8]` (`:131`) give direct access.

`DESIGN.md` §3.2 writes `rasterise(...) -> Pixmap // 8-bit grey`; `tiny_skia::Pixmap` is 32-bit premultiplied RGBA, so that return type was wrong in two ways. This replaces it.

**Why `Mask` wins, in the order that matters:**

1. **Anti-aliasing is a per-call argument, not a struct field.** `Mask::fill_path(&mut self, path: &Path, fill_rule: FillRule, anti_alias: bool, transform: Transform)` (`mask.rs:259`). `Pixmap::fill_path` takes `&Paint`, and `Paint::anti_alias` defaults to **`true`** (`painter.rs:47`, default at `:80`). `DESIGN.md` §5 says anti-aliasing is a per-element decision; on `Mask` that is structural, on `Pixmap` it is a convention a call site can forget.
2. **The `Paint::colorspace` question disappears entirely.** `Mask::fill_path` blits through `RasterPipelineBlitter::new_mask` (`pipeline/blitter.rs:214-242`), which is a plain linear-u8 `lerp(dst, 255, coverage)` with no `Paint` anywhere. `DESIGN.md` §5's open worry about tiny-skia 0.12's new `Paint::colorspace` changing which anti-aliased edge pixels survive a threshold is not a worry we have. (For the record: the field is real, at `painter.rs:56`, `ColorSpace` defaults to `Linear` at `color.rs:455`, and `Linear` is a proven no-op — `expand_stage`/`compress_stage` return `None` for it at `color.rs:522-545`. Anyone who does reach for a `Pixmap` must set it explicitly and must never use `Gamma2` or `SimpleSRGB`, which measurably move a 50 %-covered edge dot from luma 127 to 180+ and can make a 1.4-dot anti-aliased stem vanish at some sub-dot phases.)
3. **A quarter of the memory**, 30 KB rather than 123 KB for a 320 × 96 label. Irrelevant at this size, but it also means the whole label fits comfortably in L1 for the halftone pass.
4. **Coverage is already ink.** No RGBA→luma step, no coefficient choice, no `demultiply()`. `DESIGN.md` §5 names Rec.601 while `tiny_skia::MaskType::Luminance` uses Rec.709 (`mask.rs:64-88`); with a coverage buffer the question only arises for colour image import, where §4.6 settles it.

**The costs, stated plainly.** `Mask` has no `fill_rect` and no `stroke_path`. `Mask::fill_path` silently early-returns with a `log::warn!` on paths whose bounds are near-zero in width or height (`mask.rs:267-271`) — so a one-dot rule expressed as a filled path vanishes. And there is no zero-conversion path to a Slint `Image`.

All three costs are paid by the same decision, which is the actual point:

### 3.2 Hard-edged geometry is written directly, never scan-converted

**Rectangles, rules, barcode bars, QR and DataMatrix modules, and image blits are integer memsets into `mask.data_mut()`. They never touch tiny-skia's scan converter.**

```rust
impl InkMap {
    /// Every hard-edged mark on the label goes through here. Integer dots in,
    /// full ink out, clipped to the canvas. No rasteriser, no rounding, no
    /// transform: rotation is baked into the coordinates before the call.
    pub fn fill_dots(&mut self, x: i32, y: i32, w: u32, h: u32, ink: u8) {
        if w == 0 || h == 0 { return; }
        let (cw, ch) = (i64::from(self.w), i64::from(self.h));
        let x0 = x.max(0) as i64;              let y0 = y.max(0) as i64;
        let x1 = (i64::from(x) + i64::from(w)).min(cw);
        let y1 = (i64::from(y) + i64::from(h)).min(ch);
        if x0 >= x1 || y0 >= y1 { return; }
        let stride = self.w as usize;
        let data = self.mask.data_mut();
        for row in y0..y1 {
            let base = row as usize * stride;
            data[base + x0 as usize .. base + x1 as usize].fill(ink);
        }
    }
}
```

That one function removes four separate hazards that a researcher measured on real code, and it is worth naming each, because each is a live bug on a 96-dot head:

- **`Rect::round()` is a floor, not a round.** `rect.rs:313` documents "adding 0.5 and discarding the fractional portion"; `floating_point.rs:81` is `saturate_from(x.floor() + 0.5)` — floor *first*, then a truncating cast that throws the 0.5 away. Skia's original adds before flooring. Measured: sweeping a rect's left edge from 10.0 to 10.875 in eighths, `fill_rect` with AA off started at dot 10 every single time. A systematic half-dot leftward bias, on an axis where one dot is 1 % of the label.
- **`fill_rect` and `fill_path` disagree at fractional phases.** For `l = 10.5`, `fill_rect` inks columns 10 and 11; `fill_path` inks 11 and 12.
- **`fill_rect` only takes its fast path under an identity transform** (`painter.rs:194`); with any transform it degrades to `fill_path` and inherits the degenerate-bounds drop. A zero-height rule painted under `translate(0, 0.5)` paints nothing at all.
- **tiny-skia 0.11.4 panics on exactly our shape.** `Pixmap::new(100, 10)` plus `fill_rect(Rect::from_ltrb(88.1, 0.0, 89.6, 10.0))` with `anti_alias: true` panics at `scan/hairline_aa.rs:124` in a debug build and silently takes the wrong branch in release. That is a 1.5-dot anti-aliased bar near the edge of a small pixmap — a barcode bar on a 96-dot label — and `cargo test` is a debug build. **0.12 is a correctness floor, not a preference**, and the direct-write path never enters that code at all.

**What tiny-skia is actually used for, therefore, is one thing: filling glyph outline paths.** Plus, later, any genuinely vector artwork that arrives rotated. That is a small enough surface to reason about, and it is a surface where a real polygon rasteriser with correct winding and curve flattening is worth having and not worth writing.

**One contained exception.** `resvg::render(tree: &usvg::Tree, transform: tiny_skia::Transform, pixmap: &mut tiny_skia::PixmapMut)` (`resvg-0.47.0/src/lib.rs:34-38`) demands a `Pixmap`. SVG import therefore allocates a scratch `Pixmap` at the element's dot size, renders into it, and converts to ink through `ink_from_luma`. That is the only place a `Pixmap` exists in this crate, and it is worth it: SVG is the only import format that rasterises *at* the destination resolution with no resampling at all.

---

## 4. The rendering pipeline

```
Label (document)
  │  bind()                       templates → resolved strings, one row → one page ($times copies)
  ▼
Label (bound)
  │  layout()                     parley 0.10 → line boxes, glyph runs, measured extents
  │  encode()                     code128 / barcoders / qrcode / datamatrix → module runs
  ▼
draw list                         every mark reduced to dots or to a glyph outline
  │
  ├─ hard marks ──────────────▶ InkMap::fill_dots            direct memset, no AA, exact
  ├─ glyph outlines ──────────▶ skrifa → tiny_skia::PathBuilder → Mask::fill_path
  ├─ raster images ───────────▶ linear-light resize → ink_from_luma → row copy
  └─ SVG ─────────────────────▶ scratch Pixmap → resvg::render → ink_from_luma
  ▼
InkMap { mask: 8-bit coverage, zone: per-dot element id }
  │  binarise()                   per-zone halftone; diffusion never crosses a zone edge
  ▼
BitPlane                          MSB-first, set bit = black, rows padded right
  │  relabel-proto::encode_page    applies the model's rotation
  ▼
Vec<Frame>
```

### 4.1 Where anti-aliasing is on, and where it is off

| Mark | AA | Mechanism |
|---|---|---|
| Rectangle fill and stroke | **off** | `fill_dots`, four calls for a stroked rect |
| Horizontal / vertical rule | **off** | `fill_dots`, always as a rect — never a path (§3.1) |
| Diagonal line | **off** | integer Bresenham with a width, written through `fill_dots` per span |
| Barcode bar | **off** | `fill_dots`, x = `module_dots × module_index` exactly |
| QR / DataMatrix module | **off** | `fill_dots`, one call per module |
| Glyph, mono-hinted | **off** | `Mask::fill_path(.., anti_alias = false, ..)` |
| Glyph, smooth | **on** | `Mask::fill_path(.., anti_alias = true, ..)` |
| Raster image | n/a | resampling filter, then halftone; never scan-converted |
| SVG | **on** | resvg's own, into the scratch pixmap |

The rule behind the table: **anything whose meaning is carried by its edge position gets no anti-aliasing, because a half-lit dot is a coin flip at binarisation time and a scan failure at the till.** Anything whose meaning is carried by its shape may have anti-aliasing, because a shape survives a threshold.

For the glyph case there is a further trap worth knowing even though the ink canvas avoids it: `fill_path`'s AA is 4× supersampled (`scan/path_aa.rs:21`, `SUPERSAMPLE_SHIFT = 2`; `:285` shifts coverage by `8 - 2*SHIFT`), giving 17 coverage levels in steps of 16. A threshold of exactly 128 lands on a step boundary, so an edge at exactly half coverage is decided by a tie-break. **Prefer 120 or 136, or fix `<` versus `<=` deliberately and test it.** The default is 128 with `<` and it is a placeholder until the print test in §9 replaces it.

### 4.2 Text: layout

parley 0.10.0 with fontique's system-font backend **off**:

```rust
use parley::{FontContext, LayoutContext, Layout, StyleProperty, Alignment, AlignmentOptions};
use parley::fontique::{Collection, CollectionOptions, Blob, GenericFamily, SourceCache};

pub struct TextEngine {
    fcx: FontContext,
    lcx: LayoutContext<()>,
}

impl TextEngine {
    pub fn new(embedded: &[&'static [u8]]) -> Result<TextEngine, RenderError> {
        // system_fonts: false is deterministic output AND no libfontconfig at
        // all from this crate's own use. Note that once relabel-gui adds slint,
        // feature unification turns fontique/system back on graph-wide — so
        // relabel-render must still BUILD and TEST standalone with it off, and
        // must never DEPEND on a system font being present.
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });

        let mut first = None;
        for bytes in embedded {
            let blob = Blob::new(std::sync::Arc::new(*bytes));
            // MUST be checked. register_fonts returns Vec<(FamilyId,
            // Vec<FontInfo>)> (fontique-0.10.0/src/collection/mod.rs:202-206)
            // and an empty Vec is how a rejected face reports itself.
            let registered = collection.register_fonts(blob, None);
            let (family, _) = registered.into_iter().next()
                .ok_or(RenderError::FontRejected)?;
            first.get_or_insert(family);
        }
        let sans = first.ok_or(RenderError::NoFonts)?;

        // Without this the failure is SILENT and total. With system fonts off,
        // GenericFamilyMap::default() is empty, so the default
        // FontFamily::Source("sans-serif") resolves to nothing,
        // FontSelector::select_font returns None, and parley's
        // `while let Some(font) = current_font.take()` loop (shape/mod.rs:335)
        // never runs: an empty layout, no error, not even tofu.
        collection.set_generic_families(GenericFamily::SansSerif, core::iter::once(sans));

        Ok(TextEngine {
            fcx: FontContext { collection, source_cache: SourceCache::default() },
            lcx: LayoutContext::new(),
        })
    }
}
```

Laying out one text element:

```rust
// scale = 1.0, so one layout unit is one printer dot. quantize = true.
// Signature verified at parley-0.10.0/src/context.rs:87-93:
//   pub fn ranged_builder<'a>(&'a mut self, fcx: &'a mut FontContext,
//                             text: &'a str, scale: f32, quantize: bool)
//       -> RangedBuilder<'a, B>
let mut b = self.lcx.ranged_builder(&mut self.fcx, text, 1.0, true);
b.push_default(StyleProperty::FontSize(ppem));                  // integer, on purpose
b.push_default(StyleProperty::LineHeight(LineHeight::Absolute(line_dots)));
let mut layout: Layout<()> = b.build(text);
layout.break_all_lines(Some(frame_width_dots as f32));          // layout.rs:161
layout.align(Alignment::Start, AlignmentOptions::default());    // layout.rs:177
```

**Why parley and not the alternatives.** It is the version Slint 1.17.1 compiles (`i-slint-core-1.17.1/Cargo.toml:233-236` requires `0.10.0` with `complex-scripts`), so it unifies to one copy and costs nothing. It re-exports fontique (`pub use fontique;`), so there is no separate dependency line for the font collection. It brings real shaping through harfrust, Unicode line breaking through `icu_segmenter`, bidi, and per-cluster font fallback.

`cosmic-text` 0.19.0 pins `harfrust 0.5.0` and `skrifa 0.40.0` against Slint's 0.8.4 and 0.42.1 — two duplicate stacks plus six more crates — and offers no hinting control. `ab_glyph` is **Apache-2.0 only** (not the usual dual) and has zero hits for "hint" in its sources; it also does no shaping at all, which is the ceiling `thermark`, the closest prior art, actually hit: `font.glyph_id(ch)` per char and `caret += h_advance(gid)`, so no kerning, no marks, no fallback, no bidi. `fontdue` is the same shape.

**What building it showed.** `parley = { version = "0.10", default-features = false, features = ["std"] }` is the whole dependency; fontique is reached as `parley::fontique`. `complex-scripts` stays off: it swaps in ICU's dictionary and LSTM segmenters for scripts the embedded faces cannot draw, and adds 3.8 MB to a release binary for byte-identical Latin output. The engine above needs three more things than it shows: a `Monospace` generic, a Latin fallback (`append_fallbacks`), and a query proving each generic resolves before any layout runs. Parley reports nothing itself: no fonts, fonts with no generic mapped, and characters no face has all lay out silently - as nothing, or as the missing-glyph box - so relabel checks registration, resolves both generics, and names each character the style's faces lack - a symbol only the mono face has is named when set in sans - before laying out. `FontSize(inf)` hangs line breaking, and so does a finite size whose product with the line height overflows `f32`; `break_all_lines(Some(NaN))` trips an assertion in release builds. So sizes, line heights and widths are validated first, and bounded: at most 4096 dots to the em, a line height of at most 100, and 10,000 characters. `\r\n` lays out as two breaks and a tab as the missing-glyph box, so both are normalised, along with the other line separators: VT, FF and NEL are control characters with no glyph, and U+2028 and U+2029 become `\n` too so that every separator behaves alike. Any other control character is named and refused. The default-ignorable characters a keyboard inserts, such as the variation selector after ©, are left to the shaper, which hides them. It adds 37 crates to `Cargo.lock` and brings three more already locked for tests into the shipped graph - `foldhash`, `hashbrown` and `syn` - all MIT, Apache-2.0 or Unicode-3.0 except `foldhash` (Zlib), and raises relabel-render's floor to Rust 1.88. Building a `FontContext` per render costs microseconds, so there is no global state.

### 4.3 Text: outlines, hinting, and the ppem problem

**This is the single hardest thing the crate does, and the reason is arithmetic.**

Measured on the shipped files: Noto Sans Regular's `l` stem is 88/1000 em, x-height 536, cap height 714. At the sizes a 12 mm label forces:

| ppem | stem (dots) | x-height (dots) | cap (dots) |
|---|---|---|---|
| 14 | 1.23 | 7.5 | 10.0 |
| 16 | 1.41 | 8.6 | 11.4 |
| 18 | 1.58 | 9.7 | 12.9 |
| 20 | 1.76 | 10.7 | 14.3 |

Noto's natural line height is 1.362 em, so **four lines in 96 dots is 17.6 ppem**, five lines is 14.1, six is 11.75. Every stem in that range is between 1.2 and 1.8 dots. Unhinted, the two stems of an `n` round differently and one comes out one dot wide while the other comes out two. **That asymmetry, not softness, is what makes small unhinted text look broken.** Grid-fitting is the fix, and `Target::Mono` with anti-aliasing off is the pairing FreeType's mono mode has used for thirty years.

`swash` cannot do it. `swash-0.2.10/src/scale/hinting_cache.rs:27-30` hard-codes

```rust
const HINTING_MODE: HintingMode = HintingMode::Smooth {
    lcd_subpixel: Some(LcdLayout::Horizontal),
    preserve_linear_metrics: true,
};
```

and `ScalerBuilder::hint(bool)` can only turn *that* instance on or off. Not merely "smooth instead of mono" — smooth tuned for horizontal RGB subpixels, which is the wrong optimisation target for a 1-bit thermal head by any measure. Worse, it fails open: `hinting_cache.rs:23` is `HintingInstance::new(..).ok()` and `mod.rs:453-462` falls through to `None`, so unhinted output is indistinguishable from hinted at the call site. Slint's own bridge never hints at all (`i-slint-renderer-software-1.17.1/fonts/vectorfont.rs:170-180` uses `Render::new(&[Source::Outline]).format(Format::Alpha)`).

So relabel-render goes to skrifa directly. Signatures verified in `skrifa-0.42.1`:

```rust
// src/outline/hint.rs:290-295
pub fn HintingInstance::new<'a>(
    outline_glyphs: &OutlineGlyphCollection,
    size: Size,
    location: impl Into<LocationRef<'a>>,
    options: impl Into<HintingOptions>,
) -> Result<Self, DrawError>;

// src/outline/hint.rs:15-24
pub struct HintingOptions { pub engine: Engine, pub target: Target }
// :38-59  pub enum Engine { Interpreter, Auto(Option<GlyphStyles>), #[default] AutoFallback }
// :92-96  pub enum Target { Mono, Smooth { .. } }   // Mono == FT_LOAD_TARGET_MONO

// src/outline/mod.rs:199, :346-350
pub fn DrawSettings::hinted(instance: &'a HintingInstance, is_pedantic: bool) -> Self;
pub fn OutlineGlyph::draw<'s>(&self, settings: impl Into<DrawSettings<'a>>,
                              pen: &mut impl OutlinePen)
    -> Result<AdjustedMetrics, DrawError>;

// read-fonts-0.39.2/src/model/pen.rs:8-25, re-exported at skrifa outline/mod.rs:105
pub trait OutlinePen {
    fn move_to(&mut self, x: f32, y: f32);
    fn line_to(&mut self, x: f32, y: f32);
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32);
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32);
    fn close(&mut self);
}
```

The pen is twenty lines. Note `curve_to` maps to tiny-skia's `cubic_to`, and that skrifa emits font-space y-up while parley's baseline y grows downward, so y is flipped:

```rust
struct MaskPen { b: tiny_skia::PathBuilder, ox: f32, oy: f32 }

impl skrifa::outline::OutlinePen for MaskPen {
    fn move_to(&mut self, x: f32, y: f32) { self.b.move_to(self.ox + x, self.oy - y) }
    fn line_to(&mut self, x: f32, y: f32) { self.b.line_to(self.ox + x, self.oy - y) }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.b.quad_to(self.ox + cx, self.oy - cy, self.ox + x, self.oy - y)
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.b.cubic_to(self.ox + c0x, self.oy - c0y,
                        self.ox + c1x, self.oy - c1y,
                        self.ox +   x, self.oy -   y)
    }
    fn close(&mut self) { self.b.close() }
}
```

`PathBuilder::finish()` returns `Option<Path>` and yields `None` for an empty glyph such as a space — check it, do not unwrap.

The run loop, with the two things that are easy to get wrong:

```rust
for line in layout.lines() {
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(run) = item else { continue };
        let r = run.run();
        let font = r.font();
        let fref = skrifa::FontRef::from_index(font.data.data(), font.index)?;
        let coords: Vec<NormalizedCoord> =
            r.normalized_coords().iter().map(|&c| NormalizedCoord::from_bits(c)).collect();
        let outlines = fref.outline_glyphs();

        // Engine::Auto(None) is FORCED, not AutoFallback. skrifa computes
        //   prefer_interpreter = !(max_instructions == 0 && fpgm.is_empty()
        //                          && prep.is_empty())
        // (outline/glyf/mod.rs:114). Google's variable Noto builds carry a
        // SEVEN-BYTE stub `prep` and no `fpgm`, so AutoFallback selects the
        // TrueType interpreter — which then does nothing — instead of the
        // autohinter that would have helped. Static ttfautohint faces carry a
        // real fpgm/prep/cvt and would be fine either way; forcing Auto makes
        // the outcome independent of which build the user drops in.
        let hint = HintingInstance::new(
            &outlines, Size::new(r.font_size()), LocationRef::new(&coords),
            HintingOptions { engine: Engine::Auto(None), target: Target::Mono })?;

        for g in run.positioned_glyphs() {          // Glyph { id, style_index, x, y, advance }
            let Some(og) = outlines.get(GlyphId::new(g.id)) else { continue };
            // parley does NOT round x. Snapping here keeps every stem on the
            // dot grid at the cost of up to half a dot of inter-letter spacing
            // per glyph — see §9, this is the open trade.
            let mut pen = MaskPen::new(g.x.round(), g.y.round());
            og.draw(DrawSettings::hinted(&hint, false), &mut pen)?;
            if let Some(path) = pen.finish() {
                mask.fill_path(&path, FillRule::Winding, /* anti_alias */ false,
                               Transform::identity());
            }
        }
    }
}
```

Keep `PathStyle::FreeType`, which is the default: `(DrawInstance::Hinted { .. }, PathStyle::HarfBuzz)` returns `Err(DrawError::HarfBuzzHintingUnsupported)` (`outline/mod.rs:392`).

**The default raster mode is chosen by ppem.** `GlyphRaster::MonoHinted` with `anti_alias = false` at or below 24 ppem; `Antialiased` above. The cutover is per element and overridable, and the honest position is that **24 is a guess** — §9 is the print test that replaces it with a measurement. The reason mono-hinted is the *default* rather than the option is that the failure mode of anti-aliasing at 1.2–1.8-dot stems is phase-dependent stem weight, which is exactly the ragged look small text must not have; the failure mode of mono hinting is a slightly stiffer letterform, which is what every thermal label printer in the world has looked like for thirty years.

The two implementations that exist disagree wildly about the threshold that goes with anti-aliased text, which is itself evidence that nobody has measured it: niimbluelib treats any pixel not exactly `0xffffff` as ink (`image_encoder.ts:120`), a threshold near 1/255 that fattens every glyph by about a dot on each edge; `thermark` drops coverage below 0.3 (`font.rs:257`), i.e. threshold 77. Neither is 128.

**What measurement changed** (the three embedded static faces, 12 to 24 ppem). `Engine::AutoFallback` does not pick the autohinter for these faces: they carry `fpgm` and `prep`, so it resolves to the TrueType interpreter - and their ttfautohint bytecode moves no x coordinate at all, so the interpreter leaves stems exactly as unhinted ones. Forcing `Engine::Auto(None)` is right for these faces too, but because ttfautohint here hints only y, not because of stub tables. Under `Auto` with `Target::Mono`, one probe of 108 came out asymmetric - a horizontal stroke of the mono zero - against 28 to 38 for every other condition, and every sub-dot origin phase gave symmetric stems. It stayed clean through 24 ppem, so there is no cutover to anti-aliasing: `hinted` is the default at every size, and `smooth` (unhinted, anti-aliased, thresholded) is the P6 alternative. Glyphs are placed on parley's advances with origins rounded; the autohinter's adjusted advances differ by five to ten percent of a line and would contradict the line breaks. `is_pedantic` has no effect under `Engine::Auto`, which runs none of the fonts' TrueType programs; it stays false for the day the engine is the interpreter, where these fonts' programs read one control value out of range and pedantic mode reports an error on hundreds of non-Latin glyphs.

**Where ink meets the frame.** Parley aligns by advance, but hinted ink can reach more than a dot past a glyph's advance - an `f`, a `T` - or start before its origin - a `j`. Each line is therefore moved by whole dots to bring all of its ink inside the frame, whenever such a shift exists; a line too wide for one is left alone. The local raster is grown to take in every control point of the glyph path before the frame is cut out of it. tiny-skia runs its clipper whenever a control point leaves the mask, and the clipper moved dots up to four rows inside the edge; on the grown raster, clipped text is the same dots as unclipped. A local raster is at most 8191 dots on a side, a metre at 203 dpi. Past that, tiny-skia fills a mask in tiles, clipping at every seam, and its anti-aliased filler drops to hard edges.

### 4.4 Fonts

Embedded, always. A label must rasterise identically on every machine, and with fontconfig `dlopen`'d there may be no system fonts to enumerate on Linux anyway.

**Condensed, not monospace.** Measured advance sums for `"Shelf B4 - Widget 12mm x 40mm"` on a 320-dot line at 18 ppem: Noto Sans Regular 276.1 dots (≈34 characters fit), **Noto Sans Condensed 226.4 dots (≈41 characters, +21 %)**, Noto Sans **Mono** Condensed 276.7 dots — *identical to the proportional Regular*. Monospace buys column alignment and nothing else. Condensed buys a fifth more text on every line, which on a 320-dot label is the difference between a part number fitting and not.

Ship three, all OFL 1.1, all with real ttfautohint tables (`fpgm` 3596 / `prep` 261 / `cvt ` 300 verified on the static faces):

- `NotoSans-Condensed.ttf` — the default.
- `NotoSans-CondensedSemiBold.ttf` — beats `Synthesis::embolden` faux-bold when stems are 1.5 dots.
- `NotoSansMono-Condensed.ttf` — only for the column-alignment case.

Unsubset they are 617 KB / 620 KB / 597 KB, about 1.8 MB. **Subset to Latin-1 + Latin Extended-A + punctuation at build time and commit the subsets** with provenance in `THIRD-PARTY.md`, which gets each under about 150 KB; committing the subsets rather than adding a Python build step keeps the Flatpak and CI builds offline. Before doing so, read the OFL.txt actually shipped with the bundled version and record whether it carries a Reserved Font Name — Google Fonts' Noto normally does not, but a subset is a derivative and an RFN would force a rename.

**CJK is not embeddable**: `NotoSansCJK-VF.ttc` is 32.7 MB. Offer user font import through `Collection::register_fonts`, and offer opt-in system fonts in the GUI only.

**As shipped: the full faces, not subsets.** The three files are the unmodified upstream hinted builds, 1.83 MB together. Unmodified, they raise no Reserved Font Name question, and they keep Greek (μ included), Cyrillic, and the euro, ohm and minus signs that a Latin subset would drop - characters labels use. Subsetting was tried and is recorded for the day size matters: `skera` 0.7 (fontations) subsets without changing a glyph or a kerning pair, but a subset must also keep the superscript features and ranges or the autohinter's Latin superscript style changes five glyphs; the `fontcull-klippa` fork changed 90 to 134 kerning pairs per Latin subset and should not be used. Provenance and checksums are in `THIRD-PARTY.md`.

**A correction to `DESIGN.md` §4.** That table declares `yeslogic-fontconfig-sys = { version = "6", features = ["dlopen"] }` as a direct target-conditional dependency, with the note that "the top-level `slint` crate re-exports no feature that does this, so a direct target-conditional dependency is the only manifest-level route." That is not the only route. `fontique-0.10.0/Cargo.toml` declares

```
fontconfig-dlopen = ["yeslogic-fontconfig-sys?/dlopen"]
```

so `fontique = { version = "0.10", default-features = false, features = ["fontconfig-dlopen"] }` reaches the same unification without a target gate and without naming a C library in relabel's manifest. The `?` means it only fires if something else in the graph enabled the optional dependency, which is exactly the condition we care about. Keep `RUST_FONTCONFIG_DLOPEN=1` in CI as belt and braces either way. The mechanism the note describes is real — `yeslogic-fontconfig-sys-6.0.1/build.rs:1-10` does `pkg_config::find_library("fontconfig").unwrap()` unless `dlopen` or that env var is set, an unconditional panic on a machine without `fontconfig-devel` — and so is the graceful degradation: with dlopen and no library present, `fontique-0.10.0/src/backend/fontconfig.rs:604-611` checks `LIB_RESULT.is_err()` and returns an empty default. Also for `THIRD-PARTY.md`: `fontdb`'s `fontconfig` feature is `fontconfig-parser`, pure Rust — it is *not* a C dependency. libfontconfig comes only from fontique.

### 4.5 Rotation

**Rotation is applied to coordinates in relabel-render, never handed to tiny-skia as a matrix, and never to `fill_dots`.**

For `D0`/`D90`/`D180`/`D270` an element's local dot coordinates map onto canvas dot coordinates by a swap and two negations. That keeps every hard mark exactly on the grid and keeps `fill_dots` on its clip-and-memset fast path. For rotated *text*, the layout runs in the element's local frame and the glyph origin plus the pen's emitted coordinates are transformed on the way into the `PathBuilder` — the path is built pre-rotated, so `Mask::fill_path` still gets `Transform::identity()`.

**As built for text, rotation is an index remap, not a path transform.** The text is laid out and filled upright in its own frame, and the finished coverage is turned onto the canvas by moving bytes - the same quarter-turn arithmetic as a 2D symbol's module grid - so a turned label is the upright one on its side, dot for dot, grey levels included. On a `Top` target the canvas is still the label in reading orientation, so the same turn places it; the transpose that target's axis swap would otherwise need cancels against the swap itself.

There is no arbitrary-angle rotation, for any element. It would mean fractional geometry everywhere, which a 1-bit device renders ugly, and it would put every rect back through the scan converter. `Rot` is serialised as a number so the model can gain it later without a schema break.

### 4.6 Images

Decode:

```rust
// NOT image::open — that is ImageReader::open(path)?.decode()
// (images/dynimage.rs:1632-1636), which picks the format from the file
// EXTENSION only, so a mis-named file fails.
let mut r = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
// Limits::default() caps allocation at 512 MiB with NO dimension cap
// (io/limits.rs:49-56). Note `limits` returns (), so it does not chain.
let mut lim = Limits::default();
lim.max_image_width = Some(20_000);
lim.max_image_height = Some(20_000);
r.limits(lim);
let img = r.decode()?;
```

Composite over white **before** converting to ink:

```rust
// to_luma8() DISCARDS alpha. Verified: Rgba([0,0,0,0]) yields luma 0, so every
// logo PNG with a knocked-out background becomes a solid black rectangle.
// (The cast path is dynimage.rs:331 -> buffer.rs:1673 -> cicp.rs:983 ->
// cicp.rs:1059, whose (4,3) arm at :1123-1133 drops rgba[3].)
// to_luma_alpha8 (dynimage.rs:355) does carry alpha through.
let la = img.to_luma_alpha8();
let ink = |l: u8, a: u8| -> u8 {
    // Over white paper, then invert to ink. Straight (non-premultiplied)
    // alpha, which PNG guarantees; the WebP/AVIF/TIFF decoders in this tree
    // were NOT audited for that and should be before those formats are offered.
    let v = (u32::from(l) * u32::from(a) + 255 * (255 - u32::from(a)) + 127) / 255;
    255 - v as u8
};
```

Resize:

```rust
// resize() documents "This method typically assumes that the input is
// scene-linear light" (sample.rs:962-963) and does NO conversion. Linearise,
// resize, re-encode: measured RMS against an exact linear-light box average
// for a 5x downscale is 11.0 gamma-encoded vs 1.7 linear-light. That is a six-
// fold win, an order of magnitude more than any filter choice buys.
let lin = to_linear_f32(&la);
let small = imageops::resize(&lin, dst_w, dst_h, FilterType::Triangle);
let ink_rows = from_linear_to_ink(&small);
```

**Triangle, not Lanczos3.** At a 5× downscale the non-Nearest filters are within noise of each other (RMS 10.9–11.1) because `horizontal_sample` widens the support by the downscale ratio (`sample.rs:259-261`, and again at `:511-513` for the vertical pass), so every kernel converges to an area average. Lanczos3 costs 28.3 ms against Triangle's 9.9 ms on a 3000 × 2000 source, for nothing. Nearest is the only genuinely bad choice (RMS 43.5) and is what `ImageFit::Dots` uses — deliberately — when the artwork already fits in whole dots, because any resampling destroys pixel-art detail that cannot be recovered at 96 dots.

**As built.** Import (`artwork::import`, `artwork::import_within`) is the only place a file is decoded. Drawing unpacks the element's own PNG through one PNG decoder under explicit limits, so a shared label reaches nothing else, and never an SVG parser.

- **Limits.** `into_decoder()`, which reading the EXIF orientation needs, skips the allocation reservation `decode()` makes, and a header alone then made the decoder allocate gigabytes and abort the process. So import checks the sides (16 384), then the pixels (40 million, on the header, because WebP's own buffers ignore `max_alloc`), then reserves the raster itself - and, for a progressive JPEG, the coefficient store its decoder allocates before reading a scan and never counts. A test with a counting allocator holds each hostile header - PNG, BMP, GIF, baseline and progressive JPEG - under 1 MiB. `MAX_ALLOC` bounds those decoder buffers and, separately, the resampling buffer: `image`'s resize runs its vertical pass into a four-channel float buffer as wide as the source and as tall as the result, 2 GiB for a wide source stretched into a tall frame, so import runs whichever order is cheaper, on the transpose when that is the one. Import also holds one float per source pixel while it converts, and refuses a frame drawing would refuse.
- **Tone.** Integer Rec.601 luma from straight RGBA, composited over white in linear light through constant tables rather than `powf`, whose result depends on the platform's maths library.
- **Resampling.** The claim above that every kernel converges to an area average is wrong: at 5× Triangle is a tent twice the ratio wide. The test compares import against that same tent computed in double precision - RMS under 0.5, against over 10 for a gamma-space resize.
- **Threshold.** Line art that was resampled, or has soft alpha edges, gets threshold 187, the largest code below half light in linear terms, so a dot prints when more than half of it is ink. At 128 a stroke one dot wide after a 5× downscale vanishes entirely, because it covers at most three quarters of any dot.
- **SVG.** Rendered by resvg 0.47 at import only, because it aborts on an uncapped filter region or pattern tile. No `text`, no `raster-images`, crisp edges by default; `<text>`, `<image>`, `<foreignObject>`, `<pattern>`, `<filter>`, `<marker>` and CSS filter functions are refused. Groups needing a layer of their own are capped at 64, eight deep, and clip paths and masks at 64 before usvg sees them: usvg follows a chain of them by recursion, and a long chain overflowed the stack and aborted the process. A marker is copied once per vertex of every path it decorates, which with visible overflow escapes the group cap entirely.
- **Drawing.** The stored artwork is placed as text is: upright, turned by index, and blended, so white leaves what lies beneath alone. The element's zone is its whole box - so its own threshold and halftone reach its white too - less the dots another element already holds where the artwork is white or within two levels of it, so a faint JPEG or scanned surround does not re-threshold the text beneath. A frame changed since import is resampled nearest-neighbour, in integers, and reported.

The existing CLI code at `crates/relabel-cli/src/bitmap.rs` scales by print direction (`Top` constrains width, `Left` constrains height). **That logic does not move.** The canvas is authored in reading orientation and `BitPlane::oriented()` handles the head; duplicating the direction policy in two crates is how they drift apart.

### 4.7 Binarisation, and the zone plane

```rust
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Halftone {
    #[default] Threshold,
    FloydSteinberg,
    Atkinson,
    Sierra3,
    Bayer8,
}

#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub halftone: Halftone,
    pub threshold: u8,
    /// 0.0..=1.0 scale on the diffused error. niimblue exposes this and it is
    /// the cheapest way to soften a texture that is too crunchy at 96 dots
    /// without changing kernel.
    pub strength: f32,
    pub serpentine: bool,
    pub invert: bool,
}

pub struct BinariseOpts<'a> {
    /// Applied to every dot before quantising. Dot gain, paper sensitivity and
    /// the printer's density knob all collapse into this one curve.
    /// IDENTITY by default — see below.
    pub tone: &'a [u8; 256],
    /// Zone 0 is the page default; 1..=254 are per-element overrides, painted
    /// as each element's BOUNDING BOX in z-order.
    pub zones: &'a [Zone],
}

pub fn binarise(ink: &InkMap, o: &BinariseOpts<'_>) -> BitPlane;
```

**The zone plane is the structural answer to a correctness problem, not an ergonomic nicety.** `DESIGN.md` §5 dithers the whole page. Floyd–Steinberg and Atkinson diffuse error across element boundaries, so a photograph elsewhere on the label pushes error into a barcode's bars and into its quiet zone regardless of how carefully the bars themselves were drawn with anti-aliasing off. The fix is one line inside the diffusion loop:

```rust
// Error never leaves the zone it was generated in. This is what stops a
// photograph's dither landing inside a barcode's quiet zone.
if zone[dst] != zone[src] { return; }
```

Bounding boxes rather than coverage, because a bounding box is explicable in the inspector ("this element's box is thresholded") and a coverage mask is not.

**As built:** zones are the dots an element inks, not its box - a border drawn last would otherwise claim the whole label (`Canvas::paint_zone`). An image is the exception: it claims its box, less what another element holds where the image is near white (§4.6). `zone_table(label, page)` gives `binarise` each image's own halftone and threshold, and `rasterise_plane` applies it.

**The halftone set is five, and the omissions are deliberate.** Measured on a 320 × 96 zone plate, local fidelity as 3×3-blurred RMS against the source: Floyd–Steinberg 16.3, Burkes 18.9, Stucki 19.8, Sierra-3 20.9, Jarvis 20.9, Atkinson 21.4, Bayer-8 22.4, plain threshold 31.5. Jarvis, Stucki and Burkes are tonally *identical* to Floyd–Steinberg — every error-conserving kernel has the same tone reproduction, because tone is a property of the dot, not the kernel — and worse on fidelity, and their ±2-column footprint smears error across 4 % of a 96-dot axis. They add a dropdown entry and nothing else.

Atkinson stays for a reason that is not tone. Share of fired dots with no orthogonal neighbour, flat field at level 128: Bayer-8 100 %, Floyd–Steinberg 90.9 %, Burkes 68.4 %, Sierra-3 27.0 %, Jarvis 23.3 %, **Atkinson 0.0 %**. A lone dot is precisely the mark that direct-thermal dot gain distorts most, because it has no neighbour to overlap and all its spread lands on bare paper. Floyd–Steinberg's mid-grey checkerboard is a field of lone dots at Nyquist. Whether that reads as mud on a real D110_M is §9's print test, but the mechanism is real and measurable.

Serpentine scanning is **on by default** for every error-diffusion mode: FS 16.7 → 16.3, Atkinson 22.8 → 21.4, Jarvis 22.2 → 20.9. Small, consistent, free.

**The tone curve defaults to identity, and that is a considered position against the received wisdom.** Simulating the printed page — dither a flat field, paint each fired cell as a disc of *d* pitches, union the discs, Murray–Davies with paper R = 0.88 and thermal black R = 0.08, Yule–Nielsen n = 1.8 — gives mean absolute error in code values across levels 32…224 for Floyd–Steinberg:

| pipeline | gain 1.00 | 1.15 | 1.30 |
|---|---|---|---|
| dither the code value as-is | 52.8 | 29.9 | **18.9** |
| + gamma 1.4 pre-lighten | 67.1 | 45.2 | 28.9 |
| + gamma 1.8 pre-lighten | 76.9 | 57.3 | 42.2 |
| dither in linear light | **22.2** | 27.4 | 36.9 |

Naive code-value dithering is far too light, and **dot gain is what rescues it**. Adding a lightening gamma — the received wisdom, and what the one thermal-printing writeup found in the wild does with `gammaImage(1.8)` — makes it monotonically worse at every gain level in this model. A D110_M has already printed correctly with no curve at all. Guessing a gamma would regress a working path. So: `tone` is a 256-entry LUT, it is identity until a printed step wedge produces a real one, and §9 says how that wedge is made.

**Do not use `image::imageops::colorops::dither`**, which `DESIGN.md` §4's dependency table proposes. It panics on any image narrower than 2 px: `colorops.rs:501` calls `image.get_pixel_mut(x + 1, y)` with `x = 0` before the inner loop, and `get_pixel_mut` panics out of bounds (`images/buffer.rs:951-960`). Verified: `1x8` panics, `1x1` panics, `0x8` panics; `8x1` is fine. A one-dot-wide element on a 96-dot head is ordinary. It is also non-serpentine, fixed Floyd–Steinberg, and has no threshold parameter. The kernel loop is eighty lines and the one already in `crates/relabel-cli/src/bitmap.rs:69-115` is correct at width 1 — its bounds-checked `push` closure is exactly right. **That code moves here**, with three changes: composite RGBA over white first, drop the direction-dependent scaling, and take `&InkMap` instead of `image::GrayImage`. The `clap::ValueEnum` derive on `Dither` stays behind in the CLI as a mirror enum, because a library crate does not depend on clap.

---

## 5. Symbologies

### 5.1 What ships

| Symbology | Encoder | Notes |
|---|---|---|
| Code 128 (auto A/B/C) | `code128` 0.2.1 | The default and the right answer for 90 % of label content |
| Code 39 | `barcoders` 2.0 | 2:1 only |
| ITF / Interleaved 2 of 5 | `barcoders` 2.0 | `TF::interleaved` |
| Codabar | **relabel's own**, from the standard's table | `barcoders` gets B, C and `+` wrong, see §5.3 |
| EAN-13 / EAN-8 | `barcoders` 2.0 | refused on small media, see §6.3 |
| UPC-A | `barcoders` 2.0 | through **relabel's own** wrapper, see below |
| QR (Normal 1–40) | `qrcode` 0.14.1 | Micro QR **not offered**, see §8 |
| DataMatrix (incl. DMRE) | `datamatrix` 0.3.3 | the best symbology for this label shape |

**Not shipping: Code 93, UPC-E, PDF417, Aztec, GS1-128.** Reasons in §8.

### 5.2 Code 128 gets its own crate, and why

`barcoders` does **no** subset selection. The caller must embed `À` (U+00C0), `Ɓ` (U+0181) or `Ć` (U+0106) as the first character to select START-A/B/C, and again mid-string for each latch, plus `Ź ź Ż ż Ž` for FNC1-4 and SHIFT (`code128.rs:13-53`, parser at `:287-329`). `Code128::new("HELLO")` is `Err(Error::Character)`. And getting the switching wrong is expensive: `"AB-12345678"` costs 156 modules in one set against 123 optimally — 27 % more label, on a device where the whole label is 320 dots.

Leaking those escape characters into the document would be a corrupt format. So relabel would have to compute optimal switching itself. `code128` 0.2.1 already does, correctly, in 500 lines, under MIT OR Apache-2.0, with `thiserror` 2 as its only dependency — already in `Cargo.lock`.

```rust
// Verified in code128-0.2.1/src/lib.rs
pub struct Bar { pub width: u8, pub space: u8 }                  // :73-81
pub struct BarCoordinate { pub x: u32, pub width: u8 }           // :85-93
impl Code128 {
    pub fn encode(data: &[u8]) -> Self;                          // :134
    pub fn encode_str(text: &str) -> Option<Self>;               // :144, Latin-1
    pub fn bars(&self) -> impl Iterator<Item = Bar> + '_;        // :149
    pub fn bar_coordinates(&self) -> impl Iterator<Item = BarCoordinate> + '_;  // :156
    pub fn len(&self) -> usize;                                  // :169
}
```

`len()` is `indices.len() * 11 + 2 + 20` — **both 10X quiet zones are already in it**, and `bar_coordinates()` starts at x = 10 with the last bar ending at `len() - 10`. So the blitter is four lines and needs no quiet-zone arithmetic of its own:

```rust
let code = Code128::encode(data.as_bytes());
let x_dots = u32::from(el.module_dots);
for BarCoordinate { x, width } in code.bar_coordinates() {
    ink.fill_dots((origin_x + (x * x_dots) as i32), origin_y,
                  u32::from(width) * x_dots, bar_h, 255);
}
// total footprint, for preflight and for the UI's live capacity readout:
let total_dots = code.len() as u32 * x_dots;
```

Its default `Encoder::Mixed` produced a longer symbol than a provably-optimal dynamic-programming encoder on **0 of 30 000** printable-ASCII samples, so no tuning is needed. It has no FNC1, so no GS1-128 (§8).

### 5.3 `barcoders`, and what is fenced off

`barcoders = { version = "2.0", default-features = false, features = ["std"] }`. Defaults off drops the ascii/json/svg generators and the optional `image` dependency; `cargo tree -p barcoders` then prints one line. **Keep `std`** or `Error` loses its `std::error::Error` impl (`src/error.rs:34`) and `#[from]` in a `thiserror` enum stops working.

Every symbology is `T::new<S: AsRef<str>>(data: S) -> Result<T, Error>` then `encode(&self) -> Vec<u8>`, one `u8` per module, 1 = bar, no quiet zone, no height — exactly what a device-dot blitter wants. **With one exception: `TF` has no `new`.** It is `TF::interleaved` / `TF::standard` (`tf.rs:45`, `:65`). So do not write a `trait Barcode { fn new(..) }` blanket abstraction over the crate — it cannot express `TF`. Use a per-symbology `match` in relabel's own enum.

Four defects. The first three were found by reading the shipped source; the fourth only by reading rendered symbols back through a decoder, which is the argument for doing that to every symbology.

**1. Code 93 emits a wrong check character on ~4.3 % of inputs — so Code 93 does not ship.** `src/sym/code93.rs` declares `('[', …)` at *both* index 45 and index 46 (lines 64 and 65 of the table; confirmed by reading the file). `char_encoding` does a linear `find`, so index 46 is unreachable and whenever a modulo-47 checksum lands there the index-45 pattern is emitted instead. Measured: 8538 / 200 000 numeric inputs, 4.27 %. Worse than a single wrong symbol — when the C checksum lands on 46, `k_checksum_char` pushes that wrong `'['` onto the data and recomputes, so the K symbol is wrong too. And a third consequence: `valid_chars()` unzips the same table, so `Code93::new("]")` returns `Err(Character)` and the library can express only 46 of Code 93's 47 values.

Upstream fixed it in commit `7f8df633` (2024-12-10) and has not released since 2.0.0 (2024-04-03). The one-line fix is `(']', [1,0,0,1,1,0,0,1,0])` at that second line. **relabel does not vendor the patch in v1**, because unlike Code 128 — which was cross-validated by round-tripping through `code128`'s own independent ISO/IEC 15417 decoder — there is no independent Code 93 decoder available to verify a patched output against, and shipping an unverified fix to a checksum is worse than not shipping the symbology. Revisit when a handful of vectors from a known-good generator exist.

**2. `UPCA` is a type alias that silently encodes a different number.** `pub type UPCA = EAN13;` (`ean13.rs:96`) with `valid_len() = 12..13` and a `Parse` implementation that treats the range end as inclusive (`sym.rs:42`). So it wants **the 12 EAN-13 digits before the check digit**, i.e. `"0"` plus the 11 UPC data digits. Handing it a real 12-digit UPC-A produces a valid but *different* EAN-13, with no error at all: `UPCA::new("003600029145")` and `UPCA::new("036000291452")` both return `Ok` and encode differently. `DESIGN.md` §6.3 flags this as "UPC-A wants 12 characters, not 11" — the reality is worse, because it is silent. **relabel never exposes the alias.** It takes 11 digits, or 12 with the check digit validated, and calls `EAN13::new("0" + first_11)` itself.

**3. Codabar accepts data with no start/stop character.** `Codabar::new("12345")` returns `Ok` and encodes 49 modules of unscannable output; the A–D guard requirement appears only in a doc comment at `codabar.rs:7`. relabel validates: first and last characters in A–D, none elsewhere.

**4. Codabar's table is wrong for three of its twenty characters — so relabel encodes Codabar itself.** Its B and C patterns are swapped: `A40156B` scans as `A40156C`. And its `+` is twelve modules with five wide elements, where the standard's has three; zbar does not decode a symbol containing it at all. Every other character, and all of Code 39, ITF, EAN-13, EAN-8 and the UPC-A wrapper, reads back correctly. Codabar is twenty characters of seven elements each, so relabel carries the standard's table itself (2:1, one narrow space between characters) rather than a fence around the crate's.

Also fenced, though it is documented behaviour rather than a defect: `TF::interleaved` appends a mod-10 check digit to odd-length input (`tf.rs:51-56`), so the value scanned is not the value typed. relabel refuses odd lengths and says to add a leading zero.

Also worth knowing: `Code128::new` checks `data.len() < 2` in *bytes* (`code128.rs:279`), and `'À'` is two UTF-8 bytes, so `Code128::new("À")` returns `Ok` and encodes a data-free symbol. Irrelevant once `code128` owns that symbology, but it is a good indication of the crate's validation depth. Every reachable `panic!`/`expect` in `encode()` (`code39.rs:104`/`:111`, `code93.rs:124-127`, `code11.rs:95`, `codabar.rs:101`, `ean13.rs:109`) is guarded by `Parse::parse` having already validated the character set — each was checked individually — but the guard is the public constructor, so relabel must never construct these types by any other route.

### 5.4 QR

```rust
// Verified in qrcode-0.14.1/src/lib.rs
pub fn QrCode::new<D: AsRef<[u8]>>(data: D) -> QrResult<Self>;                        // :75
pub fn QrCode::with_error_correction_level<D: AsRef<[u8]>>(d: D, ec: EcLevel)
    -> QrResult<Self>;                                                                // :92
pub fn QrCode::with_version<D: AsRef<[u8]>>(d: D, v: Version, ec: EcLevel)
    -> QrResult<Self>;                                                                // :115
pub fn QrCode::width(&self) -> usize;                                                 // :173
pub fn QrCode::into_colors(self) -> Vec<Color>;                                       // :223
```

`qrcode = { version = "0.14.1", default-features = false }` — zero transitive dependencies with defaults off; its defaults pull `image`, `svg` and `pic`, none of which relabel uses. Read the modules back with `into_colors()` (row-major, `Color::Dark` is a black module) and blit at integer dot scale. `to_vec()`/`into_vec()` are `#[deprecated]`. `QrError` is not re-exported at the root - it is `qrcode::types::QrError` - and `QrCode` derives only `Clone`, so it has no `Debug`. Empty data is accepted and encodes a version 1 symbol holding nothing; relabel refuses it before the call. Capacity comes from calling the encoder, never from ISO/IEC 18004's single-mode table: the crate's greedy segmenter splits digit runs out of alphanumeric text, which usually costs a little capacity and occasionally gains some (V17-L holds 938 letters but 894 of `A-Z0-9` cycled; V3-H holds 35 letters but 36 cycled).

**relabel builds each symbol from the crate's parts** - `optimize::Parser` and `Optimizer`, `bits::Bits`, `QrCode::with_bits` - rather than through `QrCode::new` or `with_version`, for two reasons found in review. The segmenter takes any byte pair inside Shift JIS's ranges for Kanji mode, and UTF-8 kana often qualifies: `ああああ` went out in Kanji mode, which a decoder that follows the standard reads as Shift JIS and shows as `縺ゅ≠縺ゅ≠`. relabel's text is always UTF-8, so every Kanji segment is demoted to Byte before optimising. And the automatic path tunes its segmentation for version 9, 26 or 40 and fits the result into the smallest version that holds it, while `with_version` re-tunes for the one version it is given; the two occasionally disagree, so a version the automatic path reported could be refused when pinned. relabel decides both with one rule - a version holds the data if its own tuning or any of those three fits - and the automatic choice is the smallest version that holds it, so it is always pinnable. For data the crate would not send in Kanji mode it is never larger than the crate's own choice. For data it would - kana, CJK, and other text whose UTF-8 bytes happen to pair up as Shift JIS - relabel's symbol can be a version or two larger, and the largest holds about a fifth less kana, because Byte mode spends 16 bits on each pair of bytes where Kanji mode spends 13; the crate's smaller symbol is the one that reads back wrong. The optimiser depends on a version only through its length-field class - 1 to 9, 10 to 26, 27 to 40 - so data that fits no class at version 40 fits nothing, and relabel refuses it after that check rather than trying all forty versions. `with_bits` panics on a stream that was never terminated; relabel always terminates it first.

**Never call `with_version` with `Version::Micro`.** `bits.rs:739` is

```rust
let padding_bytes_count = data_bytes_length - self.data.len();
```

where `data_bytes_length = data_length / 8`. M1 (20 bits) and M3 (84/68 bits) have a four-bit final codeword, so that division truncates and `self.data.len()` can exceed it. Debug: `attempt to subtract with overflow`. **Release: the underflowed `usize` reaches `.take(n)` and `Vec::extend`, and it dies with `capacity overflow` — so this is not a debug-only hazard and no mitigation may assume it is.** Minimal repro: `QrCode::with_version(b"01234567890123456789", Version::Micro(3), EcLevel::L)`. A 200 000-case fuzz over `Micro(1..=4) × {L,M,Q,H} × len 1..=40` found seven distinct panic classes; the exhaustive characterisation is simpler and stronger than the sample — the bug can fire for exactly those `(version, ec)` pairs whose `DATA_LENGTHS` entry is not a multiple of 8, which is `Micro(1) L` (20), `Micro(3) L` (84) and `Micro(3) M` (68), of which the last two are reachable. Do not treat "seven cases" as a blocklist.

`Micro(0)` and negative Micro versions panic only in debug builds, at a checked cast (`cast.rs:67`); a release build returns `Err(UnsupportedCharacterSet)` instead, where `InvalidVersion` would be the right error. Upstream `kennytm/qrcode-rust#89` has been open since 2025-11-01. It survived because the crate's only Micro test uses `Version::Micro(2)`, whose 40/32-bit capacities *are* multiples of 8.

**The automatic path is provably safe.** `bits::encode_auto` (`bits.rs:902`) documents that it will not consider Micro versions, and all 40 Normal `DATA_LENGTHS` rows are multiples of 8, so `QrCode::new` and `with_error_correction_level` cannot reach line 739. The document model therefore represents only `Normal(1..=40)`, and the UI never offers Micro QR. That costs something real — M4 at 4 dots per module holds 21 alphanumeric characters in 84 × 84 dots against V1's 25 in 87 × 87 at 3 dots, genuinely better dots-per-character — and §8 says what would have to happen to get it back.

### 5.5 DataMatrix

```rust
// Verified in datamatrix-0.3.3/src/lib.rs and src/placement.rs
pub fn DataMatrix::encode<I: Into<SymbolList>>(data: &[u8], symbol_list: I)
    -> Result<DataMatrix, DataEncodingError>;                                  // :171
pub fn DataMatrix::encode_str<I: Into<SymbolList>>(text: &str, symbol_list: I)
    -> Result<DataMatrix, DataEncodingError>;                                  // :183
pub fn DataMatrix::bitmap(&self) -> Bitmap<bool>;                              // :164
pub fn Bitmap::<B>::pixels(&self) -> impl Iterator<Item = (usize, usize)> + '_; // placement.rs:642
```

`datamatrix = "0.3.3"`, Apache-2.0 OR MIT, `no_std` + alloc, two dependencies (`arrayvec`, `flagset`). Quiet zone is **one module**, not four, which is most of why it wins on this hardware (§6.4).

Three behaviours are fenced off in `relabel-render::code2d`, each confirmed by running it:

- **Macro headers are on by default, and wrong.** A `||` written as `&&` makes any data beginning `[)>` RS `05` GS or `[)>` RS `06` GS count as an ISO/IEC 15434 envelope even without its trailer: the last two bytes are silently dropped, and data under nine bytes panics. Every symbol is built with `DataMatrixBuilder::new().with_macros(false)`.
- **Choosing among several sizes fails on long data.** For a list of more than one size, the capacity check uses each size's Base256 lower bound and reports `SymbolListEmpty` well below the real capacity - 48 digits against the crate's default rectangle list (ISO/IEC 16022 sizes only), whose `Rect16x48` holds 98; with the DMRE rectangles that relabel's `rectangle` policy uses, it gives up at 117 digits where `Rect26x64` holds 236. relabel tries its candidates one at a time, each pinned, in the crate's own order - fewest data codewords, then the shorter diagonal - and takes the first that holds the data and fits the element's frame at its module size.
- **`encode_str` adds a UTF-8 ECI for any control character**, a tab or newline included, because its Latin-1 test admits only printable characters. Text that fits Latin-1 - DataMatrix's default character set - is encoded as plain Latin-1 bytes through `encode`, with no ECI; only text beyond it gets one.

`SymbolSize` exposes no dimensions, so relabel keeps its own table of all 48 sizes, and a test checks every row against the bitmap the crate produces. `DataEncodingError` is `datamatrix::data::DataEncodingError`, and has no `Display`.

**Verification is by decoders that share nothing with the encoders.** QR symbols are read back through `rqrr`, a port of quirc, and through `rxing`: rqrr hands a Kanji-mode segment back as its raw bytes, so only a decoder that applies Shift JIS, as the standard says, notices text wrongly sent in Kanji mode. DataMatrix symbols are read back through `rxing`, a pure-Rust port of ZXing and the only pure-Rust DataMatrix decoder found that shares no code with the encoder; the `zxing-cpp` crate reads DataMatrix too, but wraps the C++ library and needs a C++ toolchain to build it. Both are dev-dependencies and never ship. The crate's own `DataMatrix::decode` shares its size tables, placement and Galois-field code with the encoder, so a wrong table would round-trip cleanly through it. One case `rxing` cannot check: after a UTF-8 ECI, its Base256 decoding appends the bytes as UTF-8 text behind a second ECI of its own, returning the payload mis-decoded and twice. That test pins the ECI against ISO/IEC 16022's arithmetic instead (codeword 241, then 27 for ECI 26) and reads the rendered symbol back through the crate's decoder, which proves the drawing but not the encoder. Where a decoder would accept a mirror image - rqrr reads one - the tests compare modules directly. For a local sweep, `zbarimg --raw -Sbinary` reads QR byte-exactly; plain `--raw` re-encodes non-ASCII byte data. zbar has no DataMatrix.

**On a `Top` target the symbol is transposed before it is placed.** `Target::place` swaps the document's axes for `Top` - the feed runs down the canvas and the head across it - and a swap is a reflection, so a grid laid along the document's axes would print mirrored. A linear barcode survives a mirror; a 2D symbol does not. Text and images will need the same treatment when they arrive.

For `THIRD-PARTY.md`: **`flagset` 0.4.7 is Apache-2.0 only**, not dual-licensed. Permissive and MIT-compatible to distribute, but it carries an attribution/NOTICE obligation and a patent grant with a termination clause that relabel's own MIT text does not. Recorded in `THIRD-PARTY.md`.

---

## 6. Small-label constraints

**This section is what stops the UI offering things that cannot work.** Everything below is for the reference hardware: a D110_M, 96-dot head, 40 × 12 mm labels, 203 dpi, so a 320 × 96-dot canvas and one dot = 25.4/203 = **0.12512 mm = 4.926 mil**.

### 6.1 The universal rule

Every capacity figure in the UI is computed by calling the encoder and multiplying, never by a formula in the UI. Code 128's symbol count depends on subset switching and cannot be predicted from string length: `"12345678"` is 6 symbols, `"ABC-1234"` is 9, `"A1B2C3D4E5"` is 12.

```rust
/// Exact, microseconds, and the only number the Designer's capacity readout
/// is allowed to display.
pub fn barcode_width_dots(sym: Symbology, data: &str, module_dots: u16)
    -> Result<u32, BarcodeError>;
```

### 6.2 Code 128 across 320 dots

Total modules including both 10X quiet zones is `11n + 22`, where *n* is the symbol count from `Code128::encode(d).len()`.

| module width | mm | max digits | max A/B characters |
|---|---|---|---|
| 1 dot | 0.125 | 48 | 24 |
| **2 dots** | **0.250** | **18** | **9** |
| 3 dots | 0.375 | 8 | 4 |
| 4 dots | 0.500 | 4 | 2 |

**One dot is below every published floor** and thermal bar growth at that width is a ±30 % error on a nominal 5 mil element. Two dots is 0.250 mm = 9.85 mil, which is exactly the GS1 "other applications" minimum and the industry's 10-mil rule. **Two dots is the floor; three is safe.**

**But orientation moves that floor on this printer, and this is the part nobody documents.** `devices.json` gives D110/D110_M `print_direction_deg = 90`, and `BitPlane::oriented(Left)` is `rotate_cw()` (`crates/relabel-proto/src/image.rs:114-131`), which maps canvas *x* to *row index*. So a barcode reading along the 320-dot axis has its bars **perpendicular to media travel — ladder orientation — where bar width is set by feed-stepper accuracy rather than head-element pitch.** Picket fence, the accurate orientation, is only 96 dots long on this model. B1 and B21 (`direction = 0`) get picket fence for free on a horizontal barcode; the whole D11/D110/D101/B16/B18 family does not.

**Therefore: default `module_dots` to 3 on `direction = 90` models and 2 on `direction = 0` models. Clamp the minimum to 2 everywhere. Offer 1 only behind an explicit "out of spec" toggle.** This is inference from the 10-mil rule plus the head-versus-motor argument, not measurement; §9 has the two-minute experiment that settles it.

Bar height: ISO/IEC 16388 §4.7 e) wants ≥ 5.0 mm **or** 15 % of symbol width excluding quiet zones. At X = 2 with 9 characters that is 5.03 mm = 40 dots of the 96 available, leaving 56 for human-readable text and margins. Workable — but 12–16 dots of HRI under a 40-dot barcode is the same open legibility question as §4.3.

Other symbologies at X = 2 across 320 dots: Code 39 fits 8 characters (`13C + 45` modules), ITF fits 14 digits (`9n + 28`).

### 6.3 EAN-13 and UPC-A do not fit. The UI refuses them, and says why.

EAN-13 is 113X total (95 + 11 left + 7 right). Nominal X is 0.330 mm; permitted magnification 80–200 %.

- X = 2 dots → 226 dots = 28.3 mm wide, but that is **75.8 % magnification, below the 80 % floor**.
- X = 3 dots → 339 dots = **42.4 mm, wider than the 40 mm label**.
- There is no integer dot count at 203 dpi between 80 % and 113.8 %. The only compliant magnifications on a 203 dpi head are 113.8 %, 151.7 % and 189.6 %.

And the height kills it regardless of width: nominal bar height 22.85 mm, 18.28 mm even at 80 %, against a **12 mm label**. GS1 fixes the width-to-height ratio for omnidirectional scanning and discourages truncation. Same story for UPC-A (113X, same height), EAN-8 (81X, 18.23 mm nominal, 14.58 mm at 80 %) and UPC-E (67X, 22.85 mm).

**No EAN or UPC symbol can be printed to specification on a 40 × 12 mm label at any magnification, because the minimum compliant bar height exceeds the label height.** The first compliant EAN-13 on a 203 dpi head is X = 3 dots: 42.4 mm wide with 26.0 mm of bars, needing a label of at least 45 × 30 mm.

So the Designer **refuses** EAN-13, EAN-8, UPC-A and UPC-E on a 12 mm roll, with a message that says which constraint failed and offers Code 128 as the substitute. Refuse, not warn: a warned-past EAN-13 is a label that will not scan at a till, which is a support ticket with a worse ending than a refusal.

### 6.4 QR and DataMatrix

QR side is `17 + 4V` modules plus a 4-module quiet zone on each side. Footprint is `module_dots × (side + 8)`.

| dots/module | mm | max version | side | +QZ | footprint | L alnum / byte | M | Q | H |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 0.125 | V17 | 85 | 93 | 93 × 93 | 938 / 644 | 734 / 504 | 531 / 364 | 408 / 280 |
| 2 | 0.250 | **V5** | 37 | 45 | 90 × 90 | 154 / 106 | 122 / 84 | 87 / 60 | 64 / 44 |
| 3 | 0.375 | **V1** | 21 | 29 | 87 × 87 | 25 / 17 | 20 / 14 | 16 / 11 | 10 / 7 |
| 4 | 0.500 | — | — | — | V1 needs 116 dots | — | — | — | — |

Capacities were measured by binary search against the crate itself and match ISO/IEC 18004 Table 7 exactly (V1-L 41/25/17, V2-L 77/47/32, V3-L 127/77/53).

**One dot is unusable** — Denso Wave's own formula gives 0.125 mm per module and every published floor is 0.33–0.40 mm; standard thermal guidance is ≥ 3 printer dots because direct-thermal dot gain bleeds adjacent dark modules together and breaks light ones apart. **Two dots is marginal, three is the shipping default**, and the UI shows the module size in mm beside the dot count and the version actually chosen.

**DataMatrix is dramatically better on this hardware**, because its quiet zone is one module rather than four:

| symbol | +QZ | max module | footprint | numeric / alnum |
|---|---|---|---|---|
| Square 22×22 | 24 × 24 | 4 dots | 96 × 96 | 60 / 43 |
| Square 26×26 | 28 × 28 | 3 dots | 84 × 84 | 88 / 64 |
| Rect 12×36 | 38 × 14 | 6 dots | 228 × 84 | 44 / 31 |
| **Rect 16×48** | 50 × 18 | **5 dots (0.63 mm)** | 250 × 90 | **98 / 72** |

Seventy-two alphanumeric characters at 0.63 mm modules, against twenty-five at 0.375 mm for the best QR that fits. `Rect16x48` is one of ISO/IEC 16022's own rectangles, not a DMRE size. The DMRE rectangles of ISO/IEC 21471 go further at 4 dots: `Rect20x64` holds 124 alphanumeric characters in 264 × 88 dots, `Rect22x48` 106 in 200 × 96, `Rect16x64` 91 in 264 × 72. All measured against the crate with each size pinned, and every figure in the table above agrees. Whether handheld scanners read DMRE is not established; ZXing's port does. The Designer should suggest DataMatrix whenever a QR element's data does not fit at ≥ 3 dots per module.

A helper drives all of this:

```rust
impl ModuleGrid {
    pub fn footprint(&self, module_dots: u16) -> (u32, u32);
    /// Largest integer module size that fits the box. This is the whole UI
    /// story: show the version, the module size in dots and mm, refuse below 2.
    pub fn fit(&self, max_w: u32, max_h: u32) -> Option<u16>;
}
```

Note the caveat on the label geometry itself. `vendor/devices.json` gives D110 a `max_print_width_mm` of 15 but a label width range topping out at 12, and `build.rs` derives `printhead_px` from the **latter**, so the generated table already says 96 — verified by reading the generated `models.rs`, and matching what the hardware prints. The two fields mean different things: 15 mm is the widest *media* the transport accepts, 12 mm the widest it can *print on*, and `ModelSpec` keeps both. Ninety-six is the conservative case and every conclusion holds at 120; if 120 dots turn out to be addressable, a V2 QR at 3 dots (111 dots with quiet zone, 47 alphanumeric at L) becomes reachable. `DESIGN.md` §8.1 item 9 is the experiment.

### 6.5 Text

96 dots along the head axis, Noto's 1.362 em natural line height, and the arithmetic from §4.3:

| lines | line box (dots) | natural ppem | x-height (dots) | characters per 320-dot line, condensed |
|---|---|---|---|---|
| 3 | 32 | 23.5 | 12.6 | ≈ 31 |
| **4** | **24** | **17.6** | **9.4** | **≈ 42** |
| 5 | 19 | 14.1 | 7.6 | ≈ 52 |
| 6 | 16 | 11.75 | 6.3 | ≈ 63 |

At `line_height: 1.0` those ppem figures rise by 36 %, which is why 1.0 is the default rather than Noto's natural spacing — a 24-dot line box at ppem 24 is a materially more legible label than the same box at ppem 17.6, and inter-line space is the thing there is least reason to spend dots on here.

**Four to six lines is the honest ceiling.** Six lines at 11.75 ppem gives a 6.3-dot x-height, which is at or past the point where `0`/`O` and `1`/`l`/`I` stop being distinguishable. The Designer should show the line count and the resulting x-height in dots, not just the point size, because the point size means nothing to an operator and "your lower-case letters are 6 dots tall" means everything.

### 6.6 What the Designer must therefore do

- Show remaining capacity live for every barcode, computed from the encoder.
- Refuse EAN/UPC on media below 45 × 30 mm, with the reason.
- Refuse module widths below 2 dots without an explicit override; default to 3 on `direction = 90` models.
- Show the QR version and DataMatrix symbol that were actually chosen, and the module size in both dots and mm.
- Suggest DataMatrix when a QR will not fit at 3 dots.
- Show text as x-height in dots alongside the em size.
- Never offer a font size below the smallest that produces a 5-dot x-height.
- Draw the media guides from `media::Bounds` and `RepeatPlan::part`, which are what preflight reads, so a guide and a report cannot disagree about where content may go.

---

## 7. Preview and threading

### 7.1 Into a Slint image

There is **no greyscale pixel buffer in Slint**. `SharedImageBuffer` has exactly three variants — RGB8, RGBA8, RGBA8-premultiplied — so the preview costs three bytes per pixel whatever we do. Use `from_rgb8`, not `from_rgba8`: 25 % less data and the printer plane has no alpha.

```rust
// Verified in i-slint-core-1.17.1/graphics/image.rs, all re-exported from the
// slint crate root:
//   :122  impl<Pixel: Clone + Default> SharedPixelBuffer<Pixel>
//         pub fn new(width: u32, height: u32) -> Self
//   :90   impl<Pixel: Clone> SharedPixelBuffer<Pixel>
//         pub fn make_mut_slice(&mut self) -> &mut [Pixel]
//   :794  pub fn Image::from_rgb8(buffer: SharedPixelBuffer<Rgb8Pixel>) -> Self
//   :151  pub type Rgb8Pixel = rgb::RGB8;   // #[repr(C)] { r, g, b }, Copy + Default
```

relabel-render supplies the expansion with no toolkit type in the signature, so the GUI owns the two lines that touch `slint`:

```rust
/// Draw a 1-bit plane into RGB8 at a whole-number scale, into a buffer the
/// caller owns and sized from `Extent`. A short buffer is filled as far as it
/// reaches rather than refused.
pub fn plane_to_rgb8(plane: &BitPlane, scale: Scale, out: &mut [u8]);

/// Same, for the pre-threshold grey map, whose byte is already 0 for ink and
/// 255 for paper and is copied through unchanged.
pub fn grey_to_rgb8(grey: &GreyMap, scale: Scale, out: &mut [u8]);
```

Measured cost (release build, worst of three runs): a 320 × 96 plane expands in 12 µs at 1:1 and 69 µs at 8×, against 62 µs and 117 µs for the same rows built a dot at a time; the 256-entry byte-triple table is what buys that, and it is rebuilt on every call for about 1 µs. The largest page by area in the registry, B4 Pro at 1229 × 4134 - not the 1276 this paragraph used to give - is 1.2 ms through the table and 7.3 ms a dot at a time, so the 3.45 ms claimed here was both optimistic and inconsistent with its own 0.030 ms at 320 × 96, which scales to about 5 ms. Reducing that page to fit a 900 × 700 pane is 1.0 ms. `DESIGN.md` §5's "about 0.1 ms" for 384 × 320 is right, and conservative.

**The expansion is never the bottleneck.** A full 320 × 96 label — 40 hard modules, 3 rules, 120 anti-aliased cubic glyph paths, plus the threshold pass — rasterises in **216 µs** in release. `DESIGN.md` §5's hedge that a re-render on every keystroke "will not stay interactive" is true at 1229 dots wide and false at 96; drop it for the small-label case and keep it as a note for the wide models. Debounce exists for the rasteriser at 1229 dots, not for the preview.

Three facts about Slint that change how this is wired:

**Always hand over a fresh buffer; never cache and mutate one.** `SharedVector::detach` is a no-op at refcount 1 (`sharedvector.rs:205-212`), so allocate-fill-hand-over is copy-free. A buffer already given to an `Image` has refcount > 1 and deep-copies on the next `make_mut_slice()` — that is slint-ui/slint#5950, roughly two seconds for a 6500 × 6500 buffer.

**Deduplicate on the Rust side.** `Property::set` does compare old against new (`properties.rs:1023-1028`), but `SharedImageBuffer`'s `PartialEq` compares **data pointers**, not contents (`graphics/image.rs:211-225`). A freshly allocated buffer with identical pixels always compares unequal, marks the property dirty, and forces a fresh texture upload. So hash the bound document or the plane bytes and skip the set. (The converse is the good news: setting a 15 MB image never costs a 15 MB memcmp.)

**The texture upload is once per re-render, not once per frame.** `from_rgb8` stamps `cache_key: ImageCacheKey::Invalid` (`image.rs:794-799`), which makes femtovg's `TextureCacheKey::new` return `None` and bypass the shared texture cache — but the result still lands in the per-item `ItemCache`, whose `get_or_update_cache_entry` (`item_rendering.rs:54-70`) re-runs only when a tracked property goes dirty. For a raster image the tracked reads are `source`, `tiling`, `rendering` and `colorize`; `target_size` and `image_fit` are read only inside the `is_svg()` branch, so **resizing the preview element does not re-upload**.

### 7.2 HiDPI, which `DESIGN.md` §5 does not mention

`ImageFit::Preserve` preserves size *in logical pixels* and is "still scaled by the scale factor" (`enums.rs:281-284`). At 1.25× or 1.5× fractional scaling — the Windows and GNOME default — nearest-neighbour then gives some printer dots one physical pixel and their neighbours two. An operator judging a dither would be looking at an artefact of the display, not of the label. Two fixes, and both are used:

1. **Size in physical pixels.** Slint has a `phx` unit (`expression_tree.rs:617`). `width: px-w * 1phx; height: px-h * 1phx; image-fit: fill;` makes source-to-target an exact integer ratio in physical pixels, taking both numbers from one `preview::Extent` - which is the same figure as `plane-w * zoom` when magnifying and the correct one when reducing.
2. **Do the scaling in Rust**, so source size equals target size and no filtering runs at all - in *both* directions. Nearest-neighbour is worse minifying than magnifying, and every model but the 96-dot family needs minifying: at 1/6 the renderer would sample one dot in thirty-six. The expanded buffer is capped at `preview::MAX_BYTES`, 64 MiB, which holds every page in the registry at two pixels to the dot - B4 Pro at 2× is 58.1 MiB, and the ET10, whose 1600-dot head is the widest, stops at a 200 mm label and so at 29.3 - and above the cap the answer is a smaller scale rather than a renderer-side fallback. `image-rendering: pixelated` stays on the element as a belt for the fractional-position case below, not as a scaling strategy.

`image-rendering` is real and works where it matters: `ImageItem` has `in property <ImageRendering> image-rendering` (`i-slint-compiler-1.17.1/builtins.slint:373`) with `Smooth | Pixelated` (`i-slint-common-1.17.1/enums.rs:311-316`), and femtovg maps `Pixelated` to `femtovg::ImageFlags::NEAREST` (`images.rs:333-336`). The software renderer ignores the property entirely but its sampler already discards the fractional part (`draw_functions.rs:196-208`), so it is nearest either way — pixelated is correct on both renderers relabel's feature set enables.

One residual risk not settled by reading: `itemrenderer.rs:896-900` translates by `physical_distance` with no rounding, so an element at a fractional logical position shifts the dot grid by a sub-pixel. Whether the winit path rounds it later could not be determined from source. One screenshot on a 1.25×-scaled display settles it.

### 7.3 Threading

Read `slint-1.17.1/lib.rs:300-311` before writing any of this. It states that "Tokio futures aren't guaranteed to hand off their work to separate threads and may therefore not complete, because the Slint runtime can't drive the Tokio runtime", and that the current-thread scheduler "cannot be used in Slint main thread".

**So `slint::spawn_local` must never hold a btleplug future.** It is for runtime-agnostic futures only — `rfd::AsyncFileDialog` is the one rpgp actually uses it for, and that use transfers verbatim.

```
GUI thread (Slint event loop)              worker thread (tokio multi-thread)
──────────────────────────────             ─────────────────────────────────
cmd_tx.send(Cmd::Print { pages, cancel })  rt.block_on(job::print(&session, .., |p| {
ui.on_cancel(|| cancel_tx.send(true))          let w = ui_weak.clone();
◀── set_progress ──────────────────────        let _ = w.upgrade_in_event_loop(
                                                   move |ui| ui.set_page(p.page as i32));
                                           }))
```

`invoke_from_event_loop(func: impl FnOnce() + Send + 'static) -> Result<(), EventLoopError>` (`api.rs:1260`) is callable from any thread. `Weak::upgrade_in_event_loop(impl FnOnce(T) + Send + 'static)` (`api.rs:1201`) is that plus the upgrade, and drops silently if the window is gone. `Weak::upgrade()` returns `None` off-thread (`api.rs:1145-1151`) and **`Weak::unwrap()` panics off-thread** (`api.rs:1157-1163`) — never use it in a worker.

Unlike rpgp, relabel needs **one runtime alive for the whole app**, because the `Session` owns the BLE link across operations. rpgp's `OnceLock<Runtime>` plus `std::thread::spawn` per operation does not transfer; its `BusyGuard` (clearing the busy flag on panic) and its `invoke_from_event_loop` completion half do.

Progress rate is not a problem: `Timeouts::poll_interval` is 150 ms (`crates/relabel-core/src/job.rs:58`), so roughly seven `invoke_from_event_loop` calls a second. No coalescing needed.

**Cancel must be cooperative, and this is a change to `relabel-core`.** `job::print` (`job.rs:118-124`) takes `on_progress: impl FnMut(Progress)` and has no cancel parameter; `one_job` (`job.rs:183-188`) sends `PrintEnd` unconditionally *after* `run` returns. `JoinHandle::abort()` drops the future mid-`await`, that line never executes, and the paper never retracts. Add a `tokio::sync::watch::Receiver<bool>` checked in `wait_for_pages`'s loop alongside `check_deadline`, and `tokio::select!` on the `session.request` awaits. `tokio`'s `sync` feature is already in the workspace manifest, so this adds no dependency.

**Debounce** with a single `slint::Timer` held in an `Rc<Timer>` captured by the edit callbacks. `Timer` is `!Send` (`PhantomData<*mut ()>`, `timers.rs:62-65`) and stops when dropped, so it lives on the event-loop thread. `Timer::start` explicitly restarts an already-started timer (`timers.rs:70-72`), so `t.start(TimerMode::SingleShot, Duration::from_millis(80), cb)` on every edit *is* the debounce. `restart()` is the wrong call — it "does nothing if the timer was never started" (`timers.rs:132-136`). Hang it off `changed <prop> => { … }` blocks, which parse in 1.17 (`parser/element.rs:91`, `:489`), rather than wiring every widget callback.

### 7.4 The canvas editor, briefly

There is **no `Canvas` element in Slint 1.17** — neither `builtins.slint` nor `widgets/` has one, and `DragArea`/`DropArea` are HTML5-style data transfer that move a payload, not geometry. Everything is hand-rolled.

The one mechanism worth recording here because it is easy to get wrong: the documented drag idiom (`mouse-x - pressed-x`, as `SliderBase` uses at `widgets/common/slider-base.slint:64-67`) is only valid when the `TouchArea` does not move. A canvas element's `TouchArea` moves with it. Use `absolute-position`, which is a real output property on every element (`typeregister.rs:321`, type `Point`) and lowers to `ItemAbsolutePosition(self) + { x: self.x, y: self.y }` (`passes/lower_absolute_coordinates.rs`), with every ancestor's x and y registered as a dependency (`binding_analysis.rs:728-740`) so it is reactive:

```slint
function ptr-x() -> length {
    self.absolute-position.x + self.mouse-x - canvas.absolute-position.x
}
```

Snap in **dot** space, not pixel space: `Math.round((ptr-x() - grab-dx) / root.dot)`. And write back a whole rebuilt struct once per event rather than four field assignments — a field assignment on a repeater row is rewritten into read-whole-struct / rebuild / assign-whole-row (`llr/lower_expression.rs:357-398`, `generator/rust.rs:3210-3234`), so four assignments are four round-trips.

**The second mechanism, and it is the inspector's rather than the canvas's: never bind a property a widget assigns to itself.** `Property::set` asks the current binding to intercept the write and, when it does not, calls `remove_binding` — permanently (`i-slint-core-1.17.1/properties.rs:1005-1020`, with the default `intercept_set` returning false at `:348-360`). Only a two-way binding intercepts (`properties/two_way_binding.rs:21-31`). Every assignment inside a `.slint` file lowers to that same `Property::set` (`generator/rust.rs:3206`), and the std widgets assign to their own values on every edit: `SpinBoxBase::update-value` does `root.value = value` (`widgets/common/spinbox-base.slint:21-26`), `LineEdit.text` chains down to a `TextInput.text` the core editor writes, `ComboBoxBase::select` assigns `current-index`, and CheckBox, Switch and Slider all do the same. So a field filled by `text: root.pushed` shows the value the operator typed for ever after, hands that stale value back when the next element is selected, and tells a screen reader the same stale value through `accessible-value`. There is no compiler diagnostic: the only binding-related diagnostics in `i-slint-compiler/passes/` are a binding-loop deprecation and "Assignment in a pure context". The fix is a `public function` on the pane that **assigns** each editable value, invoked from Rust after the model row it reads is written: it is synchronous, it needs no generation counter, it lands when the value has not changed, and it generates byte-for-byte the same code as a `<=>` alias's setter, which the compiler collapses away anyway (`passes/remove_aliases.rs`). Three further measurements rule the std `SpinBox` out of a numeric field entirely: its value is an `int`, so the document's two floats cannot use one; it refuses a value outside its own `minimum..maximum` without assigning, without raising `edited` and without rewriting its text, so the box displays a number it does not hold (`spinbox-base.slint:22, 44-50`); a lone minus sign is erased on the keystroke, because a text that is not a float and not empty is replaced with the current value (`:88-94`), and a feed coordinate is legitimately negative; and the step it announces is hard-coded to `(maximum - minimum) / 100` (`fluent/spinbox.slint:60`). A `LineEdit` inside a wrapper that carries the accessibility node — role, label, description, value, minimum, maximum, step, read-only and the set-value/increment/decrement actions, with `accessible-role: none` on the inner editor, which is what `SpinBox` does for its own `TextInput` — has none of those four faults and is testable through `set_accessible_value` with no key events and no event loop. Note that `accessible-role` must be a constant expression (`passes/lower_accessibility.rs:25-38`), so a field whose role follows its kind does not build: that is why there is one component per role rather than one with a flag.

Slint's own design-mode selection frame at `tools/lsp/ui/components/resizer.slint` is worth **reading** and must not be copied: it is `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`, none of which is MIT-compatible. Using the toolkit under Royalty-free-2.0 is fine; lifting its source is the same mistake `DESIGN.md` §9 already refuses to make with the GPL protocol clients. Worth noting that it does not move the element live during a move-drag — it computes the final position at pointer-up — which is a tell that the moving-`TouchArea` arithmetic is awkward, and `absolute-position` is the way out of it.

**Accessibility** (`DESIGN-GAPS.md` #20): the inspector's numeric fields are the primary keyboard path and drag is the accelerator, never the only route. Per element, a zero-sized `FocusScope { x: 0; y: 0; width: 0; height: 0; }` with `forward-focus` to it and `focus-scope.focus()` from the `TouchArea`'s `pointer-event`, so clicking both selects and focuses without the scope eating the mouse — exactly `SliderBase`'s pattern. Arrows nudge one dot, Shift+arrows eight. `AccessibleRole` (`enums.rs:475-540`) has **no canvas or graphic role**, so the honest mapping is `list` on the canvas and `list-item` plus `accessible-item-index`/`accessible-item-count` per element, with `accessible-value` reading "Text, feed 12, head 4, 96 by 24 dots" and `accessible-action-set-value` parsing it back. Slint's own canvas editor handles exactly one key (Delete) and declares zero `accessible-*` properties, so there is no in-tree precedent to copy.

Testing: `i-slint-backend-testing = "1.17"` as a dev-dependency. `ElementHandle::mock_drag(target: LogicalPosition, button: PointerEventButton)` (`search_api.rs:1055`) presses at the element centre, interpolates at least two intermediate `PointerMoved` events on mock time, and releases — so the whole drag-and-snap path is testable with no event loop and no display. `accessible_value()` / `set_accessible_value()` cover the keyboard path.

---

## 8. Dependencies

`[workspace.dependencies]` additions for `relabel-render`, in the manifest style the workspace already uses — the comment says *why the feature set is what it is*, not what the crate does.

```toml
# The rasteriser. Pure Rust: no build.rs, no `links`, no [build-dependencies],
# no extern "C" anywhere in tiny-skia or tiny-skia-path. 0.12 is a CORRECTNESS
# FLOOR, not a preference: 0.11.4 panics at scan/hairline_aa.rs:124 on an
# anti-aliased 1.5-dot rect near the edge of a small pixmap - a barcode bar on
# a 96-dot label - in debug builds, and takes a wrong branch silently in
# release. `cargo test` is a debug build. Arrives free with Slint 1.17.1 via
# std -> i-slint-core/std -> svg -> resvg 0.47 -> tiny-skia 0.12.0, with
# default features on, so asking for less than resvg does buys nothing while
# slint is in the same graph. BSD-3-Clause -> THIRD-PARTY.md.
tiny-skia = "0.12"

# Shaping, line breaking, bidi and per-cluster font fallback. Exactly the
# version i-slint-core 1.17.1 requires (its Cargo.toml:233-236, with
# complex-scripts), so it unifies to one copy. `default-features = false`
# drops parley/system -> fontique/system -> yeslogic-fontconfig-sys, so THIS
# crate links no C and produces byte-identical output on every machine.
# Feature unification will turn fontique/system back on once relabel-gui adds
# slint; relabel-render must still build and test standalone with it off, and
# must never depend on a system font being present. Re-exports fontique, so
# there is no separate fontique dependency line.
parley = { version = "0.10", default-features = false, features = ["std", "complex-scripts"] }

# Glyph outlines, and the only route to Target::Mono grid-fitting - swash
# hard-codes HintingMode::Smooth with horizontal LCD subpixels and offers no
# way to change it. "0.42" is caret-on-a-0.x, which cargo locks to 0.42.x, so
# it cannot drift to the 0.44 that swash also allows; no `=` pin is needed or
# possible at this granularity. Same version parley 0.10 requires, so one copy.
skrifa = "0.42"

# Enable the dlopen escape for fontique's libfontconfig, without naming a C
# library in relabel's own manifest. fontique-0.10.0/Cargo.toml declares
# `fontconfig-dlopen = ["yeslogic-fontconfig-sys?/dlopen"]`, so this reaches
# the same unification DESIGN.md section 4 uses a target-conditional
# yeslogic-fontconfig-sys dependency for - and does it on every platform with
# no target gate. The `?` means it only fires if something else enabled the
# optional dependency, which is exactly the case we care about.
fontique = { version = "0.10", default-features = false, features = ["fontconfig-dlopen"] }

# Import of user artwork, decoded once, at import, and stored in the label as
# a grey PNG. Every codec here is pure Rust; avif-native (dep:dav1d) is C and
# stays off. Pinned exactly: relabel-render works around this version's gaps in
# its allocation limits (§4.6). NOTE: imageops::colorops::dither is NOT used -
# it panics on any image narrower than 2 px.
image = { version = "=0.25.10", default-features = false, features = ["png", "jpeg", "bmp", "gif", "webp"] }

# SVG import, rasterised at import only (§4.6). Default features off: no
# `text` - SVG text is refused, and would bring a second, unhinted shaping
# stack and a Zlib-only slotmap - and no `raster-images`, whose decoders ignore
# image's limits. resvg turns tiny-skia's default features back on, so its PNG
# codec is compiled in. With image and base64 this adds 18 crates to
# Cargo.lock, every one MIT or offering it.
resvg = { version = "=0.47.0", default-features = false }
# The embedded PNG travels in the label's JSON as base64.
base64 = { version = "0.22.1", default-features = false, features = ["std"] }

# Code 128 only, and it is here rather than barcoders because barcoders makes
# the caller hand-embed U+00C0/U+0181/U+0106 to select subsets A/B/C. Getting
# that wrong cost 27% extra label width on a measured 11-character sample -
# 156 modules against 123 - and leaking those escapes into the document would
# be a corrupt file format. Its only dependency is thiserror 2, already in the
# lock. bar_coordinates() already includes both 10X quiet zones in its
# coordinate space, so the blitter needs no quiet-zone arithmetic.
code128 = "0.2"

# EAN-13/8, Code 39, ITF. Defaults off drops the ascii/json/svg
# renderers and the optional `image` dependency, leaving ZERO transitive
# crates; `std` must stay or Error loses its std::error::Error impl
# (src/error.rs:34) and #[from] stops working. Code 93 is NOT exposed (a
# duplicate table key produces a wrong check character on 4.3% of inputs,
# fixed upstream, unreleased since 2024-04) and neither is the UPCA type alias
# (`pub type UPCA = EAN13` at ean13.rs:96 silently encodes a DIFFERENT number
# when handed a real 12-digit UPC-A). Its Codabar is not used either: the
# table swaps B and C and encodes `+` unreadably.
barcoders = { version = "2.0", default-features = false, features = ["std"] }

# QR. Zero transitive dependencies with defaults off; its defaults pull image,
# svg and pic. Version::Micro is never passed to with_version - bits.rs:739
# underflows on M1/M3, whose final codeword is 4 bits, and it is a `capacity
# overflow` abort in release, not merely a debug assertion.
qrcode = { version = "0.14.1", default-features = false }

# DataMatrix, including the DMRE rectangles, which hold roughly three times a
# QR's payload per dot on a 40 x 12 mm label because the quiet zone is one
# module rather than four. no_std + alloc, two dependencies. NOTE for
# THIRD-PARTY.md: its flagset dependency is Apache-2.0 ONLY, not dual.
datamatrix = "0.3"

# Config, saved printers, recent labels, the learned roll catalogue. NOTE for
# THIRD-PARTY.md: dirs-sys pulls option-ext 0.2.0, which is MPL-2.0 - the same
# weak-copyleft class as serialport, and listed there beside it.
dirs = "6"

# Atomic saves: temp file in the destination directory, then rename. Promoted
# from dev-dependency; a label editor that corrupts a file on a crash mid-save
# is unforgivable and the fix is fifteen lines.
tempfile = "3"
```

### 8.1 Versions that must diverge, and what it costs

**None of the above diverges from what Slint 1.17.1 compiles.** `tiny-skia` 0.12.0, `parley` 0.10.0, `skrifa` 0.42.x, `resvg` 0.47.0 and `image` 0.25.x are all exactly what the toolkit already brings, and `code128`, `barcoders`, `qrcode`, `datamatrix`, `dirs` and `tempfile` are not in the Slint graph at all, so there is nothing for them to collide with.

Three honest caveats:

1. **`Cargo.lock` currently pins none of it.** The lock has 136 packages and no `slint`, `parley`, `skrifa`, `swash`, `fontique`, `tiny-skia`, `resvg`, `rustybuzz` or `fontdb`. `crates/relabel-render` is named in `[workspace.dependencies]` but is not a `members` entry and has no manifest. So the instruction "reuse the versions Cargo.lock already compiles" cannot be satisfied by reading this lockfile — every version above was verified against the vendored 1.17.1 crate manifests plus `i-slint-core-1.17.1`'s own published lock. **Re-check the real lockfile the first time `slint` is added, before committing to any of these.**
2. **Declared constraints are ranges, not pins.** `i-slint-core-1.17.1` declares `image = "0.25"`, `parley = "0.10.0"`, `resvg = "0.47.0"`, `skrifa = "0.42"`, `swash = "0.2.7"`. Relabel's own resolve may legitimately differ from Slint's dev lock; state these as range-plus-currently-resolving-version, never as pins.
3. **Two duplicates already exist and are not relabel-render's doing.** `tiny-skia 0.11.4` arrives via `sctk-adwaita` (Wayland client-side decorations), and `skrifa 0.44.0` may arrive via `swash`, whose requirement is the unusually wide `>= 0.31.1, <= 0.44` — a range spanning several semver-incompatible 0.x minors, so cargo is free to select 0.44 alongside parley's 0.42.x rather than unifying. Whether it does is resolver-dependent; one lock was observed with both. Neither duplicate is caused by anything in this crate and neither is worth fighting.

One more thing to watch on the first GUI build: `png` reaches `flate2`, which defaults to the `rust_backend`/miniz_oxide path. If anything in the eventual Slint + btleplug graph turns on `flate2/zlib` or `flate2/zlib-ng`, feature unification links C libz across the whole build and quietly breaks the README's pure-Rust claim. `cargo tree -e features -i flate2` on that build answers it. `miniz_oxide` is already duplicated at 0.8.9 and 0.9.1, which is separate and harmless.

### 8.2 What was rejected, and why

- **`raqote` 0.8.5** — BSD-3-Clause and it does have `AntialiasMode::None`, so it would work. But it is not in the Slint graph, it pulls `png 0.17` against Slint's 0.18 (a duplicated raster stack, which is exactly what the reuse constraint forbids), plus `euclid`, `lyon_geom`, `typed-arena` and `sw-composite` — and its text path needs `font-kit`, which is freetype/dwrote/core-text. Strictly dominated.
- **`vello_cpu` 0.2.0** — conceptually the best fit of anything examined: `set_aliasing_threshold(Option<u8>)` does coverage-threshold-to-solid *inside* the renderer, which is precisely the 1-bit device operation. But 0.2.0 was published 2026-08-07 and 0.1.0 on 2026-07-29, it is not in the graph, and it brings `vello_common`, `peniko`, `kurbo`, `color` and `fearless_simd` — a whole second stack beside Slint's. Revisit in a year.
- **`femtovg`** — in the Slint tree, which is superficially attractive, but it is a GPU canvas over OpenGL or wgpu with no headless software path. It cannot rasterise a label without a window.
- **A hand-rolled scanline filler** — already half-adopted: §3.2 hand-rolls every integer blit, because rects and modules need no library at all. What is *not* hand-rolled is glyph outline filling, which needs correct winding, curve flattening and a supersampled or analytic polygon rasteriser. tiny-skia's is already paid for by the Slint dependency and costs 216 µs per label.
- **`cosmic-text` 0.19.0, `ab_glyph` 0.2.32, `fontdue` 0.9.4** — §4.2.
- **`rxing` 0.9.2** — Apache-2.0 only, 24 transitive crates including `regex`, `chrono`, `num` and `iana-time-zone`, and an `image = "=0.25.8"` exact pin - behind an optional feature, off in relabel's use, so it does not in fact hold `image` back. It is the only crate with UPC-E and PDF417 writers, and it is the only thing that can *decode* — which is the one argument that would change the calculus, if relabel ever wants a "verify this printed label from a webcam" flow.
- **`bwipp-rs` 0.1.1** — the single candidate if MicroPDF417, Aztec, DotCode, Han Xin or GS1 composite is ever wanted. Both versions published on the same day by one author, 855 downloads, `image = ">=0.25, <0.25.10"` (which measurably forces the workspace's `image` down to 0.25.9), and it depends on `qrcode 0.14` and `datamatrix 0.3` anyway. Its `render_matrix` draws one square cell per matrix row, so its PDF417 comes out at 1× row height against ISO 15438's ≥ 3X, and its own Micro QR port fails on the same M3 codeword arithmetic that breaks `qrcode`.
- **`pdf417` 0.2.1** — does not compile on stable at all: `#![feature(const_mut_refs)]` at `src/lib.rs:69`, `error[E0554]`. Last release 2023-10.
- **`scanning` 2.1.0** — a published snapshot of `barcoders` master with both defects fixed and a real `upca.rs`. Useful only as proof the fixes are small; it is a hard fork republished under a new name by a third party, its declared repository 404s, both versions were published within one minute in May 2025, and it has under 1000 downloads.
- **`dither` 1.3.10** — last published 2021, 462 recent downloads, palette-oriented rather than 1-bit, and it cannot express the zone mask. The kernels are eighty lines.
- **`csv` 1.4** — see §10: the reader is hand-written, in relabel-core.

---

## 9. Public API

```rust
// ─── document ────────────────────────────────────────────────────────────────
impl Label {
    pub fn from_json(s: &str) -> Result<Label, LoadError>;
    /// Refuses what would not reload as the same label.
    pub fn to_json(&self) -> Result<String, LoadError>;
}
pub fn bind(label: &Label, b: &Bindings<'_>) -> Result<Label, BindError>;

// ─── rasterise ───────────────────────────────────────────────────────────────
/// Draw a BOUND label at the target's dot size: ink coverage with its zone
/// plane, so the caller can preview grey before choosing a halftone, and a
/// report of everything not drawn as asked. An element that fails is left out
/// and reported, and the rest are drawn. Never anti-aliases a hard-edged mark;
/// never scales a barcode.
pub fn rasterise(label: &Label, t: &Target) -> Result<(GreyMap, RenderReport), RenderError>;

pub struct RenderReport {
    pub failed: Vec<(ElementId, Failure)>,
    pub off_canvas: Vec<ElementId>,
    pub rotation_approximated: Vec<ElementId>,
    pub text_clipped: Vec<(ElementId, TextClip)>,
    pub image_resampled: Vec<(ElementId, (u32, u32), (u32, u32))>,
    pub image_clipped: Vec<ElementId>,
    pub zone_shared: Vec<ElementId>,
    pub unimplemented: Vec<&'static str>,
    pub unrecognised: Vec<String>,
}

/// Why an element was not drawn, keeping the encoder's own numbers.
pub enum Failure {
    Barcode(BarcodeError),     // TooWide { needed, available }, ...
    Code(CodeError),           // TooLarge { needed, available }, ...
    Text(TextError),
    Artwork(ArtworkError),
}

/// Ink coverage to one bit per dot. `BitPlane` is relabel-proto's, so this is
/// the crate's exit point.
pub fn binarise(g: &GreyMap, o: &BinariseOpts<'_>) -> BitPlane;

// ─── the one-call path, which is what relabel-cli and the print button use ───
pub fn rasterise_plane(label: &Label, t: &Target) -> Result<BitPlane, RenderError>;

// ─── preflight ───────────────────────────────────────────────────────────────
/// Bind and draw EVERY row before a single frame is sent.
/// A 500-label run that fails at label 300 has burned 300 labels of stock.
pub fn preflight(label: &Label, data: Option<&Table>, t: &Target, now: CivilTime)
    -> Result<Preflight, RenderError>;

pub struct Preflight {
    pub times: Vec<u32>,       // labels per row, from `$times`; [1] with no data
    pub issues: Vec<Issue>,    // row-wide first, then by row, in document order
}
pub struct Issue {
    pub row: Option<usize>,    // None: the same on every row, reported once
    pub element: Option<ElementId>,
    pub name: String,          // the element's user-facing name, not its id
    pub kind: IssueKind,
    pub severity: Severity,    // Note < Warn < Refuse
}
pub enum IssueKind {
    TargetMismatch { canvas: (i32, i32), target: (u16, u16) },
    DuplicateId,
    NoRows,
    UnreachableColumn { column: String },
    UnresolvedKey { key: String, near: Option<String> },
    FilterIgnored { key: String, filter: String },
    ClockShadowed,
    SizeUnpinned,
    Bind(BindError),
    BadTimes(TimesError),
    Skipped,
    Failed(Failure),
    TextClipped(TextClip),
    OffCanvas,
    RotationApproximated,
    ImageResampled { stored: (u32, u32), drawn: (u32, u32) },
    ImageClipped,
    ZoneShared,
    Unimplemented(&'static str),
    Unrecognised(String),
}
```

**What preflight refuses** is anything that would print a wrong or missing mark: a symbol that does not fit its frame, a symbol off the canvas (the canvas would clip its bars), a placeholder no column answers in symbol data (it would scan as the literal `{sku}`), a row that cannot be bound or whose `$times` is not a whole number, and a label drawn for another head. Clipped text, an approximated rotation and an unresolved key in text are warnings: they print, visibly not as drawn. The cost is one render of the whole label bound only to the clock, for everything no row can change, and then one render per printing row of only the elements that name a column. Copies from `$times` are identical, so a row is drawn once however many times it prints.

```rust
// ─── capacity, for the Designer's live readouts ──────────────────────────────
pub fn barcode_width_dots(sym: Symbology, data: &str, module_dots: u16)
    -> Result<u32, BarcodeError>;
pub fn qr_grid(data: &[u8], ec: QrEc, version: Option<u8>)
    -> Result<ModuleGrid, RenderError>;
pub fn dm_grid(data: &[u8], symbol: Option<DmSymbol>)
    -> Result<ModuleGrid, RenderError>;

// ─── preview, with no toolkit type in the signature ──────────────────────────
pub const MAX_BYTES: usize;                     // 64 MiB
pub struct Scale;                              // up(n) / down(n) / for_pane(..)
pub struct Extent { pub px_w: u32, pub px_h: u32, pub len: usize }
pub fn plane_to_rgb8(plane: &BitPlane, scale: Scale, out: &mut [u8]);
pub fn grey_to_rgb8(grey: &GreyMap, scale: Scale, out: &mut [u8]);
pub fn plane_stamp(plane: &BitPlane, scale: Scale) -> u64;
pub fn grey_stamp(grey: &GreyMap, scale: Scale) -> u64;
```

Sizing the canvas from a model is the caller's job and goes through proto's own helpers, so the arithmetic exists once:

```rust
// relabel-core, resolving a ModelSpec into a Target
let head = relabel_proto::models::fit_to_head(spec.printhead_px, spec.printhead_px)?;
let feed = relabel_proto::models::mm_to_px(label_len_mm, spec.dpi);
let target = Target { head_px: head, feed_px: feed, dpi: spec.dpi };
```

**One consequence of the encoder's rotation that the renderer must handle, not the encoder.** For a `PrintDirection::Left` model, `encode_page` rotates first and then computes `cols = ceil(document_head_axis / 8) * 8`, erroring with `PageTooWide` if that exceeds the head. So a canvas whose head axis is 100 dots on a 96-dot head is rejected outright — correct. But a canvas of 92 dots is *accepted*, padded to 96, and `rotate_cw`'s `dst(c, r) = src(r, h-1-c)` puts the four pad columns at high x, so the label prints hard against head column 0 rather than centred. The vendor record's own `printMethodName` is 居中 ("centred") for both D11 and B1. **relabel-render therefore pads the canvas to exactly `head_px` itself, and centres the content inside it, rather than letting the encoder's byte padding decide.** `Canvas::head` must equal the target's head axis; preflight refuses a label when it does not (`TargetMismatch`).

---

## 10. What is deferred, and why

**Micro QR.** `qrcode` 0.14.1 panics on M1 and M3 (§5.4) in release as well as debug, and the one-line `saturating_sub` mitigation stops the panic without producing a spec-conformant symbol — the four-bit final codeword needs real handling, the crate ships no M1/M3 Annex I test to verify against, and `bwipp-rs`'s independent port fails the same arithmetic. It is a genuine loss on this hardware (M4 at 4 dots holds 21 alphanumeric in 84 × 84 against V1's 25 in 87 × 87 at 3 dots), and it comes back when either upstream fixes it properly or someone validates a patch against the ISO vectors. There is also an unanswered product question underneath: cheap warehouse laser scanners frequently do not read Micro QR at all.

**Code 93.** §5.3. Not a capability gap so much as a refusal to ship an unverified checksum fix.

**UPC-E.** Neither `barcoders` 2.0 nor its master has any encoder. It is 51 modules, a ten-row parity table and about forty lines of zero-suppression rules, cheap to write and test against known vectors — but it is the least-requested symbology on the list and pulling `rxing` for it would cost `regex`, `chrono`, `num`, `once_cell` and `unicode-segmentation`. Write it when someone asks.

**PDF417 and Aztec.** PDF417 is the wrong shape before it is a crate problem: a 2-column, 10-row symbol is 107X by 34X, so at X = 2 dots it occupies 214 × 68 of a 320 × 96 label for about thirty characters — worse than a V5 QR (154 alphanumeric in 90 × 90) and far worse than DataMatrix Rect 16×48. Revisit only for a driver's-licence or GS1 composite requirement, at which point `bwipp-rs` is the single candidate.

**GS1-128 with application identifiers.** `code128` 0.2.1 cannot emit FNC1; `barcoders` can (via `Ź`). Either keep both Code 128 encoders for that one path, or add FNC1 to a vendored `code128` — one extra symbol value, 102, plus a way to express it in the input. Not v1.

**Code 39 wide-to-narrow ratios other than 2:1.** ISO/IEC 16388 §4.7 b) permits 2.0:1 through 3.0:1, and 3:1 is more tolerant of print growth at small X, at a cost of three extra modules per character. `barcoders` hardcodes 2.0 in its bit tables, so supporting it means post-processing the module vector or not using that table. Only worth doing if a scan test shows 2:1 failing at X = 2.

**Two-colour.** `DESIGN.md` §9 already defers the `0x8A` double-colour path (three models, zero reported successful prints, and two mutually exclusive documented payload shapes). The document model has no colour axis and `Ink` is deliberately two-valued; adding red later means a second plane, not a wider pixel.

**Arbitrary rotation.** §4.5. `Rot` serialises as a number so this is not a schema break when it arrives.

**Media geometry, past step 10.** Per-inked-dot accounting, so a full-bleed fill on rounded stock is not warned about where no ink is actually lost - the keep-out check is per element frame, which is exact for the tail and conservative for a die-cut. The inset's severity is capped at Warn even for a symbol until print test P8 settles which of `blindZone`'s four edges is which. A tail in the middle of a strip, which v1's `at_start` cannot express. And a flattened unknown map on the seven structs that still lack one, `Tail` and `Fold` included - which is why `axis` had to be added now rather than by a later version.

**Embedded bitmap strikes.** `skrifa::bitmap::BitmapStrikes` exists and is exactly right in principle — `BitmapStrikes::new(&font)`, `glyph_for_size(Size, GlyphId) -> Option<BitmapGlyph>`, `BitmapData::Mask(MaskData { bpp: 1|2|4|8, is_packed, data })` with `decode_to_slice` (`skrifa-0.42.1/src/bitmap.rs:101`, `:353-400`). A 1-bpp strike at 12 or 14 px is a type designer's answer to the question hinting only approximates, and Cozette (MIT), Terminus (OFL 1.1) and Spleen (BSD-2-Clause) all ship `.otb`. Two risks are unverified and must be checked before committing: an `.otb` has no `glyf` or `CFF `, so `OutlineGlyphCollection::get()` returns `None` for every glyph and this is a wholly separate code path; and whether fontique's `FontInfo::from_source` (which needs a valid cmap) and harfrust will accept and shape a bitmap-only face at all is untested.

**CSV parsing.** Stays out of this crate. The document stores the binding — path, delimiter, whether there is a header — and the editor or the command line supplies a `Table`. The crate's whole justification in `DESIGN.md` §3 is that it has a heavy but self-contained rasterisation tail; a file-format parser is not part of rasterising. The reader is relabel-core's `read_csv`, hand-written with no dependency, because the rules that matter are the ones readers disagree on: a quote is special only at the start of a field, a stray or unclosed quote is an error with its line rather than a quietly different field, and a blank line is skipped rather than printed as a blank label. If a full parser is wanted later it is `csv` 1.4 (MIT OR Unlicense, pure Rust: `csv-core`, `itoa`, `ryu`, `serde_core`), which the lock already carries - `rxing`, a dev-dependency of relabel-render, reaches it through `codepage-437`'s build script - so taking it would add nothing that is not already built for the tests. For v1 the reader accepts UTF-8 with or without a BOM, refuses UTF-16 by name, and **errors clearly** on anything else rather than pulling `encoding_rs`' megabyte of tables for the Excel CP-1252 case. Whether that is the right call depends on how common a non-UTF-8 export is for this audience, which could not be settled from the sources read; if it turns out to be the norm, the error message becomes a support burden and `encoding_rs` earns its place.

**Variable data beyond one CSV file.** `$page`, `$total` and a counter; an `inline` binding, which the niimblue importer needs; niimblue's `\n` cell escape; text shrink-to-fit (§2.6); a clock per label rather than per job; and local time zones, which is why the command line asks for `--now` rather than guess UTC. relabel-core's job engine still does unchecked `u16` arithmetic on copies, which is why the command line keeps to one job per label. Its deadline is now per page rather than per job, and a job-level command the printer refuses is sent again - without which a second label never printed at all (PROTOCOL-NOTES 3b).

**niimblue import.** Abandoned at `RENDER-PLAN.md` step 11: a real export shows fabric 7.4.0 capitalised type names and a per-object `originX`/`originY`, so the mapping sketched here misplaces or drops most objects. The sketch below is kept for the parts a rewrite could still use, not as a specification. Worth doing, behind a `niimblue-import` feature, one-way and best-effort, returning `(Label, Vec<ImportWarning>)`. Read `canvas.objects` as `serde_json::Value`, whitelist `type` ∈ {textbox, i-text, text, rect, line, circle, image, QRCode, Barcode}, compute effective bounds as `left + width * scaleX`, snap `angle` to the nearest 90°, drop `skewX`/`skewY`/`styles`/`path` with a warning, and map `Barcode.encoding` (`"EAN13" | "CODE128B"`) plus its `printText`/`scaleFactor`/`fontSize`/`fontFamily` onto relabel's fields. About five hundred lines and a day or two.

**Do not reimplement fabric.js, and do not export it.** Faithful fabric means `originX`/`originY` anchors, `strokeUniform`, per-character `styles[]`, text-on-`path` and browser font metrics — that is a canvas library, and any inaccuracy in an *export* path lands in the user's saved file rather than in a scratch import. Separately and much cheaper: niimblue's `LabelPresetSchema` is a flat struct (`width, height, unit, dpmm, printDirection, title` plus the shape/split/tail/mirror fields) and is worth importing wholesale as a media-preset list. An hour's work that partly closes `DESIGN-GAPS.md` #7.

**Do not copy Thermalith's `label-stock.json` or its `RollDefinition` schema.** It is GPL-3.0. Individual measurements are facts and are not copyrightable, but a curated selection and arrangement can attract protection, and an MIT project should not have that argument. The *design pattern* — a learned roll catalogue keyed by the RFID barcode, with the dimensions coming from the user because the tag read carries no size — is an idea, is free to adopt, and is what relabel should do; the file is not.

**Incremental and dirty-region rendering.** `DESIGN.md` §5 defers this "until measured". It has now been measured for the small case: 216 µs for a full 320 × 96 label including 120 anti-aliased glyph paths and the threshold pass. Stay deferred, and drop the hedge for 96-dot heads specifically.

---

## 11. What only a printer can settle

These belong on `DESIGN.md` §8.1's M1 bring-up checklist, beside the `cols` and counter-byte experiments. Every one of them currently has a placeholder default in the code, and each placeholder is marked as such.

1. **The binarisation threshold.** 128 is a placeholder, not a calibrated default. Print one label carrying the same text at thresholds 96/112/128/144/160 and read it back under magnification. This also settles the `<` versus `<=` tie-break at the 1/16 coverage step boundary (§4.1).

2. **Anti-aliased-then-thresholded text against mono-hinted hard-edged text.** The literature is one-sided but not decisive for this head, and the two implementations that exist disagree by a factor of six about the threshold (§4.3). Concrete test: one 40 × 12 mm label per condition, text `"Shelf B4 Widget 0O1lI8B"` plus a four-line block, at 12/14/16/18/20/24 ppem, for four conditions — (a) unhinted + AA + threshold 128, (b) unhinted, no AA, (c) `Target::Mono` hinted, no AA, (d) `Target::Mono` hinted + AA + threshold 128 — printed at density low/default/high, photographed at 1:1, judged on whether `0`/`O` and `1`/`l`/`I` stay distinguishable at the smallest size. Add `Engine::Auto` against `Engine::Interpreter` on the static ttfautohint faces as conditions (e) and (f); they are one line apart and ttfautohint's own output is tuned for smooth targets, so it is not obvious which wins in mono. **This test chooses the default in §4.3 and the ppem cutover.**

3. **The smallest barcode module width that actually scans off a D110_M in ladder orientation.** Print one Code 128 payload at X = 1, 2, 3 and 4 dots on a single label; scan each with a phone camera and with a laser or CCD gun. Two minutes, and it settles the single most important number in the barcode design. Everything in §6.2 is inference from the 10-mil rule plus the head-versus-motor argument.

4. **The same, for QR: does a 2-dot module scan?** V5 at 2 dots beside V1 at 3 dots on one 40 × 12 mm label. This decides whether the default is 3 or 2 and whether V2 is reachable at all.

5. **Dot gain, as a number.** A step wedge of flat patches at levels 16…240, Floyd–Steinberg, no tone curve, photographed or scanned and measured. That single print produces the 256-entry tone LUT and settles whether any curve is needed. Print it at three density settings, because density and dot gain are the same knob from the paper's point of view. Also print a 50 % checkerboard beside a 50 % horizontal-stripe pattern of the same nominal coverage: if they differ in density, thermal history along the feed axis is significant, no static tone curve fixes it, and the mitigation would be biasing the ditherer against vertical runs.

6. **Whether Atkinson's clustering advantage survives on paper.** Same photograph, Floyd–Steinberg beside Atkinson, at 203 dpi. The measurement says FS puts 90.9 % of its mid-grey dots as isolated singles and Atkinson 0 %; whether that reads as "FS mid-greys close into mud" is a print, and it decides which kernel is the default.

7. **Whether a fractional module width is recoverable by a real scanner.** Measured at 1.6 dots per module on a 40-module Code-128-shaped pattern, run lengths came out 3,2,3,2,3,1,4,1,3,2 — bars and spaces varying by a third of their width — and snapping each edge to a whole dot first did not help (3,1,4,1,3,2,3,2…), because the information is not there. The honest answer may be that relabel computes the largest integer module width that fits and refuses non-integer scaling, which is what §2.3 already specifies. Whether a scanner tolerates the fractional case is empirical.

8. **Bar width reduction.** On a 203 dpi direct-thermal head a nominally 2-dot bar often prints 2.2–2.5 dots, biasing every bar-to-space ratio one way. ZPL's `^BY` and every industrial driver expose a compensation for it. Measure printed bar and space widths under magnification first; **do not implement it before measuring.**

9. **`blindZone` edge order.** The vendor record gives D11 gap paper `"1.0|0.0|0.0|0.0"` and B1 all zeros. One millimetre is 8 dots — 8 % of a 96-dot axis. Which of the four edges is which is unknown; a 1 mm bar printed hard against each edge of a D11 label resolves it in one print, and `Canvas::inset` is where the answer lands.

10. **Sub-pixel preview placement on a fractionally scaled display.** §7.2. One screenshot at 1.25×.

Two things that are *not* on this list because reading settled them: the `Paint::colorspace` question (§3.1 removes it by not using `Paint`), and whether `tiny-skia`'s `Rect::round` floor is worth an upstream report (it contradicts its own doc comment and diverges from Skia's `sk_float_round2int`, but §3.2 stops depending on it either way — worth filing after checking whether resvg or Slint rely on the current behaviour).