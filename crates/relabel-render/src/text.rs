//! Text: laid out with parley, drawn from skrifa's outlines, placed upright.
//!
//! # From a string to ink, in three stages that never mix
//!
//! 1. **Upright, in a local frame.** The text is laid out and its glyphs filled
//!    as if the element were unrotated: a buffer as wide as the frame's reading
//!    width and as deep as the frame. Hinting, the dot-snapped glyph origins
//!    and the scan converter all see the same upright frame whatever the
//!    rotation or the printer, so a turned label is the same dots on its side.
//! 2. **Turned onto the canvas** by index remapping, which moves bytes and
//!    never recomputes one. The canvas is the label in reading orientation on
//!    every target, so the buffer is turned by the element's rotation and
//!    nothing else - the transpose a `Top` target's axis swap would need
//!    cancels against that same swap.
//! 3. **Blended** by [`Canvas::blend_coverage`], which uses tiny-skia's own
//!    arithmetic, so stage 3 matches filling the outline straight into the
//!    canvas dot for dot.
//!
//! [`Canvas::blend_coverage`]: crate::canvas::Canvas::blend_coverage
//!
//! # Hinting, measured rather than assumed
//!
//! At the 12 to 24 ppem a 96-dot label forces, a Noto Sans Condensed stem is
//! 1.0 to 2.0 dots. Unhinted, the two stems of an `n` land on different dot
//! phases and come out one and two dots wide: that asymmetry, not softness, is
//! what makes small text look broken. Measured over the three embedded faces
//! at six sizes, skrifa's autohinter with `Target::Mono` left one probe of
//! 108 asymmetric - a horizontal stroke - and every origin phase symmetric.
//! The fonts' own ttfautohint bytecode, run by skrifa's interpreter, moves no
//! x coordinate at all, so it leaves stems exactly as unhinted ones; and
//! `Engine::AutoFallback` picks that interpreter for these faces, because they
//! carry `fpgm` and `prep`. So the autohinter is chosen by name.
//!
//! Glyphs are positioned on parley's own advances, their origins rounded to
//! whole dots. The autohinter's adjusted advances differ from those by five to
//! ten percent of a line, which would break the lines parley measured.
//!
//! # Fonts
//!
//! Embedded, and nothing else: fontique's system backend is compiled out, so a
//! label rasterises identically on every machine. That makes several failures
//! silent in parley - no font, a font with no generic family mapped, a
//! character the face has no glyph for - and each is turned into an error here,
//! naming the characters where there are any to name.

use crate::coverage::Coverage;
use crate::document::{GlyphRaster, TextAlign, TextWeight};
use parley::fontique::{
    Blob, Collection, CollectionOptions, FallbackKey, FamilyId, GenericFamily, QueryFamily,
    QueryStatus, Script, SourceCache,
};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontWeight, FontWidth, Layout, LayoutContext,
    LineHeight, PositionedLayoutItem, StyleProperty,
};
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::{
    DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, Target as HintTarget,
};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::{FillRule, Mask, Path, PathBuilder, Transform};

/// Noto Sans Condensed, the default face. See `THIRD-PARTY.md`.
pub const SANS_REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Condensed.ttf");
/// Noto Sans Condensed SemiBold: at 1.5-dot stems a real bold beats a faked one.
pub const SANS_SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSans-CondensedSemiBold.ttf");
/// Noto Sans Mono Condensed, for text that must line up in columns.
pub const MONO_REGULAR: &[u8] = include_bytes!("../fonts/NotoSansMono-Condensed.ttf");

/// The family names a document may use. The empty string means the default.
pub const SANS: &str = "Noto Sans";
pub const MONO: &str = "Noto Sans Mono";

/// The smallest cap on a local frame's area, in dots. The rasteriser raises
/// it to the size of the canvas, so a frame that fits the label is never
/// refused for its area, while a frame billions of dots on a side - it came
/// from a file - is refused rather than allocated. Sides are capped
/// separately, at [`MAX_SIDE`].
pub const MAX_LOCAL_DOTS: u64 = 1 << 22;
/// The longest side of a local raster, in dots: a metre at 203 dpi. Past it
/// tiny-skia fills a mask in tiles, clipping the path at every seam, and its
/// anti-aliased filler quietly drops to hard edges; a text frame that long is
/// refused instead.
pub const MAX_SIDE: u32 = 8191;
/// The largest em size accepted, in dots: half a metre at 203 dpi, and far past
/// any label. Beyond it the glyphs' own arithmetic stops being meaningful.
pub const MAX_PPEM: f32 = 4096.0;
/// The largest line height accepted, as a multiple of the em size.
pub const MAX_LINE_HEIGHT: f32 = 100.0;
/// The longest text accepted, in characters. Layout and outlining cost grows
/// with it, and no label needs more.
pub const MAX_TEXT_CHARS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    #[error("font {index} was rejected: it has no usable family name or character map")]
    FontRejected { index: usize },
    #[error("no registered font answers for the {0} family")]
    GenericUnresolved(&'static str),
    #[error(
        "no embedded family is called {0:?}; the embedded families are \"Noto Sans\" and \"Noto Sans Mono\""
    )]
    UnknownFamily(String),
    #[error("{family} has no {weight} face embedded")]
    NoSuchWeight {
        family: &'static str,
        weight: &'static str,
    },
    #[error("{what} {value:?} is not one this build knows")]
    UnknownValue { what: &'static str, value: String },
    #[error("the {family} face has no glyph for {chars}")]
    MissingGlyphs { family: &'static str, chars: String },
    #[error("no font produced any glyphs for the text")]
    NoFaceResolved,
    #[error("the layout produced a coordinate that is not a number")]
    NonFinite,
    #[error("{0}")]
    BadParam(&'static str),
    #[error("font data could not be read: {0}")]
    Font(String),
    #[error("a glyph outline could not be drawn: {0}")]
    Draw(String),
    #[error("a {w}x{h} text raster is too large to draw")]
    FrameTooLarge { w: u32, h: u32 },
}

/// How a text element's style reaches the engine.
#[derive(Debug, Clone)]
pub struct TextStyle<'a> {
    /// A family name from the document; the empty string is the default.
    pub family: &'a str,
    /// Em size in dots.
    pub ppem: f32,
    /// Line pitch as a multiple of the em size.
    pub line_height: f32,
    pub weight: TextWeight,
    pub align: TextAlign,
    pub raster: GlyphRaster,
}

/// How the text sat in its frame. Integers only, so the render report stays
/// comparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextClip {
    /// Lines the layout produced, up to the last one with anything to draw.
    pub lines: u16,
    /// Of those, the lines whose whole line box lies within the frame's depth.
    pub lines_whole: u16,
    /// How far the widest line's advance, less its trailing whitespace, runs
    /// past the frame's reading width, rounded up to whole dots. Layout
    /// overflow, not ink: it is non-zero only when one unbreakable word is
    /// wider than the frame.
    pub overhang_dots: u32,
    /// Dots with any coverage that fell outside the frame and were cut. This
    /// catches what line boxes miss: a descender below the last line, an
    /// accent above the first.
    pub ink_clipped_dots: u32,
}

impl TextClip {
    pub fn is_clipped(&self) -> bool {
        self.lines_whole < self.lines || self.overhang_dots > 0 || self.ink_clipped_dots > 0
    }
}

/// The text laid out and rasterised upright in its local frame.
#[derive(Debug, Clone)]
pub struct LocalText {
    pub cov: Coverage,
    pub clip: TextClip,
}

/// A font collection holding only the faces it was given, and a layout
/// context. Cheap to build - microseconds, measured - so the rasteriser makes
/// one per render rather than keeping global state.
pub struct TextEngine {
    fcx: FontContext,
    lcx: LayoutContext<()>,
    faces: Vec<&'static [u8]>,
    sans_count: usize,
}

fn register(c: &mut Collection, bytes: &'static [u8], index: usize) -> Result<FamilyId, TextError> {
    // `register_fonts` has no error: an empty result is how rejected bytes -
    // unparseable, or with no family name or character map - report
    // themselves.
    let registered = c.register_fonts(Blob::new(Arc::new(bytes)), None);
    match registered.as_slice() {
        [(family, faces)] if !faces.is_empty() => Ok(*family),
        _ => Err(TextError::FontRejected { index }),
    }
}

fn resolves(fcx: &mut FontContext, generic: GenericFamily) -> bool {
    let mut query = fcx.collection.query(&mut fcx.source_cache);
    query.set_families([QueryFamily::Generic(generic)]);
    let mut found = false;
    query.matches_with(|_| {
        found = true;
        QueryStatus::Stop
    });
    found
}

/// The code points the shaper hides when a face has no glyph for them -
/// harfrust's own list (`hb/unicode.rs`, `is_default_ignorable`) - such as the
/// variation selector a phone keyboard puts after a copyright sign. Refusing
/// them would refuse text that draws exactly as meant.
fn default_ignorable(c: char) -> bool {
    matches!(u32::from(c),
        0x00AD | 0x034F | 0x061C | 0x17B4..=0x17B5 | 0x180B..=0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F
        | 0xFE00..=0xFE0F | 0xFEFF | 0xFFF0..=0xFFF8
        | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

impl TextEngine {
    /// The engine with relabel's own embedded faces.
    pub fn new() -> Result<TextEngine, TextError> {
        TextEngine::with_faces(&[SANS_REGULAR, SANS_SEMIBOLD], &[MONO_REGULAR])
    }

    /// An engine with the given faces, sans first. Every face must register
    /// and both generic families must resolve, or this fails - because the
    /// alternative is parley laying text out in no font at all: an empty
    /// layout, no error, not even a placeholder glyph.
    pub fn with_faces(
        sans: &[&'static [u8]],
        mono: &[&'static [u8]],
    ) -> Result<TextEngine, TextError> {
        // Never `FontContext::new()`: its collection enumerates system fonts.
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let mut sans_ids = Vec::new();
        for (i, bytes) in sans.iter().enumerate() {
            sans_ids.push(register(&mut collection, bytes, i)?);
        }
        let mut mono_ids = Vec::new();
        for (i, bytes) in mono.iter().enumerate() {
            mono_ids.push(register(&mut collection, bytes, sans.len() + i)?);
        }
        let sans_family = *sans_ids
            .first()
            .ok_or(TextError::GenericUnresolved("sans-serif"))?;
        let mono_family = *mono_ids
            .first()
            .ok_or(TextError::GenericUnresolved("monospace"))?;
        // Generic families start empty with system fonts off, so the default
        // style's "sans-serif" would otherwise resolve to nothing.
        collection.set_generic_families(GenericFamily::SansSerif, std::iter::once(sans_family));
        collection.set_generic_families(GenericFamily::Monospace, std::iter::once(mono_family));
        collection.append_fallbacks(
            FallbackKey::new(Script::from_bytes(*b"Latn"), None),
            std::iter::once(sans_family),
        );
        let mut fcx = FontContext {
            collection,
            source_cache: SourceCache::default(),
        };
        for (generic, name) in [
            (GenericFamily::SansSerif, "sans-serif"),
            (GenericFamily::Monospace, "monospace"),
        ] {
            if !resolves(&mut fcx, generic) {
                return Err(TextError::GenericUnresolved(name));
            }
        }
        Ok(TextEngine {
            fcx,
            lcx: LayoutContext::new(),
            faces: sans.iter().chain(mono).copied().collect(),
            sans_count: sans.len(),
        })
    }

    /// Lay text out at scale 1.0 - one layout unit is one dot - with lines
    /// broken at `width` dots.
    pub fn layout(
        &mut self,
        text: &str,
        style: &TextStyle<'_>,
        width: f32,
    ) -> Result<Layout<()>, TextError> {
        // Each of these reaches parley as garbage or worse: an infinite size or
        // line pitch hangs line breaking, a NaN width trips an assertion in
        // release builds too, a NaN size lays out at NaN coordinates, and a
        // zero size lays out as zeros without complaint.
        if !(style.ppem.is_finite() && style.ppem > 0.0 && style.ppem <= MAX_PPEM) {
            return Err(TextError::BadParam(
                "the text size must be a positive number no larger than 4096",
            ));
        }
        if !(style.line_height.is_finite()
            && style.line_height > 0.0
            && style.line_height <= MAX_LINE_HEIGHT)
        {
            return Err(TextError::BadParam(
                "the line height must be a positive number no larger than 100",
            ));
        }
        if !(width.is_finite() && width > 0.0) {
            return Err(TextError::BadParam("the frame must have a positive width"));
        }
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(TextError::BadParam(
                "the text is longer than 10000 characters",
            ));
        }
        let (generic, family) = match style.family {
            "" | SANS => (GenericFamily::SansSerif, SANS),
            MONO => (GenericFamily::Monospace, MONO),
            other => return Err(TextError::UnknownFamily(other.to_string())),
        };
        let weight = match &style.weight {
            TextWeight::Regular => FontWeight::NORMAL,
            TextWeight::SemiBold if generic == GenericFamily::Monospace => {
                return Err(TextError::NoSuchWeight {
                    family: MONO,
                    weight: "SemiBold",
                });
            }
            TextWeight::SemiBold => FontWeight::SEMI_BOLD,
            TextWeight::Unknown(v) => {
                return Err(TextError::UnknownValue {
                    what: "the weight",
                    value: v.clone(),
                });
            }
        };
        let alignment = match &style.align {
            TextAlign::Start => Alignment::Start,
            TextAlign::Center => Alignment::Center,
            TextAlign::End => Alignment::End,
            TextAlign::Unknown(v) => {
                return Err(TextError::UnknownValue {
                    what: "the alignment",
                    value: v.clone(),
                });
            }
        };
        self.check_coverage(text, generic, family)?;
        // A whole number of dots, so every line starts on the dot grid.
        let line_dots = (style.ppem * style.line_height).round().max(1.0);
        let mut b = self.lcx.ranged_builder(&mut self.fcx, text, 1.0, true);
        b.push_default(StyleProperty::FontSize(style.ppem));
        b.push_default(StyleProperty::LineHeight(LineHeight::Absolute(line_dots)));
        // Without this a normal-width "Noto Sans" registered alongside - an
        // imported font, one day - would win the match.
        b.push_default(StyleProperty::FontWidth(FontWidth::CONDENSED));
        b.push_default(generic);
        b.push_default(StyleProperty::FontWeight(weight));
        let mut layout: Layout<()> = b.build(text);
        layout.break_all_lines(Some(width));
        layout.align(alignment, AlignmentOptions::default());
        check(text, &layout, family)?;
        Ok(layout)
    }

    /// Refuse, by name, characters the style's faces have no glyph for,
    /// before parley draws them as the empty-box glyph without a word.
    ///
    /// Checked against the faces the style can reach - the sans faces for the
    /// sans family; the mono face and, through the Latin fallback, the sans
    /// faces for mono - so a symbol only the mono face has is named when set
    /// in sans. Every control character but a line break is named too: none
    /// has a glyph.
    fn check_coverage(
        &self,
        text: &str,
        generic: GenericFamily,
        family: &'static str,
    ) -> Result<(), TextError> {
        let (sans, mono) = self.faces.split_at(self.sans_count);
        let reachable: Vec<&'static [u8]> = if generic == GenericFamily::Monospace {
            mono.iter().chain(sans).copied().collect()
        } else {
            sans.to_vec()
        };
        let fonts: Vec<FontRef<'_>> = reachable
            .iter()
            .filter_map(|b| FontRef::new(b).ok())
            .collect();
        let mut missing: Vec<char> = Vec::new();
        for c in text.chars() {
            if c == '\n' || default_ignorable(c) || missing.contains(&c) {
                continue;
            }
            if c.is_control() || !fonts.iter().any(|f| f.charmap().map(c).is_some()) {
                missing.push(c);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let shown: Vec<String> = missing
            .iter()
            .take(8)
            .map(|c| format!("{c:?} (U+{:04X})", u32::from(*c)))
            .collect();
        let more = missing.len().saturating_sub(8);
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        Err(TextError::MissingGlyphs {
            family,
            chars: format!("{}{tail}", shown.join(", ")),
        })
    }

    /// Every glyph of a layout as one path in local dot coordinates, origins
    /// rounded to whole dots.
    ///
    /// One path rather than one per glyph, so that two anti-aliased glyphs
    /// that touch are unioned by the fill rule instead of blended twice.
    pub fn glyph_path(
        &self,
        layout: &Layout<()>,
        raster: &GlyphRaster,
    ) -> Result<Option<Path>, TextError> {
        self.glyph_path_fitted(layout, raster, None)
    }

    /// [`glyph_path`](Self::glyph_path), with each line's ink brought inside
    /// `[0, width)` where the line has room.
    ///
    /// Parley aligns by advance, and ink can reach past a glyph's advance - a
    /// hinted `f` or `T` by over a dot, a `j` left of its origin - so a
    /// right-aligned word ending in one would lose a column at the frame's
    /// edge in a frame with room to spare. Each line is shifted by whole dots,
    /// which keeps every hinted stem on the grid, and only when all of its ink
    /// then fits.
    fn glyph_path_fitted(
        &self,
        layout: &Layout<()>,
        raster: &GlyphRaster,
        width: Option<f32>,
    ) -> Result<Option<Path>, TextError> {
        let mut pb = PathBuilder::new();
        // One hinting instance per face, size and location - its setup is the
        // costly part - rather than one per glyph run.
        let mut hinters: HashMap<(usize, u32, u32, Vec<i16>), HintingInstance> = HashMap::new();
        for line in layout.lines() {
            let mut lb = PathBuilder::new();
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let r = run.run();
                let font = r.font();
                let data = font.data.data();
                let fref = FontRef::from_index(data, font.index)
                    .map_err(|e| TextError::Font(format!("{e:?}")))?;
                let raw_coords = r.normalized_coords();
                let coords: Vec<NormalizedCoord> = raw_coords
                    .iter()
                    .map(|&c| NormalizedCoord::from_bits(c))
                    .collect();
                let outlines = fref.outline_glyphs();
                let size = Size::new(r.font_size());
                let key = (
                    data.as_ptr() as usize,
                    font.index,
                    r.font_size().to_bits(),
                    raw_coords.to_vec(),
                );
                let hint = match raster {
                    GlyphRaster::Hinted => {
                        if !hinters.contains_key(&key) {
                            let h = HintingInstance::new(
                                &outlines,
                                size,
                                LocationRef::new(&coords),
                                HintingOptions {
                                    engine: Engine::Auto(None),
                                    target: HintTarget::Mono,
                                },
                            )
                            .map_err(|e| TextError::Draw(format!("{e:?}")))?;
                            hinters.insert(key.clone(), h);
                        }
                        hinters.get(&key)
                    }
                    GlyphRaster::Smooth => None,
                    GlyphRaster::Unknown(v) => {
                        return Err(TextError::UnknownValue {
                            what: "the raster",
                            value: v.clone(),
                        });
                    }
                };
                for g in run.positioned_glyphs() {
                    let Some(og) = outlines.get(GlyphId::new(g.id)) else {
                        continue;
                    };
                    let mut pen = Pen {
                        b: &mut lb,
                        ox: g.x.round(),
                        oy: g.y.round(),
                    };
                    // `is_pedantic` has no effect under `Engine::Auto`, which
                    // runs none of the fonts' TrueType programs. It stays off
                    // for the day the engine is the interpreter: there these
                    // fonts' programs read one control value out of range,
                    // which pedantic mode reports as an error on hundreds of
                    // non-Latin glyphs, and FreeType's semantics read as zero.
                    let drawn = match hint {
                        Some(h) => og.draw(DrawSettings::hinted(h, false), &mut pen),
                        None => og.draw(
                            DrawSettings::unhinted(size, LocationRef::new(&coords)),
                            &mut pen,
                        ),
                    };
                    drawn.map_err(|e| TextError::Draw(format!("{e:?}")))?;
                }
            }
            let Some(mut path) = lb.finish() else {
                continue;
            };
            if let (Some(w), Some(b)) = (width, path.compute_tight_bounds()) {
                // The whole-dot shifts that keep every inked dot of the line
                // inside the frame. A line with no such shift is left where
                // parley put it, so clipped text is the same dots as unclipped.
                let (lo, hi) = (-b.left().floor(), w - b.right().ceil());
                let dx = if lo <= hi { 0.0f32.clamp(lo, hi) } else { 0.0 };
                if dx != 0.0 {
                    path = match path.transform(Transform::from_translate(dx, 0.0)) {
                        Some(p) => p,
                        None => continue,
                    };
                }
            }
            pb.push_path(&path);
        }
        Ok(pb.finish())
    }

    /// Lay out and rasterise upright into a `w` x `h` local frame, refusing
    /// any raster larger than `limit` dots.
    ///
    /// The mask is grown to take in all of the glyphs' ink, and the frame then
    /// cut out of it by index. Growing is what keeps clipped text honest: a
    /// path that crosses a mask's edge has its edges chopped by tiny-skia's
    /// clipper, which moved dots up to four rows inside the edge in
    /// measurement - so a frame-sized mask would make clipped text differ from
    /// the same text unclipped, near the cut. It also lets the ink that was
    /// cut be counted.
    pub fn render_local(
        &mut self,
        text: &str,
        style: &TextStyle<'_>,
        w: u32,
        h: u32,
        limit: u64,
    ) -> Result<LocalText, TextError> {
        if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > limit {
            return Err(TextError::FrameTooLarge { w, h });
        }
        let text = normalise(text);
        let layout = self.layout(&text, style, w as f32)?;
        let mut clip = TextClip::default();
        let mut widest = 0.0f32;
        // Line boxes stack from 0 by line height. Not parley's block
        // coordinates: those are the face's ascent-to-descent box, which
        // overhangs every line box at a line height of 1.0. The counts are
        // committed only at a line with something to draw, so a trailing
        // newline, the one line parley makes for empty text, and spaces
        // wrapped past the end of a line are never reported as lost.
        let (mut top, mut seen, mut whole) = (0.0f32, 0u16, 0u16);
        for line in layout.lines() {
            let m = line.metrics();
            seen = seen.saturating_add(1);
            let bottom = top + m.line_height;
            if bottom.round() <= h as f32 {
                whole = whole.saturating_add(1);
            }
            top = bottom;
            widest = widest.max(m.advance - m.trailing_whitespace);
            // Both tests are needed: the empty line after a final newline has
            // no glyph run but a positive advance, and spaces wrapped onto a
            // line of their own are a glyph run with nothing but whitespace.
            let drawn = m.advance - m.trailing_whitespace > 0.0
                && line
                    .items()
                    .any(|i| matches!(i, PositionedLayoutItem::GlyphRun(_)));
            if drawn {
                clip.lines = seen;
                clip.lines_whole = whole;
            }
        }
        clip.overhang_dots = (widest - w as f32).ceil().max(0.0) as u32;

        let empty = LocalText {
            cov: Coverage::new(w, h),
            clip,
        };
        let Some(path) = self.glyph_path_fitted(&layout, &style.raster, Some(w as f32))? else {
            return Ok(empty);
        };
        // The grown extent covers the path's control points, not just its ink:
        // tiny-skia turns its clipper on whenever a control point leaves the
        // mask. It is worked out in f64 and refused before any integer
        // conversion, because a huge size puts glyph bounds near f32's range.
        // A mask may grow to four times the frame's own limit.
        let b = path.bounds();
        let fx0 = f64::from(b.left()).floor().min(0.0);
        let fy0 = f64::from(b.top()).floor().min(0.0);
        let fx1 = f64::from(b.right()).ceil().max(f64::from(w));
        let fy1 = f64::from(b.bottom()).ceil().max(f64::from(h));
        let (fw, fh) = (fx1 - fx0, fy1 - fy0);
        if !(fw.is_finite() && fh.is_finite())
            || fw > f64::from(MAX_SIDE)
            || fh > f64::from(MAX_SIDE)
            || fw * fh > 4.0 * limit as f64
        {
            return Err(TextError::FrameTooLarge {
                w: fw.min(f64::from(u32::MAX)) as u32,
                h: fh.min(f64::from(u32::MAX)) as u32,
            });
        }
        let (gx0, gy0) = (fx0 as i64, fy0 as i64);
        let path = if gx0 != 0 || gy0 != 0 {
            match path.transform(Transform::from_translate(-(gx0 as f32), -(gy0 as f32))) {
                Some(p) => p,
                None => return Ok(empty),
            }
        } else {
            path
        };
        let Some(mut mask) = Mask::new((fw as u32).max(1), (fh as u32).max(1)) else {
            return Ok(empty);
        };
        let anti_alias = style.raster == GlyphRaster::Smooth;
        mask.fill_path(&path, FillRule::Winding, anti_alias, Transform::identity());
        let whole = Coverage {
            w: mask.width(),
            h: mask.height(),
            data: mask.take(),
        };
        let cov = whole.crop(-gx0, -gy0, w, h);
        clip.ink_clipped_dots = (whole.inked() - cov.inked()) as u32;
        Ok(LocalText { cov, clip })
    }
}

/// Every line break to `\n`, which parley breaks on, and tabs to spaces, since
/// there are no tab stops and a tab would otherwise draw as the missing-glyph
/// box. A `\r\n` would otherwise lay out as two breaks.
fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace(
            ['\r', '\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}'],
            "\n",
        )
        .replace('\t', " ")
}

fn check(text: &str, layout: &Layout<()>, family: &'static str) -> Result<(), TextError> {
    let mut runs = 0usize;
    let mut notdef = 0usize;
    for line in layout.lines() {
        let m = line.metrics();
        if !(m.baseline.is_finite() && m.advance.is_finite() && m.offset.is_finite()) {
            return Err(TextError::NonFinite);
        }
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            runs += 1;
            for g in run.positioned_glyphs() {
                if !(g.x.is_finite() && g.y.is_finite()) {
                    return Err(TextError::NonFinite);
                }
                if g.id == 0 {
                    notdef += 1;
                }
            }
        }
    }
    // "" and "\n" legitimately produce no runs.
    if runs == 0 && text.chars().any(|c| !c.is_whitespace()) {
        return Err(TextError::NoFaceResolved);
    }
    // A backstop behind `check_coverage`, which names characters; this can
    // only count glyphs.
    if notdef > 0 {
        return Err(TextError::MissingGlyphs {
            family,
            chars: format!("{notdef} characters"),
        });
    }
    Ok(())
}

/// skrifa's outline pen into a tiny-skia path. Font space is y-up and the
/// layout's y grows downward, so y is flipped about the baseline.
struct Pen<'a> {
    b: &'a mut PathBuilder,
    ox: f32,
    oy: f32,
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.b.move_to(self.ox + x, self.oy - y)
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.b.line_to(self.ox + x, self.oy - y)
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.b
            .quad_to(self.ox + cx, self.oy - cy, self.ox + x, self.oy - y)
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.b.cubic_to(
            self.ox + c0x,
            self.oy - c0y,
            self.ox + c1x,
            self.oy - c1y,
            self.ox + x,
            self.oy - y,
        )
    }
    fn close(&mut self) {
        self.b.close()
    }
}
