# `relabel-render` — ordered build plan

Ordering rule: each step ends with something the existing `relabel-cli` can put on paper, and something `cargo test` can check on a machine with no printer. Every rasterisation step is deterministic, so the test harness is golden files plus exact byte comparison — no image-diff tolerance, no snapshot fuzz.

---

## 0. Decisions to make before step 1 (they shape every step)

These are settled from the dossier and should be written into `DESIGN.md` as corrections before code lands, because three of them contradict what §3.2, §4 and §5 currently say.

| Decision | Ruling | Corrects |
|---|---|---|
| Drawing surface | `tiny_skia::Pixmap` (opaque white ground), **not** `Mask` | Mask has no `fill_rect` and no `stroke_path`, so every rect becomes a path and inherits the degenerate-bounds drop (`painter.rs:227-231`, `mask.rs:270-274`). The Mask argument was "avoids `Paint::colorspace` gamma" — but `ColorSpace::Linear` is a proven no-op (`color.rs:522-545` returns `None` for every gamma stage), so that cost is zero. 4× memory on a 30 KB buffer is irrelevant. |
| `rasterise` return type | `GreyMap { w: u16, h: u16, px: Vec<u8>, zone: Vec<u8> }`, 0 = ink, 255 = paper | DESIGN.md §3.2 says `-> Pixmap // 8-bit grey`; `tiny_skia::Pixmap` is 32-bit premultiplied RGBA. `GreyMap` also keeps tiny-skia out of the public API and is the natural input to the dither kernels. |
| Luminance | Read the **premultiplied red byte**. Opaque white ground + opaque ink ⇒ every pixel is alpha 255 ⇒ `r == g == b == luma`. Rec.601 vs Rec.709 only matters on a colour image blit; pick **Rec.601** there to match §5 and niimblue, and say so in one comment. | §5 specifies 0.299/0.587/0.114 while `image` uses Rec.709 (`color.rs:606`) and `Mask::from_pixmap` uses Rec.709 (`mask.rs:64-88`). |
| Hard vs soft ink | Exactly two paint constructors: `hard()` (`anti_alias: false`) for bars, modules, rules, rects, image blits; `soft()` (`anti_alias: true`) for glyph outlines only. No third. | Makes §5's per-element rule structural instead of a convention a call site forgets. |
| Geometry snapping | Snap every module/rect edge to integer dots in our own code; always pass `Transform::identity()` to `fill_rect`; express one-dot rules as rects, never paths. | `Rect::round()` is a floor (`floating_point.rs:81`), `fill_rect` degrades to `fill_path` under any non-identity transform (`painter.rs:194`), and `fill_path` silently drops near-zero-height bounds. |
| Element isolation | **One** mechanism: a parallel per-dot `zone: Vec<u8>` painted with each element's bounding box in z-order; error diffusion refuses to cross a zone boundary. No separate "stencil OR'd in after binarise". | Two dossier dimensions proposed two mechanisms for the same problem. The zone map subsumes the stencil: hard elements are already exactly 0 or 255 in the grey map, so a `Threshold` zone reproduces them bit-exactly. One pass, correct under overlap, explicable in the inspector. |
| Rotation | 90° steps only, applied by transforming coordinates in `relabel-render`; never hand tiny-skia a rotation matrix. `BitPlane::oriented` at the proto boundary handles print direction. | Everything measured about tiny-skia's snapping and degenerate handling assumes the identity transform. |
| `image::imageops::colorops::dither` | **Delete it from the §4 dependency table.** | It calls `get_pixel_mut(x+1, y)` with `x = 0` before the inner loop (`colorops.rs:501`) and panics on any image narrower than 2 px. A 1-dot-wide element on a 96-dot head is ordinary. |
| Canvas height | Comes from `ModelSpec::printhead_px` via `models::fit_to_head`, and for a `Left` model must **equal** the head width, not merely be ≤ it. | `encode_page` rotates first; `ProtoError::PageTooWide` fires on 320×100, and 320×92 would print shifted after `rotate_cw`. |

Golden-file format, used from step 1 onward: **PBM `P4`** for planes and **PGM `P5`** for grey maps. PBM P4's bit layout — 8 px/byte, MSB first, 1 = black, rows padded to a byte — is byte-identical to `BitPlane::bits()`. So a golden file is a header plus the plane's own bytes, is viewable in any image viewer, diffs sanely, and needs no PNG encoder and no hashing crate. This is worth more than it sounds: it makes every later step's test a one-line byte comparison.

Manifest additions (workspace):

```toml
members = ["crates/relabel-proto", "crates/relabel-render", "crates/relabel-core", "crates/relabel-cli"]

tiny-skia   = { version = "0.12.0", default-features = false, features = ["std", "simd"] }
parley      = { version = "0.10.0", default-features = false, features = ["std", "complex-scripts"] }
skrifa      = "=0.42"          # pinned: 0.44 is in the registry and would resolve up
qrcode      = { version = "0.14.1", default-features = false }
code128     = "0.2.1"
barcoders   = { version = "2.0.0", default-features = false, features = ["std"] }
datamatrix  = "0.3.3"
image       = { version = "=0.25.10", default-features = false, features = ["png","jpeg","bmp","gif","webp"] }
resvg       = { version = "=0.47.0", default-features = false }
base64      = { version = "0.22.1", default-features = false, features = ["std"] }
```

Never at least `tiny-skia` 0.12: 0.11.4 panics on `Pixmap::new(100,10)` + a 1.5-dot AA `fill_rect` near the edge (`scan/hairline_aa.rs:124`), which is a barcode bar on a 96-dot label, in a debug build, which is what `cargo test` is.

New `THIRD-PARTY.md` entries: tiny-skia + tiny-skia-path **BSD-3-Clause** ("Copyright (c) 2011 Google Inc." *and* "Copyright (c) 2020 Yevhenii Reizner"), arrayref BSD-2-Clause, flagset **Apache-2.0 only** (via datamatrix), option-ext **MPL-2.0** (via dirs), embedded fonts OFL 1.1, and the Slint licence triple if any Slint source is read for reference.

---

## 1. Crate skeleton, canvas, hard primitives, threshold — and a printable ruler

**Delivers.** `crates/relabel-render` as a workspace member. `Target { width_px, height_px, dpi }`, `RenderError`, `Canvas` over `tiny_skia::Pixmap` with `hard()`/`soft()` paints and dot-snapped `fill_rect_dots` / `hline_dots` / `stroke_line`, `GreyMap`, and `binarise_threshold(&GreyMap, u8) -> BitPlane`. Plus one concrete artefact: `render::calibration::ruler(&Target) -> GreyMap` — a full-head-width bar and a millimetre scale along the feed, ticks every mm, numbered every 5 mm, rendered deliberately longer than any label so the paper runs out mid-scale.

New `relabel-cli` subcommand `ruler [--head 96] [--length-mm 60]` that renders it, thresholds it, and hands the plane to the existing `encode_page` path. That is the first printable increment and it closes DESIGN-GAPS #7 (how the user learns the label length) with no protocol work at all — it is the trick that already worked on this project.

**Depends on.** `relabel-proto` (`BitPlane`, `models::{fit_to_head, mm_to_px}`), `tiny-skia`, `thiserror`.

**Tested by.** Golden `.pbm` for the ruler at 320×96 and at 192×96. Unit tests for the three measured tiny-skia traps, each of which is a regression test for a real defect, not a tautology: a zero-height `hline_dots` paints exactly one row; a 1-dot rule at y=39 leaves rows 39 and 41 empty and row 40 fully inked; a rect whose left edge sweeps 10.0→10.875 always starts at dot 10 through our snapping wrapper *and* our wrapper rejects the fractional input rather than silently flooring it. Plus `fit_to_head` round-trip: a `Left` target's `height_px` equals `printhead_px` exactly, and constructing a target that does not is an error, not a warning.

**Size.** ~400 lines plus ~150 of tests. One to two days.

**Print-gated.** Nothing in the code. The *artefact* is the print test — see **P1** below.

---

## 2. Document model and file format

**Delivers.** `Label`, `Media`, `Canvas`, `Common`, `Element` (the six §3.2 types), `Dpmm { num, den }` as an exact rational, `Rot`, `Ink { Black, White }`, serde to a pretty-printed `.relabel` JSON, and `rasterise(&Label, &Target) -> Result<GreyMap, RenderError>` dispatching only `Rect` and `Line` for now (the others return `RenderError::Unimplemented`). Geometry is `i32` **dots** throughout; the one deliberate `f32` is font size.

Evolution machinery from day one, because it is nearly free now and impossible to retrofit: `format` literal tag, integer `version`, `#[serde(default)]` on every added field, `#[serde(flatten)] unknown: Map<String, Value>` for unknown top-level keys, and

```rust
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElementOrRaw { Known(Element), Raw(serde_json::Value) }
```

`#[serde(other)]` cannot do this job — `serde_derive` rejects it on non-unit variants and on untagged enums (`internals/check.rs:169,178,187`) — so it can only *discard* a future element type. Untagged passthrough is the only shape that survives a round-trip by an older build.

**Depends on.** Step 1. `serde`, `serde_json`.

**Tested by.** `tests/corpus/v1/*.relabel` fixtures that are never deleted, each with its golden `.pbm`. Round-trip tests: parse → serialise → byte-identical. A fixture containing an invented `"type": "future_widget"` element survives a load/save cycle unchanged and renders as nothing with a warning. A fixture with an unknown top-level key keeps it. Corrupt JSON and a `Raw` element with no string `"type"` both error rather than being silently accepted (untagged enums give poor messages, so this needs an explicit validation pass).

Add a `cargo-fuzz` target over `serde_json::from_str::<Label>` here — cheap, and the document is the second place after the frame decoder where foreign bytes reach parsing code.

**Size.** ~500 lines plus corpus. Two days.

**Print-gated.** Nothing.

---

## 3. Binarisation: move the dither kernels, add zones

**Delivers.** `binarise(&GreyMap, &Binarise<'_>) -> BitPlane` with `Halftone { Threshold, FloydSteinberg, Atkinson, Bayer8, Sierra3 }`, `Zone { halftone, threshold, strength, serpentine, invert }`, a `tone: &[u8; 256]` LUT **defaulting to identity**, and error diffusion that refuses to push across a zone boundary.

**What moves rather than being rewritten.** The kernel loop at `crates/relabel-cli/src/bitmap.rs:69-115` — the serpentine scan, the bounds-checked `push` closure, the clamp to `-255..510`, the Floyd and Atkinson coefficient tables — is correct and hardware-proven, including at width 1 where `image`'s own dither panics. It moves essentially verbatim; the only change to the loop body is the zone guard `if zone[j] != zone[i] { continue; }` inside `push`.

**What does not move.**
- `bitmap.rs:47-64`, the direction-dependent scaling. Delete it. `BitPlane::oriented(head.direction)` already does this at the proto boundary and the canvas is authored in reading orientation; duplicating the policy is how the two drift apart.
- `bitmap.rs:37-39`, `image::open(...).to_luma8()`. Rewritten in step 7, not moved — it silently discards alpha (a transparent-background logo becomes a solid black rectangle, verified), it sniffs format from the file extension only, and it sets no decode limits.
- `bitmap.rs::preview()`, the half-block terminal renderer. CLI-specific; stays in the CLI.
- `#[derive(clap::ValueEnum)] enum Dither`. Stays in the CLI as a three-variant mirror that maps onto `render::Halftone`; a library crate must not depend on clap.
- The four tests at the bottom of `bitmap.rs` that assert the *scaling* policy die with the code they test. The polarity/inversion test moves.

**Depends on.** Step 1 (GreyMap). Independent of step 2.

**Tested by.** Golden `.pbm` per kernel over a fixed 320×96 gradient and a zone-plate fixture. A 1×8 and an 8×1 input produce output rather than panicking (the explicit regression against the `image` crate's bug). Zone containment: render a Code 128-shaped bar pattern beside a photograph, once with the photo present and once with it absent, and assert the bar columns are **bit-identical** — that is the test that proves diffusion never leaks into a quiet zone, and it is the whole justification for the zone map. Serpentine on/off both stable. `tone` identity is a no-op at the byte level.

**Size.** ~250 lines moved, ~150 new, ~200 of tests. One to two days.

**Print-gated.** The threshold default and the tone LUT. **This is the single most load-bearing print test in the plan** — see **P2**, **P3**, **P7**. Until they run, 128 is a placeholder and the tone curve stays identity. Do not ship a lightening gamma on the strength of received wisdom: in simulation, adding one made tone reproduction monotonically *worse* at every dot-gain level, and the D110_M has already printed correctly with no curve at all.

---

## 4. Linear barcodes

**Delivers.** `BarcodeEl { symbology, data, module_dots: u16, ratio, bar_height, quiet_zone, hri, checksum }` — **`module_dots`, never millimetres**, and every bar an exact integer multiple of it. Code 128 through `code128` 0.2.1 (`Code128::encode(&[u8])`, `bar_coordinates() -> impl Iterator<Item = BarCoordinate { x, width }>`, `len()`, with both 10X quiet zones already in the coordinate space); everything else through `barcoders` 2.0. A pure `width_dots(sym, data, module_dots) -> Result<u32, BarcodeError>` computed from the encoder's own module count, so the inspector can show remaining capacity live.

Defects in `barcoders` that must be handled here and not discovered later:
- **Code 93** emits a wrong check character on ~4.3% of inputs (`code93.rs:65` declares `('[', …)` twice, so table index 46 is unreachable through the linear `find`). Either vendor the one-character fix (`(']', [1,0,0,1,1,0,0,1,0])`) with a comment naming upstream commit `7f8df633`, or do not ship Code 93. Do not ship it unpatched.
- **`pub type UPCA = EAN13`** with `valid_len() = 12..13` wants the 12 EAN digits *before* the check digit. A real 12-digit UPC-A encodes silently as a different, valid, wrong barcode. Write our own wrapper; never expose the alias.
- **Codabar** accepts data with no A–D start/stop and returns 49 modules of unscannable output. Validate ourselves.
- **Codabar's table** swaps B and C, and encodes `+` as a pattern zbar will not decode. Found only by reading rendered output back through a decoder. Encode Codabar from the standard's table instead of fencing the crate's.
- **`TF::interleaved`** appends a mod-10 check digit to odd-length input without saying so, so the value scanned is not the value typed. Refuse odd lengths and say to add a leading zero.

Also: `Code128::new` in barcoders needs a hand-embedded `À`/`Ɓ`/`Ć` selector, which is exactly why Code 128 goes through `code128` instead — and why the document stores the user's plain data, never the selector characters.

Overflow default for barcodes is **`Fail`**, not clip. A clipped Code 128 scans as the wrong value.

**Depends on.** Steps 1–3 (hard paints, zone map, document model).

**Tested by.** Golden `.pbm` per symbology at a fixed module width. Round-trip Code 128 output through `code128`'s own ISO/IEC 15417 decoder (it ships one) for a corpus of payloads — that is a real independent check, not a self-consistency check. Assert bar run-lengths are exactly `n × module_dots` for every bar in a rendered symbol. Assert `width_dots` matches the rendered extent exactly. Read every golden, and a symbol containing every character of every table, back through an independent decoder (zbar) before committing — that is how the Codabar table was caught, and no in-tree test could have caught it. Assert EAN-13/UPC-A/EAN-8/UPC-E are **refused** on a 12 mm label with a message, since no EAN/UPC symbol is compliant on 40 × 12 mm at any magnification (nominal bar height 22.85 mm against a 12 mm label; even at the 80% floor it is 18.28 mm).

**Size.** ~600 lines plus a vendored patch. Two to three days.

**Print-gated.** The minimum module width. See **P4**. Default `module_dots` to 3 on `print_direction_deg = 90` models (the D11/D110 family prints barcodes in *ladder* orientation, where bar width is set by feed-stepper accuracy rather than head-element pitch) and 2 on direction-0 models, clamped to a floor of 2, with 1 only behind an explicit out-of-spec toggle. That default is inference, not measurement, until P4 runs.

---

## 5. 2D symbologies

**Delivers.** `QrEl { data, module_dots, ec, version: Option<u8>, out_of_spec }` via `qrcode` 0.14.1, built from its parts (`Parser`, `Optimizer`, `Bits`, `QrCode::with_bits`) so Kanji mode never carries UTF-8 and an automatic version is always pinnable, and `DataMatrixEl { data, module_dots, size: DmSize, out_of_spec }` via `datamatrix` 0.3.3, where `DmSize` is `square`, `rectangle`, `any` or a pinned `RxC`. Quiet zones are the specifications' own, four modules and one, and not a document field. On a `Top` target a symbol is transposed before placing, because that target's axis swap is a reflection. `ModuleGrid { w, h, quiet, dark: Vec<bool> }` plus `footprint(module_dots)` and `fit(max_w, max_h) -> Option<u16>`, which is the whole UI story — show the chosen version, the module size in dots and mm, refuse below 2.

**One hard rule:** never call `QrCode::with_version` with `Version::Micro(_)`. `bits.rs:739` is `data_bytes_length - self.data.len()` where `data_bytes_length = data_length / 8`; M1 (20 bits) and M3 (84/68) have a 4-bit final codeword, so that division truncates and the subtraction underflows — a subtract-overflow panic in debug, a `capacity overflow` in release. Upstream kennytm/qrcode-rust#89, open since 2025-11-01. The auto path (`new`, `with_error_correction_level`) is provably safe: `encode_auto` never selects a Micro version and all 40 Normal `DATA_LENGTHS` rows are multiples of 8.

Pin the version in the document rather than letting the encoder choose per row — otherwise a CSV batch silently changes QR size between rows and shifts the layout.

Three `datamatrix` behaviours to fence, not discover: macro headers are on by default and mangle or panic on envelope-looking data (build with `with_macros(false)`); a list of several sizes reports `SymbolListEmpty` far below capacity (try sizes one at a time, pinned); `encode_str` adds a UTF-8 ECI for a tab or newline (encode Latin-1 text as bytes).

DataMatrix earns its place on this hardware: quiet zone is 1 module, not 4, so `Rect16x48` holds 72 alphanumeric characters at 5 dots/module (0.63 mm, comfortably above every published floor) in 250 × 90 dots, against 25 characters at 3 dots for the best QR that fits.

**Depends on.** Steps 1–4.

**Tested by.** Rendered symbols read back by decoders that share no code with their encoders, as dev-dependencies: `rqrr` and `rxing` for QR, `rxing` for DataMatrix. The exception is DataMatrix text beyond Latin-1, which `rxing` mis-decodes after a UTF-8 ECI; that symbol's ECI is checked against ISO/IEC 16022's arithmetic and read back through the crate's own decoder (RENDER-DESIGN §5.5). Symbols on a `Top` target are compared module by module with the encoder's own, because a decoder may accept a mirror image. Golden `.pbm` for known payloads at fixed module sizes. A regression test asserting `with_version(_, Version::Micro(3), EcLevel::L)` is unreachable from our public API (a `#[test]` over the sizing enum, plus a grep-style test that the string `Version::Micro` appears nowhere outside a comment). `fit()` monotonicity. Module-grid blits land on exact integer dot boundaries and survive `BitPlane::rotate_cw` losslessly — round-trip a symbol through `oriented(Left)` and back and assert equality.

**Size.** ~400 lines. One to two days.

**Print-gated.** Module size floor: **P5**.

---

## 6. Text — the hard one

**Delivers.** `parley` 0.10 layout at scale 1.0 (one layout unit = one printer dot) with `quantize = true`, glyph outlines from `skrifa` 0.42 through a ~20-line `OutlinePen` into a `tiny_skia::PathBuilder`, filled into the canvas. Embedded fonts, registered explicitly, with `fontique`'s system backend **off** in `relabel-render`.

Four things that are not obvious and each of which is a day lost if missed:

1. **`swash` cannot do this job.** Its hinting mode is a private const — `HintingMode::Smooth { lcd_subpixel: Some(LcdLayout::Horizontal), … }` (`scale/hinting_cache.rs:27-30`) — with no API to change it, and `hint(true)` fails open silently. `Target::Mono` (the FreeType `FT_LOAD_TARGET_MONO` equivalent, the correct target for a 1-bit head) is reachable only through `skrifa::outline::HintingInstance::new(&outlines, size, location, HintingOptions { engine, target })`.
2. **Force `Engine::Auto(None)`, not `Engine::AutoFallback`**, unless the face carries a real `fpgm`/`cvt`. Google's variable fonts ship a 7-byte stub `prep` and no `fpgm`, and skrifa's `prefer_interpreter = !(max_instructions == 0 && fpgm.is_empty() && prep.is_empty())` (`outline/glyf/mod.rs:114`) therefore selects the TrueType interpreter — which then does nothing — instead of the autohinter that would have helped. Ship **static** faces.
3. **No system fonts is a silent failure, not an error.** With `fontique/system` off, `GenericFamilyMap::default()` is empty, `"sans-serif"` resolves to nothing, and parley's shaping loop never runs: empty layout, no error, no tofu. So `register_fonts` must be return-checked and `set_generic_families` + `append_fallbacks` called explicitly.
4. **Underline and strikethrough are rectangles**, never zero-height paths.

Fonts: Noto Sans **Condensed** Regular + SemiBold (measured +21% characters per line against the proportional Regular at 18 ppem on a 320-dot line), and Noto Sans Mono Condensed only for the column-alignment case — it is the same width as the proportional Regular, so it buys nothing but alignment. Subset to Latin at build time, commit the subsets with provenance rather than adding a Python build step. CJK is not embeddable (`NotoSansCJK-VF.ttc` is 32.7 MB); offer user font import.

Why this is genuinely hard, stated plainly: Noto Sans' `l` stem is 88/1000 em, so at the 14–18 ppem a 12 mm label forces, every stem is **1.2 to 1.8 dots**. That is exactly the regime where an unhinted outline gives one 1-dot stem and one 2-dot stem inside the same `n`. That asymmetry, not softness, is what makes small text look broken, and no amount of kernel or threshold tuning fixes it. Four lines in 96 dots is 17.6 ppem; five is 14.1; six is 11.75.

**As built.** `hinted` (autohinter, `Target::Mono`, hard edges) is the default at every size - measurement found no reason for the 24 ppem cutover - and `smooth` (unhinted, anti-aliased) is the P6 alternative, both per element. The fonts are the full upstream faces, not subsets. Text is laid out upright in its own frame and turned onto the canvas by index, which also places it correctly on a `Top` target. Text that overflows its frame is drawn clipped and reported with numbers (`text_clipped`), never drawn outside the frame; a trailing newline is not counted as lost text. A frame with no area, a size or line height no label could use, and a character the style's faces lack are each an error for that element alone. Alignment, weight and raster are open value sets, like `repeat`: a value from a later build survives a save, and the element is reported as not drawn. See RENDER-DESIGN §4.2 to §4.5.

**Depends on.** Steps 1–2.

**Tested by.** Golden `.pbm` per (string, face, ppem, hinting mode) over a fixed matrix — deterministic because the fonts are embedded and system fonts are off. An explicit test that a `FontContext` built with no registered font produces `Err`, not an empty layout. Advance-width tests at fixed ppem. A "font registration rejected" test using deliberately corrupt bytes.

Note for CI: feature unification means that once `relabel-gui` adds Slint, `fontique/system` gets turned on graph-wide. `relabel-render` must still build and pass standalone with it off, and must never *depend* on system fonts being present. Add that as a CI job, not a comment.

**Size.** ~700 lines plus the font subsets. Three to five days, and it is the step most likely to overrun.

**Print-gated.** Whether anti-aliased-then-thresholded beats mono-hinted hard-edged at these sizes. **P6.** This cannot be argued to a conclusion: the two existing implementations disagree (niimbluelib treats anything not exactly `0xffffff` as ink, i.e. threshold ≈ 1/255, which fattens every AA glyph by about a dot per edge; thermark drops coverage < 0.3, i.e. threshold 77), and neither is 128. Build both paths behind one enum and let the paper choose.

---

## 7. Images and SVG

**Delivers.** `ImageEl` with embedded base64 bytes (resampled to at most the canvas dot grid *at import*, so an embedded PNG is a few KB — which removes the only argument for a zip container), `origin: Option<PathBuf>` as a breadcrumb, fit modes, and a per-element halftone override that lands as a zone.

Decode with `ImageReader::new(Cursor::new(bytes)).with_guessed_format()?` and explicit `Limits` (`Limits::default()` caps allocation at 512 MiB with no dimension cap). Composite RGBA over white **before** luma. Linearise → `imageops::resize(FilterType::Triangle)` → re-encode: measured RMS against an exact linear-light box average is 11.0 gamma-encoded versus **1.7** linear-light, a 6× win, an order of magnitude larger than any filter choice. Triangle, not Lanczos3 — at a 5× downscale all non-Nearest filters are within noise (`horizontal_sample` widens support by the ratio, so every kernel converges to an area average) and Lanczos3 costs 3× the time for nothing. `Fit::Dots` skips resampling entirely when the artwork already fits in whole dots.

SVG through `resvg` 0.47 rendered straight into the label pixmap with a fitting `Transform` — the only import format with no resampling loss at all. Caveat: `usvg::Options::fontdb` is an empty database, so SVG `<text>` renders as nothing until we register our embedded fonts into it.

**Depends on.** Steps 1–3, 6 (for the SVG text fontdb).

**Tested by.** Golden `.pbm` for a fixture set including a fully transparent PNG (must come out white, not a black rectangle — the direct regression against the current CLI behaviour), a 1×N and N×1 image, a mis-named file (`.png` extension on JPEG bytes) which must decode via sniffing, and a decompression-bomb header which must be refused by `Limits` rather than allocating. A linear-vs-naive resize comparison asserting the linear path's RMS against a computed reference.

**Size.** ~450 lines. Two days.

**Print-gated.** Nothing new beyond P2/P3.

**As built.** The element stores `path` (kept under its v1 name), `png` - base64 of an 8-bit grey PNG sized for the frame at import - `fit`, and the `halftone` and `threshold` import chose; no `origin` and no zip. Import is the only place a file is decoded, under side, pixel and memory limits it enforces itself, because `image`'s `into_decoder()` skips the allocation check `decode()` makes. SVG is rendered at import only, without `text` or `raster-images`, with anything that could abort resvg refused. The filter note above is corrected in RENDER-DESIGN §4.6: at 5× Triangle is a tent, not an area average, so the resize test uses the same tent in double precision. Resampled line art gets threshold 187. The CLI prints files through the same import, so a transparent PNG prints white and a mis-named JPEG prints at all; it also prints WebP and SVG, sizing an SVG's millimetres at the model's pitch. The same change moves it from Rec.709 to Rec.601 and from a gamma-space Lanczos resize to a linear-light triangle one, applies EXIF orientation, and refuses files over 64 MiB or 40 megapixels that it used to print. At its default threshold of 128, resampled line art now thins where it used to darken, and it says so, suggesting 187. The SVG goldens run resvg and tiny-skia's floating-point geometry at import; they are verified on x86_64 Linux only. The step adds 18 crates to `Cargo.lock`, all MIT or offering it; `rxing`'s `image` pin is optional and off, so it does not hold `image` back.

---

## 8. Variable data and preflight

**Delivers.** niimblue's placeholder grammar adopted verbatim — `/{\s*(\$?\w+)\s*(?:\|\s*(.*?)\s*)?}/` giving `{sku}`, `{ sku }`, `{dt|YYYY-MM-DD}` — applied to text, barcode and QR data alike, with an unresolved key **left as written** rather than blanked. `$times` reserved as the row-repeat column. `bind(&Label, &Bindings) -> Result<Label, BindError>` returning a *new* document, so the editor's is immutable and "preview row 37" is one call. A bare `{sku}` in a barcode's data field *is* direct field binding; no second mechanism.

`preflight(&Label, &DataBinding, &Target) -> Preflight` renders and measures **every** row before a single page is sent, reporting `(row, element_id, element_name, Diagnostic)`. This is not a nicety: twenty characters of Code 128 is ~255 modules = 510 dots at the 2-dot minimum, wider than a whole 320-dot label, so overflow is the common case on this hardware, and a 500-label run that fails at label 300 has burned 300 labels of stock.

The CSV **parser** stays out of `relabel-render`. The document stores only the binding (path, delimiter, has_header); the GUI supplies `Vec<Vec<String>>`. If a parser is wanted later, `csv` 1.3 (MIT OR Unlicense, pure Rust). For v1, accept UTF-8 with BOM stripping and error clearly on anything else rather than pulling `encoding_rs`' megabyte of tables.

**Depends on.** Steps 2, 4, 5, 6.

**Tested by.** Grammar table tests including the unresolved-key case and `$times`. Preflight over a fixture CSV with a deliberately over-long row asserting the exact `(row, element_id)` reported. `bind` purity: the input `Label` is unchanged.

**Size.** ~350 lines. One to two days.

**As built.** The grammar is matched by a hand-written scanner in one pass, with JavaScript's character sets - `\s` includes U+FEFF and not U+0085, `\w` is ASCII, and a filter stops at a line terminator - and is checked against a backtracking transliteration of the regex on 200 000 generated strings; the regex itself is quadratic on a run of `{a|`. The grammar table's expected values come from niimblue's own regex run in Node. A placeholder resolves to a column first - its filter ignored, an empty cell blanking it - then `{dt}`, formatted by a port of dayjs's tokens with `YYYY-MM-DD HH:mm:ss` as the default, then as written. DataMatrix data is bound too. Three niimblue behaviours are not copied, because each prints a wrong label without a word: only the table's own columns resolve (niimblue's lookup reaches `{constructor}`), a row must be exactly as long as the header, and `$times` is a whole number from 0 to 10 000 or empty for 1 (niimblue's `parseInt` prints three labels for `3abc` and none for `abc`). A bound field is capped at 64 KiB. The clock is an injected `CivilTime`, read once per job, so a preview matches the print; the crate still reads no clock and no file. `preflight(&Label, Option<&Table>, &Target, CivilTime)` draws the whole label once, bound only to the clock, for everything no row can change, then only the elements that name a column, once for each row that prints; a row repeated by `$times` is drawn once. An element that names a column is bound on every row even when it is hidden, because binding is where a field too long or an ambiguous column shows up, and that stops the run whether or not the element prints. Each `Issue` carries the row, the element and its name, a typed kind that keeps the encoder's numbers - `TooWide { needed: 528, available: 304 }`, not a string - and a severity. An unresolved key refuses in symbol data, where it would scan as the literal `{sku}`, and warns in text; a label drawn for another head is refused. The overflow figure above undercounts: twenty characters of Code 128 at two dots a module need 550 dots once the quiet zones are counted. The fixture is `tests/corpus/v1/variables.relabel` with `variables.csv` beside it: five rows, the third too long for its Code 128 frame, the fifth `$times` 0. The CSV reader is relabel-core's `read_csv`, hand-written: UTF-8 with or without a BOM, UTF-16 refused by name, a quote special only at the start of a field, a stray or unclosed quote an error with its line, blank lines skipped. The CLI's `label` command checks every row before it connects, needs `--now` when the label prints the time - there is no time zone database to guess from - and prints one job per label. That last part needed a fix in relabel-core: the D110_M refuses the settings and `PrintStart` of a second job for a moment with a bare `0x00`, which relabel took for silence and streamed a page into a printer with no job open, so a refused step is now sent again (PROTOCOL-NOTES 3b). The job deadline is per page rather than per job, and each job of a split run is timed from its own start. TextFit, `$page`, `$total`, counters, an inline binding and niimblue's `\n` cell escape are deferred. No dependency is added. Print-gated: the fixture with `$times` 2 on a D110.

---

## 9. Preview surfaces

**Delivers.** Two functions in `relabel-render`, neither of which mentions Slint: `expand_1bit(&BitPlane, zoom: u32, out: &mut [u8])` and `expand_grey(&GreyMap, zoom: u32, out: &mut [u8])`, both writing RGB8. Zoom is done here, in Rust, by building one output row and `copy_from_slice`-ing it `zoom-1` times — measured 0.094 ms for 320×96 at 4×, 0.209 ms at 8×, against a re-render budget dominated entirely by rasterisation.

Doing the zoom in Rust rather than letting the toolkit scale is not premature optimisation, it is correctness: `ImageFit::Preserve` preserves size in *logical* pixels and is still multiplied by the scale factor, so on a 1.25× or 1.5× display — the Windows/GNOME default — nearest-neighbour gives some printer dots one physical pixel and their neighbours two. An operator judging a dither would be looking at an artefact. When the GUI lands, size in `phx` with `image-fit: fill` as a second belt.

There is no Luma8 pixel buffer in Slint; the greyscale pane is 3 bytes per pixel whatever we do.

**Depends on.** Steps 1, 3.

**Tested by.** Byte-exact expansion tests at zoom 1, 3, 4. Output length assertions. No display, no event loop, no Slint dependency.

**Size.** ~120 lines. Half a day.

**As built.** Two functions in a new `preview` module - `plane_to_rgb8(&BitPlane, Scale, &mut [u8])` and `grey_to_rgb8(&GreyMap, Scale, &mut [u8])` - which is RENDER-DESIGN §7.1's naming with `ink_` corrected to `grey_`. §7.1's `255 - coverage` is corrected too, and it was the load-bearing error in the step: `InkMap` became `GreyMap` in step 1 and its polarity is already 0 for ink and 255 for paper, so the byte is copied through unchanged and the subtraction would have drawn every label as a photographic negative - which compiles, passes every length assertion, and is wrong only to the eye. The zoom is not an integer multiplier but a `Scale` that goes both ways. Only the 96-dot family fits a pane at one pixel to the dot - the widest head in the registry is the ET10's 1600 dots and the largest page by area is B4 Pro's 1229 x 4134, not the 1276 x 4134 §7.1 quotes - and the fallback §7.2 names for the rest, letting the renderer scale, is nearest-neighbour decimation that drops a one-dot rule outright. A reduction is an area average over the block of dots behind each pixel, rounded away from paper so no ink is ever lost to it, and `Scale::for_pane` answers what a pane actually asks. `Extent` replaces the design's `rgb8_len`, because a pane needs the two physical-pixel dimensions and the length from one value or it can size a buffer of the right length and the wrong shape, and it carries the cap §7.2 asks for: `MAX_BYTES` is 64 MiB, which is every page in the registry at two pixels to the dot, asserted by a test that walks `relabel_proto::models::MODELS` rather than quoting a figure. The output row is built through the 256-entry table and copied down, not a dot at a time as Delivers says - 12 us against 62 us for a 320 x 96 label, 1.2 ms against 7.3 ms for a whole page. A buffer of the wrong length is filled as far as it reaches rather than refused, the rule `GreyMap::from_px` and `binarise` already follow, and every byte inside the extent is written, because a Slint pixel buffer arrives black rather than white and an unwritten tail is a black band. `plane_stamp` and `grey_stamp` are added for the deduplication §7.1 asks the Rust side to do, kept out of `Extent` so a layout pass pays nothing for them. 302 lines against the estimate of about 120: the reduction is 60 of the difference, `Scale` and `Extent` 90, the stamps 25, and giving any of them up puts dot-grid arithmetic back in the GUI. The zone plane gets no pane; it would need a colour policy this crate has kept out of and a public id-to-element mapping, and `rasterise::zone_ids` is private. The tests are in-file against a dot-at-a-time reference expander, in both directions of the scale and at widths that are not multiples of eight, with no golden and no fixture; the one that matters most is `set_padding_bits_past_the_width_are_not_drawn`, because a row padded on the right spills its pad dots into the row below. No dependency is added and no toolkit type appears in any signature. One thing a screen settles rather than an argument is still owed: §7.2's residual, that `itemrenderer.rs:896-900` translates by `physical_distance` without rounding, needs one screenshot on a 1.25x display.

---

## 10. Media geometry: shape, tail, fold, repeat

**Delivers.** `shape`, `tail { pos, length }`, `fold { axis, parts }` as **media geometry and editing guides**, and `Repeat { None, Copy, Flip }` as a render-time transform of the rendered region.

Two things this gets right that niimblue does not. First, `Repeat` is a transform of the rendered plane, not a mutation of the element list — undo-safe, composes with dithering, and gets flipped text correct for free. Second, `printDirection` is **not** in the document at all: `encode_page` already calls `plane.oriented(head.direction)` from the model registry, so the document is always in reading orientation and orientation is the printer's business. Keep an advisory `meta.authored_for { model, dpmm, w, h }` so the app can warn on a mismatched head.

The tail is real paper to keep clear — niimblue's `getLabelBounds()` subtracts `tailLength` from the canvas — so it constrains layout even though it draws nothing.

**Depends on.** Steps 1–2.

**Tested by.** Golden `.pbm` for a half-split label under `Copy` and under `Flip`, asserting the flipped half is the 180° rotation of the first. Bounds tests with a tail.

**Size.** ~200 lines. One day.

**As built.** One new module, `media`, plus `axis: Axis { Feed, Head, Unknown(Value) }` on both `Tail` and `Fold` - the field Delivers asked for, three steps late, and the one field whose loss *inverts* a meaning rather than dropping one, since neither struct has a flattened unknown map and a probe confirmed a later build's `axis` key is dropped on load and absent on save. Delivers' `tail { pos, length }` and `fold { axis, parts }` were never what step 2 built: the shipped types are `Tail { at_start, len_dots, axis }` and `Fold { at_dots, parts, axis }`, and `at_dots` is the absolute *position* of the first crease rather than a pitch - panel 0 runs from the start of the content, after a leading tail, to the crease, and that length is the pitch of the rest. That reading is the only one that expresses the product the tail exists for: a cable flag with a 40-dot tail and a crease at 180 is two 140-dot panels, which neither a canvas-origin pitch nor a content-origin pitch can say without moving the crease when the tail changes. `authored_for` is `{ model, head_px, dpi }` rather than the plan's `{ model, dpmm, w, h }`, and it now has a reader: `IssueKind::AuthoredElsewhere`, at Note. The load-bearing correction is to this step's own third paragraph. The tail does **not** subtract from the canvas the way niimblue's `getLabelBounds()` does, because the page handed to the printer has to cover the whole label - the paper travels that far, and `Target::require_head` demands the head axis match exactly, so a shortened canvas prints shifted rather than inset. `media::Bounds` carries three rectangles instead: `canvas`, `content` less the tail, and `usable` less `Canvas::inset`, which finally has a reader too. The die-cut is resolved against the canvas rather than stored, since a circle's centre and diameter follow from `Canvas` and a second authority over them would make a disagreeing file meaningless. `fits` and `OffCanvas` are untouched - off the canvas is a different sentence from on paper the label does not keep - so `RenderReport::keepout` reports `Keepout { Repeated, Tail, Inset, Shape }` per element, Refuse for a symbol whose dots the label will not carry and Warn otherwise, with the inset capped at Warn even for a symbol until print test P8 says which of its four edges is which. `Repeat` is applied to the finished `BitPlane` after `binarise`, inside `rasterise_plane`, and that is not an implementation detail: diffusion runs in the canvas frame, so a region rotated before it is dithered disagrees with its own half turn in 6,209 of 15,360 dots under Floyd-Steinberg, and a grey copy sharing its source's zone id moves 3,794 dots of the panel that was already right. After binarisation the turned panel is the half turn exactly, panel 0 does not move, and the halftone zones need no copy - all 254 ids are spent on elements, and collapsing a copy into the one spare would throw away the per-element fence inside it. `Flip` alternates with more than two panels, which is a Z-fold and degenerates to the plan's half split at two; what it assumes is the axis the reader turns the tag about, and a reader who opens it like a book wants `Copy` instead - a coin flip no software can settle, recorded as print test P9. The dots are copied one at a time through `get`/`set` into a clone of the plane rather than by shifting packed rows: 0.059 ms for a 320 x 96 flip and 9.6 ms for the largest page in the registry, against a sub-byte shift at an unaligned crease being exactly where a one-dot offset hides. `Repeat::Unknown` widened from `String` to `Value`, because `"repeat": {"kind":"tile"}` was a hard load error that lost the whole file. Nothing is rejected at load - a negative tail, `parts: 0`, a crease at `i32::MAX` and a canvas side of -4 all load, round-trip and are reported - and three traps were found doing it: `i32::clamp` panics when its low bound exceeds its high one, which made every render of a negative canvas with a rounded die-cut panic in release as well as debug; `u16` panel arithmetic wraps a copy onto a real row; and the work has to be bounded by the page rather than the document, which counting only the panels that have room does. 600 code lines against the estimate of about 200 - `media` 503, preflight 42, rasterise 28, the schema 22, `lib` 5 - itemising as the die-cut containment (about 140, and shape is a quarter of this step's title), the fault enum with its ten sentences and severity table (about 90, which is what this crate's reporting costs), `repeat_grey` with `rasterise_preview` (about 40, without which the grey pane shows one panel while the dot pane shows two), and `AuthoredElsewhere` (about 18). Deferred with reasons: per-inked-dot accounting, so a full-bleed fill on rounded stock warns where no ink is actually lost; flattened unknown maps on the other seven structs; and a mid-strip tail, which v1 cannot express. No number here has touched paper.

---

## 11. niimblue import (abandoned)

**Delivers.** One-way, best-effort `import_niimblue(&str) -> (Label, Vec<ImportWarning>)` behind a `niimblue-import` feature. Read `canvas.objects` as `serde_json::Value`, whitelist `type ∈ {textbox, i-text, text, rect, line, circle, image, QRCode, Barcode}`, compute effective bounds as `left + width*scaleX`, snap `angle` to the nearest 90°, drop `skewX`/`skewY`/`styles[]`/`path` with a warning. Separately import `LabelPreset` wholesale as a media-preset list — a flat struct, an hour's work, and it partly closes DESIGN-GAPS #7.

**Do not reimplement fabric.js and do not export it.** Faithful fabric means `originX`/`originY` anchors, `strokeUniform`, per-character `styles[]`, text-on-path and browser font metrics. Any inaccuracy in an *export* path lands in the user's saved file rather than in a scratch import.

**Depends on.** Steps 2, 4, 5, 6, 10.

**Tested by.** A corpus of real exported templates, each asserting the warning list as well as the geometry.

**Size.** ~500 lines. One to two days.

**Not built, and why.** Abandoned after a real exported template was obtained from the running app and read. Two of the three rules Delivers states above are wrong, and both fail silently. The export is fabric **7.4.0**, whose `type` strings are capitalised class names - `"Textbox"`, `"Barcode"` - so of the nine types whitelisted above only `Barcode` would ever match and every other object would be dropped without a word. And `originX`/`originY` vary **per object**: the text box in the sample is `center`/`center` where the barcode is `left`/`top`, so `left + width*scaleX` reads a text box's centre as its left edge and misplaces it by half its width - 38 dots of a 240-dot label in that file. An importer written to this section would have placed most objects wrongly and dropped the rest, and its tests would have passed, because fixtures written from the same assumptions agree with them. What the export does confirm: a fabric coordinate is exactly one printer dot (a 30 x 12 mm label at 203 dpi exports as 240 x 96), and `label` carries `printDirection`, `size`, `shape`, `split`, `splitParts`, `tailPos`, `tailLength` and `mirror` as `DESIGN-GAPS.md` #8 describes them. Neither the export nor the design worked up against the old assumptions was kept, so reviving this step starts by exporting a real template again - a few minutes in the running app - and rewriting the object mapping against it; the honest test corpus is real exports rather than fixtures this project writes for itself. The step earned its keep anyway: designing it is what found the barcode axis defect fixed in `rasterise`, which had a linear barcode running down the canvas on the 53 registry models whose head runs across it.

---

## 12. Canvas editor (`relabel-gui`, not `relabel-render`)

Listed here for ordering, but it is a different crate and a different skill. `relabel-render` must know nothing about it.

**The genuinely hard parts, and they are not the ones people expect.** There is no `Canvas` element in Slint 1.17 — `DragArea`/`DropArea` move *data*, not geometry — so everything is hand-rolled. The load-bearing detail is that the documented drag idiom (`mouse-x - pressed-x`, as `SliderBase` uses it) is only valid when the TouchArea does not move, and a canvas element's TouchArea moves with its element. The fix is `absolute-position`, which lowers to `ItemAbsolutePosition(self) + { x: self.x, y: self.y }` and depends reactively on every ancestor's x/y, so `self.absolute-position.x + self.mouse-x` is invariant under the element moving. Snap in **dot** space, not pixel space, and write back one whole rebuilt struct per event (a field assignment on a repeater row is rewritten into read-whole-struct/rebuild/assign-whole-row anyway).

Slint's own design-mode resizer (`tools/lsp/ui/components/resizer.slint`) is worth reading and **must not be transliterated** — `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`, none MIT-compatible. Note also that it does not move the element live during a move-drag; it computes the final position at pointer-up. That is a tell that the moving-TouchArea maths is awkward, and it is the problem `absolute-position` solves.

Accessibility is a real constraint (DESIGN-GAPS #20) and there is no in-tree precedent to copy: Slint's own canvas handles exactly one key (Delete) and declares zero `accessible-*` properties. `AccessibleRole` has no canvas or graphic role, so `list` on the canvas and `list-item` + `accessible-item-index`/`accessible-item-count` per element is the closest honest mapping. The inspector's numeric fields stay the primary keyboard path; drag is an accelerator, never the only route.

**Tested by.** `i-slint-backend-testing`'s `ElementHandle::mock_drag(target, button)`, which presses at the element centre, interpolates intermediate move events on mock time, and releases — so drag-and-snap is testable with no display and no event loop. Plus `accessible_value()` / `set_accessible_value()` for the keyboard path.

**Size.** Two weeks, honestly. It is the largest single item in the project after the protocol layer.

**As built, first slice.** `relabel-gui` exists and is the only crate that names `slint`, pinned `~1.17` with `default-features = false`: the defaults compile in `backend-qt` and `renderer-skia`, which the README's "no Qt, no C++" depends on staying out, and they ask for a `system-tray` crate that is not even resolvable here. The renderer is `renderer-software` alone rather than femtovg - the canvas is a nearest-neighbour blit of a 1-bit plane and a few dozen rectangles, so there is no GPU work to do, and it keeps seven Apache-2.0-only OpenGL crates out of `deny.toml`. The slice opens a `.relabel` file, draws it at four device pixels to the dot behind hairline element frames, selects by click or Tab, drags snapped in dot space, nudges by one dot with an arrow and by one millimetre of the label's own pitch with Shift, publishes a `list`/`list-item` accessibility tree that reads and writes placements as sentences, undoes and redoes, and saves atomically. It does not print, connect, create or resize an element, or show the grey pane: those are named slices after it, and the inspector is the next one. The `Label` lives in Rust inside one `Doc` - one command in, one outcome out, no toolkit type in any signature - and `.slint` holds a flat row projection with no rules in it, so the compiler's repeater write-back rewrite never fires. Sixty-two tests run with no display, through `i-slint-backend-testing`, including the drag and the keyboard. The binary has been run on Linux, on the session's Wayland compositor and on a bare X server: it opens the corpus label and draws it at four pixels to the dot, with its element frames and its axis note, and it shows the clipping the risk list predicted - a 320-dot label at that zoom is 1280 device pixels against a 900-pixel window, and this slice has neither a fit nor a scroll, which is the first thing an operator meets. It is still owed a launch on the macOS and Windows test machines, where the class of fault is a missing runtime rather than a layout. Three defects found in review are fixed and pinned by tests: `VecModel::set_vec` emits a model reset that drops the repeater instance holding keyboard focus, so an arrow key after an undo moved nothing; the accessibility sentence parsed a number out of the element's own name, so an element called "header" read back the wrong placement; and `Bounds::keepout` answers `Tail` for any frame off the canvas, so an element dragged off the label was reported as sitting on the tail rather than off the label. The licence cost is paid in the open: `deny.toml` names 24 crates one by one, `about.toml` points at the licence file of 11 that publish one without saying where, `packaging/third-party-notices.sh` names the 25 that publish none, and Slint's own royalty-free text - which cargo-about cannot carry, a `LicenseRef` having no canonical text - is vendored under `packaging/licenses` and appended by the script, checked on every run against the copy the crates ship. **Section 2 of that licence requires Slint's `AboutSlint` widget in an About screen, and this slice has none, so nothing may be released until it does.** See THIRD-PARTY.md.

**As built, the inspector.** The selected element's numeric and text fields for all seven element types, in a pane beside the canvas, as the fixed run of sixteen generic slots `relabel-gui`'s `ins` module fills: which fields a kind has is `Element::own_keys`'s answer and the `.slint` holds no second copy of it, no unit conversion, no range and no rule. Every field is a text box whose accessibility node carries its name, its unit, its range and its step, because the std `SpinBox` is unusable for this - its value is an `int`, it refuses an out-of-range number in silence while leaving it on screen, a minus sign cannot be typed into it, and the step it announces is `(maximum - minimum) / 100`. The named hazard from step 12's design is confirmed and is worse than stated: a one-way binding into a widget's own value is removed by the widget's first write, permanently, with no compiler diagnostic, and the stale value is what a screen reader is told too - so nothing editable is bound, and one `public function` assigns every value from a pushed model row. A keystroke stages its text in `Doc`; Return, the keyboard arriving somewhere else, an assistive technology's write, and every other command commit it, which makes one field visit one undo step and makes the toolkit's focus-then-press ordering harmless: that commit runs after the press has already selected another element and pushed its values, so a commit that carried the field's text would write one element's number onto another. It is the keyboard arriving elsewhere and not the field's blur, because a blur is also what a window losing activation and a field's own context menu deliver, and a change handler cannot tell them apart: committing on the blur wrote half a number while the operator was in another window, and selecting on the matching focus-in threw the typed digit away when they came back. The slots share one index of where the keyboard is instead, and the canvas resets it when it takes the keyboard. While the pointer is down on an element a field edit waits for the release and lands as the step after the drag's, so undo never passes through the drag without the edit and a cancelled drag cannot take the edit away. Typing clears the status line, so the same refusal made twice is a change the live region announces, and an empty optional field's meaning is the placeholder of the node a reader lands on. The fields are pushed after every outcome including `Effect::Nothing`, because a refused edit and a placement clamped onto the value the element already held are exactly the cases where a field holds something the document does not. A typed placement is clamped by the same `geom::keep_reachable` the pointer obeys and the clamp is said out loud; `geom::reach` is split out so the interval the field announces is the interval the clamp enforces. Open value sets - an unknown font family, symbology, alignment, weight, rasterisation, image fit, halftone or DataMatrix size - are edited as their own spelling and kept exactly as typed, with a note naming what this build can draw, because a chooser's only way to show a foreign value is to offer to replace it and that is the one way this slice could lose one. F6 cycles the two panes and Up and Down step a number field, both through capturing `FocusScope`s that refuse Tab focus and therefore cost no stop. F6 reads which pane has the keyboard from focus rather than from a toggle, which Tab, Shift+Tab and a click all left stale. F6 back leaves the keyboard on the canvas surface, where the shell still sends the arrows to the selected element. Ctrl+Z, Ctrl+Shift+Z in either case of the letter, and Ctrl+Y are captured beside F6 so undo and redo work from inside a field. Both panes live in one `Designer` shell that the application window and the test probe each wrap, so the keyboard paths the tests drive are the ones the application runs. The canvas is anchored at the label's start inside its column rather than centred: a child with a position is left out of its column's size, which is what keeps a 1280-pixel canvas from pushing the pane out of the window, and with a pane beside it a centred canvas lost the reference label's leftmost element off the view. The rightmost elements of a wide label are out of view instead until there is a fit or zoom control, and an element out of view is still editable through the pane once it is selected. `ui/canvas.slint` changed in two places, neither of them the pane's business. An element's zero-size focus scope sits on the part of its frame that is on the canvas rather than at the frame's origin, because a negative feed the field accepts had put that point outside the column's clip and taken the element out of the Tab order with 39 of its 40 dots in view. The surface also says when it takes the keyboard, which is the one way out of the pane that selects nothing. The canvas probes are unchanged. `Effect` gained a join, because a commit flushed by a selection is a row change and a selection at once, and the arm that drew only the row would have left the border on the element the operator had just left. `a11y::say` now neutralises the two quote marks inside an element's name; `parse_say` already read a sentence back to its last closing quote, so this keeps the sentence's delimiters unambiguous rather than fixing a misread. It does not set a flag, resize, re-import artwork, show preflight's issues beside the selection, or fix the canvas's clipping - which it makes worse by its own width, and which a `ScrollView` must not be used for: a Flickable whose viewport exceeds its geometry takes the grab eight logical pixels into a drag, and it would not restore the accessibility tree, which is gated on plain visibility rather than on clipping-by-Flickable. Still owed a launch on the macOS and Windows test machines: `TextInput` selects its contents on focus-in only for Tab navigation and not at all on Apple, which is a behavioural fork no test here can see.

---

## Print tests, and exactly what each settles

Nothing above blocks on hardware to *compile and pass tests*. These settle defaults that are currently placeholders. Each is one label.

| # | Print | Settles | Gates |
|---|---|---|---|
| **P1** | The step-1 ruler: full-head-width bar plus a mm scale along the feed, rendered longer than the label so the paper runs out mid-scale. | The label length, and whether `max_print_height_mm` is a firmware ceiling or a UI limit. | The preset table and the designer's default canvas (DESIGN-GAPS #7). |
| **P2** | One label, the same word at thresholds 96/112/128/144/160, read back under magnification. Plus a step wedge of flat patches at levels 16…240, Floyd–Steinberg, **no tone curve**, at densities 1/2/3. | The binarisation threshold default and the 256-entry tone LUT — or that no curve is needed. | Step 3's defaults. Density and dot gain are the same knob from the paper's point of view, hence three densities. |
| **P3** | The same photograph as FS / Atkinson / Bayer-8 / plain threshold. Then a 50% checkerboard beside a 50% horizontal-stripe pattern at identical nominal coverage. | Which kernel is the default. The second half tests thermal *history* — if the stripes are denser than the checkerboard, printed tone depends on scan order and no static tone curve fixes it. | Step 3's default halftone. Measurement says Atkinson puts 0.0% of mid-grey dots as isolated singles against Floyd–Steinberg's 90.9%; whether that reads as "FS mid-greys close up into mud" is a paper question. |
| **P4** | The same Code 128 payload at X = 1, 2, 3 and 4 dots on one label. Scan each with a phone camera and with a laser/CCD gun. | The minimum module width, in ladder orientation, on this head. | Step 4's `module_dots` default and the validator's clamp. Currently inferred from the 10-mil industry rule plus a head-vs-motor argument, not measured. |
| **P5** | V1 QR at 3 dots, V5 at 2 dots, and DataMatrix `Rect16x48` at 5 dots, on one 40 × 12 mm label. | Whether a 2-dot module scans, and therefore whether DataMatrix should be the *recommended* 2D symbology rather than an option. | Step 5's defaults. |
| **P6** | Six conditions — unhinted+AA+t128 / unhinted+noAA / mono-hinted+noAA / mono-hinted+AA+t128 / `Engine::Auto` / ttfautohint interpreter — at 12/14/16/18/20/24 ppem, at three densities. Judge on whether `0`/`O` and `1`/`l`/`I` stay distinguishable at the smallest legible size. | The text pipeline. | Step 6. This is the one that decides whether small text is legible at all, and it is not answerable by argument. |
| **P7** | Ruled lines 1, 2 and 3 dots wide, photographed under magnification, measured. | Thermal dot gain — does a nominal 1-dot stem print as ~1.4 dots? | Feeds P2's tone curve *and* P4's module width. Cheapest test here; run it first. |
| **P8** | A 1 mm bar hard against each of the four label edges. | Which edge the vendor's `blindZone: "1.0|0.0|0.0|0.0"` refers to. | The canvas inset. 1 mm is 8 dots — 8% of a 96-dot head. |
| **P9** | One fold-over flag, `repeat: "flip"`, with a mark that is asymmetric along the feed, read from both sides. | Whether the reader turns a fold-over tag end-over-end — a 180° turn, which is what ships — or about the crease, which wants `Copy`. | Nothing in the code: step 10 ships the plan's 180°. It settles whether `Flip` is the right default for this stock, and whether the tail lands where the author measured it. |

Run **P7** and **P1** first; they are the cheapest and they feed the others.

---

## What is not in this plan, deliberately

- **Threading, cancellation and the tokio/Slint boundary.** A `relabel-core`/`relabel-gui` concern. Worth noting one finding while it is fresh: `job::print` (`job.rs:118-124`) has no cancel parameter and `one_job` (`job.rs:183-188`) sends `PrintEnd` unconditionally *after* `run` returns — so `JoinHandle::abort()` drops the future mid-await and the paper never retracts. Cancellation must be cooperative, a `watch::Receiver<bool>` checked next to `check_deadline`. That is a `relabel-core` change and belongs on its own ticket.
- **PDF417.** `pdf417` 0.2.1 does not compile on stable (`#![feature(const_mut_refs)]`). Beyond the crate, the symbology is the wrong shape: a 2-column, 10-row symbol at X = 2 dots is 214 × 68 of a 320 × 96 label for about 30 characters — worse than a V5 QR and far worse than DataMatrix `Rect16x48`.
- **Micro QR.** Blocked on a live upstream panic. Revisit if someone verifies a `saturating_sub` patch against the ISO 18004 Annex I M1/M3 vectors — the crate ships no such test, and its only Micro test uses M2, whose capacities happen to be multiples of 8, which is exactly why the bug survived.
- **`rxing`.** Apache-2.0 only, 24 transitive crates including `regex` and `chrono`, and an `image = "=0.25.8"` pin that would drag the workspace's `image` below what is already in `Cargo.lock`. The only thing it does that nothing else does is *decode*, which is a different feature.

## One correction to the brief

The brief says "reuse the versions Slint 1.17 already compiles — check `Cargo.lock`." The current `this workspace’s `Cargo.lock`` contains **no** slint, tiny-skia, resvg, parley, swash, rustybuzz, fontdb or skrifa — Slint is not yet a dependency of this workspace and `crates/relabel-render` has no manifest (it is declared in `[workspace.dependencies]` but is not a member). Every version above is verified against the vendored crate manifests in `the vendored crate sources under `$CARGO_HOME/registry/src/`` plus `i-slint-core-1.17.1`'s own published lock, not against this lockfile. The registry cache is *not* evidence of resolution — it holds tiny-skia at both 0.11.4 and 0.12.0, and skrifa at both 0.42.1 and 0.44.0. **Re-check the real lockfile once `slint` is added**, before treating any of these as unified.