//! Getting an image file onto a print head.
//!
//! Deliberately minimal: scale to fit, then reduce to one bit per pixel. The
//! full compositor - text, barcodes, QR, layout - is `relabel-render`'s job.
//! What is here is what bring-up needs, and it is the half that decides whether
//! a photograph comes out as a picture or as a black rectangle.

use anyhow::{Context, Result, bail};
use image::imageops::FilterType;
use relabel_proto::{BitPlane, PrintDirection};

/// How a grey pixel becomes black or white.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Dither {
    /// Hard cut at the threshold. Right for line art, text and barcodes, where
    /// a half-lit module is a scan failure.
    Threshold,
    /// Floyd-Steinberg. The usual choice for photographs.
    Floyd,
    /// Atkinson. Diffuses only three quarters of the error, which keeps more
    /// contrast and tends to suit thermal paper better than Floyd does.
    Atkinson,
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

    let (w, h) = (img.width(), img.height());
    let mut grey: Vec<i16> = img.pixels().map(|p| i16::from(p.0[0])).collect();
    let mut plane = BitPlane::new(w as u16, h as u16);

    let t = i16::from(threshold);
    for y in 0..h {
        // Serpentine: alternating the scan direction stops diffusion errors
        // building into visible diagonal streaks.
        let ltr = y % 2 == 0;
        for i in 0..w {
            let x = if ltr { i } else { w - 1 - i };
            let idx = (y * w + x) as usize;
            let old = grey[idx];
            let lit = old < t;
            let black = if invert { !lit } else { lit };
            plane.set(x as u16, y as u16, black);

            if dither == Dither::Threshold {
                continue;
            }
            let new = if lit { 0 } else { 255 };
            let err = old - new;
            let mut push = |dx: i32, dy: i32, num: i32, den: i32| {
                let nx = if ltr { x as i32 + dx } else { x as i32 - dx };
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    return;
                }
                let j = (ny as u32 * w + nx as u32) as usize;
                grey[j] = (grey[j] + (i32::from(err) * num / den) as i16).clamp(-255, 510);
            };
            match dither {
                Dither::Floyd => {
                    push(1, 0, 7, 16);
                    push(-1, 1, 3, 16);
                    push(0, 1, 5, 16);
                    push(1, 1, 1, 16);
                }
                Dither::Atkinson => {
                    // Denominator 8 over six neighbours: only six eighths of the
                    // error is diffused, which is the point of the kernel.
                    push(1, 0, 1, 8);
                    push(2, 0, 1, 8);
                    push(-1, 1, 1, 8);
                    push(0, 1, 1, 8);
                    push(1, 1, 1, 8);
                    push(0, 2, 1, 8);
                }
                Dither::Threshold => unreachable!("handled above"),
            }
        }
    }
    Ok(plane)
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
}
