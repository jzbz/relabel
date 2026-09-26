//! Artwork: an image file turned into dots, once, when it is imported.
//!
//! # Why at import, and never when drawing
//!
//! Decoding is where the danger is. A few bytes of PNG header can claim a
//! 100000 x 100000 image, and `image` 0.25.10 allocates what a header claims
//! before it reads a pixel; an SVG can ask resvg for a pattern tile large
//! enough to abort the process. So all of that happens here, under explicit
//! limits, on a file the user chose, and what lands in the label is a small PNG
//! already sized for its frame. Drawing a label, one from someone else
//! included, only ever unpacks that PNG ([`from_stored`]).
//!
//! # The pipeline
//!
//! 1. Decode under limits - [`MAX_SIDE`], [`MAX_PIXELS`], [`MAX_ALLOC`] - and
//!    apply the EXIF orientation, so a phone photograph is the right way up.
//! 2. Rec.601 luma from straight RGBA, composited over white paper in linear
//!    light. Never `to_luma8`: it uses Rec.709 and drops alpha, which is how a
//!    transparent PNG used to print as a black rectangle.
//! 3. Crop and resample in linear light with a triangle filter. Resampling
//!    gamma-encoded values darkens thin strokes.
//! 4. Back to sRGB codes, stored as an 8-bit grey PNG.
//!
//! # The halftone import chooses
//!
//! Written into the element, so a later change of heuristic never moves a
//! saved label. Continuous tone gets Floyd-Steinberg. Line art gets a hard
//! threshold - at 187 rather than 128 when it was resampled or has soft alpha
//! edges, because in a linear-light result a dot half covered by ink is coded
//! 187 or 188. At 128 a dot would print only when it is at least 78 percent
//! ink, and a downscaled stroke loses dots: 42 percent of them, measured at 5x.
//!
//! 187 keeps every stroke a dot wide or more, but a stroke shrunk thinner than
//! a dot covers less than half of every dot it crosses, and prints nothing at
//! 187: a 600-pixel logo drawn in 2-pixel strokes, shrunk to 80 dots, is no more
//! than about a third ink in any dot. So line art that a resample shrinks keeps
//! 187, and such strokes are drawn into its stored grey, whole and one dot wide
//! ([`join_thin_strokes`]). That is more ink than they had, since a printed line
//! cannot be thinner than a dot. They are found on the source's ink pixels
//! resampled as the artwork is: those at least half ink, and fainter ones lying
//! on a line a pixel or two across, in a mark where that line somewhere holds
//! half a pixel of ink across it, which is how a stroke anti-aliased thinner
//! than a pixel on transparency is drawn. A dot joins where that ink peaks
//! across it and falls to half within a dot or two, never across a gap between
//! two marks, and never on the soft edge of a shape. What joins is thinned to
//! one dot without breaking a line or opening a hole, and is stored at 187. A
//! pale tone holds none of the source's ink, so it never joins and stays paper.
//! A tint made of ink pixels is tone, and a dot joins only where at least half
//! its ink is a stroke's. A mark that reaches across less than a dot and a half
//! is a speck, or a dot or a clump of a dither or a screen. One that does, but
//! reaches less than 64 pixels, is a stroke only where it is longer than it is
//! wide, which a blob, a screen's round dot or a full stop is not, and where
//! fewer than 80 other marks lie within 72 pixels of it. In the small text
//! measured, anti-aliased or 1-bit with stems a pixel or wider, no letter has
//! more than 62 around it; no clump of a random dither of 2 to 30 percent ink
//! that reaches a dot and a half has fewer than 91. Shrunk 1.5, 2, 3, 4, 7.5 and
//! 15 times, random, ordered and error-diffused 1-bit dithers of 2 to 30 percent
//! join nothing, as does a random one of 5, 10 or 20 percent in eight samples of
//! each at two, three and four times. Denser 1-bit tints can still join: ordered
//! ones of 37.5 to 44 percent and random ones of 40 to 50 percent, shrunk 1.5 to
//! 4 times, run together into one mark past 64 pixels, which is then a stroke. A
//! black stroke less than half a pixel wide on transparency, straight and
//! touching no other stroke, holds less than half a pixel of ink across it and
//! is taken for a pale line, so the stems of l, i and j in a thin face, 6 dots
//! to the x-height at one and a half and two times, and a lone rule 0.3 to 0.45
//! pixels wide, are lost; curved or cornered strokes of that width print. A
//! stroke a dot or wider already prints at 187 and keeps its width.
//! Floyd-Steinberg already prints a photograph's tone by diffusion, an SVG is
//! drawn at the size it lands rather than resampled, and an enlargement shrinks
//! nothing, so none of them is touched.

use crate::document::{Common, ImageEl, ImageFit, ImageHalftone};
use crate::srgb::{from_linear, to_linear};
use crate::target::Dpmm;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngDecoder, PngEncoder};
use image::codecs::webp::WebPDecoder;
use image::error::LimitErrorKind;
use image::imageops::{self, FilterType};
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, GenericImageView, ImageBuffer, ImageDecoder, ImageEncoder,
    ImageError, ImageFormat, ImageReader, Limits, Luma, LumaA, Pixel, Rgba,
};
use std::io::Cursor;

/// The largest file import reads, in bytes.
pub const MAX_INPUT: usize = 64 << 20;
/// The longest side a raster may declare. Checked on the header, before any
/// pixel is allocated.
pub const MAX_SIDE: u32 = 16_384;
/// The most pixels a raster may declare. Also checked on the header, because
/// the WebP decoder's own buffers ignore [`MAX_ALLOC`]. A full-resolution
/// 48-megapixel photograph is refused, and wants scaling down first.
pub const MAX_PIXELS: u64 = 40_000_000;
/// The most memory the decoder's own buffers may take - the decoded raster,
/// and a progressive JPEG's coefficients - and, separately, the resampling
/// buffer. Import also holds one `f32` per source pixel while it converts,
/// and for line art it shrinks two bytes more, so its peak is at most about
/// twice this plus 240 MB.
pub const MAX_ALLOC: u64 = 256 << 20;
/// Import's threshold for line art that was resampled or has soft edges: the
/// largest code whose linear light is below one half, so a dot prints when
/// more than half of it is ink. It is also the grey import stores the dots it
/// draws to keep strokes shrunk thinner than a dot whole ([`join_thin_strokes`]).
pub const LINE_ART_THRESHOLD: u8 = 187;

/// Grey dots, row-major: 0 is ink, 255 is paper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Artwork {
    /// The dots as an 8-bit grey PNG, as an image element stores them.
    ///
    /// Best compression, with whichever of two filter strategies comes out
    /// smaller: adaptive filtering suits a photograph, none at all suits a
    /// dithered or flat image, and the difference is up to a third.
    pub fn to_png(&self) -> Result<Vec<u8>, ArtworkError> {
        let mut best: Option<Vec<u8>> = None;
        for filter in [PngFilter::Adaptive, PngFilter::NoFilter] {
            let mut out = Vec::new();
            PngEncoder::new_with_quality(&mut out, CompressionType::Best, filter)
                .write_image(&self.px, self.w, self.h, ExtendedColorType::L8)
                .map_err(|e| ArtworkError::Encode(e.to_string()))?;
            if best.as_ref().is_none_or(|b| out.len() < b.len()) {
                best = Some(out);
            }
        }
        best.ok_or_else(|| ArtworkError::Encode("nothing was written".into()))
    }
}

/// What a file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Png,
    Jpeg,
    Gif,
    Bmp,
    WebP,
    Svg,
}

/// Something import did or ignored that the person importing should know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportNote {
    /// The file's EXIF orientation was applied.
    OrientationApplied,
    /// The file carries a colour profile, which was not applied: it was read
    /// as sRGB.
    IccProfileIgnored,
    /// An animated PNG or WebP: only its first frame was used.
    FirstFrameOnly,
    /// One pixel to one dot, and larger than the frame: cropped evenly.
    Cropped { from: (u32, u32), to: (u32, u32) },
    /// Fit cover, and another shape from the frame's: cropped evenly to the
    /// frame's shape, `from` the source to its middle `to` - pixels for a
    /// raster, dots of the scaled box for an SVG.
    CoverCropped { from: (u32, u32), to: (u32, u32) },
    /// An SVG with content that drew nothing at all.
    SvgDrewNothing,
}

impl std::fmt::Display for ImportNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportNote::OrientationApplied => {
                f.write_str("turned upright, as its EXIF orientation says")
            }
            ImportNote::IccProfileIgnored => {
                f.write_str("its colour profile was not applied; it was read as sRGB")
            }
            ImportNote::FirstFrameOnly => {
                f.write_str("it is animated; only its first frame was used")
            }
            ImportNote::Cropped { from, to } => write!(
                f,
                "at one pixel to a dot it is {}x{}, so it was cropped to {}x{}",
                from.0, from.1, to.0, to.1
            ),
            ImportNote::CoverCropped { from, to } => write!(
                f,
                "to cover its frame it was cropped from {}x{} to the middle {}x{}",
                from.0, from.1, to.0, to.1
            ),
            ImportNote::SvgDrewNothing => f.write_str("the SVG drew nothing"),
        }
    }
}

/// A file turned into dots, ready to become an element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub art: Artwork,
    pub source: SourceFormat,
    /// The source's own size: pixels, or for an SVG its size in dots.
    pub source_px: (u32, u32),
    pub halftone: ImageHalftone,
    pub threshold: u8,
    /// The raster was resampled to fit. Why import chose a line-art threshold
    /// of 187 rather than 128: this, or soft alpha edges.
    pub resampled: bool,
    /// How many dots import stored at [`LINE_ART_THRESHOLD`] to draw strokes
    /// shrunk thinner than a dot whole and one dot wide ([`join_thin_strokes`]);
    /// 0 for anything else.
    pub joined: u64,
    pub notes: Vec<ImportNote>,
}

impl Imported {
    /// The artwork as an image element stores it: base64 of its grey PNG.
    /// What [`Imported::into_element`] writes, for a caller that builds the
    /// element itself - one keeping an element's own halftone, say.
    pub fn stored(&self) -> Result<String, ArtworkError> {
        Ok(STANDARD.encode(self.art.to_png()?))
    }

    /// The element that draws this artwork, with its halftone and threshold
    /// written in. `path` is only a breadcrumb; nothing opens it.
    pub fn into_element(
        self,
        common: Common,
        path: String,
        fit: ImageFit,
    ) -> Result<ImageEl, ArtworkError> {
        Ok(ImageEl {
            common,
            path,
            png: self.stored()?,
            fit,
            halftone: Some(self.halftone),
            threshold: Some(self.threshold),
        })
    }
}

/// Where artwork of one size lands in a frame of another: a box `bw` x `bh`
/// whose top-left dot is at (`ox`, `oy`) in the frame. The offsets are
/// negative where the box overflows - a cover, or dots larger than the frame.
/// Centred, and where the spare or overflow is odd the extra dot is on the
/// right and bottom, as import's crops leave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub bw: u32,
    pub bh: u32,
    pub ox: i64,
    pub oy: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArtworkError {
    #[error("the file is empty")]
    Empty,
    #[error("the file is {bytes} bytes; artwork files are limited to 64 MiB")]
    InputTooLarge { bytes: usize },
    #[error("not an image this build can read: PNG, JPEG, GIF, BMP, WebP or SVG")]
    NotAnImage,
    #[error("{0}")]
    Unsupported(String),
    #[error("a side of the image is longer than 16384 pixels; scale it down first")]
    SideTooLong,
    #[error("the image is {w}x{h}, over the 40-megapixel limit; scale it down first")]
    TooManyPixels { w: u32, h: u32 },
    #[error("decoding the image would need more than 256 MiB")]
    TooMuchMemory,
    #[error("the image could not be decoded: {0}")]
    Decode(String),
    #[error("the JPEG has no end marker, so it may be cut short, with part of the picture missing")]
    JpegEndsEarly,
    #[error("the artwork could not be stored as PNG: {0}")]
    Encode(String),
    #[error("{0}")]
    Svg(SvgRefusal),
    #[error("{what} {value:?} is not one this build knows")]
    UnknownValue { what: &'static str, value: String },
    #[error("the image frame has no area")]
    EmptyFrame,
    #[error("a {w}x{h} image frame is too large to draw")]
    FrameTooLarge { w: u32, h: u32 },
    #[error("no embedded artwork (path: {path:?}); import it again")]
    NoArtwork { path: String },
    #[error("the embedded artwork is {bytes} bytes of text, more than this label could need")]
    StoredTooLong { bytes: usize },
    #[error("the embedded artwork is not valid base64")]
    BadBase64,
    #[error("the embedded artwork is not a PNG")]
    NotPng,
    #[error("the embedded artwork is {w}x{h}, too large to draw")]
    StoredTooLarge { w: u32, h: u32 },
}

/// Why an SVG was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SvgRefusal {
    #[error("the SVG is {bytes} bytes; SVG artwork is limited to 2 MiB")]
    TooLarge { bytes: usize },
    #[error("compressed SVG is not accepted; save it uncompressed")]
    Compressed,
    #[error("the SVG is not UTF-8")]
    NotUtf8,
    #[error("the SVG is not well-formed XML: {0}")]
    Xml(String),
    #[error("the SVG uses {0}, which relabel does not draw")]
    Elements(String),
    #[error("the SVG uses filter effects, which relabel does not draw")]
    Filters,
    #[error("the SVG has {0} clip paths and masks; relabel draws at most 64")]
    TooManyClips(usize),
    #[error("the SVG has {0} masked, clipped or translucent groups; relabel draws at most 64")]
    TooManyGroups(usize),
    #[error(
        "the SVG nests masked, clipped or translucent groups {0} deep; relabel draws at most 8"
    )]
    TooDeep(usize),
    #[error("a {w}x{h} SVG raster is too large; SVG artwork is limited to 262144 dots")]
    TooManyDots { w: u32, h: u32 },
    #[error("the SVG's dashed lines come to {0} dashes; relabel draws at most 1000000")]
    TooManyDashes(u64),
    #[error("the SVG could not be read: {0}")]
    Parse(String),
}

/// The most dots an SVG is drawn into: a raster over it is refused with
/// [`SvgRefusal::TooManyDots`].
pub const SVG_MAX_DOTS: u64 = 1 << 18;

/// Import artwork for a frame: `upright` is the frame as its content sees it,
/// which is what [`crate::rasterise::local_size`] returns for the element.
///
/// `dpmm` is the label's pitch, which gives an SVG's millimetres their dots.
pub fn import(
    bytes: &[u8],
    upright: (u32, u32),
    fit: &ImageFit,
    dpmm: Dpmm,
) -> Result<Imported, ArtworkError> {
    if let ImageFit::Unknown(v) = fit {
        return Err(ArtworkError::UnknownValue {
            what: "the fit",
            value: v.clone(),
        });
    }
    let (w, h) = upright;
    if w == 0 || h == 0 {
        return Err(ArtworkError::EmptyFrame);
    }
    // The frame drawing accepts on any label a real head makes: sides as for
    // text, and an area of at most MAX_LOCAL_DOTS.
    if w > crate::text::MAX_SIDE
        || h > crate::text::MAX_SIDE
        || u64::from(w) * u64::from(h) > crate::text::MAX_LOCAL_DOTS
    {
        return Err(ArtworkError::FrameTooLarge { w, h });
    }
    run(bytes, Want::Frame(upright, fit), dpmm)
}

/// Import artwork at its own size, shrunk to fit within `max` along each
/// bounded side and never enlarged. The bring-up CLI's policy, where the head
/// width is the only bound there is.
pub fn import_within(
    bytes: &[u8],
    max: (Option<u32>, Option<u32>),
    dpmm: Dpmm,
) -> Result<Imported, ArtworkError> {
    run(bytes, Want::Within(max), dpmm)
}

/// Where the artwork is to go.
#[derive(Clone, Copy)]
pub(crate) enum Want<'a> {
    Frame((u32, u32), &'a ImageFit),
    Within((Option<u32>, Option<u32>)),
}

/// The box artwork `src` occupies in `frame` under `fit`, in whole dots.
///
/// Integer arithmetic only, shared by import and drawing, and idempotent:
/// artwork imported for a frame occupies the same box when drawn in it, so
/// drawing is a straight copy. `None` when either size is empty, or the fit
/// is not one this build knows.
pub fn fit_box(src: (u32, u32), frame: (u32, u32), fit: &ImageFit) -> Option<Placement> {
    let (iw, ih) = (u64::from(src.0), u64::from(src.1));
    let (fw, fh) = (u64::from(frame.0), u64::from(frame.1));
    if iw == 0 || ih == 0 || fw == 0 || fh == 0 {
        return None;
    }
    // Each product is under 2^64: both factors are under 2^32.
    let (bw, bh) = match fit {
        ImageFit::Stretch => (fw, fh),
        ImageFit::Dots => (iw, ih),
        ImageFit::Contain if fw * ih <= fh * iw => (fw, (ih * fw / iw).max(1)),
        ImageFit::Contain => ((iw * fh / ih).max(1), fh),
        ImageFit::Cover if fw * ih >= fh * iw => (fw, (ih * fw).div_ceil(iw)),
        ImageFit::Cover => ((iw * fh).div_ceil(ih), fh),
        ImageFit::Unknown(_) => return None,
    };
    Some(Placement {
        bw: u32::try_from(bw).ok()?,
        bh: u32::try_from(bh).ok()?,
        // Division truncates toward zero, so an odd overflow is cropped one
        // dot more on the right and bottom, as import's crops are.
        ox: (fw as i64 - bw as i64) / 2,
        oy: (fh as i64 - bh as i64) / 2,
    })
}

/// Shrink to fit each bounded side, keeping the shape; never enlarge.
pub(crate) fn shrink(src: (u32, u32), max: (Option<u32>, Option<u32>)) -> (u32, u32) {
    let (mut w, mut h) = (u64::from(src.0), u64::from(src.1));
    if let Some(mw) = max.0.map(u64::from)
        && mw > 0
        && w > mw
    {
        h = (h * mw / w).max(1);
        w = mw;
    }
    if let Some(mh) = max.1.map(u64::from)
        && mh > 0
        && h > mh
    {
        w = (w * mh / h).max(1);
        h = mh;
    }
    (w as u32, h as u32)
}

/// Rec.601 luma, in integers: the weights `DESIGN.md` names, and niimblue's.
pub(crate) fn luma601(r: u8, g: u8, b: u8) -> u8 {
    ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) + 500) / 1000) as u8
}

/// What the halftone choice rests on: of the pixels at least half opaque, how
/// many are mid-tones, and whether any edge is soft.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Stats {
    counted: u64,
    mid: u64,
    partial_alpha: bool,
}

impl Stats {
    /// One pixel: its luma and its alpha.
    pub(crate) fn add(&mut self, luma: u8, alpha: u8) {
        if alpha >= 128 {
            self.counted += 1;
            if (33..=222).contains(&luma) {
                self.mid += 1;
            }
        }
        if alpha != 0 && alpha != 255 {
            self.partial_alpha = true;
        }
    }
}

/// The halftone and threshold import writes. See the module note.
pub(crate) fn classify(s: &Stats, resampled: bool, svg: bool) -> (ImageHalftone, u8) {
    // More than one pixel in fifty a mid-tone.
    if s.mid * 50 > s.counted {
        (ImageHalftone::FloydSteinberg, 128)
    } else if !svg && (resampled || s.partial_alpha) {
        (ImageHalftone::Threshold, LINE_ART_THRESHOLD)
    } else {
        // resvg's anti-aliasing is coverage in code space, not linear light,
        // so an SVG's half-covered dot is already near 128.
        (ImageHalftone::Threshold, 128)
    }
}

/// Draw strokes shrunk thinner than a dot into shrunk line art's grey `px`, `w`
/// by `h`, whole and one dot wide, at [`LINE_ART_THRESHOLD`]; returns how many
/// dots it stored there. `inked` is the source's ink pixels resampled as `px`
/// was, row-major like it: 0 where a dot holds none of them. `strokes` is the
/// same for only the pixels of strokes, leaving out specks and the dots and
/// clumps of a dither or a screen.
///
/// A dot that prints at 187 is an anchor, and never changes. A dot above 187 and
/// short of paper, 255, is a candidate where its ink `v`, at least half of it
/// a stroke's:
///
/// 1. peaks across it, in a row, a column or a diagonal: no less than the dot
///    either side, and falling to half of `v` or less within one dot on one side
///    and two on the other, so the stroke is at most two dots across once
///    filtered - seen on a dot inside the artwork on one side at least, since
///    outside is not known to be paper;
/// 2. is not a gap: along no such direction is the most ink within two dots on
///    both sides more than twice `v`, which is a space between two marks;
/// 3. is not a fringe: no neighbour that is neither an anchor nor a candidate
///    holds more than twice `v`, which is the soft edge of a shape;
/// 4. and is not a speck: some anchor or candidate is beside it.
///
/// Candidates are then thinned, least ink first, wherever that breaks no line
/// and opens no hole: first peeled from each side in turn where the band they
/// lie in runs along that side, the one of each pair across it with less ink
/// going, then any that are left. What stands is stored at 187.
///
/// Integer arithmetic throughout, and a few bytes a dot. Outside the artwork
/// holds no ink and never stands. Nothing joins unless `px`, `inked` and
/// `strokes` each hold `w` by `h` bytes.
pub fn join_thin_strokes(w: u32, h: u32, px: &mut [u8], inked: &[u8], strokes: &[u8]) -> u64 {
    let (w, h) = (i64::from(w), i64::from(h));
    let dots = px.len();
    if dots as u64 != (w * h) as u64 || inked.len() != dots || strokes.len() != dots {
        return 0;
    }
    let index = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && x < w && y < h {
            Some((y * w + x) as usize)
        } else {
            None
        }
    };
    let ink = |x: i64, y: i64| index(x, y).map_or(0, |k| u32::from(inked[k]));
    let place = |k: usize| (k as i64 % w, k as i64 / w);
    let candidate = |k: usize| {
        let v = u32::from(inked[k]);
        if px[k] <= LINE_ART_THRESHOLD || px[k] == 255 || v == 0 {
            return false;
        }
        // Mostly the ink of strokes, not of specks or a dither's dots.
        if 2 * u32::from(strokes[k]) < v {
            return false;
        }
        let (x, y) = place(k);
        let side = |(dx, dy): (i64, i64), n: i64| ink(x + n * dx, y + n * dy);
        let within = |(dx, dy): (i64, i64), n: i64| index(x + n * dx, y + n * dy).is_some();
        // The ink falls on dots inside the artwork, on one side at least: outside
        // is not known to be paper, so a texture's corner is not a peak.
        let peak = ACROSS.iter().any(|&e| {
            // To half or less at `m` dots on one side and `n` on the other.
            let falls = |m: i64, n: i64| {
                2 * side(e, m) <= v && 2 * side(e, n) <= v && (within(e, m) || within(e, n))
            };
            v >= side(e, -1) && v >= side(e, 1) && (falls(-1, 1) || falls(-1, 2) || falls(1, -2))
        });
        let gap = ACROSS.iter().any(|&e| {
            let before = side(e, -1).max(side(e, -2));
            let after = side(e, 1).max(side(e, 2));
            2 * v < before.min(after)
        });
        peak && !gap
    };
    // What each dot is: paper, a candidate still standing, or an anchor.
    const PAPER: u8 = 0;
    const STANDS: u8 = 1;
    const ANCHOR: u8 = 2;
    let mut kind: Vec<u8> = (0..dots)
        .map(|k| {
            if px[k] <= LINE_ART_THRESHOLD {
                ANCHOR
            } else if candidate(k) {
                STANDS
            } else {
                PAPER
            }
        })
        .collect();
    let ring = |k: usize| {
        let (x, y) = place(k);
        RING.iter()
            .filter_map(move |&(dx, dy)| index(x + dx, y + dy))
    };
    // The fringe of a shape, judged against the candidates as first found, so a
    // dot beside a crossing, itself a candidate, is safe.
    let first = kind.clone();
    for k in (0..dots).filter(|&k| first[k] == STANDS) {
        let v = u32::from(inked[k]);
        if ring(k).any(|j| first[j] == PAPER && 2 * v < u32::from(inked[j])) {
            kind[k] = PAPER;
        }
    }
    // Specks: nothing beside them to join.
    let first = kind.clone();
    for k in (0..dots).filter(|&k| first[k] == STANDS) {
        if ring(k).all(|j| first[j] == PAPER) {
            kind[k] = PAPER;
        }
    }
    let mut order: Vec<usize> = (0..dots).filter(|&k| kind[k] == STANDS).collect();
    order.sort_by_key(|&k| (inked[k], k));
    let stands = |kind: &[u8], x: i64, y: i64| index(x, y).is_some_and(|k| kind[k] != PAPER);
    let removable = |kind: &[u8], k: usize| {
        let (x, y) = place(k);
        can_remove(inked, k, |dx, dy| {
            index(x + dx, y + dy).filter(|&j| kind[j] != PAPER)
        })
    };
    // Peeled a side at a time: N, S, W, E, each the neighbour that must not stand.
    const SIDES: [(i64, i64); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
    loop {
        let mut changed = false;
        for (dx, dy) in SIDES {
            // The dots of a standing run through (x, y) along (ax, ay), itself
            // included, up to four.
            let run = |kind: &[u8], x: i64, y: i64, ax: i64, ay: i64| {
                let mut n = 1;
                for sign in [-1, 1] {
                    let mut s = 1;
                    while n < 4 && stands(kind, x + sign * s * ax, y + sign * s * ay) {
                        n += 1;
                        s += 1;
                    }
                }
                n
            };
            let peel: Vec<usize> = order
                .iter()
                .copied()
                .filter(|&k| {
                    let (x, y) = place(k);
                    if kind[k] != STANDS || stands(&kind, x + dx, y + dy) {
                        return false;
                    }
                    // Only where the band runs along this side, which keeps the
                    // ends of a stem two dots wide.
                    let along = run(&kind, x, y, dy.abs(), dx.abs());
                    if along < run(&kind, x, y, dx.abs(), dy.abs()) {
                        return false;
                    }
                    // And only the one of a pair across it with less ink.
                    index(x - dx, y - dy)
                        .filter(|&o| kind[o] != PAPER)
                        .is_none_or(|o| inked[k] <= inked[o])
                })
                .collect();
            for k in peel {
                if kind[k] == STANDS && removable(&kind, k) {
                    kind[k] = PAPER;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    loop {
        let mut changed = false;
        for &k in &order {
            if kind[k] == STANDS && removable(&kind, k) {
                kind[k] = PAPER;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut joined = 0;
    for (g, &k) in px.iter_mut().zip(&kind) {
        if k == STANDS {
            *g = LINE_ART_THRESHOLD;
            joined += 1;
        }
    }
    joined
}

/// Whether a standing candidate `k` can go: `standing(dx, dy)` is the neighbour
/// at that offset when it stands. With none, never - a stroke thinned to one dot
/// keeps it. With one, only a faint end, holding less than half its ink. With
/// more, only where they are one piece around it ([`ONE_PIECE`]), so removing it
/// breaks no line, and one of its four sides is open, so it opens no hole: a
/// crossing keeps its centre.
fn can_remove(inked: &[u8], k: usize, standing: impl Fn(i64, i64) -> Option<usize>) -> bool {
    let (mut bits, mut n, mut only) = (0usize, 0, k);
    for (b, &(dx, dy)) in RING.iter().enumerate() {
        if let Some(j) = standing(dx, dy) {
            bits |= 1 << b;
            n += 1;
            only = j;
        }
    }
    match n {
        0 => false,
        1 => 2 * u32::from(inked[k]) < u32::from(inked[only]),
        _ => {
            ONE_PIECE[bits]
                && [(0, -1), (0, 1), (-1, 0), (1, 0)]
                    .iter()
                    .any(|&(dx, dy)| standing(dx, dy).is_none())
        }
    }
}

/// A dot's eight neighbours, clockwise from the top left.
const RING: [(i64, i64); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
];

/// For each set of neighbours, bit `b` for `RING[b]`: whether they are one
/// piece, two of them touching when their offsets differ by at most one on each
/// axis.
const ONE_PIECE: [bool; 256] = one_piece();

const fn one_piece() -> [bool; 256] {
    let mut table = [false; 256];
    let mut bits: usize = 1;
    while bits < 256 {
        // Grow a piece from the lowest neighbour until it stops growing.
        let mut piece = bits & bits.wrapping_neg();
        loop {
            let mut grown = piece;
            let mut a = 0;
            while a < 8 {
                let mut b = 0;
                while b < 8 {
                    if piece & (1 << a) != 0
                        && bits & (1 << b) != 0
                        && (RING[a].0 - RING[b].0).abs() <= 1
                        && (RING[a].1 - RING[b].1).abs() <= 1
                    {
                        grown |= 1 << b;
                    }
                    b += 1;
                }
                a += 1;
            }
            if grown == piece {
                break;
            }
            piece = grown;
        }
        table[bits] = piece == bits;
        bits += 1;
    }
    table
}

fn run(bytes: &[u8], want: Want<'_>, dpmm: Dpmm) -> Result<Imported, ArtworkError> {
    if bytes.is_empty() {
        return Err(ArtworkError::Empty);
    }
    if bytes.len() > MAX_INPUT {
        return Err(ArtworkError::InputTooLarge { bytes: bytes.len() });
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(ArtworkError::Svg(SvgRefusal::Compressed));
    }
    // Sniffed from the bytes, never taken from a file name: a JPEG saved as
    // `.png` is common, and `image::open` refuses it.
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ArtworkError::Decode(e.to_string()))?;
    match reader.format() {
        Some(format) => raster(bytes, reader, format, want),
        None if looks_like_svg(bytes) => {
            let r = crate::svg::rasterise(bytes, want, dpmm)?;
            let (halftone, threshold) = classify(&r.stats, false, true);
            Ok(Imported {
                art: r.art,
                source: SourceFormat::Svg,
                source_px: r.natural,
                halftone,
                threshold,
                resampled: false,
                joined: 0,
                notes: r.notes,
            })
        }
        None => Err(ArtworkError::NotAnImage),
    }
}

/// Markup, after an optional byte-order mark and whitespace. The SVG parser
/// decides the rest.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    b.iter().find(|c| !c.is_ascii_whitespace()) == Some(&b'<')
}

fn image_error(e: ImageError) -> ArtworkError {
    match e {
        ImageError::Limits(l) => match l.kind() {
            LimitErrorKind::DimensionError => ArtworkError::SideTooLong,
            LimitErrorKind::InsufficientMemory => ArtworkError::TooMuchMemory,
            _ => ArtworkError::Decode(l.to_string()),
        },
        ImageError::Unsupported(u) => ArtworkError::Unsupported(u.to_string()),
        other => ArtworkError::Decode(other.to_string()),
    }
}

type Lin = ImageBuffer<Luma<f32>, Vec<f32>>;
/// An image of any pixel type, as `resampled` takes one.
type Buf<P> = ImageBuffer<P, Vec<<P as Pixel>::Subpixel>>;

fn raster(
    bytes: &[u8],
    mut reader: ImageReader<Cursor<&[u8]>>,
    format: ImageFormat,
    want: Want<'_>,
) -> Result<Imported, ArtworkError> {
    let source = match format {
        ImageFormat::Png => SourceFormat::Png,
        ImageFormat::Jpeg => SourceFormat::Jpeg,
        ImageFormat::Gif => SourceFormat::Gif,
        ImageFormat::Bmp => SourceFormat::Bmp,
        ImageFormat::WebP => SourceFormat::WebP,
        other => {
            return Err(ArtworkError::Unsupported(format!(
                "{other:?} images are not supported"
            )));
        }
    };
    // Before anything is reserved: the decoder fills a missing scan with flat grey
    // rather than failing, so a file cut short would come back a grey photograph,
    // and one holding only its headers would cost a full decode first. A last scan
    // that is whole and lacks only the end marker looks the same until it is
    // decoded, so it is refused too, and the sentence says only what was found.
    if format == ImageFormat::Jpeg && jpeg_ends_early(bytes) {
        return Err(ArtworkError::JpegEndsEarly);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits.clone());
    let mut dec = reader.into_decoder().map_err(image_error)?;
    let (w, h) = dec.dimensions();
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(ArtworkError::TooManyPixels { w, h });
    }
    // The decoder above is opaque, so animation is asked of a second one that
    // reads only the header - before the raster is reserved, so it has the
    // whole budget to read that header with. A GIF's frames are not counted.
    let animated = match format {
        ImageFormat::Png => PngDecoder::with_limits(Cursor::new(bytes), limits.clone())
            .and_then(|d| d.is_apng())
            .unwrap_or(false),
        ImageFormat::WebP => WebPDecoder::new(Cursor::new(bytes))
            .map(|d| d.has_animation())
            .unwrap_or(false),
        _ => false,
    };
    // `decode()` makes this reservation; `into_decoder()`, which reading the
    // orientation needs, does not - and without it, a header alone can make
    // the decoder allocate gigabytes and abort the process.
    limits.reserve(dec.total_bytes()).map_err(image_error)?;
    // A progressive JPEG's decoder also allocates every coefficient before it
    // reads a scan, and counts none of it: a few kilobytes of header then cost
    // hundreds of megabytes.
    if format == ImageFormat::Jpeg
        && let Some(coefficients) = progressive_jpeg_bytes(bytes)
    {
        limits.reserve(coefficients).map_err(image_error)?;
    }

    let mut notes = Vec::new();
    if animated {
        notes.push(ImportNote::FirstFrameOnly);
    }
    let orientation = dec.orientation().map_err(image_error)?;
    if orientation != Orientation::NoTransforms {
        notes.push(ImportNote::OrientationApplied);
    }
    if dec.icc_profile().map_err(image_error)?.is_some() {
        notes.push(ImportNote::IccProfileIgnored);
    }
    let mut img = DynamicImage::from_decoder(dec).map_err(image_error)?;
    img.apply_orientation(orientation);

    let (iw, ih) = img.dimensions();
    let (lin, stats) = linear_grey(&img);
    drop(img);
    let src = Lin::from_raw(iw, ih, lin)
        .ok_or_else(|| ArtworkError::Decode("the pixel count did not match the size".into()))?;
    // Where the source's ink goes is wanted only if this is line art, which
    // the halftone choice resampled or not can already tell.
    let line_art = classify(&stats, true, false).0 == ImageHalftone::Threshold;
    let Shaped {
        out,
        resampled,
        inked,
    } = shape(src, want, &mut notes, line_art)?;
    let (ow, oh) = out.dimensions();
    let mut art = Artwork {
        w: ow,
        h: oh,
        px: out.into_raw().into_iter().map(from_linear).collect(),
    };
    let (halftone, threshold) = classify(&stats, resampled, false);
    let joined = match &inked {
        Some(m) if halftone == ImageHalftone::Threshold => {
            join_thin_strokes(ow, oh, &mut art.px, &m.all, &m.strokes)
        }
        _ => 0,
    };
    Ok(Imported {
        art,
        source,
        source_px: (iw, ih),
        halftone,
        threshold,
        resampled,
        joined,
        notes,
    })
}

/// Every pixel as linear light over white paper, and the statistics the
/// halftone choice needs. Straight RGBA8, one pixel at a time, so no second
/// copy of the whole image is made; a 16-bit source loses its low byte, which
/// a one-bit printer never sees.
fn linear_grey(img: &DynamicImage) -> (Vec<f32>, Stats) {
    let (w, h) = img.dimensions();
    let mut lin = Vec::with_capacity(w as usize * h as usize);
    let mut stats = Stats::default();
    for (_, _, Rgba([r, g, b, a])) in img.pixels() {
        let y = luma601(r, g, b);
        let alpha = f32::from(a) / 255.0;
        stats.add(y, a);
        lin.push(to_linear(y) * alpha + (1.0 - alpha));
    }
    (lin, stats)
}

/// The artwork shaped for where it is going.
struct Shaped {
    out: Lin,
    /// It was resampled, which the halftone choice needs.
    resampled: bool,
    /// For line art the resample shrinks on at least one side: the source's ink
    /// pixels resampled the same way.
    inked: Option<Inked>,
}

/// The source's ink pixels, weighed ([`ink_px`]), resampled as the artwork was,
/// row-major like its dots: 0 where a dot holds none of them.
struct Inked {
    /// Every ink pixel.
    all: Vec<u8>,
    /// Only the pixels of strokes, marks that reach a dot and a half and are
    /// neither round nor part of a texture, or reach 64 pixels ([`clear_specks`]).
    strokes: Vec<u8>,
}

/// Crop and resample for where the artwork is going.
fn shape(
    src: Lin,
    want: Want<'_>,
    notes: &mut Vec<ImportNote>,
    line_art: bool,
) -> Result<Shaped, ArtworkError> {
    let (iw, ih) = src.dimensions();
    let (fw, fh) = match want {
        Want::Within(max) => {
            let (w, h) = shrink((iw, ih), max);
            return resampled_art(src, w, h, line_art);
        }
        Want::Frame(frame, _) => frame,
    };
    let Want::Frame(_, fit) = want else {
        unreachable!()
    };
    match fit {
        ImageFit::Stretch => resampled_art(src, fw, fh, line_art),
        ImageFit::Contain => {
            let (bw, bh) = fit_box((iw, ih), (fw, fh), fit).map_or((fw, fh), |p| (p.bw, p.bh));
            resampled_art(src, bw, bh, line_art)
        }
        ImageFit::Cover => {
            // Crop to the frame's shape, then resample: never build the
            // covering box, which for a frame one dot deep and a square source
            // would be enormous.
            let (w, h, fw64, fh64) = (u64::from(iw), u64::from(ih), u64::from(fw), u64::from(fh));
            let (cw, ch) = if w * fh64 > h * fw64 {
                (((h * fw64 + fh64 / 2) / fh64).clamp(1, w) as u32, ih)
            } else {
                (iw, ((w * fh64 + fw64 / 2) / fw64).clamp(1, h) as u32)
            };
            if (cw, ch) != (iw, ih) {
                notes.push(ImportNote::CoverCropped {
                    from: (iw, ih),
                    to: (cw, ch),
                });
            }
            let crop = imageops::crop_imm(&src, (iw - cw) / 2, (ih - ch) / 2, cw, ch).to_image();
            resampled_art(crop, fw, fh, line_art)
        }
        ImageFit::Dots | ImageFit::Unknown(_) => {
            let whole = |out| Shaped {
                out,
                resampled: false,
                inked: None,
            };
            if iw <= fw && ih <= fh {
                return Ok(whole(src));
            }
            let (cw, ch) = (iw.min(fw), ih.min(fh));
            notes.push(ImportNote::Cropped {
                from: (iw, ih),
                to: (cw, ch),
            });
            let crop = imageops::crop_imm(&src, (iw - cw) / 2, (ih - ch) / 2, cw, ch).to_image();
            Ok(whole(crop))
        }
    }
}

/// [`resampled`], and for line art it shrinks on at least one side its ink
/// pixels resampled the same way, all of them and those of strokes alone, which
/// [`join_thin_strokes`] finds strokes shrunk thinner than a dot on. An
/// enlargement shrinks no stroke, and pixel art enlarged would otherwise gain
/// specks at its staircase corners.
fn resampled_art(src: Lin, w: u32, h: u32, line_art: bool) -> Result<Shaped, ArtworkError> {
    let (sw, sh) = src.dimensions();
    let ink_px = if line_art && (w < sw || h < sh) {
        let (mut m, faint) = ink_px(&src)
            .ok_or_else(|| ArtworkError::Decode("the pixel count did not match the size".into()))?;
        clear_specks(&mut m, w, h, faint);
        Some(m)
    } else {
        None
    };
    let (out, was) = resampled(src, w, h)?;
    let inked = match ink_px {
        Some(m) if was => {
            let m = resampled(m, w, h)?.0.into_raw();
            let (all, strokes) = m.as_chunks::<2>().0.iter().map(|&[a, s]| (a, s)).unzip();
            Some(Inked { all, strokes })
        }
        _ => None,
    };
    Ok(Shaped {
        out,
        resampled: was,
        inked,
    })
}

/// Line art's ink pixels, before they are resampled, weighed ([`ink_px`]):
/// every one in the first channel, and in the second only those of strokes
/// ([`clear_specks`]).
type InkPx = ImageBuffer<LumaA<u8>, Vec<u8>>;

/// The linear light below which a pixel is faint ink: a tenth of it ink or more.
const FAINT: f32 = 0.9;

/// Line art's ink pixels, weighed. A pixel at least half ink, below half light,
/// weighs 255. A fainter one weighs twice its ink, short of 255, where it is a
/// ridge - a line at most two pixels across, found as [`join_thin_strokes`]
/// finds one across dots - and nothing elsewhere. Those are the pixels of a
/// stroke anti-aliased thinner than a pixel, which is faint along most of its
/// length. The soft edge of a mark is no ridge, being beside darker pixels, and
/// nor is a pale fill: flat, it falls nowhere, but where it is itself a line a
/// pixel or two across.
///
/// The strokes' channel starts as which weighed pixels are half ink ([`HALF`]):
/// at least half ink alone, or with the pixel beside it across its line. A line
/// that is, is ink however it falls across the pixels - a 0.75-pixel line split
/// evenly between two rows is three eighths ink in each - and a pale line that
/// is not stays paper. The rest are [`FAINT_PX`]. Also returned: whether any
/// pixel was faint.
fn ink_px(src: &Lin) -> Option<(InkPx, bool)> {
    let (sw, sh) = src.dimensions();
    let (w, h) = (i64::from(sw), i64::from(sh));
    let v = src.as_raw();
    // How much of a pixel is ink; outside the source, none.
    let ink = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && x < w && y < h {
            1.0 - v[(y * w + x) as usize]
        } else {
            0.0
        }
    };
    // Where pixel (x, y), `d` ink, is a ridge: the most ink across its line.
    let ridge = |x: i64, y: i64, d: f32| {
        let side = |(dx, dy): (i64, i64), n: i64| ink(x + n * dx, y + n * dy);
        ACROSS
            .iter()
            .filter(|&&e| d >= side(e, -1) && d >= side(e, 1))
            .flat_map(|&e| {
                let falls = |m: i64, n: i64| 2.0 * side(e, m) <= d && 2.0 * side(e, n) <= d;
                [
                    falls(-1, 1).then_some(d),
                    falls(-1, 2).then(|| d + side(e, 1)),
                    falls(1, -2).then(|| d + side(e, -1)),
                ]
            })
            .flatten()
            .reduce(f32::max)
    };
    let mut faint = false;
    let mut px = Vec::with_capacity(2 * v.len());
    for (k, &l) in v.iter().enumerate() {
        let (a, state) = if l < 0.5 {
            (255, HALF)
        } else if l >= FAINT {
            (0, 0)
        } else {
            let (x, y, d) = (k as i64 % w, k as i64 / w, 1.0 - l);
            match ridge(x, y, d) {
                Some(across) => {
                    faint = true;
                    let a = ((510.0 * d).round() as u8).min(254);
                    (a, if across > 0.5 { HALF } else { FAINT_PX })
                }
                None => (0, 0),
            }
        };
        px.extend([a, state]);
    }
    Some((InkPx::from_raw(sw, sh, px)?, faint))
}

/// In the strokes' channel of [`InkPx`] before [`clear_specks`]: a weighed
/// pixel that is half ink, alone or across its line ([`ink_px`]).
const HALF: u8 = 254;
/// In the strokes' channel of [`InkPx`] before [`clear_specks`]: any other
/// weighed pixel.
const FAINT_PX: u8 = 255;

/// Across a line: a row, a column and both diagonals.
const ACROSS: [(i64, i64); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];

/// How far, in source pixels, a mark that reaches a dot and a half must reach to
/// be a stroke wherever it lies and whatever its shape. A shorter one is a
/// stroke only where [`clear_specks`] finds it longer than it is wide and outside
/// a texture. The letters of small text with caps of 6 to 11 dots are marks of 9
/// to 36 pixels shrunk one and a half to three times, and the longest clumps of
/// random 1-bit dithers of a quarter and three tenths ink, in squares of 120 to
/// 320 pixels, reach 21 to 44.
const STROKE_PIXELS: i64 = 64;

/// The side, in source pixels, of the cells [`clear_specks`] counts marks in.
const TEXTURE_CELL: u32 = 8;

/// How many cells on each side of a mark's own [`clear_specks`] counts the other
/// marks in: 9 by 9 cells, 72 pixels square.
const TEXTURE_CELLS: i64 = 4;

/// The fewest other marks in that square that make a mark shorter than
/// [`STROKE_PIXELS`] part of a texture. Measured shrunk 1.5 to 15 times, no
/// letter of up to five lines of small text, anti-aliased or 1-bit with stems a
/// pixel or wider, had more than 62 around it, and no clump of a random 1-bit
/// dither of 2 to 30 percent that reaches a dot and a half had fewer than 91.
const TEXTURE_MARKS: i64 = 80;

/// How far a mark must reach, in source pixels, to be a stroke whatever the
/// shrink: it bounds the pixels [`clear_specks`] holds for one mark at a time.
const SPECK_REACH: u32 = 256;

/// Clear from the strokes' channel of `m` every mark of ink pixels, 8-connected,
/// that is not a stroke: tone rather than a line. A mark whose reach on both axes
/// is short of a dot and a half of the `w` by `h` it is resampled to is a speck,
/// scan noise, or a dot or a clump of a dither or a screen. One that reaches a
/// dot and a half, and [`STROKE_PIXELS`] or [`SPECK_REACH`], is a stroke. One
/// between is a stroke only where both hold:
///
/// 1. it is longer than it is wide: its reach squared is more than twice its
///    pixels, where a square blob's is once, two side by side twice, and a round
///    dot's, a screen's or a full stop's, about 1.3 times. A mark the artwork's
///    edge cuts is judged as though it went on past that edge in mirror image, so
///    half a round dot is still round;
/// 2. it lies outside a texture: fewer than [`TEXTURE_MARKS`] other marks, of any
///    size short of a stroke's, have the middles of their bounds in the
///    [`TEXTURE_CELLS`] cells of [`TEXTURE_CELL`] pixels on each side of the cell
///    its own middle is in, counted in proportion where the artwork's edge cuts
///    that square.
///
/// What touches a stroke is part of it. Clear from both channels every mark of
/// faint pixels alone ([`ink_px`]), none of them half ink: a pale line is not a
/// stroke.
///
/// Marks are followed from their pixels that are half ink ([`HALF`]), each only
/// until it reaches as far as a stroke must or touches a mark that did, so at
/// most [`SPECK_REACH`] squared of its pixels are held at a time, and the channel
/// itself keeps the state. A mark followed whole is counted in its cell, a byte
/// for each [`TEXTURE_CELL`] squared pixels, and one between a speck and a stroke
/// is judged once every mark has been counted. A speck is followed whole, so the
/// faint pixels ([`FAINT_PX`]) still not reached beside a stroke are the
/// stroke's, and are then taken in from it, again at most that many held at a
/// time, when `faint` says [`ink_px`] weighed any.
fn clear_specks(m: &mut InkPx, w: u32, h: u32, faint: bool) {
    // In the strokes' channel while it works, besides [`HALF`] and [`FAINT_PX`]
    // not yet reached: a pixel reached, one of a stroke, and one of a mark
    // between a speck and a stroke, not yet judged. A speck's pixels go to 0.
    const SEEN: u8 = 1;
    const STROKE: u8 = 2;
    const BETWEEN: u8 = 3;
    let (sw, sh) = m.dimensions();
    let (sw64, sh64) = (i64::from(sw), i64::from(sh));
    // Whether `span` pixels of `source` reach a dot and a half of `dots`.
    let dot_and_a_half =
        |span: i64, dots: u32, source: u32| 2 * span * i64::from(dots) >= 3 * i64::from(source);
    let reaches = |span: i64, dots: u32, source: u32| {
        (dot_and_a_half(span, dots, source) && span >= STROKE_PIXELS)
            || span >= i64::from(SPECK_REACH)
    };
    let spans = |(x0, x1, y0, y1): (i64, i64, i64, i64)| {
        reaches(x1 - x0 + 1, w, sw) || reaches(y1 - y0 + 1, h, sh)
    };
    // The marks followed whole, by the cell the middle of their bounds is in.
    let (cw, ch) = (
        i64::from(sw.div_ceil(TEXTURE_CELL)),
        i64::from(sh.div_ceil(TEXTURE_CELL)),
    );
    let mut cells = vec![0u8; (cw * ch) as usize];
    let cell = |(x0, x1, y0, y1): (i64, i64, i64, i64)| {
        let side = i64::from(TEXTURE_CELL);
        ((x0 + x1) / 2 / side, (y0 + y1) / 2 / side)
    };
    // Pixel `k` in the first channel, and in the strokes'.
    let (all, at) = (|k: usize| 2 * k, |k: usize| 2 * k + 1);
    // The eight around pixel `k`, as (x, y, index).
    let around = move |k: usize| {
        let (x, y) = (k as i64 % sw64, k as i64 / sw64);
        RING.iter().filter_map(move |&(dx, dy)| {
            let (nx, ny) = (x + dx, y + dy);
            (nx >= 0 && ny >= 0 && nx < sw64 && ny < sh64).then_some((
                nx,
                ny,
                (ny * sw64 + nx) as usize,
            ))
        })
    };
    let px: &mut [u8] = m;
    let (mut stack, mut seen) = (Vec::new(), Vec::new());
    for start in 0..px.len() / 2 {
        if px[at(start)] != HALF {
            continue;
        }
        px[at(start)] = SEEN;
        stack.push(start);
        seen.push(start);
        let (x, y) = (start as i64 % sw64, start as i64 / sw64);
        let mut bounds = (x, x, y, y);
        let mut stroke = spans(bounds);
        'mark: while !stroke && let Some(k) = stack.pop() {
            for (nx, ny, j) in around(k) {
                match px[at(j)] {
                    STROKE => {
                        stroke = true;
                        break 'mark;
                    }
                    HALF | FAINT_PX => {
                        px[at(j)] = SEEN;
                        stack.push(j);
                        seen.push(j);
                        let (x0, x1, y0, y1) = bounds;
                        bounds = (x0.min(nx), x1.max(nx), y0.min(ny), y1.max(ny));
                        if spans(bounds) {
                            stroke = true;
                            break 'mark;
                        }
                    }
                    _ => {}
                }
            }
        }
        let mark = if stroke {
            STROKE
        } else {
            let (cx, cy) = cell(bounds);
            let n = &mut cells[(cy * cw + cx) as usize];
            *n = n.saturating_add(1);
            let (x0, x1, y0, y1) = bounds;
            let (across, down) = (x1 - x0 + 1, y1 - y0 + 1);
            // Cut by the artwork's edge: mirrored past it.
            let (mx, my) = (
                1 + i64::from(x0 == 0 || x1 == sw64 - 1),
                1 + i64::from(y0 == 0 || y1 == sh64 - 1),
            );
            let reach = (mx * across).max(my * down);
            let long = reach * reach > 2 * mx * my * seen.len() as i64;
            let between = dot_and_a_half(across, w, sw) || dot_and_a_half(down, h, sh);
            if between && long { BETWEEN } else { 0 }
        };
        for &k in &seen {
            px[at(k)] = mark;
        }
        stack.clear();
        seen.clear();
    }
    // Each mark between a speck and a stroke, now that every mark is counted:
    // followed whole again, and a stroke unless it lies in a texture.
    let texture = |(gx, gy): (i64, i64)| {
        let (mut marks, mut counted) = (0, 0);
        for y in (gy - TEXTURE_CELLS).max(0)..=(gy + TEXTURE_CELLS).min(ch - 1) {
            for x in (gx - TEXTURE_CELLS).max(0)..=(gx + TEXTURE_CELLS).min(cw - 1) {
                marks += i64::from(cells[(y * cw + x) as usize]);
                counted += 1;
            }
        }
        let side = 2 * TEXTURE_CELLS + 1;
        // Less the mark itself, and in proportion where the edge cuts the square.
        (marks - 1) * side * side >= TEXTURE_MARKS * counted
    };
    for start in 0..px.len() / 2 {
        if px[at(start)] != BETWEEN {
            continue;
        }
        px[at(start)] = SEEN;
        stack.push(start);
        seen.push(start);
        let (x, y) = (start as i64 % sw64, start as i64 / sw64);
        let mut bounds = (x, x, y, y);
        while let Some(k) = stack.pop() {
            for (nx, ny, j) in around(k) {
                if px[at(j)] == BETWEEN {
                    px[at(j)] = SEEN;
                    stack.push(j);
                    seen.push(j);
                    let (x0, x1, y0, y1) = bounds;
                    bounds = (x0.min(nx), x1.max(nx), y0.min(ny), y1.max(ny));
                }
            }
        }
        let mark = if texture(cell(bounds)) { 0 } else { STROKE };
        for &k in &seen {
            px[at(k)] = mark;
        }
        seen.clear();
    }
    // The faint pixels a stroke was not followed into, taken in from it. When
    // the stack is full a pixel is still taken in, and what lies beyond it is
    // found on the next pass.
    let most = (SPECK_REACH * SPECK_REACH) as usize;
    let mut again = faint;
    while again {
        again = false;
        for start in 0..px.len() / 2 {
            if px[at(start)] != STROKE {
                continue;
            }
            stack.push(start);
            while let Some(k) = stack.pop() {
                for (_, _, j) in around(k) {
                    if px[at(j)] == FAINT_PX {
                        px[at(j)] = STROKE;
                        if stack.len() < most {
                            stack.push(j);
                        } else {
                            again = true;
                        }
                    }
                }
            }
        }
    }
    for k in 0..px.len() / 2 {
        match px[at(k)] {
            STROKE => px[at(k)] = px[all(k)],
            // Faint, and never reached from ink: a pale line of its own.
            FAINT_PX => {
                px[all(k)] = 0;
                px[at(k)] = 0;
            }
            _ => {}
        }
    }
}

/// Resample in linear light with a triangle filter, unless the size is
/// already right: the artwork, or line art's ink pixels. Never
/// `imageops::thumbnail`, which adds 0.5 - half the full range - to every
/// floating-point sample.
///
/// `resize` runs its vertical pass first, into a four-channel `f32` buffer as
/// wide as the source and as tall as the result: 16 bytes a dot. A wide source
/// into a tall frame makes that gigabytes, where the same filter run the other
/// way round, on the transpose, needs a few megabytes. So the cheaper order is
/// used, and refused past [`MAX_ALLOC`] - which, with the frame and source
/// capped as they are, only an unbounded `import_within` can reach.
fn resampled<P>(src: Buf<P>, w: u32, h: u32) -> Result<(Buf<P>, bool), ArtworkError>
where
    P: Pixel + 'static,
    P::Subpixel: 'static,
{
    let (sw, sh) = src.dimensions();
    if (sw, sh) == (w, h) {
        return Ok((src, false));
    }
    let down_first = u64::from(sw) * u64::from(h);
    let across_first = u64::from(w) * u64::from(sh);
    if 16 * down_first.min(across_first) > MAX_ALLOC {
        return Err(ArtworkError::TooMuchMemory);
    }
    // Transposing copies the source, so it is only worth it when the buffer
    // it saves is large.
    if across_first < down_first && 16 * down_first > MAX_ALLOC / 4 {
        let t = transpose(&src);
        drop(src);
        let r = imageops::resize(&t, h, w, FilterType::Triangle);
        return Ok((transpose(&r), true));
    }
    Ok((imageops::resize(&src, w, h, FilterType::Triangle), true))
}

fn transpose<P: Pixel>(src: &Buf<P>) -> Buf<P> {
    let (w, h) = src.dimensions();
    ImageBuffer::from_fn(h, w, |x, y| *src.get_pixel(y, x))
}

/// Whether a JPEG's bytes run out before its end-of-image marker: its segments
/// walked by their lengths, so a thumbnail inside an APP segment is stepped over
/// whole, and each scan's entropy-coded data to the next marker that is neither a
/// stuffed zero nor a restart. Anything after the end marker is not looked at.
/// False for markers that cannot be read, which the decoder then judges itself:
/// only running out of bytes is ending early.
fn jpeg_ends_early(b: &[u8]) -> bool {
    let mut i = 2;
    loop {
        match b.get(i) {
            None => return true,
            Some(0xFF) => {}
            Some(_) => return false,
        }
        // Fill bytes before the marker.
        while b.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let Some(&marker) = b.get(i + 1) else {
            return true;
        };
        i += 2;
        match marker {
            0xD9 => return false,
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let Some(len) = b.get(i..i + 2) else {
            return true;
        };
        let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
        if len < 2 {
            return false;
        }
        i += len;
        if i > b.len() {
            return true;
        }
        if marker == 0xDA {
            // To the next marker: a zero after 0xFF is a stuffed byte, a restart
            // belongs to the scan, and a run of 0xFF is fill before a marker.
            loop {
                let Some(at) = b.get(i..).and_then(|r| r.iter().position(|&x| x == 0xFF)) else {
                    return true;
                };
                let j = i + at;
                match b.get(j + 1) {
                    None => return true,
                    Some(0x00 | 0xD0..=0xD7 | 0xFF) => i = j + 1,
                    Some(_) => {
                        i = j;
                        break;
                    }
                }
            }
        }
    }
}

/// The coefficient store a progressive JPEG's decoder allocates before it
/// reads any scan: two bytes per coefficient, for every component, over the
/// frame padded to whole 16-dot blocks - an upper bound, since subsampled
/// components need less. `None` for a baseline JPEG, which needs no store, or
/// for markers that cannot be read, which the decoder then refuses itself.
fn progressive_jpeg_bytes(b: &[u8]) -> Option<u64> {
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if matches!(marker, 0x01 | 0xD0..=0xD8) {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            if !matches!(marker, 0xC2 | 0xC6 | 0xCA | 0xCE) {
                return None;
            }
            let f = b.get(i + 4..i + 10)?;
            let h = u64::from(u16::from_be_bytes([f[1], f[2]]));
            let w = u64::from(u16::from_be_bytes([f[3], f[4]]));
            let pad = |v: u64| v.div_ceil(16) * 16;
            return Some(pad(w) * pad(h) * u64::from(f[5]) * 2);
        }
        i += 2 + len;
    }
    None
}

/// Unpack an image element's stored artwork for drawing, refusing anything
/// larger than `limit` dots.
///
/// One decoder, never a sniffed one: whatever a shared label holds reaches
/// PNG's decoder under explicit limits, and nothing else. An 8-bit grey PNG,
/// which is what import writes, is used as it is; any other PNG goes through
/// the same compositing as import.
pub fn from_stored(png: &str, limit: u64) -> Result<Artwork, ArtworkError> {
    // Room for base64 of an uncompressed PNG of `limit` grey dots, with its
    // overheads: anything longer holds more than any frame here could use.
    let max_len = 4 * (8 * limit + (1 << 20)).div_ceil(3);
    if png.len() as u64 > max_len {
        return Err(ArtworkError::StoredTooLong { bytes: png.len() });
    }
    let bytes = STANDARD.decode(png).map_err(|_| ArtworkError::BadBase64)?;
    if !bytes.starts_with(PNG_MAGIC) {
        return Err(ArtworkError::NotPng);
    }
    // The size is read from the header chunk before any decoder sees it, so a
    // refusal can say what the size was.
    if let Some((w, h)) = ihdr(&bytes) {
        let side = crate::text::MAX_SIDE;
        if w > side || h > side || u64::from(w) * u64::from(h) > limit {
            return Err(ArtworkError::StoredTooLarge { w, h });
        }
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(crate::text::MAX_SIDE);
    limits.max_image_height = Some(crate::text::MAX_SIDE);
    limits.max_alloc = Some(8 * limit);
    let dec =
        PngDecoder::with_limits(Cursor::new(&bytes[..]), limits.clone()).map_err(image_error)?;
    let (w, h) = dec.dimensions();
    if u64::from(w) * u64::from(h) > limit {
        return Err(ArtworkError::StoredTooLarge { w, h });
    }
    limits.reserve(dec.total_bytes()).map_err(image_error)?;
    let px = match DynamicImage::from_decoder(dec).map_err(image_error)? {
        DynamicImage::ImageLuma8(g) => g.into_raw(),
        other => linear_grey(&other).0.into_iter().map(from_linear).collect(),
    };
    Ok(Artwork { w, h, px })
}

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The size a PNG's header chunk declares: the eight-byte signature, then
/// IHDR's length and tag, then its width and height, 24 bytes in all. `None`
/// when those bytes are not there or are not a PNG's.
fn ihdr(bytes: &[u8]) -> Option<(u32, u32)> {
    let b = bytes.get(..24)?;
    if !b.starts_with(PNG_MAGIC) || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
    Some((w, h))
}

/// The size stored artwork declares, read from its PNG header alone: no pixel
/// is decoded, so it is cheap enough for a row rebuilt on every drag move.
///
/// The header's 24 bytes are exactly the first 32 base64 characters. They are
/// taken as bytes, never as a `&str` slice, which would panic inside a
/// multi-byte character of a hand-edited value. `None` for anything that does
/// not begin as a PNG does; [`from_stored`] still decides whether it draws.
pub fn stored_size(png: &str) -> Option<(u32, u32)> {
    let head = STANDARD.decode(png.as_bytes().get(..32)?).ok()?;
    ihdr(&head)
}
