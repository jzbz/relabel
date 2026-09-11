//! Getting an image file onto a print head.
//!
//! Decoding and scaling are `relabel-render`'s, through the same import as the
//! designer's image elements: the format sniffed from the bytes rather than
//! the file name, EXIF orientation applied, transparency composited over white,
//! Rec.601 luma, and resampling in linear light. What stays here is bring-up
//! policy - shrink to the head, never enlarge - and the command-line mirror of
//! the halftones.

use anyhow::{Context, Result};
use relabel_proto::{BitPlane, PrintDirection};
use relabel_render::artwork::{self, Imported};
use relabel_render::{BinariseOpts, Dpmm, GreyMap, Halftone, Zone, binarise};

/// How a grey pixel becomes black or white.
///
/// A command-line mirror of [`Halftone`]: the library cannot derive a `clap`
/// trait without depending on `clap`, and a rendering crate has no business
/// knowing about argument parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Dither {
    /// Hard cut at the threshold. Right for line art, text and barcodes, where
    /// a half-lit module is a scan failure.
    Threshold,
    /// Floyd-Steinberg. The usual choice for photographs, and the default.
    Floyd,
    /// Atkinson. Diffuses only six eighths of the error and almost never fires
    /// an isolated dot, which tends to suit thermal paper.
    Atkinson,
    /// Three-row Sierra, between the two above.
    Sierra,
    /// Ordered 8x8 dither: a regular texture, and no error travels at all.
    Bayer,
}

impl From<Dither> for Halftone {
    fn from(d: Dither) -> Halftone {
        match d {
            Dither::Threshold => Halftone::Threshold,
            Dither::Floyd => Halftone::FloydSteinberg,
            Dither::Atkinson => Halftone::Atkinson,
            Dither::Sierra => Halftone::Sierra3,
            Dither::Bayer => Halftone::Bayer8,
        }
    }
}

/// Load an image file and reduce it to a 1-bit plane sized for the print head.
///
/// `head_px` of zero means the model is unknown; nothing is scaled down in that
/// case, because inventing a width would produce a confidently wrong label.
pub fn load(
    path: &std::path::Path,
    head_px: u16,
    direction: PrintDirection,
    dpmm: Dpmm,
    dither: Dither,
    threshold: u8,
    invert: bool,
) -> Result<BitPlane> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    load_bytes(&bytes, head_px, direction, dpmm, dither, threshold, invert)
        .with_context(|| format!("importing {}", path.display()))
}

/// [`load`], from bytes already in memory. `dpmm` gives an SVG's millimetres
/// their dots.
pub fn load_bytes(
    bytes: &[u8],
    head_px: u16,
    direction: PrintDirection,
    dpmm: Dpmm,
    dither: Dither,
    threshold: u8,
    invert: bool,
) -> Result<BitPlane> {
    // Which axis of the image lies across the head depends on the model. A
    // `Left` printer - the whole 96-dot D11 family - takes a landscape strip
    // whose *height* is the label width, so that is the axis to constrain.
    //
    // This is not `BitPlane::oriented`'s job: that *rotates*, this *resizes*
    // an image larger than the head, and without it printing any large image
    // fails outright. A label's image elements carry their own frames; a file
    // printed directly has only the head to go by.
    let head = (head_px != 0).then_some(u32::from(head_px));
    let max = match direction {
        PrintDirection::Top => (head, None),
        PrintDirection::Left => (None, head),
    };
    let imported = artwork::import_within(bytes, max, dpmm)?;
    for note in &imported.notes {
        eprintln!("note: {note}");
    }
    if let Some(hint) = threshold_hint(&imported, dither, threshold) {
        eprintln!("note: {hint}");
    }

    let art = imported.art;
    let w = u16::try_from(art.w).context("image is wider than 65535 dots")?;
    let h = u16::try_from(art.h).context("image is taller than 65535 dots")?;
    let grey = GreyMap::from_px(w, h, art.px);
    let zones = [Zone {
        halftone: dither.into(),
        threshold,
        strength: 1.0,
        invert,
    }];
    Ok(binarise(&grey, &BinariseOpts::with_zones(&zones)))
}

/// Import chose a threshold of 187 for this line art - see
/// [`artwork::LINE_ART_THRESHOLD`] - and the command line asked for less,
/// which will thin its strokes: say why, and what to pass instead.
fn threshold_hint(imported: &Imported, dither: Dither, threshold: u8) -> Option<String> {
    let t = artwork::LINE_ART_THRESHOLD;
    if dither != Dither::Threshold || imported.threshold != t || threshold >= t {
        return None;
    }
    let why = if imported.resampled {
        "was resampled"
    } else {
        "has soft edges"
    };
    Some(format!(
        "this line art {why}; --threshold {t} keeps more of its strokes"
    ))
}

/// Render a plane as text, so a dry run shows what would be printed.
pub fn preview(plane: &BitPlane, max_cols: u16) -> String {
    let step = (plane.width() / max_cols.max(1)).max(1);
    let mut out = String::new();
    let mut y = 0;
    while y < plane.height() {
        let mut x = 0;
        while x < plane.width() {
            // Two rows per line of text, using half-block characters, so the
            // aspect ratio in a terminal is roughly square.
            let top = plane.get(x, y);
            let bottom = y + step < plane.height() && plane.get(x, y + step);
            out.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
            x += step;
        }
        out.push('\n');
        y += step * 2;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, codecs::jpeg::JpegEncoder, codecs::png::PngEncoder};

    fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let px: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&px, w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    }

    fn black(w: u32, h: u32) -> Vec<u8> {
        png(w, h, [0, 0, 0, 255])
    }

    /// A `Top` printer constrains the image *width* to the head.
    #[test]
    fn top_direction_fits_the_width() {
        let p = load_bytes(
            &black(400, 100),
            96,
            PrintDirection::Top,
            Dpmm::DPI_203,
            Dither::Threshold,
            128,
            false,
        )
        .unwrap();
        assert_eq!(p.width(), 96);
        assert_eq!(p.height(), 24, "aspect ratio preserved: 100 * 96 / 400");
    }

    /// A `Left` printer - the whole D11 family - constrains the *height*,
    /// because the label reads as a landscape strip.
    #[test]
    fn left_direction_fits_the_height() {
        let p = load_bytes(
            &black(400, 200),
            96,
            PrintDirection::Left,
            Dpmm::DPI_203,
            Dither::Threshold,
            128,
            false,
        )
        .unwrap();
        assert_eq!(p.height(), 96);
        assert_eq!(p.width(), 192);
    }

    /// An unknown head must not invent a width.
    #[test]
    fn an_unknown_head_does_not_rescale() {
        let p = load_bytes(
            &black(300, 40),
            0,
            PrintDirection::Top,
            Dpmm::DPI_203,
            Dither::Threshold,
            128,
            false,
        )
        .unwrap();
        assert_eq!((p.width(), p.height()), (300, 40));
    }

    #[test]
    fn polarity_and_inversion() {
        let dark = black(8, 1);
        let p = load_bytes(
            &dark,
            0,
            PrintDirection::Top,
            Dpmm::DPI_203,
            Dither::Threshold,
            128,
            false,
        )
        .unwrap();
        assert!(p.get(0, 0), "a black pixel prints");
        let p = load_bytes(
            &dark,
            0,
            PrintDirection::Top,
            Dpmm::DPI_203,
            Dither::Threshold,
            128,
            true,
        )
        .unwrap();
        assert!(!p.get(0, 0), "inverted, it does not");
    }

    /// Transparency is paper. Reading only the colour channels made a
    /// transparent PNG print as a solid black block.
    #[test]
    fn transparent_pixels_do_not_print() {
        let clear = png(8, 8, [0, 0, 0, 0]);
        let p = load_bytes(
            &clear,
            0,
            PrintDirection::Top,
            Dpmm::DPI_203,
            Dither::Threshold,
            128,
            false,
        )
        .unwrap();
        assert!((0..8).all(|y| (0..8).all(|x| !p.get(x, y))));
    }

    /// The format comes from the bytes: a JPEG saved with a `.png` name
    /// prints, where opening it by extension failed.
    #[test]
    fn a_misnamed_file_is_read_by_its_contents() {
        let px = vec![0u8; 16 * 16 * 3];
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg)
            .write_image(&px, 16, 16, image::ExtendedColorType::Rgb8)
            .unwrap();
        let path =
            std::env::temp_dir().join(format!("relabel-cli-misnamed-{}.png", std::process::id()));
        std::fs::write(&path, &jpeg).unwrap();
        let p = load(
            &path,
            0,
            PrintDirection::Top,
            Dpmm::DPI_203,
            Dither::Threshold,
            128,
            false,
        );
        let _ = std::fs::remove_file(&path);
        let p = p.unwrap();
        assert_eq!((p.width(), p.height()), (16, 16));
        assert!(p.get(8, 8));
    }

    /// The hint says why import chose 187, and only when the command line
    /// asked for less.
    #[test]
    fn the_threshold_hint_names_its_reason() {
        let big =
            artwork::import_within(&black(400, 100), (Some(96), None), Dpmm::DPI_203).unwrap();
        let hint = threshold_hint(&big, Dither::Threshold, 128).unwrap();
        assert!(hint.contains("was resampled"), "{hint}");
        assert_eq!(threshold_hint(&big, Dither::Threshold, 200), None);
        assert_eq!(threshold_hint(&big, Dither::Floyd, 128), None);
        let soft = artwork::import_within(&png(8, 8, [0, 0, 0, 128]), (None, None), Dpmm::DPI_203)
            .unwrap();
        let hint = threshold_hint(&soft, Dither::Threshold, 128).unwrap();
        assert!(hint.contains("soft edges"), "{hint}");
    }

    /// Every command-line choice maps onto a distinct library kernel.
    #[test]
    fn each_dither_maps_to_its_own_halftone() {
        use clap::ValueEnum;
        let mapped: Vec<Halftone> = Dither::value_variants()
            .iter()
            .map(|d| (*d).into())
            .collect();
        for (i, a) in mapped.iter().enumerate() {
            for b in &mapped[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
