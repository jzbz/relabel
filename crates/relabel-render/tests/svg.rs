//! SVG artwork: rendered at import onto the dot grid, and everything a
//! hostile file could use refused by name.

mod common;

use relabel_render::artwork::{self, ArtworkError, ImportNote, Imported, SvgRefusal};
use relabel_render::document::*;
use relabel_render::{Dpmm, Target, rasterise_plane};

const NS: &str = r#"xmlns="http://www.w3.org/2000/svg""#;

fn import(svg: &str, frame: (u32, u32), fit: ImageFit) -> Result<Imported, ArtworkError> {
    artwork::import(svg.as_bytes(), frame, &fit, Dpmm::DPI_203)
}

fn refusal(svg: &str) -> SvgRefusal {
    match import(svg, (96, 96), ImageFit::Contain) {
        Err(ArtworkError::Svg(r)) => r,
        other => panic!("not refused as SVG: {other:?}"),
    }
}

/// Millimetres land on the dot grid: 40 x 12 mm is 320 x 96 at 8 dots per
/// millimetre.
#[test]
fn millimetres_land_on_the_dot_grid() {
    let svg =
        format!(r#"<svg {NS} width="40mm" height="12mm"><rect width="100%" height="100%"/></svg>"#);
    let i = artwork::import_within(svg.as_bytes(), (None, None), Dpmm::DPI_203).unwrap();
    assert_eq!(i.source_px, (320, 96));
    assert_eq!((i.art.w, i.art.h), (320, 96));
    assert!(i.art.px.iter().all(|&v| v == 0));
}

/// Edges are crisp: a hairline four tenths of a dot deep prints as a full
/// row, where anti-aliasing would leave a row of pale grey that a threshold
/// drops.
#[test]
fn a_hairline_survives_the_threshold() {
    let svg = format!(
        r#"<svg {NS} width="96" height="10"><rect x="0" y="4.3" width="96" height="0.4"/></svg>"#
    );
    let i = import(&svg, (96, 10), ImageFit::Dots).unwrap();
    assert_eq!(i.threshold, 128);
    assert!(
        i.art.px[4 * 96..5 * 96].iter().all(|&v| v == 0),
        "{:?}",
        &i.art.px[4 * 96..5 * 96]
    );
    assert!(i.art.px[..4 * 96].iter().all(|&v| v == 255));
    // The control: the same line, anti-aliased because the file asks for it.
    let soft = svg.replace("<rect ", r#"<rect shape-rendering="geometricPrecision" "#);
    let i = import(&soft, (96, 10), ImageFit::Dots).unwrap();
    assert!(i.art.px[4 * 96..5 * 96].iter().all(|&v| v > 0 && v < 255));
}

fn drawn(i: Imported, frame: RectDots) -> relabel_proto::BitPlane {
    let el = i
        .into_element(Common::new(1, frame), "art.svg".into(), ImageFit::Contain)
        .unwrap();
    let mut l = Label::new(320, 96, Dpmm::DPI_203);
    l.elements.push(ElementOrRaw::Known(Element::Image(el)));
    rasterise_plane(&l, &Target::new(320, 96, 203).unwrap()).unwrap()
}

#[test]
fn svg_artwork_matches_its_goldens() {
    let shapes = format!(
        r##"<svg {NS} width="96" height="96" viewBox="0 0 48 48">
             <circle cx="24" cy="24" r="16" fill="none" stroke="black" stroke-width="3"/>
             <path d="M4 44 L44 4" stroke="black" stroke-width="1.5"/>
             <rect x="30" y="30" width="12" height="12" fill="#444"/>
           </svg>"##
    );
    let i = import(&shapes, (96, 96), ImageFit::Contain).unwrap();
    let whole = RectDots {
        feed: 0,
        head: 0,
        feed_len: 96,
        head_len: 96,
    };
    common::check("svg-crisp", &drawn(i, whole));

    let wide = format!(
        r#"<svg {NS} width="60mm" height="20mm"><rect x="2mm" y="2mm" width="56mm" height="16mm" rx="4mm" fill="none" stroke="black" stroke-width="1mm"/></svg>"#
    );
    let i = import(&wide, (200, 96), ImageFit::Contain).unwrap();
    assert_eq!((i.art.w, i.art.h), (200, 66));
    let f = RectDots {
        feed: 100,
        head: 0,
        feed_len: 200,
        head_len: 96,
    };
    common::check("svg-fit-contain", &drawn(i, f));
}

/// Everything a hostile or merely unsupported file could use, refused by
/// name, each before resvg would draw it.
#[test]
fn what_relabel_does_not_draw_is_refused() {
    let wrap = |body: &str| format!(r#"<svg {NS} width="96" height="96">{body}</svg>"#);
    for (body, name) in [
        (r#"<text x="10" y="20">Hi</text>"#, "<text>"),
        (
            r#"<image href="/etc/hostname" width="10" height="10"/>"#,
            "<image>",
        ),
        (
            r#"<foreignObject width="10" height="10"/>"#,
            "<foreignObject>",
        ),
        (
            r#"<defs><pattern id="p" patternUnits="userSpaceOnUse" width="3000000" height="3000000"><rect width="10" height="10"/></pattern></defs><rect width="96" height="96" fill="url(#p)"/>"#,
            "<pattern>",
        ),
        (
            r#"<filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="3000000" height="3000000"><feFlood/></filter><rect width="10" height="10" filter="url(#f)"/>"#,
            "<filter>",
        ),
    ] {
        match refusal(&wrap(body)) {
            SvgRefusal::Elements(found) => assert!(found.contains(name), "{found}"),
            other => panic!("{name}: {other:?}"),
        }
    }
    // A CSS filter function gets past any check of element names.
    assert_eq!(
        refusal(&wrap(
            r#"<rect width="10" height="10" style="filter: blur(5px)"/>"#
        )),
        SvgRefusal::Filters
    );
    let masked = |n: usize| {
        let mut s =
            String::from(r#"<mask id="m"><rect width="96" height="96" fill="white"/></mask>"#);
        for _ in 0..n {
            s += r#"<g mask="url(#m)"><rect width="96" height="96"/></g>"#;
        }
        wrap(&s)
    };
    assert!(import(&masked(64), (96, 96), ImageFit::Contain).is_ok());
    assert_eq!(refusal(&masked(65)), SvgRefusal::TooManyGroups(65));
    let nested = |n: usize| {
        wrap(&format!(
            "{}<rect width=\"9\" height=\"9\"/>{}",
            r#"<g opacity="0.5">"#.repeat(n),
            "</g>".repeat(n)
        ))
    };
    assert!(import(&nested(8), (96, 96), ImageFit::Contain).is_ok());
    assert_eq!(refusal(&nested(9)), SvgRefusal::TooDeep(9));
}

/// Dashes are counted over the whole drawing before any is drawn. tiny-skia
/// stops dashing one path past a million dashes, but a few thousand short paths
/// each dashed far finer than a dot stay under that one by one, and together
/// cost minutes of work while every later import waits.
#[test]
fn dashes_finer_than_a_dot_are_counted_before_drawing() {
    let paths = |n: usize, dash: &str| {
        let mut s = format!(r#"<svg {NS} width="30" height="10" viewBox="0 0 30 10">"#);
        for i in 0..n {
            let y = 0.5 + (i % 9) as f32;
            s += &format!(
                r#"<path d="M0 {y} L30 {y}" stroke="black" stroke-width="0.1" stroke-dasharray="{dash}"/>"#
            );
        }
        s + "</svg>"
    };
    // An ordinary dashed rule draws, and so does one near the cap.
    assert!(import(&paths(4, "2 1"), (96, 96), ImageFit::Contain).is_ok());
    assert!(import(&paths(6, "0.0001"), (96, 96), ImageFit::Contain).is_ok());
    // Twenty paths of 150000 dashes each: under tiny-skia's cap one by one.
    let began = std::time::Instant::now();
    let r = refusal(&paths(20, "0.0001"));
    assert!(
        began.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        began.elapsed()
    );
    assert_eq!(
        r.to_string(),
        "the SVG's dashed lines come to 3000000 dashes; relabel draws at most 1000000"
    );
}

/// The ways a file can be too much, or not SVG at all.
#[test]
fn oversized_compressed_and_malformed_files_are_refused() {
    let gz = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0];
    assert_eq!(
        artwork::import(&gz, (96, 96), &ImageFit::Contain, Dpmm::DPI_203).err(),
        Some(ArtworkError::Svg(SvgRefusal::Compressed))
    );
    let huge = format!(
        "<svg {NS} width=\"9\" height=\"9\">{}</svg>",
        " ".repeat(2 << 20)
    );
    assert!(matches!(refusal(&huge), SvgRefusal::TooLarge { .. }));
    let mut latin1 = format!("<svg {NS} width=\"9\" height=\"9\"><!-- ").into_bytes();
    latin1.extend_from_slice(&[
        0xE9, b' ', b'-', b'-', b'>', b'<', b'/', b's', b'v', b'g', b'>',
    ]);
    assert_eq!(
        artwork::import(&latin1, (96, 96), &ImageFit::Contain, Dpmm::DPI_203).err(),
        Some(ArtworkError::Svg(SvgRefusal::NotUtf8))
    );
    let laughs = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;"><!ENTITY e "&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;"><!ENTITY f "&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;"><!ENTITY g "&f;&f;&f;&f;&f;&f;&f;&f;&f;&f;"><!ENTITY h "&g;&g;&g;&g;&g;&g;&g;&g;&g;&g;"><!ENTITY i "&h;&h;&h;&h;&h;&h;&h;&h;&h;&h;"><!ENTITY j "&i;&i;&i;&i;&i;&i;&i;&i;&i;&i;"><!ENTITY k "&j;&j;&j;&j;&j;&j;&j;&j;&j;&j;">]><svg {NS} width="9" height="9"><desc>&k;</desc></svg>"#
    );
    assert!(matches!(refusal(&laughs), SvgRefusal::Xml(_)));
    let many = format!(
        "<svg {NS} width=\"9\" height=\"9\">{}</svg>",
        "<g/>".repeat(100_001)
    );
    assert!(matches!(refusal(&many), SvgRefusal::Xml(_)));
}

/// A `<use>` of another file draws nothing: usvg never follows one. What
/// keeps `<image>` from reading files is tested in `svg.rs` itself, past the
/// refusal by name.
#[test]
fn an_external_reference_reads_nothing() {
    let svg = format!(
        r##"<svg {NS} width="20" height="20"><use href="other.svg#a"/><use href="/etc/passwd#x"/></svg>"##
    );
    let i = import(&svg, (20, 20), ImageFit::Contain).unwrap();
    assert!(i.art.px.iter().all(|&v| v == 255));
}

#[test]
fn an_svg_that_draws_nothing_is_noted() {
    let svg = format!(
        r#"<svg {NS} width="20" height="20"><rect width="20" height="20" fill="white"/></svg>"#
    );
    let i = import(&svg, (20, 20), ImageFit::Contain).unwrap();
    assert_eq!(i.notes, vec![ImportNote::SvgDrewNothing]);
}

/// Markers, and chains of clip paths or masks, are refused before usvg
/// builds anything: a marker is copied once per path vertex, and a long chain
/// overflows usvg's stack, which aborts the process.
#[test]
fn markers_and_long_clip_chains_are_refused() {
    let wrap = |body: &str| format!(r#"<svg {NS} width="96" height="96">{body}</svg>"#);
    let marker = wrap(
        r##"<marker id="m" overflow="visible" markerWidth="1" markerHeight="1"><rect width="1" height="1"/></marker><path d="M0 0 L1 1 L2 2 L3 3" stroke="black" marker-mid="url(#m)"/>"##,
    );
    match refusal(&marker) {
        SvgRefusal::Elements(found) => assert!(found.contains("<marker>"), "{found}"),
        other => panic!("{other:?}"),
    }
    let mut chain = String::from(r#"<clipPath id="c0"><rect width="9" height="9"/></clipPath>"#);
    for i in 1..5000 {
        chain += &format!(
            r#"<clipPath id="c{i}" clip-path="url(#c{})"><rect width="9" height="9"/></clipPath>"#,
            i - 1
        );
    }
    chain += r#"<rect width="9" height="9" clip-path="url(#c4999)"/>"#;
    assert_eq!(refusal(&wrap(&chain)), SvgRefusal::TooManyClips(5000));
    let cycle = wrap(
        r#"<clipPath id="a" clip-path="url(#b)"><rect width="9" height="9"/></clipPath><clipPath id="b" clip-path="url(#a)"><rect width="9" height="9"/></clipPath><rect width="9" height="9" clip-path="url(#a)"/>"#,
    );
    let _ = import(&cycle, (96, 96), ImageFit::Contain);
}

/// A cover and a one-to-one placement are centred, and the one-to-one
/// overflow is noted.
#[test]
fn svg_cover_and_dots_are_centred() {
    let half = format!(r#"<svg {NS} width="100" height="50"><rect width="50" height="50"/></svg>"#);
    let i = import(&half, (50, 50), ImageFit::Cover).unwrap();
    assert_eq!((i.art.w, i.art.h), (50, 50));
    assert!(i.art.px[25 * 50 + 10] == 0 && i.art.px[25 * 50 + 40] == 255);
    assert_eq!(
        i.notes,
        vec![ImportNote::CoverCropped {
            from: (100, 50),
            to: (50, 50)
        }]
    );
    let i = import(&half, (40, 40), ImageFit::Dots).unwrap();
    assert_eq!((i.art.w, i.art.h), (40, 40));
    assert!(i.art.px[20 * 40 + 10] == 0 && i.art.px[20 * 40 + 30] == 255);
    assert_eq!(
        i.notes,
        vec![ImportNote::Cropped {
            from: (100, 50),
            to: (40, 40)
        }]
    );
}

/// An SVG raster over the dot budget is refused, and a byte-order mark does
/// not hide an SVG.
#[test]
fn svg_size_budget_and_byte_order_mark() {
    let big = format!(r#"<svg {NS} width="600" height="600"><rect width="9" height="9"/></svg>"#);
    assert!(matches!(
        artwork::import_within(big.as_bytes(), (None, None), Dpmm::DPI_203),
        Err(ArtworkError::Svg(SvgRefusal::TooManyDots {
            w: 600,
            h: 600
        }))
    ));
    let bom = format!(
        "{}\n  <svg {NS} width=\"9\" height=\"9\"><rect width=\"9\" height=\"9\"/></svg>",
        '\u{feff}'
    );
    assert!(import(&bom, (9, 9), ImageFit::Contain).is_ok());
}
