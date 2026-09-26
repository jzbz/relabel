//! Import: image files to dots. Every fixture is generated in code - see
//! `common/fixtures.rs` - and every expected value is computed independently
//! of the code under test where that is possible.

#[path = "common/fixtures.rs"]
mod fixtures;

use base64::Engine;
use fixtures::*;
use relabel_render::Dpmm;
use relabel_render::artwork::{self, ArtworkError, ImportNote, Imported, SourceFormat, fit_box};
use relabel_render::document::{Common, ImageFit, ImageHalftone, RectDots};
use relabel_render::srgb;

const DPMM: Dpmm = Dpmm::DPI_203;
const FITS: [ImageFit; 4] = [
    ImageFit::Contain,
    ImageFit::Cover,
    ImageFit::Stretch,
    ImageFit::Dots,
];

fn import(bytes: &[u8], frame: (u32, u32), fit: ImageFit) -> Result<Imported, ArtworkError> {
    artwork::import(bytes, frame, &fit, DPMM)
}

/// The sRGB transfer function in double precision, straight from the
/// standard.
fn lin64(c: u8) -> f64 {
    let s = f64::from(c) / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// The code whose linear value is nearest `v`.
fn nearest_code(v: f64) -> u8 {
    (0..=255u8)
        .min_by(|&a, &b| (lin64(a) - v).abs().total_cmp(&(lin64(b) - v).abs()))
        .unwrap()
}

fn rms(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let s: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
        .sum();
    (s / a.len() as f64).sqrt()
}

/// Transparency is paper. Reading the colour channels alone made a
/// transparent PNG print as a solid black rectangle.
#[test]
fn a_transparent_png_is_paper() {
    let b = png_rgba(8, 8, |_, _| [0, 0, 0, 0]);
    let i = import(&b, (8, 8), ImageFit::Contain).unwrap();
    assert_eq!(i.art.px, vec![255; 64]);
}

/// Rec.601, not the Rec.709 `image` uses for its own grey conversions.
#[test]
fn colour_becomes_rec601_luma() {
    for (rgb, want) in [([255, 0, 0], 76), ([0, 255, 0], 150), ([0, 0, 255], 29)] {
        let b = png_rgba(1, 1, |_, _| [rgb[0], rgb[1], rgb[2], 255]);
        let i = import(&b, (1, 1), ImageFit::Dots).unwrap();
        assert_eq!(i.art.px, vec![want], "{rgb:?}");
    }
}

/// Alpha is composited over paper in linear light: half-transparent black is
/// half as much light as paper, which is code 187 or 188 - not the 127 or 128
/// that mixing the codes gives.
#[test]
fn alpha_is_composited_in_linear_light() {
    for a in [127u8, 128] {
        let b = png_rgba(1, 1, |_, _| [0, 0, 0, a]);
        let i = import(&b, (1, 1), ImageFit::Dots).unwrap();
        let want = nearest_code(1.0 - f64::from(a) / 255.0);
        assert_eq!(i.art.px, vec![want], "alpha {a}");
        assert!((187..=188).contains(&want), "{want}");
        assert_ne!(i.art.px[0], 255 - a, "not a composite of the codes");
    }
}

/// The format comes from the bytes, never from a name.
#[test]
fn every_enabled_format_decodes_by_its_contents() {
    let half = |x: u32| if x < 8 { 0u8 } else { 255 };
    for (bytes, format) in [
        (png_l8(16, 8, |x, _| half(x)), SourceFormat::Png),
        (jpeg_rgb(16, 8, |x, _| [half(x); 3]), SourceFormat::Jpeg),
        (bmp_rgb(16, 8, |x, _| [half(x); 3]), SourceFormat::Bmp),
        (
            gif_rgba(16, 8, |x, _| [half(x), half(x), half(x), 255]),
            SourceFormat::Gif,
        ),
        (
            webp_lossless(16, 8, |x, _| [half(x), half(x), half(x), 255]),
            SourceFormat::WebP,
        ),
    ] {
        let i = import(&bytes, (16, 8), ImageFit::Dots).unwrap();
        assert_eq!(i.source, format);
        assert_eq!((i.art.w, i.art.h), (16, 8), "{format:?}");
        assert!(i.art.px[16 * 4 + 1] < 64, "{format:?}: dark on the left");
        assert!(
            i.art.px[16 * 4 + 14] > 192,
            "{format:?}: light on the right"
        );
    }
    let nothing = import(b"hello, not an image", (8, 8), ImageFit::Contain);
    assert_eq!(nothing.err(), Some(ArtworkError::NotAnImage));
    assert_eq!(
        import(b"", (8, 8), ImageFit::Contain).err(),
        Some(ArtworkError::Empty)
    );
}

/// Images one pixel wide or high, into frames one dot wide or high, under
/// every fit.
#[test]
fn one_pixel_wide_and_high_images_fit_every_frame() {
    for (src, frame) in [
        ((1, 500), (1, 96)),
        ((500, 1), (96, 1)),
        ((1, 500), (96, 96)),
        ((500, 1), (96, 96)),
        ((1, 1), (96, 96)),
    ] {
        let b = png_l8(src.0, src.1, |_, _| 0);
        for fit in FITS {
            let i = import(&b, frame, fit.clone()).unwrap();
            let (w, h) = (i.art.w, i.art.h);
            assert!(
                w >= 1 && h >= 1 && w <= frame.0 && h <= frame.1,
                "{src:?} {frame:?} {fit:?}: {w}x{h}"
            );
            if matches!(fit, ImageFit::Cover | ImageFit::Stretch) {
                assert_eq!((w, h), frame, "{src:?} {fit:?} fills the frame");
            }
            assert!(
                i.art.px.iter().all(|&v| v == 0),
                "{src:?} {frame:?} {fit:?}: all ink"
            );
        }
    }
}

/// Every EXIF orientation is applied, each checked against an index remap
/// written here from the EXIF definitions.
#[test]
fn every_exif_orientation_is_applied() {
    let (w, h) = (3u32, 2u32);
    let v = |x: u32, y: u32| (40 * (y * w + x)) as u8;
    for o in 1..=8u8 {
        let b = png_with_exif(w, h, v, o);
        let i = import(&b, (8, 8), ImageFit::Dots).unwrap();
        let turned = o >= 5;
        let (ow, oh) = if turned { (h, w) } else { (w, h) };
        assert_eq!((i.art.w, i.art.h), (ow, oh), "orientation {o}");
        let want: Vec<u8> = (0..oh)
            .flat_map(|y| (0..ow).map(move |x| (x, y)))
            .map(|(x, y)| match o {
                1 => v(x, y),
                2 => v(w - 1 - x, y),
                3 => v(w - 1 - x, h - 1 - y),
                4 => v(x, h - 1 - y),
                5 => v(y, x),
                6 => v(y, h - 1 - x),
                7 => v(w - 1 - y, h - 1 - x),
                _ => v(w - 1 - y, x),
            })
            .collect();
        assert_eq!(i.art.px, want, "orientation {o}");
        assert_eq!(
            i.notes.contains(&ImportNote::OrientationApplied),
            o != 1,
            "orientation {o}"
        );
    }
}

/// What import cannot honour, it says.
#[test]
fn animation_and_colour_profiles_are_noted() {
    let i = import(&apng(4, 4), (4, 4), ImageFit::Dots).unwrap();
    assert!(
        i.notes.contains(&ImportNote::FirstFrameOnly),
        "{:?}",
        i.notes
    );
    let i = import(&png_with_icc(4, 4), (4, 4), ImageFit::Dots).unwrap();
    assert!(
        i.notes.contains(&ImportNote::IccProfileIgnored),
        "{:?}",
        i.notes
    );
    let plain = import(&png_l8(4, 4, |_, _| 0), (4, 4), ImageFit::Dots).unwrap();
    assert!(plain.notes.is_empty(), "{:?}", plain.notes);
}

/// Line art for the resampling tests: a ring, bars and a diagonal, black on
/// white, 500 pixels square.
fn logo(x: u32, y: u32) -> u8 {
    let (dx, dy) = (f64::from(x) - 250.0, f64::from(y) - 250.0);
    let r = (dx * dx + dy * dy).sqrt();
    let ring = (150.0..170.0).contains(&r);
    let bars = y > 400 && x % 40 < 4;
    let diagonal = x.abs_diff(y) < 2 && y < 380;
    if ring || bars || diagonal { 0 } else { 255 }
}

/// Triangle weights for one axis, as `image` 0.25.10 computes them
/// (`imageops/sample.rs`, `horizontal_sample`), in double precision.
fn tri_weights(n_in: usize, n_out: usize) -> Vec<(usize, Vec<f64>)> {
    let ratio = n_in as f64 / n_out as f64;
    let sratio = ratio.max(1.0);
    (0..n_out)
        .map(|o| {
            let centre = (o as f64 + 0.5) * ratio;
            let left = ((centre - sratio).floor() as i64).clamp(0, n_in as i64 - 1) as usize;
            let right =
                ((centre + sratio).ceil() as i64).clamp(left as i64 + 1, n_in as i64) as usize;
            let c = centre - 0.5;
            let mut ws: Vec<f64> = (left..right)
                .map(|i| {
                    let t = ((i as f64 - c) / sratio).abs();
                    if t < 1.0 { 1.0 - t } else { 0.0 }
                })
                .collect();
            let s: f64 = ws.iter().sum();
            ws.iter_mut().for_each(|w| *w /= s);
            (left, ws)
        })
        .collect()
}

fn resample(src: &[f64], w: usize, h: usize, nw: usize, nh: usize) -> Vec<f64> {
    let (wy, wx) = (tri_weights(h, nh), tri_weights(w, nw));
    let mut tall = vec![0.0; w * nh];
    for (oy, (top, ws)) in wy.iter().enumerate() {
        for x in 0..w {
            tall[oy * w + x] = ws
                .iter()
                .enumerate()
                .map(|(k, wt)| wt * src[(top + k) * w + x])
                .sum();
        }
    }
    let mut out = vec![0.0; nw * nh];
    for y in 0..nh {
        for (ox, (left, ws)) in wx.iter().enumerate() {
            out[y * nw + ox] = ws
                .iter()
                .enumerate()
                .map(|(k, wt)| wt * tall[y * w + left + k])
                .sum();
        }
    }
    out
}

/// Resampling runs in linear light. Against the same triangle kernel computed
/// here in double precision from the sRGB formula, import is within half a
/// level; the same kernel run on the codes, as a gamma-space resize would, is
/// over ten levels out. The logo's bars and diagonal are thinner than a dot at
/// 5x, so import draws them whole at 187, and those dots alone are left out.
#[test]
fn resampling_is_in_linear_light() {
    let b = png_l8(500, 500, logo);
    let i = import(&b, (100, 100), ImageFit::Stretch).unwrap();
    let src: Vec<f64> = (0..500)
        .flat_map(|y| (0..500).map(move |x| lin64(logo(x, y))))
        .collect();
    let reference: Vec<u8> = resample(&src, 500, 500, 100, 100)
        .into_iter()
        .map(nearest_code)
        .collect();
    let kept: Vec<usize> = (0..reference.len())
        .filter(|&k| !(i.art.px[k] == 187 && reference[k] > 187))
        .collect();
    let left = reference.len() - kept.len();
    assert!(
        left as u64 <= i.joined,
        "{left} dots differ where {} were drawn",
        i.joined
    );
    let got: Vec<u8> = kept.iter().map(|&k| i.art.px[k]).collect();
    let want: Vec<u8> = kept.iter().map(|&k| reference[k]).collect();
    let got = rms(&got, &want);
    assert!(
        got < 0.5,
        "linear-light import against the reference: {got}"
    );

    let codes: Vec<f64> = (0..500)
        .flat_map(|y| (0..500).map(move |x| f64::from(logo(x, y))))
        .collect();
    let gamma: Vec<u8> = resample(&codes, 500, 500, 100, 100)
        .into_iter()
        .map(|v| v.round() as u8)
        .collect();
    let control = rms(&gamma, &reference);
    assert!(control > 10.0, "a gamma-space resize is far off: {control}");
}

/// Why resampled line art gets a threshold of 187: a stroke exactly one dot
/// wide after a 5x downscale covers at most three quarters of any dot, so at
/// 128 - which needs 78 percent - every stroke vanishes, while at 187 each
/// keeps one dot.
#[test]
fn downscaled_line_art_keeps_its_strokes() {
    let (w, h) = (500u32, 50u32);
    let ink = |x: u32| x % 23 < 5;
    let b = png_l8(w, h, |x, _| if ink(x) { 0 } else { 255 });
    let i = import(&b, (100, 10), ImageFit::Stretch).unwrap();
    assert_eq!(
        (i.halftone.clone(), i.threshold),
        (ImageHalftone::Threshold, 187)
    );
    let exact = (0..w).filter(|&x| ink(x)).count() as f64 * f64::from(h) / 25.0;
    let at = |t: u8| i.art.px.iter().filter(|&&v| v <= t).count() as f64;
    assert!(
        (at(187) - exact).abs() <= exact * 0.05,
        "at 187: {} dots against {exact} of ink",
        at(187)
    );
    assert!(
        at(128) < exact * 0.1,
        "at 128: {} dots against {exact}",
        at(128)
    );
}

/// The verify fixture's logo: 600 pixels square, drawn only in strokes 2 pixels
/// wide - two rings, a square, a cross and twelve spokes. Shrunk into the 80 dots
/// a 40 x 12 mm label leaves, it is 7.5 pixels to a dot, so no dot of it is more
/// than about a third ink, even where strokes cross.
fn thin_logo(x: u32, y: u32) -> bool {
    let (dx, dy) = (f64::from(x) + 0.5 - 300.0, f64::from(y) + 0.5 - 300.0);
    let r = (dx * dx + dy * dy).sqrt();
    let rings = (258.0..260.0).contains(&r) || (178.0..180.0).contains(&r);
    let square = (200..402).contains(&x)
        && (200..402).contains(&y)
        && !((202..400).contains(&x) && (202..400).contains(&y));
    let cross = ((299..301).contains(&y) && (40..560).contains(&x))
        || ((299..301).contains(&x) && (40..560).contains(&y));
    let spokes = (0..12).any(|k| {
        let (s, c) = (f64::from(k) * std::f64::consts::PI / 6.0).sin_cos();
        (dy * c - dx * s).abs() < 1.0 && (180.0..260.0).contains(&(dx * c + dy * s))
    });
    rings || square || cross || spokes
}

fn thin_logo_png() -> Vec<u8> {
    png_rgba(600, 600, |x, y| {
        if thin_logo(x, y) {
            [0, 0, 0, 255]
        } else {
            [0, 0, 0, 0]
        }
    })
}

/// Of the source's pixels, the share that is ink, counted here from the drawing.
fn thin_logo_ink() -> f64 {
    let ink = (0..600)
        .flat_map(|y| (0..600).map(move |x| (x, y)))
        .filter(|&(x, y)| thin_logo(x, y))
        .count();
    ink as f64 / 360_000.0
}

/// The share of `i`'s dots that print at threshold `t`.
fn share_at(i: &Imported, t: u8) -> f64 {
    i.art.px.iter().filter(|&&v| v <= t).count() as f64 / i.art.px.len() as f64
}

/// The dots of `i` that print at threshold `t`, as (x, y), row by row.
fn printed(i: &Imported, t: u8) -> Vec<(u32, u32)> {
    let w = i.art.w as usize;
    i.art
        .px
        .iter()
        .enumerate()
        .filter(|&(_, &v)| v <= t)
        .map(|(k, _)| ((k % w) as u32, (k / w) as u32))
        .collect()
}

/// How many pieces `dots` make, each dot joining any of its eight neighbours.
fn pieces(dots: &[(u32, u32)]) -> usize {
    let set: std::collections::HashSet<(i64, i64)> = dots
        .iter()
        .map(|&(x, y)| (i64::from(x), i64::from(y)))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut n = 0;
    for &start in &set {
        if !seen.insert(start) {
            continue;
        }
        n += 1;
        let mut stack = vec![start];
        while let Some((x, y)) = stack.pop() {
            for (dx, dy) in (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (dx, dy))) {
                let next = (x + dx, y + dy);
                if set.contains(&next) && seen.insert(next) {
                    stack.push(next);
                }
            }
        }
    }
    n
}

/// How many blocks of two dots by two among `dots` are printed whole.
fn blocks(dots: &[(u32, u32)]) -> usize {
    let set: std::collections::HashSet<(u32, u32)> = dots.iter().copied().collect();
    dots.iter()
        .filter(|&&(x, y)| {
            [(x + 1, y), (x, y + 1), (x + 1, y + 1)]
                .iter()
                .all(|d| set.contains(d))
        })
        .count()
}

/// A logo drawn in strokes thinner than a dot once it is shrunk prints nothing
/// at 187, since no dot of it is half ink. Import draws those strokes into the
/// stored grey instead, whole and one dot wide, at 187: more ink than the source
/// has, since a printed line cannot be thinner than a dot, but every ring, spoke,
/// the square and the cross unbroken.
#[test]
fn a_shrunk_logo_drawn_in_thin_strokes_prints_whole_lines_one_dot_wide() {
    let source = thin_logo_ink();
    assert!((0.02..0.04).contains(&source), "{source}");
    let i = artwork::import_within(&thin_logo_png(), (Some(304), Some(80)), DPMM).unwrap();
    assert_eq!((i.art.w, i.art.h, i.resampled), (80, 80, true));
    assert_eq!(
        (i.halftone.clone(), i.threshold),
        (ImageHalftone::Threshold, 187)
    );
    assert_eq!(
        share_at(&i, 186),
        0.0,
        "no dot is half ink, and every drawn dot is stored at 187"
    );
    let dots = printed(&i, 187);
    assert_eq!(pieces(&dots), 1, "one unbroken drawing");
    assert_eq!(blocks(&dots), 0, "one dot wide");
    let got = share_at(&i, 187);
    assert!(
        (0.09..0.11).contains(&got),
        "it prints {got} against the source's {source}"
    );
    assert_eq!(i.joined, dots.len() as u64, "every printed dot was drawn");
}

/// The thin logo with a pale grey fill of `grey` beside it: a disc of radius
/// `r` in the top left corner, clear of every stroke.
fn thin_logo_beside_pale_disc(grey: u8, r: f64) -> Vec<u8> {
    png_rgba(600, 600, |x, y| {
        let (dx, dy) = (f64::from(x) + 0.5 - 40.0, f64::from(y) + 0.5 - 40.0);
        if thin_logo(x, y) {
            [0, 0, 0, 255]
        } else if dx * dx + dy * dy < r * r {
            [grey, grey, grey, 255]
        } else {
            [0, 0, 0, 0]
        }
    })
}

/// A pale tone holds none of the source's ink, so nothing is drawn into it and
/// it stays paper, and the strokes beside it print exactly as they do alone: a
/// disc of grey 225, a band of grey 230 across the logo, off-white paper of 245
/// and a black panel at alpha 127 each leave every printed dot as it was.
#[test]
fn a_pale_fill_band_panel_or_paper_under_thin_strokes_changes_no_dot_they_print() {
    let within = (Some(304), Some(80));
    let prints = |i: &Imported| i.art.px.iter().map(|&v| v <= 187).collect::<Vec<_>>();
    let base = artwork::import_within(&thin_logo_png(), within, DPMM).unwrap();
    let with = |f: fn(u32, u32) -> [u8; 4]| {
        png_rgba(600, 600, move |x, y| {
            if thin_logo(x, y) {
                [0, 0, 0, 255]
            } else {
                f(x, y)
            }
        })
    };
    let band = with(|_, y| {
        if y >= 470 {
            [230, 230, 230, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let paper = with(|_, _| [245, 245, 245, 255]);
    let panel = with(|x, y| {
        if x < 100 && y < 100 {
            [0, 0, 0, 127]
        } else {
            [0, 0, 0, 0]
        }
    });
    for (what, bytes, (x, y), grey) in [
        (
            "a grey-225 disc",
            thin_logo_beside_pale_disc(225, 34.0),
            (5, 5),
            225,
        ),
        ("a grey-230 band", band, (2, 70), 230),
        ("245 paper", paper, (2, 2), 245),
        ("a black panel at alpha 127", panel, (5, 5), 188),
    ] {
        let i = artwork::import_within(&bytes, within, DPMM).unwrap();
        assert_eq!((i.art.w, i.art.h), (80, 80), "{what}");
        assert_eq!(
            (i.halftone.clone(), i.threshold),
            (ImageHalftone::Threshold, 187),
            "{what}"
        );
        assert!(
            prints(&i) == prints(&base),
            "{what}: the printed dots moved"
        );
        assert_eq!(i.art.px[y * 80 + x], grey, "{what} keeps its grey");
    }
}

/// A stroke already a dot or wider keeps its width, and a thin one beside it is
/// drawn whole: a bar 16 pixels wide and a line 2 pixels wide, shrunk 7.5 times,
/// print the bar exactly as it prints alone and the line one dot to a row.
#[test]
fn a_thin_line_beside_a_thick_bar_is_drawn_whole_and_the_bar_keeps_its_width() {
    let within = (Some(304), Some(80));
    let draw = |line: bool| {
        png_l8(600, 600, move |x, y| {
            let bar = (100..116).contains(&x);
            let thin = line && (300..302).contains(&x);
            if (40..560).contains(&y) && (bar || thin) {
                0
            } else {
                255
            }
        })
    };
    let alone = artwork::import_within(&draw(false), within, DPMM).unwrap();
    assert_eq!((alone.threshold, alone.joined), (187, 0));
    let bar = printed(&alone, 187);
    assert!(!bar.is_empty() && bar.iter().all(|&(x, _)| (12..=16).contains(&x)));
    let both = artwork::import_within(&draw(true), within, DPMM).unwrap();
    assert_eq!(both.threshold, 187);
    let dots = printed(&both, 187);
    let near: Vec<_> = dots
        .iter()
        .copied()
        .filter(|&(x, _)| (12..=16).contains(&x))
        .collect();
    assert_eq!(near, bar, "the bar prints as it does alone");
    let line: Vec<_> = dots.iter().copied().filter(|&(x, _)| x >= 30).collect();
    assert_eq!(pieces(&line), 1, "the line is whole");
    let rows: std::collections::BTreeSet<u32> = line.iter().map(|&(_, y)| y).collect();
    assert_eq!((line.len(), rows.len()), (70, 70), "one dot to a row");
}

/// A thick shape inside a thin ring prints exactly as it does alone, while the
/// ring is drawn whole and one dot wide: raising the threshold to print the ring
/// would print the disc's soft edge too.
#[test]
fn a_thick_disc_inside_a_thin_ring_keeps_its_width_while_the_ring_is_drawn_whole() {
    let within = (Some(304), Some(80));
    let draw = |ring: bool| {
        png_l8(600, 600, move |x, y| {
            let (dx, dy) = (f64::from(x) + 0.5 - 300.0, f64::from(y) + 0.5 - 300.0);
            let r = (dx * dx + dy * dy).sqrt();
            if r < 120.0 || (ring && (258.0..260.0).contains(&r)) {
                0
            } else {
                255
            }
        })
    };
    let alone = artwork::import_within(&draw(false), within, DPMM).unwrap();
    assert_eq!((alone.threshold, alone.joined), (187, 0));
    let both = artwork::import_within(&draw(true), within, DPMM).unwrap();
    assert_eq!(both.threshold, 187);
    let near = |&(x, y): &(u32, u32)| {
        let (dx, dy) = (i64::from(x) - 40, i64::from(y) - 40);
        dx * dx + dy * dy <= 25 * 25
    };
    let dots = printed(&both, 187);
    let disc: Vec<_> = dots.iter().copied().filter(near).collect();
    assert_eq!(
        disc,
        printed(&alone, 187),
        "the disc prints as it does alone"
    );
    let ring: Vec<_> = dots.iter().copied().filter(|d| !near(d)).collect();
    assert!(ring.len() > 150, "{}", ring.len());
    assert_eq!(pieces(&ring), 1, "the ring is whole");
    assert_eq!(blocks(&ring), 0, "and one dot wide");
}

/// Hatching and a dither finer than a dot are tones, not lines: once shrunk they
/// are grey, which holds ink in every dot, and printing them would turn them
/// solid. They stay paper, as a pale fill does, while the frame around the
/// hatching, a line of its own, is drawn whole.
#[test]
fn hatching_and_a_dithered_tint_finer_than_a_dot_stay_paper() {
    let within = (Some(304), Some(80));
    let hatch = png_l8(600, 600, |x, y| {
        let outer = (20..580).contains(&x) && (20..580).contains(&y);
        let inner = (24..576).contains(&x) && (24..576).contains(&y);
        let lines = (60..540).contains(&x) && (60..540).contains(&y) && y % 3 == 0;
        if (outer && !inner) || lines { 0 } else { 255 }
    });
    let i = artwork::import_within(&hatch, within, DPMM).unwrap();
    let dots = printed(&i, i.threshold);
    let inside = |&(x, y): &(u32, u32)| (10..=70).contains(&x) && (10..=70).contains(&y);
    assert_eq!(
        dots.iter().filter(|d| inside(d)).count(),
        0,
        "at {} the hatching prints",
        i.threshold
    );
    let frame: Vec<_> = dots
        .iter()
        .copied()
        .filter(|&(x, y)| x.min(y) <= 4 || x.max(y) >= 75)
        .collect();
    assert_eq!(pieces(&frame), 1, "the frame is whole");
    let (xs, ys) = (frame.iter().map(|d| d.0), frame.iter().map(|d| d.1));
    assert!(xs.clone().min() <= Some(4) && xs.max() >= Some(75));
    assert!(ys.clone().min() <= Some(4) && ys.max() >= Some(75));

    // A 1-bit dither, a quarter ink, from a fixed sequence stepped once a pixel.
    let mut state: u64 = 12345;
    let dither: Vec<bool> = (0..600 * 600)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33).is_multiple_of(4)
        })
        .collect();
    let beside = png_rgba(600, 600, |x, y| {
        let patch = x < 150 && y < 150 && dither[(y * 600 + x) as usize];
        if thin_logo(x, y) || patch {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let i = artwork::import_within(&beside, within, DPMM).unwrap();
    let patch = printed(&i, i.threshold)
        .into_iter()
        .filter(|&(x, y)| (2..=17).contains(&x) && (2..=17).contains(&y))
        .count();
    assert_eq!(patch, 0, "at {} the dither prints", i.threshold);
}

/// Which of a 600 x 600 source's pixels are ink in a 1-bit dither of `per_mille`
/// thousandths, from a fixed sequence stepped once a pixel, row by row.
fn scattered(per_mille: u64, seed: u64) -> Vec<bool> {
    let mut state = seed;
    (0..600 * 600)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) % 1000 < per_mille
        })
        .collect()
}

/// A mark smaller than a dot on both sides is a speck, not a stroke, and a sparse
/// 1-bit dither is made of them. Shrunk, each spreads a little ink over two dots
/// or four with paper around it, which looked like a stroke thinner than a dot:
/// joined, a dither of 1 percent printed as a maze over nearly a third of its
/// dots. Dithers of half a percent to five percent, a page of single black
/// pixels and one alone at eight phases stay paper, and the thin logo among the
/// specks is one drawing, one dot wide, within a dot of where it prints alone.
#[test]
fn specks_and_sparse_dithers_stay_paper_and_the_strokes_among_them_print_as_alone() {
    let within = (Some(304), Some(80));
    let nothing = |bytes: &[u8], what: &str| {
        let i = artwork::import_within(bytes, within, DPMM).unwrap();
        assert_eq!((i.art.w, i.art.h, i.resampled), (80, 80, true), "{what}");
        assert_eq!(
            (i.halftone.clone(), i.threshold),
            (ImageHalftone::Threshold, 187),
            "{what}"
        );
        assert_eq!(printed(&i, 187).len(), 0, "{what} prints");
    };
    for per_mille in [5, 10, 20, 50] {
        let ink = scattered(per_mille, 7);
        let bytes = png_l8(
            600,
            600,
            |x, y| {
                if ink[(y * 600 + x) as usize] { 0 } else { 255 }
            },
        );
        nothing(&bytes, &format!("a dither of {per_mille} per mille"));
    }
    // One pixel in a thousand, some 360 of them, each apart from the rest.
    let specks = scattered(1, 11);
    assert!((300..420).contains(&specks.iter().filter(|&&s| s).count()));
    let speck = |x: u32, y: u32| specks[(y * 600 + x) as usize];
    nothing(
        &png_l8(600, 600, |x, y| if speck(x, y) { 0 } else { 255 }),
        "single pixels",
    );
    for k in 0..8 {
        let one = png_l8(600, 600, |x, y| {
            if (x, y) == (300 + k, 300 + 3 * k % 8) {
                0
            } else {
                255
            }
        });
        nothing(&one, &format!("one pixel at phase {k}"));
    }
    let base = printed(
        &artwork::import_within(&thin_logo_png(), within, DPMM).unwrap(),
        187,
    );
    let among = png_rgba(600, 600, |x, y| {
        if thin_logo(x, y) || speck(x, y) {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let dots = printed(&artwork::import_within(&among, within, DPMM).unwrap(), 187);
    assert_eq!(pieces(&dots), 1, "the logo is whole");
    assert_eq!(blocks(&dots), 0, "one dot wide");
    // Specks on or beside a stroke are part of it, and can move where it is
    // thinned, but never by more than a dot.
    let near = |a: &[(u32, u32)], b: &[(u32, u32)]| {
        a.iter().all(|&(x, y)| {
            b.iter()
                .any(|&(u, v)| x.abs_diff(u) <= 1 && y.abs_diff(v) <= 1)
        })
    };
    assert!(near(&dots, &base) && near(&base, &dots), "the logo moved");
}

/// Whether the point (`x`, `y`) lies on the thin logo's drawing in strokes `w`
/// pixels wide, drawn along the middle of the 2-pixel strokes of [`thin_logo`]:
/// the rings, the square, the cross and the twelve spokes. `spokes` holds each
/// spoke's sine and cosine.
fn on_thin_logo(x: f64, y: f64, w: f64, spokes: &[(f64, f64)]) -> bool {
    let half = w / 2.0;
    let (dx, dy) = (x - 300.0, y - 300.0);
    let r = (dx * dx + dy * dy).sqrt();
    let rings = (r - 259.0).abs() < half || (r - 179.0).abs() < half;
    let span = |v: f64| (201.0 - half..401.0 + half).contains(&v);
    let side = |v: f64| (v - 201.0).abs() < half || (v - 401.0).abs() < half;
    let square = (side(x) && span(y)) || (side(y) && span(x));
    let arm = |v: f64| (40.0..560.0).contains(&v);
    let cross = ((y - 300.0).abs() < half && arm(x)) || ((x - 300.0).abs() < half && arm(y));
    let spoke = spokes.iter().any(|&(s, c)| {
        (dy * c - dx * s).abs() < half && (180.0..260.0).contains(&(dx * c + dy * s))
    });
    rings || square || cross || spoke
}

/// The thin logo in black strokes `w` pixels wide on transparency, anti-aliased
/// as a drawing program exports it: each pixel's alpha is how much of it the
/// strokes cover, from 8 by 8 samples.
fn thin_logo_anti_aliased(w: f64) -> Vec<u8> {
    let spokes: Vec<(f64, f64)> = (0..12)
        .map(|k| (f64::from(k) * std::f64::consts::PI / 6.0).sin_cos())
        .collect();
    png_rgba(600, 600, |x, y| {
        let (cx, cy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
        // Only a pixel within reach of a stroke is sampled.
        if !on_thin_logo(cx, cy, w + 2.0, &spokes) {
            return [0, 0, 0, 0];
        }
        let inside = (0..64)
            .filter(|&n| {
                let (i, j) = (f64::from(n % 8), f64::from(n / 8));
                on_thin_logo(
                    f64::from(x) + (i + 0.5) / 8.0,
                    f64::from(y) + (j + 0.5) / 8.0,
                    w,
                    &spokes,
                )
            })
            .count();
        [0, 0, 0, (inside * 255 / 64) as u8]
    })
}

/// A stroke thinner than a pixel, anti-aliased on transparency, is faint along
/// most of its length: half ink only where it crosses a pixel's middle, so what
/// is half ink is a row of dashes, each smaller than a dot. Counted alone, they
/// were specks, and the thin logo drawn so broke into dozens of pieces or printed
/// almost nothing. Its faint pixels now join the dashes into one stroke, and at
/// 0.5, 0.75 and 0.9 pixels it prints as the 2-pixel logo does: one drawing, one
/// dot wide. A faint line that holds half a pixel of ink across it is ink even
/// where no one pixel of it is half ink.
#[test]
fn a_logo_anti_aliased_thinner_than_a_pixel_on_transparency_prints_whole_lines() {
    let within = (Some(304), Some(80));
    for w in [0.5, 0.75, 0.9] {
        let i = artwork::import_within(&thin_logo_anti_aliased(w), within, DPMM).unwrap();
        assert_eq!((i.art.w, i.art.h, i.resampled), (80, 80, true), "{w} px");
        assert_eq!(
            (i.halftone.clone(), i.threshold),
            (ImageHalftone::Threshold, 187),
            "{w} px"
        );
        let dots = printed(&i, 187);
        assert_eq!(pieces(&dots), 1, "{w} px: one unbroken drawing");
        assert_eq!(blocks(&dots), 0, "{w} px: one dot wide");
        let got = share_at(&i, 187);
        assert!((0.09..0.11).contains(&got), "{w} px prints {got}");
    }
    // A level line of 0.75 pixels on its own, split evenly between two rows, is
    // three eighths ink in each and half ink in neither, but half a pixel of ink
    // across: it prints whole, one dot to a column.
    let line = png_rgba(600, 600, |x, y| {
        if (40..560).contains(&x) && (299..301).contains(&y) {
            [0, 0, 0, 96]
        } else {
            [0, 0, 0, 0]
        }
    });
    let i = artwork::import_within(&line, within, DPMM).unwrap();
    assert_eq!(i.threshold, 187);
    let dots = printed(&i, 187);
    let columns: std::collections::BTreeSet<u32> = dots.iter().map(|&(x, _)| x).collect();
    assert_eq!(pieces(&dots), 1, "the line is whole");
    assert_eq!(dots.len(), columns.len(), "one dot to a column");
    assert!(dots.len() >= 68, "{} dots", dots.len());
}

/// A random 1-bit dither is tone at any shrink. At one and a half to four times,
/// a dot and a half is only three to six pixels, and a dither of a twentieth ink
/// or more makes marks that long all over, which were drawn as mazes: a tenth at
/// two times printed an eighth of its dots. Its marks lie among dozens of others,
/// as the letters of small text do not, and so are tone. Dithers of 5 to 30 percent at 1.5, 2, 3
/// and 4 times, and scan noise of 2-pixel blobs at two times, join nothing.
/// While a mark that reached 16 pixels was a stroke wherever it lay, dithers of
/// a quarter and three tenths ink joined 3 to 105 dots at those shrinks.
#[test]
fn random_dithers_shrunk_one_and_a_half_to_four_times_stay_paper() {
    // A 1-bit dither `side` pixels square, of `per_mille` thousandths ink.
    let dither = |side: u32, per_mille: u64| {
        let mut state: u64 = 2024 + u64::from(side) + per_mille;
        let ink: Vec<bool> = (0..side * side)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 33) % 1000 < per_mille
            })
            .collect();
        png_l8(side, side, |x, y| {
            if ink[(y * side + x) as usize] { 0 } else { 255 }
        })
    };
    let within = (Some(304), Some(80));
    for side in [120, 160, 240, 320] {
        for per_mille in [50, 100, 200, 250, 300] {
            let i = artwork::import_within(&dither(side, per_mille), within, DPMM).unwrap();
            let what = format!("{per_mille} per mille at {side} pixels");
            assert_eq!((i.art.w, i.art.h, i.resampled), (80, 80, true), "{what}");
            assert_eq!(
                (i.halftone.clone(), i.threshold),
                (ImageHalftone::Threshold, 187),
                "{what}"
            );
            assert_eq!(i.joined, 0, "{what} joined");
        }
    }
    // Scan noise: blobs of 2 by 2 pixels, one in 32 pixels on average.
    let mut state: u64 = 99;
    let mut blobs = vec![false; 160 * 160];
    for _ in 0..800 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let (x, y) = ((state >> 33) % 159, (state >> 45) % 159);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            blobs[((y + dy) * 160 + x + dx) as usize] = true;
        }
    }
    let noise = png_l8(160, 160, |x, y| {
        if blobs[(y * 160 + x) as usize] {
            0
        } else {
            255
        }
    });
    let i = artwork::import_within(&noise, within, DPMM).unwrap();
    assert_eq!((i.threshold, i.joined), (187, 0), "2-pixel blobs");
}

/// A pale line of its own is not a stroke, even a faint one: the thin logo drawn
/// in grey 230, whose 2-pixel lines carry less than half a pixel of ink across
/// them, stays paper, as it did before faint pixels were weighed.
#[test]
fn a_pale_thin_drawing_with_less_than_half_a_pixel_of_ink_across_stays_paper() {
    let pale = png_l8(600, 600, |x, y| if thin_logo(x, y) { 230 } else { 255 });
    let i = artwork::import_within(&pale, (Some(304), Some(80)), DPMM).unwrap();
    assert_eq!(
        (i.halftone.clone(), i.threshold, i.joined),
        (ImageHalftone::Threshold, 187, 0)
    );
    assert_eq!(printed(&i, 187).len(), 0);
}

/// Straight segments `width` pixels wide, black on transparency, anti-aliased as
/// a drawing program exports them: each pixel's alpha is how much of it the
/// segments cover, from 8 by 8 samples. Each segment is (x0, y0, x1, y1).
fn segments_anti_aliased(w: u32, h: u32, segments: &[[f64; 4]], width: f64) -> Vec<u8> {
    // One bit a sample, so segments that cross are not counted twice.
    let mut covered = vec![0u64; (w * h) as usize];
    let half = width / 2.0;
    for &[x0, y0, x1, y1] in segments {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let long = (dx * dx + dy * dy).max(1e-12);
        let from = |a: f64| (a - half - 1.0).floor().max(0.0) as u32;
        let (ax, bx) = (
            from(x0.min(x1)),
            (x0.max(x1) + half + 1.0).min(f64::from(w)) as u32,
        );
        let (ay, by) = (
            from(y0.min(y1)),
            (y0.max(y1) + half + 1.0).min(f64::from(h)) as u32,
        );
        for y in ay..by {
            for x in ax..bx {
                for n in 0..64 {
                    let px = f64::from(x) + (f64::from(n % 8) + 0.5) / 8.0;
                    let py = f64::from(y) + (f64::from(n / 8) + 0.5) / 8.0;
                    let t = (((px - x0) * dx + (py - y0) * dy) / long).clamp(0.0, 1.0);
                    let (ex, ey) = (px - x0 - t * dx, py - y0 - t * dy);
                    if ex * ex + ey * ey < half * half {
                        covered[(y * w + x) as usize] |= 1 << n;
                    }
                }
            }
        }
    }
    png_rgba(w, h, |x, y| {
        let n = covered[(y * w + x) as usize].count_ones();
        [0, 0, 0, (n * 255 / 64) as u8]
    })
}

/// A stroke of a letter: the points it runs through, in order.
type Polyline = Vec<(f64, f64)>;

/// The letters of "Label nij" in the manner of a thin sans serif, each with its
/// strokes as polylines, set one after another along the baseline: in units of
/// the x-height from the start of the word on its baseline, y up. The ascender
/// is 1.45, and the dots of i and j lie above the x-height.
fn thin_word() -> Vec<(char, Vec<Polyline>)> {
    // An arc of the ellipse at (cx, cy), radii (rx, ry), from angle a to b in
    // degrees, anticlockwise with y up.
    let arc = |cx: f64, cy: f64, rx: f64, ry: f64, a: f64, b: f64| -> Polyline {
        (0..=24)
            .map(|k| {
                let t = (a + (b - a) * f64::from(k) / 24.0).to_radians();
                (cx + rx * t.cos(), cy + ry * t.sin())
            })
            .collect()
    };
    let dot = |u: f64| vec![(u, 1.3), (u, 1.36)];
    let letters: Vec<(char, f64, Vec<Polyline>)> = vec![
        ('L', 0.75, vec![vec![(0.0, 1.45), (0.0, 0.0), (0.55, 0.0)]]),
        (
            'a',
            0.8,
            vec![
                vec![(0.55, 0.0), (0.55, 0.68)],
                arc(0.3, 0.68, 0.25, 0.32, 0.0, 150.0),
                arc(0.29, 0.26, 0.26, 0.26, 0.0, 360.0),
            ],
        ),
        (
            'b',
            0.8,
            vec![
                vec![(0.0, 0.0), (0.0, 1.45)],
                arc(0.3, 0.5, 0.3, 0.5, 0.0, 360.0),
            ],
        ),
        (
            'e',
            0.8,
            vec![
                arc(0.3, 0.5, 0.3, 0.5, 0.0, 320.0),
                vec![(0.0, 0.5), (0.6, 0.5)],
            ],
        ),
        ('l', 0.75, vec![vec![(0.0, 0.0), (0.0, 1.45)]]),
        (
            'n',
            0.85,
            vec![
                vec![(0.0, 0.0), (0.0, 1.0)],
                arc(0.28, 0.62, 0.28, 0.38, 180.0, 0.0),
                vec![(0.56, 0.62), (0.56, 0.0)],
            ],
        ),
        ('i', 0.35, vec![vec![(0.0, 0.0), (0.0, 1.0)], dot(0.0)]),
        (
            'j',
            0.35,
            vec![
                vec![(0.0, 1.0), (0.0, -0.25)],
                arc(-0.18, -0.25, 0.18, 0.15, 0.0, -90.0),
                dot(0.0),
            ],
        ),
    ];
    let mut at = 0.0;
    letters
        .into_iter()
        .map(|(c, advance, strokes)| {
            let placed = strokes
                .into_iter()
                .map(|s| s.into_iter().map(|(u, v)| (u + at, v)).collect())
                .collect();
            at += advance;
            (c, placed)
        })
        .collect()
}

/// Small text in thin strokes is line art like any other: each letter of a word
/// whose x-height is 6 dots is a mark reaching 9 to 16 pixels shrunk one and a
/// half times and 12 to 21 shrunk two times, and while a mark had to reach 16
/// pixels to be a stroke, letters printed as nothing. Three lines of "Label nij"
/// in strokes 0.75 pixels wide, anti-aliased on transparency, now print every
/// letter at those shrinks, each with at least half the dots it prints shrunk 7.5
/// times, and the whole within a fifth of that.
#[test]
fn small_thin_text_shrunk_one_and_a_half_and_two_times_prints_every_letter() {
    // In dots: the x-height, the lines' pitch, and the frame.
    let (x_height, pitch, (w, h)) = (6.0, 11.5, (42u32, 40u32));
    // Each letter on each line, as its strokes' segments in dots.
    let mut letters: Vec<(char, u32, Vec<[f64; 4]>)> = Vec::new();
    for line in 0..3 {
        let base = 3.0 + 1.45 * x_height + f64::from(line) * pitch;
        let at = |(u, v): (f64, f64)| (4.0 + u * x_height, base - v * x_height);
        for (c, strokes) in thin_word() {
            let segments = strokes
                .iter()
                .flat_map(|s| s.windows(2))
                .map(|pair| {
                    let ((x0, y0), (x1, y1)) = (at(pair[0]), at(pair[1]));
                    [x0, y0, x1, y1]
                })
                .collect();
            letters.push((c, line, segments));
        }
    }
    let within = (Some(w), Some(h));
    let imported = |f: f64| {
        let segments: Vec<[f64; 4]> = letters
            .iter()
            .flat_map(|(_, _, s)| s.iter().map(|&[a, b, c, d]| [a * f, b * f, c * f, d * f]))
            .collect();
        let side = |d: u32| (f64::from(d) * f).round() as u32;
        let bytes = segments_anti_aliased(side(w), side(h), &segments, 0.75);
        let i = artwork::import_within(&bytes, within, DPMM).unwrap();
        assert_eq!((i.art.w, i.art.h, i.resampled), (w, h, true), "{f} times");
        assert_eq!(
            (i.halftone.clone(), i.threshold),
            (ImageHalftone::Threshold, 187),
            "{f} times"
        );
        i
    };
    // The dots each letter prints: those nearer its strokes than any other
    // letter's, and within a dot and a half of them.
    let per_letter = |i: &Imported| -> Vec<usize> {
        let mut n = vec![0; letters.len()];
        for (x, y) in printed(i, 187) {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let near = |segments: &[[f64; 4]]| {
                segments
                    .iter()
                    .map(|&[x0, y0, x1, y1]| {
                        let (dx, dy) = (x1 - x0, y1 - y0);
                        let long = (dx * dx + dy * dy).max(1e-12);
                        let t = (((px - x0) * dx + (py - y0) * dy) / long).clamp(0.0, 1.0);
                        (px - x0 - t * dx).hypot(py - y0 - t * dy)
                    })
                    .fold(f64::MAX, f64::min)
            };
            let (k, d) = letters
                .iter()
                .map(|(_, _, s)| near(s))
                .enumerate()
                .fold((0, f64::MAX), |b, (k, d)| if d < b.1 { (k, d) } else { b });
            if d <= 1.5 {
                n[k] += 1;
            }
        }
        n
    };
    let reference = imported(7.5);
    let (whole, want) = (share_at(&reference, 187), per_letter(&reference));
    assert!(want.iter().all(|&n| n >= 4), "{want:?}");
    for f in [1.5, 2.0] {
        let i = imported(f);
        for ((c, line, _), (&got, &want)) in letters.iter().zip(per_letter(&i).iter().zip(&want)) {
            assert!(
                2 * got >= want,
                "{f} times: {c} on line {line} prints {got} dots against {want}"
            );
        }
        let got = share_at(&i, 187);
        assert!(
            (got - whole).abs() <= whole / 5.0,
            "{f} times prints {got} against {whole}"
        );
    }
}

/// A halftone screen whose dots are a dot and a half or more across once shrunk
/// is tone, as a finer one is: each of its dots is a blob no longer than it is
/// wide, too few to a square of 72 pixels to be a texture, and prints only what
/// 187 prints of it. Shrunk two, three and four times, a 45-degree screen of
/// 5-pixel dots 6 pixels apart on each axis joins nothing, and nor does scan
/// noise of 3-pixel blobs apart from each other at two times. Without the rule that such a mark
/// must be longer than it is wide, the screen joined 675 of its 6400 dots at two
/// times.
#[test]
fn a_coarse_screen_and_scan_noise_shrunk_two_to_four_times_join_nothing() {
    let within = (Some(304), Some(80));
    for side in [160, 240, 320] {
        let bytes = png_l8(side, side, |x, y| {
            if screen(x, y, 6.0, 5.0, 3.0) { 0 } else { 255 }
        });
        let i = artwork::import_within(&bytes, within, DPMM).unwrap();
        assert_eq!(
            (i.threshold, i.joined),
            (187, 0),
            "the screen at {side} pixels"
        );
    }
    // Each blob apart from the rest: one that would touch another is not drawn.
    let mut state: u64 = 5;
    let mut blobs = vec![false; 160 * 160];
    for _ in 0..28 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let (x, y) = (1 + (state >> 33) % 156, 1 + (state >> 45) % 156);
        let near = (0..25).any(|k| blobs[((y + k / 5 - 1) * 160 + x + k % 5 - 1) as usize]);
        for k in (0..9).filter(|_| !near) {
            blobs[((y + k / 3) * 160 + x + k % 3) as usize] = true;
        }
    }
    let noise = png_l8(160, 160, |x, y| {
        if blobs[(y * 160 + x) as usize] {
            0
        } else {
            255
        }
    });
    let i = artwork::import_within(&noise, within, DPMM).unwrap();
    assert_eq!((i.threshold, i.joined), (187, 0), "3-pixel blobs");
}

/// Whether a source pixel is ink in a 45-degree screen of round dots `d` pixels
/// across, `s` pixels apart on each axis on a checker lattice, its dots centred
/// at `phase` plus multiples of `s`.
fn screen(x: u32, y: u32, s: f64, d: f64, phase: f64) -> bool {
    let (px, py) = (f64::from(x) + 0.5 - phase, f64::from(y) + 0.5 - phase);
    let (a, b) = ((px / s).floor() as i64, (py / s).floor() as i64);
    (a..=a + 1).any(|i| {
        (b..=b + 1).any(|j| {
            let (dx, dy) = (px - i as f64 * s, py - j as f64 * s);
            (i + j) % 2 == 0 && dx * dx + dy * dy < d * d / 4.0
        })
    })
}

/// A halftone screen finer than a dot is tone, and each of its dots a speck. A
/// 45-degree screen of 2- or 3-pixel dots 7.5 pixels apart on each axis, shrunk
/// 7.5 times, lands one of its dots on every other printed dot, and joined it
/// printed as a solid checkerboard, half of every dot, where its dots fell in the
/// middle of printed dots. At that phase and on dot corners, and a coarser
/// screen on dot corners, it stays paper, and the thin logo drawn over the
/// screen of 2-pixel dots is whole.
#[test]
fn a_halftone_screen_finer_than_a_dot_stays_paper_at_any_phase_and_the_logo_over_it_is_whole() {
    let within = (Some(304), Some(80));
    for (s, d, phase, what) in [
        (7.5, 2.0, 3.75, "2-pixel dots 7.5 apart, centred in dots"),
        (7.5, 3.0, 3.75, "3-pixel dots 7.5 apart, centred in dots"),
        (7.5, 3.0, 0.0, "3-pixel dots 7.5 apart, on dot corners"),
        (15.0, 4.0, 0.0, "4-pixel dots 15 apart, on dot corners"),
    ] {
        let bytes = png_l8(
            600,
            600,
            |x, y| {
                if screen(x, y, s, d, phase) { 0 } else { 255 }
            },
        );
        let i = artwork::import_within(&bytes, within, DPMM).unwrap();
        assert_eq!(
            (i.halftone.clone(), i.threshold),
            (ImageHalftone::Threshold, 187),
            "{what}"
        );
        assert_eq!(printed(&i, 187).len(), 0, "{what} prints");
    }
    let over = png_l8(600, 600, |x, y| {
        if thin_logo(x, y) || screen(x, y, 7.5, 2.0, 3.75) {
            0
        } else {
            255
        }
    });
    let i = artwork::import_within(&over, within, DPMM).unwrap();
    let dots = printed(&i, 187);
    assert_eq!(pieces(&dots), 1, "the logo is whole");
    assert_eq!(blocks(&dots), 0, "one dot wide");
    assert!(
        (0.08..0.11).contains(&share_at(&i, 187)),
        "{}",
        share_at(&i, 187)
    );
}

/// A texture that runs to the edge of the artwork is not a line at its corners:
/// outside the artwork is not known to be paper, so ink seen falling only there
/// is no peak. A checkerboard of single pixels and 45-degree hatching over the
/// whole image join nothing, where each used to join a dot or two at its
/// corners, while a frame drawn along the very edge is drawn whole.
#[test]
fn a_texture_to_the_edge_of_the_artwork_joins_nothing_at_its_corners() {
    let within = (Some(304), Some(80));
    let checker = png_l8(
        600,
        600,
        |x, y| if (x + y).is_multiple_of(2) { 0 } else { 255 },
    );
    let hatch = png_l8(
        600,
        600,
        |x, y| if (x + y).is_multiple_of(3) { 0 } else { 255 },
    );
    for (bytes, what) in [(checker, "a checkerboard"), (hatch, "hatching")] {
        let i = artwork::import_within(&bytes, within, DPMM).unwrap();
        assert_eq!((i.threshold, i.joined), (187, 0), "{what}");
    }
    let frame = png_l8(600, 600, |x, y| {
        let edge = x < 2 || y < 2 || x >= 598 || y >= 598;
        if edge || (299..301).contains(&x) {
            0
        } else {
            255
        }
    });
    let i = artwork::import_within(&frame, within, DPMM).unwrap();
    let dots = printed(&i, 187);
    assert_eq!(pieces(&dots), 1, "the frame and its bar are one drawing");
    for (x, y) in [(0, 20), (79, 60), (20, 0), (60, 79)] {
        assert!(dots.contains(&(x, y)), "({x}, {y}) of the frame is missing");
    }
}

/// Only a stroke is drawn: never a gap between two marks. Dashes stay dashes,
/// and two squares that meet only diagonally across a white square are never
/// joined across it.
#[test]
fn dashes_and_marks_apart_in_the_source_are_never_joined() {
    let within = (Some(304), Some(80));
    let dashes = png_l8(600, 600, |x, y| {
        if (299..301).contains(&y) && (30..570).contains(&x) && (x - 30) % 60 < 30 {
            0
        } else {
            255
        }
    });
    let i = artwork::import_within(&dashes, within, DPMM).unwrap();
    let dots = printed(&i, i.threshold);
    assert_eq!(pieces(&dots), 9, "nine dashes");
    assert!(dots.iter().all(|&(_, y)| y == dots[0].1), "in one row");

    let squares = png_l8(600, 600, |x, y| {
        let a = (272..288).contains(&x) && (272..288).contains(&y);
        let b = (304..320).contains(&x) && (304..320).contains(&y);
        if a || b { 0 } else { 255 }
    });
    let i = artwork::import_within(&squares, within, DPMM).unwrap();
    assert_eq!((i.threshold, i.joined), (187, 0));
    assert_eq!(pieces(&printed(&i, 187)), 2, "two squares");
}

/// The rule on planes made by hand, where every dot's grey and ink is chosen: a
/// stem two dots wide is thinned to one, kept whole rather than eaten from an
/// end; a stem one dot wide is kept and a lone dot beside nothing dropped, and
/// the same stem whose ink is none of it a stroke's is left alone; a crossing
/// keeps its centre, so no hole opens; and a line runs into the block it meets.
#[test]
fn join_thin_strokes_draws_stems_and_crossings_whole_and_leaves_a_speck() {
    // Paper everywhere, then the dots given, as (x, y, grey, ink).
    let plane = |w: u32, h: u32, dots: &[(u32, u32, u8, u8)]| {
        let (mut px, mut inked) = (vec![255u8; (w * h) as usize], vec![0u8; (w * h) as usize]);
        for &(x, y, g, i) in dots {
            px[(y * w + x) as usize] = g;
            inked[(y * w + x) as usize] = i;
        }
        (px, inked)
    };

    let stem: Vec<_> = (1..=10)
        .flat_map(|y| [(2, y, 215, 60), (3, y, 215, 60)])
        .collect();
    let (mut px, inked) = plane(6, 12, &stem);
    assert_eq!(
        artwork::join_thin_strokes(6, 12, &mut px, &inked, &inked),
        10
    );
    for y in 1..=10 {
        let row = &px[y * 6..y * 6 + 6];
        assert_eq!(
            [row[2], row[3]].iter().filter(|&&g| g == 187).count(),
            1,
            "row {y}: {row:?}"
        );
    }

    let mut stem: Vec<_> = (1..=10).map(|y| (2, y, 200, 90)).collect();
    stem.push((5, 5, 200, 90));
    let (mut px, inked) = plane(6, 12, &stem);
    assert_eq!(
        artwork::join_thin_strokes(6, 12, &mut px, &inked, &inked),
        10
    );
    assert!((1..=10).all(|y| px[y * 6 + 2] == 187));
    assert_eq!(px[5 * 6 + 5], 200, "the speck is left");
    // The same stem, its ink all from specks rather than a stroke: nothing joins.
    let (mut px, inked) = plane(6, 12, &stem);
    let before = px.clone();
    let none = vec![0u8; inked.len()];
    assert_eq!(artwork::join_thin_strokes(6, 12, &mut px, &inked, &none), 0);
    assert_eq!(px, before);

    let mut cross: Vec<_> = (0..9)
        .filter(|&k| k != 4)
        .flat_map(|k| [(k, 4, 215, 60), (4, k, 215, 60)])
        .collect();
    cross.push((4, 4, 190, 120));
    let (mut px, inked) = plane(9, 9, &cross);
    assert_eq!(
        artwork::join_thin_strokes(9, 9, &mut px, &inked, &inked),
        17
    );
    assert_eq!(px[4 * 9 + 4], 187, "the centre stays");

    let mut line: Vec<_> = (0..6).map(|x| (x, 3, 215, 60)).collect();
    line.extend((6..=9).flat_map(|x| (1..=5).map(move |y| (x, y, 0, 255))));
    let (mut px, inked) = plane(12, 7, &line);
    assert_eq!(
        artwork::join_thin_strokes(12, 7, &mut px, &inked, &inked),
        6
    );
    assert!((0..6).all(|x| px[3 * 12 + x] == 187));
}

/// Ink too faint to survive a reduction is not chased into the paper: dots that
/// came out as paper, 255, never print, whatever share of the source was ink.
#[test]
fn ink_too_faint_to_survive_a_reduction_is_not_chased_into_the_paper() {
    let specks = png_l8(
        1000,
        1000,
        |x, y| {
            if x % 20 == 0 && y % 20 == 0 { 0 } else { 255 }
        },
    );
    let i = artwork::import_within(&specks, (Some(40), Some(40)), DPMM).unwrap();
    assert_eq!((i.art.w, i.art.h), (40, 40));
    assert!(i.art.px.iter().all(|&v| v == 255), "every dot is paper");
    assert_eq!((i.halftone, i.threshold), (ImageHalftone::Threshold, 187));
    assert_eq!(i.joined, 0, "nothing to join");
}

/// Only line art that a resample shrank has strokes drawn into it. A photograph
/// keeps Floyd-Steinberg at 128, whose diffusion already prints its tone; line
/// art at one pixel to a dot keeps 128 and prints exactly the source's ink; line
/// art enlarged shrinks no stroke; an SVG, drawn at the size it lands rather
/// than resampled, keeps what its coverage says; and blank artwork keeps 187 and
/// prints nothing.
#[test]
fn photos_one_to_one_or_enlarged_line_art_svgs_and_blank_artwork_are_never_joined() {
    let one_to_one = import(&thin_logo_png(), (600, 600), ImageFit::Dots).unwrap();
    assert!(!one_to_one.resampled);
    assert_eq!(
        (one_to_one.halftone.clone(), one_to_one.threshold),
        (ImageHalftone::Threshold, 128)
    );
    assert_eq!(share_at(&one_to_one, 128), thin_logo_ink());
    assert_eq!(one_to_one.joined, 0);

    let icon = png_l8(20, 20, |x, y| {
        if [2, 17].contains(&x) || [2, 17].contains(&y) || x == y {
            0
        } else {
            255
        }
    });
    let i = import(&icon, (80, 80), ImageFit::Contain).unwrap();
    assert!(i.resampled);
    assert_eq!(
        (i.halftone.clone(), i.threshold, i.joined),
        (ImageHalftone::Threshold, 187, 0)
    );

    let photo = png_l8(400, 300, |x, y| ((x + 2 * y) % 256) as u8);
    let i = artwork::import_within(&photo, (Some(304), Some(80)), DPMM).unwrap();
    assert!(i.resampled);
    assert_eq!(
        (i.halftone.clone(), i.threshold, i.joined),
        (ImageHalftone::FloydSteinberg, 128, 0)
    );

    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 600 600"><g fill="none" stroke="black" stroke-width="2"><circle cx="300" cy="300" r="259"/><circle cx="300" cy="300" r="179"/><rect x="201" y="201" width="199" height="199"/></g></svg>"#;
    let i = artwork::import_within(svg, (Some(304), Some(80)), DPMM).unwrap();
    assert_eq!((i.source, i.resampled), (SourceFormat::Svg, false));
    assert_eq!((i.threshold, i.joined), (128, 0));

    let blank = png_l8(200, 100, |_, _| 255);
    let i = artwork::import_within(&blank, (Some(304), Some(80)), DPMM).unwrap();
    assert!(i.resampled);
    assert_eq!(share_at(&i, i.threshold), 0.0);
    assert_eq!(
        (i.halftone.clone(), i.threshold, i.joined),
        (ImageHalftone::Threshold, 187, 0)
    );
}

/// The halftone import writes, for each kind of source.
#[test]
fn import_chooses_a_halftone_for_the_source() {
    let photo = png_l8(64, 64, |x, y| (x * 2 + y) as u8);
    let i = import(&photo, (64, 64), ImageFit::Dots).unwrap();
    assert_eq!(
        (i.halftone, i.threshold),
        (ImageHalftone::FloydSteinberg, 128)
    );

    let square = |x: u32, y: u32| {
        if (20..80).contains(&x) && (20..80).contains(&y) {
            0
        } else {
            255
        }
    };
    let lines = png_l8(100, 100, square);
    let i = import(&lines, (50, 50), ImageFit::Stretch).unwrap();
    assert_eq!(
        (i.halftone, i.threshold),
        (ImageHalftone::Threshold, 187),
        "resampled"
    );
    let i = import(&lines, (100, 100), ImageFit::Dots).unwrap();
    assert_eq!(
        (i.halftone, i.threshold),
        (ImageHalftone::Threshold, 128),
        "one to one"
    );

    let soft = png_rgba(40, 40, |x, _| {
        [
            0,
            0,
            0,
            if x < 20 {
                255
            } else if x < 22 {
                128
            } else {
                0
            },
        ]
    });
    let i = import(&soft, (40, 40), ImageFit::Dots).unwrap();
    assert_eq!(
        (i.halftone, i.threshold),
        (ImageHalftone::Threshold, 187),
        "soft edges"
    );

    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect x="10" y="10" width="20" height="20"/></svg>"#;
    let i = import(svg, (40, 40), ImageFit::Contain).unwrap();
    assert_eq!(i.source, SourceFormat::Svg);
    assert_eq!(
        (i.halftone, i.threshold),
        (ImageHalftone::Threshold, 128),
        "SVG"
    );
}

/// A cover crops evenly, and one-to-one cropping is noted.
#[test]
fn cover_and_dots_crop_about_the_centre() {
    let halves = png_l8(200, 100, |x, _| if x < 100 { 0 } else { 255 });
    let i = import(&halves, (50, 50), ImageFit::Cover).unwrap();
    assert_eq!((i.art.w, i.art.h), (50, 50));
    assert!(i.art.px[25 * 50 + 5] < 64 && i.art.px[25 * 50 + 45] > 192);
    // The outer quarters are cropped away, not squeezed in.
    let edges = png_l8(
        200,
        100,
        |x, _| if !(50..150).contains(&x) { 0 } else { 255 },
    );
    let i = import(&edges, (50, 50), ImageFit::Cover).unwrap();
    assert!(i.art.px.iter().all(|&v| v > 192), "{:?}", &i.art.px[..50]);
    // And said: half of it is gone.
    assert_eq!(
        i.notes,
        vec![ImportNote::CoverCropped {
            from: (200, 100),
            to: (100, 100)
        }]
    );
    // Cropped with nothing to resample is still a crop.
    let wide = png_l8(
        400,
        100,
        |x, _| if !(20..380).contains(&x) { 0 } else { 255 },
    );
    let i = import(&wide, (100, 100), ImageFit::Cover).unwrap();
    assert!(!i.resampled);
    assert_eq!(
        i.notes,
        vec![ImportNote::CoverCropped {
            from: (400, 100),
            to: (100, 100)
        }]
    );
    assert_eq!(
        i.notes[0].to_string(),
        "to cover its frame it was cropped from 400x100 to the middle 100x100"
    );
    // The frame's own shape crops nothing, and says nothing.
    let i = import(&edges, (100, 50), ImageFit::Cover).unwrap();
    assert!(i.notes.is_empty(), "{:?}", i.notes);

    let big = png_l8(100, 100, |x, y| if x == 50 && y == 50 { 0 } else { 255 });
    let i = import(&big, (40, 40), ImageFit::Dots).unwrap();
    assert_eq!((i.art.w, i.art.h), (40, 40));
    assert_eq!(i.art.px[20 * 40 + 20], 0, "the centre stays the centre");
    assert_eq!(
        i.notes,
        vec![ImportNote::Cropped {
            from: (100, 100),
            to: (40, 40)
        }]
    );
}

/// Placement is idempotent: artwork imported for a frame occupies the same
/// box when drawn in that frame, so drawing it is a straight copy.
#[test]
fn placement_is_idempotent() {
    for iw in 1..=20u32 {
        for ih in 1..=20u32 {
            for fw in 1..=20u32 {
                for fh in 1..=20u32 {
                    let frame = (fw, fh);
                    let p = fit_box((iw, ih), frame, &ImageFit::Contain).unwrap();
                    assert!(p.bw <= fw && p.bh <= fh && (p.bw == fw || p.bh == fh));
                    assert!(p.ox >= 0 && p.oy >= 0);
                    let again = fit_box((p.bw, p.bh), frame, &ImageFit::Contain).unwrap();
                    assert_eq!(again, p, "contain {iw}x{ih} in {fw}x{fh}");

                    let c = fit_box((iw, ih), frame, &ImageFit::Cover).unwrap();
                    assert!(c.bw >= fw && c.bh >= fh && (c.bw == fw || c.bh == fh));
                    let own = fit_box(frame, frame, &ImageFit::Cover).unwrap();
                    assert_eq!((own.bw, own.bh, own.ox, own.oy), (fw, fh, 0, 0));
                }
            }
        }
    }
    assert_eq!(fit_box((0, 5), (5, 5), &ImageFit::Contain), None);
    assert_eq!(
        fit_box((5, 5), (5, 5), &ImageFit::Unknown("tile".into())),
        None
    );
}

/// The tables are the formula, and every code survives a round trip.
#[test]
fn the_srgb_tables_are_the_formula() {
    for c in 0..=255u8 {
        let t = f64::from(srgb::to_linear(c));
        assert!((t - lin64(c)).abs() < 1e-6, "{c}: {t} against {}", lin64(c));
        assert_eq!(srgb::from_linear(srgb::to_linear(c)), c);
    }
    for (i, &m) in srgb::MIDPOINTS.iter().enumerate() {
        let want = (lin64(i as u8) + lin64(i as u8 + 1)) / 2.0;
        assert!(
            (f64::from(m) - want).abs() < 1e-6,
            "midpoint {i}: {m} against {want}"
        );
    }
    assert!(srgb::TO_LINEAR.windows(2).all(|p| p[0] < p[1]));
    assert!(srgb::MIDPOINTS.windows(2).all(|p| p[0] < p[1]));
    assert_eq!(srgb::from_linear(-1.0), 0);
    assert_eq!(srgb::from_linear(2.0), 255);
}

/// A line of source up to any `//` comment, a `//` inside a string literal
/// not counting.
fn without_comment(line: &str) -> &str {
    let (mut in_string, mut escaped) = (false, false);
    let b = line.as_bytes();
    for i in 0..b.len() {
        match b[i] {
            _ if escaped => escaped = false,
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'/' if !in_string && b.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Calls that were measured to be wrong for this job, kept out of the
/// renderer's source. Comments are allowed to name them.
#[test]
fn known_wrong_calls_stay_out_of_the_source() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = without_comment(line);
            for banned in [
                "to_luma",
                "thumbnail",
                "powf",
                "grayscale(",
                "image::open",
                "ImageReader::open",
            ] {
                assert!(
                    !code.contains(banned),
                    "{}:{}: {banned}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
}

/// The continuous-tone boundary: more than one pixel in fifty a mid-tone.
/// Pixels less than half opaque are not counted at all.
#[test]
fn the_tone_boundary_is_one_pixel_in_fifty() {
    let with_mid = |n: u32| png_l8(100, 100, move |x, y| if y * 100 + x < n { 128 } else { 0 });
    let i = import(&with_mid(190), (100, 100), ImageFit::Dots).unwrap();
    assert_eq!(i.halftone, ImageHalftone::Threshold, "1.9 percent");
    let i = import(&with_mid(210), (100, 100), ImageFit::Dots).unwrap();
    assert_eq!(i.halftone, ImageHalftone::FloydSteinberg, "2.1 percent");
    let faint = png_rgba(100, 100, |x, _| {
        if x < 30 {
            [128, 128, 128, 100]
        } else {
            [0, 0, 0, 255]
        }
    });
    let i = import(&faint, (100, 100), ImageFit::Dots).unwrap();
    assert_eq!((i.halftone, i.threshold), (ImageHalftone::Threshold, 187));
}

/// Frames drawing could never accept, and values this build does not know,
/// are refused at import rather than turned into an element that fails.
#[test]
fn import_refuses_what_drawing_would() {
    let b = png_l8(8, 8, |_, _| 0);
    assert!(matches!(
        import(&b, (8, 8), ImageFit::Unknown("tile".into())),
        Err(ArtworkError::UnknownValue { .. })
    ));
    assert_eq!(
        import(&b, (8192, 8), ImageFit::Contain).err(),
        Some(ArtworkError::FrameTooLarge { w: 8192, h: 8 })
    );
    assert_eq!(
        import(&b, (4096, 2048), ImageFit::Contain).err(),
        Some(ArtworkError::FrameTooLarge { w: 4096, h: 2048 }),
        "over MAX_LOCAL_DOTS in area"
    );
    assert_eq!(
        import(&b, (0, 8), ImageFit::Contain).err(),
        Some(ArtworkError::EmptyFrame)
    );
    assert!(matches!(
        artwork::from_stored(&"A".repeat(1_500_000), 1),
        Err(ArtworkError::StoredTooLong { .. })
    ));
}

/// An animated WebP is noted, like an animated PNG.
#[test]
fn an_animated_webp_is_noted() {
    let i = import(&webp_animated(4, 4), (4, 4), ImageFit::Dots).unwrap();
    assert_eq!(i.source, SourceFormat::WebP);
    assert!(
        i.notes.contains(&ImportNote::FirstFrameOnly),
        "{:?}",
        i.notes
    );
}

/// What `stored` gives is exactly what an element built by `into_element`
/// holds, and it unpacks to the artwork it came from.
#[test]
fn stored_is_the_png_into_element_writes() {
    let lines = png_l8(48, 32, |x, y| if x % 8 < 2 || y == 16 { 0 } else { 255 });
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="30mm" height="10mm"><rect x="2mm" y="2mm" width="26mm" height="6mm"/></svg>"#;
    for bytes in [&lines[..], &svg[..]] {
        let i = artwork::import_within(bytes, (Some(304), Some(80)), DPMM).unwrap();
        let stored = i.stored().unwrap();
        let frame = RectDots {
            feed: 0,
            head: 0,
            feed_len: i.art.w as i32,
            head_len: i.art.h as i32,
        };
        let el = i
            .clone()
            .into_element(Common::new(1, frame), "x".into(), ImageFit::Contain)
            .unwrap();
        assert_eq!(stored, el.png, "{:?}", i.source);
        assert_eq!(artwork::from_stored(&stored, 1 << 22).unwrap(), i.art);
    }
}

/// The size in the header, from the first 32 characters and nothing else, and
/// no panic on anything that is not a PNG.
#[test]
fn stored_size_reads_the_header_from_the_first_32_characters() {
    let wide = stored(&art(240, 80, |x, _| if x < 120 { 0 } else { 255 }));
    assert_eq!(artwork::stored_size(&wide), Some((240, 80)));
    for s in [
        wide.clone(),
        stored(&art(1, 1, |_, _| 0)),
        stored(&art(3, 700, |_, y| y as u8)),
        base64::engine::general_purpose::STANDARD.encode(png_rgba(5, 9, |_, _| [9, 9, 9, 200])),
    ] {
        let a = artwork::from_stored(&s, 1 << 22).unwrap();
        assert_eq!(artwork::stored_size(&s), Some((a.w, a.h)));
    }
    let jpeg = base64::engine::general_purpose::STANDARD.encode(jpeg_rgb(16, 8, |_, _| [0, 0, 0]));
    for bad in [
        String::new(),
        wide[..31].to_string(),
        "xxxx".repeat(20),
        "\u{e9}".repeat(20),
        jpeg,
    ] {
        assert_eq!(artwork::stored_size(&bad), None, "{bad:?}");
    }
}

/// Straight alpha in a WebP is read as a PNG's is: the half-transparent half
/// is composited over paper, the transparent half is paper. What
/// `RENDER-DESIGN.md` 4.6 asked to see before WebP is offered.
#[test]
fn a_translucent_webp_imports_like_the_same_png() {
    let px = |x: u32, _: u32| [0, 0, 0, if x < 8 { 128 } else { 0 }];
    let webp = import(&webp_lossless(16, 8, px), (16, 8), ImageFit::Dots).unwrap();
    let png = import(&png_rgba(16, 8, px), (16, 8), ImageFit::Dots).unwrap();
    assert_eq!(webp.source, SourceFormat::WebP);
    assert_eq!(webp.art, png.art);
    assert_eq!(
        (webp.halftone, webp.threshold),
        (png.halftone, png.threshold)
    );
    for y in 0..8 {
        let row = &webp.art.px[y * 16..y * 16 + 16];
        assert!(
            row[..8].iter().all(|&v| (187..=188).contains(&v)),
            "{row:?}"
        );
        assert!(row[8..].iter().all(|&v| v == 255), "{row:?}");
    }
}

/// A JPEG cut short is refused, before it is decoded. The decoder fills a scan
/// that is missing with flat grey, so a download that stopped part way, even one
/// holding only its headers, was added as a full-size grey photograph with
/// nothing to say the file was incomplete. A cut inside the last scan cannot be
/// told from a whole scan that lacks only the end marker without decoding it, so
/// both are refused, and the sentence says what was found rather than claiming
/// part of the picture is gone.
#[test]
fn a_jpeg_cut_short_is_refused() {
    let whole = jpeg_rgb(64, 48, |x, y| [(x * 4) as u8, (y * 5) as u8, 128]);
    assert!(import(&whole, (64, 48), ImageFit::Contain).is_ok());
    let sos = whole
        .windows(2)
        .position(|m| m == [0xFF, 0xDA])
        .expect("a scan");
    for cut in [sos - 10, sos + 20, whole.len() / 2, whole.len() - 2] {
        assert_eq!(
            import(&whole[..cut], (64, 48), ImageFit::Contain).err(),
            Some(ArtworkError::JpegEndsEarly),
            "cut at {cut} of {}",
            whole.len()
        );
    }
    assert_eq!(
        ArtworkError::JpegEndsEarly.to_string(),
        "the JPEG has no end marker, so it may be cut short, with part of the picture missing"
    );
    // What follows the end is not the picture's, and no reason to refuse it.
    let mut trailing = whole.clone();
    trailing.extend_from_slice(b"\0\0trailer\xFF\xDA\0");
    assert!(import(&trailing, (64, 48), ImageFit::Contain).is_ok());
}
