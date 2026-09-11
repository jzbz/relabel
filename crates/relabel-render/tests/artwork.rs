//! Import: image files to dots. Every fixture is generated in code - see
//! `common/fixtures.rs` - and every expected value is computed independently
//! of the code under test where that is possible.

#[path = "common/fixtures.rs"]
mod fixtures;

use fixtures::*;
use relabel_render::Dpmm;
use relabel_render::artwork::{self, ArtworkError, ImportNote, Imported, SourceFormat, fit_box};
use relabel_render::document::{ImageFit, ImageHalftone};
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
/// over ten levels out.
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
    let got = rms(&i.art.px, &reference);
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
