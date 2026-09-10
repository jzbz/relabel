//! Getting an image file onto a print head.
//!
//! Loading and scaling live here because they are bring-up concerns. The
//! reduction to one bit does not: it is `relabel-render`'s, so this tool and
//! the eventual designer dither identically and there is one implementation of
//! each kernel to get right.

use anyhow::{Context, Result, bail};
use image::imageops::FilterType;
use relabel_proto::{BitPlane, PrintDirection};
use relabel_render::{BinariseOpts, GreyMap, Halftone, Zone, binarise};

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

/// Load an image and reduce it to a 1-bit plane sized for the print head.
///
/// `head_px` of zero means the model is unknown; nothing is scaled down in that
/// case, because inventing a width would produce a confidently wrong label.
pub fn load(
    path: &std::path::Path,
    head_px: u16,
    direction: PrintDirection,
    dither: Dither,
    threshold: u8,
    invert: bool,
) -> Result<BitPlane> {
    let img = image::open(path)
        .with_context(|| format!("reading {}", path.display()))?
        .to_luma8();
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        bail!("{} has no pixels", path.display());
    }

    // Which axis of the image lies across the head depends on the model. A
    // `Left` printer - the whole 96-dot D11 family - takes a landscape strip
    // whose *height* is the label width, so that is the axis to constrain.
    //
    // The render plan proposed deleting this in favour of the protocol
    // crate's `BitPlane::oriented`. That would be a mistake: `oriented`
    // *rotates*, this *resizes* an image larger than the head, and without it
    // printing any large image fails outright. It stays until image elements
    // carry their own frames and the document decides the size.
    let img = if head_px == 0 {
        img
    } else {
        let head = u32::from(head_px);
        match direction {
            PrintDirection::Top if w > head => {
                let nh = ((h as u64 * head as u64) / w as u64).max(1) as u32;
                image::imageops::resize(&img, head, nh, FilterType::Lanczos3)
            }
            PrintDirection::Left if h > head => {
                let nw = ((w as u64 * head as u64) / h as u64).max(1) as u32;
                image::imageops::resize(&img, nw, head, FilterType::Lanczos3)
            }
            _ => img,
        }
    };

    let w = u16::try_from(img.width()).context("image is wider than 65535 dots")?;
    let h = u16::try_from(img.height()).context("image is taller than 65535 dots")?;
    let grey = GreyMap::from_px(w, h, img.into_raw());
    let zones = [Zone {
        halftone: dither.into(),
        threshold,
        strength: 1.0,
        invert,
    }];
    Ok(binarise(&grey, &BinariseOpts::with_zones(&zones)))
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
    use image::{ImageBuffer, Luma};

    fn write_png(path: &std::path::Path, w: u32, h: u32, value: u8) {
        let img: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_pixel(w, h, Luma([value]));
        img.save(path).unwrap();
    }

    /// A `Top` printer constrains the image *width* to the head.
    #[test]
    fn top_direction_fits_the_width() {
        let dir = std::env::temp_dir().join("relabel-bitmap-top.png");
        write_png(&dir, 400, 100, 0);
        let p = load(&dir, 96, PrintDirection::Top, Dither::Threshold, 128, false).unwrap();
        assert_eq!(p.width(), 96);
        assert_eq!(p.height(), 24, "aspect ratio preserved: 100 * 96 / 400");
    }

    /// A `Left` printer - the whole D11 family - constrains the *height*,
    /// because the label reads as a landscape strip.
    #[test]
    fn left_direction_fits_the_height() {
        let dir = std::env::temp_dir().join("relabel-bitmap-left.png");
        write_png(&dir, 400, 200, 0);
        let p = load(
            &dir,
            96,
            PrintDirection::Left,
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
        let dir = std::env::temp_dir().join("relabel-bitmap-unknown.png");
        write_png(&dir, 300, 40, 0);
        let p = load(&dir, 0, PrintDirection::Top, Dither::Threshold, 128, false).unwrap();
        assert_eq!((p.width(), p.height()), (300, 40));
    }

    #[test]
    fn polarity_and_inversion() {
        let dark = std::env::temp_dir().join("relabel-bitmap-dark.png");
        write_png(&dark, 8, 1, 0);
        let p = load(&dark, 0, PrintDirection::Top, Dither::Threshold, 128, false).unwrap();
        assert!(p.get(0, 0), "a black pixel prints");
        let p = load(&dark, 0, PrintDirection::Top, Dither::Threshold, 128, true).unwrap();
        assert!(!p.get(0, 0), "inverted, it does not");
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
