//! Artwork: an image file turned into dots, once, when it is imported.
//!
//! # Why at import, and never when drawing
//!
//! Decoding is where the danger is. A few bytes of PNG header can claim a
//! 100000 x 100000 image, and `image` 0.25.10 allocates what a header claims
//! before it reads a pixel; an SVG can ask resvg for a filter region or a
//! pattern tile large enough to abort the process. So all of that happens
//! here, under explicit limits, on a file the user chose, and what lands in the
//! label is a small PNG already sized for its frame. Drawing a label, one from
//! someone else included, only ever unpacks that PNG ([`from_stored`]).
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
    ImageError, ImageFormat, ImageReader, Limits, Luma, Rgba,
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
/// so its peak is at most about twice this plus 160 MB.
pub const MAX_ALLOC: u64 = 256 << 20;
/// Import's threshold for line art that was resampled or has soft edges: the
/// largest code whose linear light is below one half, so a dot prints when
/// more than half of it is ink.
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
    pub notes: Vec<ImportNote>,
}

impl Imported {
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
            png: STANDARD.encode(self.art.to_png()?),
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
    #[error("the SVG could not be read: {0}")]
    Parse(String),
}

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
    let (out, resampled) = shape(src, want, &mut notes)?;
    let (ow, oh) = out.dimensions();
    let art = Artwork {
        w: ow,
        h: oh,
        px: out.into_raw().into_iter().map(from_linear).collect(),
    };
    let (halftone, threshold) = classify(&stats, resampled, false);
    Ok(Imported {
        art,
        source,
        source_px: (iw, ih),
        halftone,
        threshold,
        resampled,
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
        stats.add(y, a);
        let a = f32::from(a) / 255.0;
        lin.push(to_linear(y) * a + (1.0 - a));
    }
    (lin, stats)
}

/// Crop and resample for where the artwork is going. Returns whether it was
/// resampled, which the halftone choice needs.
fn shape(
    src: Lin,
    want: Want<'_>,
    notes: &mut Vec<ImportNote>,
) -> Result<(Lin, bool), ArtworkError> {
    let (iw, ih) = src.dimensions();
    let (fw, fh) = match want {
        Want::Within(max) => {
            let (w, h) = shrink((iw, ih), max);
            return resampled(src, w, h);
        }
        Want::Frame(frame, _) => frame,
    };
    let Want::Frame(_, fit) = want else {
        unreachable!()
    };
    match fit {
        ImageFit::Stretch => resampled(src, fw, fh),
        ImageFit::Contain => {
            let (bw, bh) = fit_box((iw, ih), (fw, fh), fit).map_or((fw, fh), |p| (p.bw, p.bh));
            resampled(src, bw, bh)
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
            let crop = imageops::crop_imm(&src, (iw - cw) / 2, (ih - ch) / 2, cw, ch).to_image();
            resampled(crop, fw, fh)
        }
        ImageFit::Dots | ImageFit::Unknown(_) => {
            if iw <= fw && ih <= fh {
                return Ok((src, false));
            }
            let (cw, ch) = (iw.min(fw), ih.min(fh));
            notes.push(ImportNote::Cropped {
                from: (iw, ih),
                to: (cw, ch),
            });
            let crop = imageops::crop_imm(&src, (iw - cw) / 2, (ih - ch) / 2, cw, ch).to_image();
            Ok((crop, false))
        }
    }
}

/// Resample in linear light with a triangle filter, unless the size is
/// already right. Never `imageops::thumbnail`, which adds 0.5 - half the full
/// range - to every floating-point sample.
///
/// `resize` runs its vertical pass first, into a four-channel `f32` buffer as
/// wide as the source and as tall as the result: 16 bytes a dot. A wide source
/// into a tall frame makes that gigabytes, where the same filter run the other
/// way round, on the transpose, needs a few megabytes. So the cheaper order is
/// used, and refused past [`MAX_ALLOC`] - which, with the frame and source
/// capped as they are, only an unbounded `import_within` can reach.
fn resampled(src: Lin, w: u32, h: u32) -> Result<(Lin, bool), ArtworkError> {
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

fn transpose(src: &Lin) -> Lin {
    let (w, h) = src.dimensions();
    Lin::from_fn(h, w, |x, y| *src.get_pixel(y, x))
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
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(ArtworkError::NotPng);
    }
    // The size is read from the header chunk before any decoder sees it, so a
    // refusal can say what the size was.
    if bytes.len() >= 24 && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
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
