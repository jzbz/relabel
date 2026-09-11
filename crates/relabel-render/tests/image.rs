//! Drawing image elements: placement under every fit, rotation and printer,
//! what lies beneath, the halftone override, and every way stored artwork can
//! fail to be drawn without taking the label with it.

mod common;
#[path = "common/fixtures.rs"]
mod fixtures;

use fixtures::*;
use relabel_proto::{BitPlane, PrintDirection};
use relabel_render::artwork::{self, Artwork};
use relabel_render::document::*;
use relabel_render::{
    Dpmm, GreyMap, RenderReport, Target, Zone, local_size, rasterise, rasterise_plane, zone_table,
};

const ROTS: [Rot; 4] = [Rot::D0, Rot::D90, Rot::D180, Rot::D270];

fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

fn image(id: u32, f: RectDots, a: &Artwork, fit: ImageFit) -> ImageEl {
    ImageEl {
        common: Common::new(id, f),
        path: String::new(),
        png: stored(a),
        fit,
        halftone: None,
        threshold: None,
    }
}

fn label_on(feed: i32, head: i32, els: Vec<Element>) -> Label {
    let mut l = Label::new(feed, head, Dpmm::DPI_203);
    l.elements = els.into_iter().map(ElementOrRaw::Known).collect();
    l
}

fn target(dir: PrintDirection, feed: i32, head: i32) -> Target {
    Target::for_label(head as u16, feed as u16, 203, dir).unwrap()
}

fn render_on(
    dir: PrintDirection,
    feed: i32,
    head: i32,
    els: Vec<Element>,
) -> (GreyMap, RenderReport) {
    rasterise(&label_on(feed, head, els), &target(dir, feed, head)).unwrap()
}

fn render(els: Vec<Element>) -> (GreyMap, RenderReport) {
    render_on(PrintDirection::Left, 320, 96, els)
}

fn plane(els: Vec<Element>) -> BitPlane {
    let l = label_on(320, 96, els);
    rasterise_plane(&l, &target(PrintDirection::Left, 320, 96)).unwrap()
}

fn rect(id: u32, f: RectDots, stroke: u16) -> Element {
    Element::Rect(RectEl {
        common: Common::new(id, f),
        stroke_dots: stroke,
        radius_dots: 0,
    })
}

/// Asymmetric, so a flip or a wrong turn shows: an L along the top and left
/// edges, a diagonal, and a grey block in one corner.
fn pattern(w: u32, h: u32) -> Artwork {
    art(w, h, |x, y| {
        if x == 0 || y == 0 || x * h == y * w {
            0
        } else if x > w * 2 / 3 && y > h * 2 / 3 {
            96
        } else {
            255
        }
    })
}

type Buf = (u16, u16, Vec<u8>);

fn crop(src: &[u8], stride: u16, x0: u16, y0: u16, w: u16, h: u16) -> Buf {
    let mut out = Vec::new();
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            out.push(src[usize::from(y) * usize::from(stride) + usize::from(x)]);
        }
    }
    (w, h, out)
}

/// `dst(x, y) = src(y, h - 1 - x)`, written independently of the crate.
fn rot_cw(b: &Buf) -> Buf {
    let (w, h, src) = (usize::from(b.0), usize::from(b.1), &b.2);
    let mut out = vec![0u8; w * h];
    for y in 0..w {
        for x in 0..h {
            out[y * h + x] = src[(h - 1 - x) * w + y];
        }
    }
    (b.1, b.0, out)
}

/// The frozen corpus's image has no embedded artwork - it predates it - and
/// is reported as such, not as unimplemented.
#[test]
fn an_image_without_artwork_is_reported_not_unimplemented() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/v1/kitchen-sink.relabel"),
    )
    .unwrap();
    let l = Label::from_json(&text).unwrap();
    let (_, r) = rasterise(&l, &Target::new(320, 96, 203).unwrap()).unwrap();
    let seven = r.failed.iter().find(|f| f.0 == 7).expect("element 7 fails");
    assert!(
        seven.1.to_string().contains("no embedded artwork"),
        "{}",
        seven.1
    );
    assert!(seven.1.to_string().contains("logo.png"), "{}", seven.1);
}

/// Artwork sized for its frame is copied dot for dot: grey levels exact, and
/// nothing reported.
#[test]
fn artwork_sized_for_its_frame_is_copied_exactly() {
    let a = pattern(64, 32);
    let (g, r) = render(vec![Element::Image(image(
        1,
        frame(10, 20, 64, 32),
        &a,
        ImageFit::Contain,
    ))]);
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(crop(&g.px, 320, 10, 20, 64, 32).2, a.px);
    for y in 0..96u16 {
        for x in 0..320u16 {
            if !((10..74).contains(&x) && (20..52).contains(&y)) {
                assert_eq!(g.at(x, y), 255, "({x}, {y})");
            }
        }
    }
}

/// White leaves what lies beneath alone: a logo with a white surround,
/// placed over a border, keeps the border.
#[test]
fn white_artwork_leaves_what_lies_beneath() {
    let logo = art(96, 96, |x, y| {
        let (dx, dy) = (i64::from(x) - 48, i64::from(y) - 48);
        if dx * dx + dy * dy < 30 * 30 { 0 } else { 255 }
    });
    let els = vec![
        rect(1, frame(0, 0, 320, 96), 3),
        Element::Image(image(2, frame(0, 0, 96, 96), &logo, ImageFit::Contain)),
    ];
    let p = plane(els.clone());
    assert!(
        p.get(1, 48) && p.get(48, 1),
        "the border shows through the surround"
    );
    assert!(p.get(48, 48), "the disc prints");
    assert!(!p.get(20, 20));
    common::check("image-logo-over-border", &p);
}

/// With white ink, the artwork's dark parts knock back to paper: a logo
/// reversed out of a black panel, or a photographic negative.
#[test]
fn white_ink_knocks_out_and_makes_a_negative() {
    let ramp = art(64, 16, |x, _| (x * 4) as u8);
    let mut e = image(2, frame(0, 0, 64, 16), &ramp, ImageFit::Contain);
    e.common.ink = Ink::White;
    let (g, r) = render(vec![rect(1, frame(0, 0, 64, 16), 0), Element::Image(e)]);
    assert!(r.is_clean(), "{r:?}");
    for x in 0..64u16 {
        let want = 255 - i32::from(ramp.px[usize::from(x)]);
        let got = i32::from(g.at(x, 5));
        assert!((got - want).abs() <= 1, "{x}: {got} against {want}");
    }
}

/// Each fit, for artwork of one shape in a frame of another. Drawn from
/// artwork not sized for the frame, so each is also a resampling report.
#[test]
fn every_fit_places_the_artwork() {
    let a = pattern(64, 32);
    for (fit, name, want) in [
        (ImageFit::Contain, "contain", Some((96, 48))),
        (ImageFit::Cover, "cover", Some((192, 96))),
        (ImageFit::Stretch, "stretch", Some((96, 96))),
        (ImageFit::Dots, "dots", None),
    ] {
        let els = vec![
            rect(1, frame(100, 0, 96, 96), 1),
            Element::Image(image(2, frame(100, 0, 96, 96), &a, fit.clone())),
        ];
        let (_, r) = render(els.clone());
        match want {
            Some(to) => assert_eq!(r.image_resampled, vec![(2, (64, 32), to)], "{name}"),
            None => assert!(
                r.image_resampled.is_empty() && r.image_clipped.is_empty(),
                "{name}: {r:?}"
            ),
        }
        common::check(&format!("image-fit-{name}"), &plane(els));
    }
}

/// One pixel to one dot, larger than the frame: cropped about the centre,
/// and reported.
#[test]
fn dots_larger_than_the_frame_are_cropped_and_reported() {
    let a = art(64, 64, |x, y| if x == 32 && y == 32 { 0 } else { 255 });
    let (g, r) = render(vec![Element::Image(image(
        3,
        frame(0, 0, 32, 32),
        &a,
        ImageFit::Dots,
    ))]);
    assert_eq!(r.image_clipped, vec![3]);
    assert!(!r.is_clean());
    assert_eq!(g.at(16, 16), 0, "the centre stays the centre");
    assert_eq!(g.px.iter().filter(|&&v| v == 0).count(), 1);
}

/// A frame changed since import redraws the artwork at the new size, and
/// says so: re-importing is the fix.
#[test]
fn a_changed_frame_is_resampled_and_reported() {
    let a = pattern(64, 32);
    let (_, r) = render(vec![Element::Image(image(
        4,
        frame(0, 0, 128, 64),
        &a,
        ImageFit::Contain,
    ))]);
    assert_eq!(r.image_resampled, vec![(4, (64, 32), (128, 64))]);
    assert!(!r.is_clean());
}

/// On a `Top` target the document's axes swap onto the canvas. The same
/// canvas rectangle described in each target's coordinates comes out
/// identical in every rotation.
#[test]
fn a_top_target_draws_the_same_dots_as_a_left_one() {
    let a = pattern(40, 30);
    for rot in ROTS {
        let els = |dir| {
            let f = match dir {
                PrintDirection::Left => frame(8, 4, 80, 60),
                PrintDirection::Top => frame(4, 8, 60, 80),
            };
            let mut e = image(1, f, &a, ImageFit::Contain);
            e.common.rot = rot;
            vec![Element::Image(e)]
        };
        let (left, rl) = render_on(PrintDirection::Left, 96, 96, els(PrintDirection::Left));
        let (top, rt) = render_on(PrintDirection::Top, 96, 96, els(PrintDirection::Top));
        assert_eq!(rl, rt, "{rot:?}");
        assert_eq!(left.px, top.px, "{rot:?}");
        assert_eq!(left.zone, top.zone, "{rot:?}");
        assert!(left.px.iter().any(|&p| p != 255), "{rot:?}: drawn");
    }
}

/// A turned element is the upright one turned, dot for dot, against a
/// rotation written in this file.
#[test]
fn quarter_turns_are_the_upright_artwork_turned() {
    let f = frame(8, 4, 80, 60);
    let upright_size =
        local_size(&Common::new(1, f), &target(PrintDirection::Left, 96, 96)).unwrap();
    assert_eq!(upright_size, (80, 60));
    let a = pattern(80, 60);
    let el = |rot| {
        let mut e = image(1, f, &a, ImageFit::Contain);
        e.common.rot = rot;
        Element::Image(e)
    };
    let (g0, _) = render_on(PrintDirection::Left, 96, 96, vec![el(Rot::D0)]);
    let up = crop(&g0.px, 96, 8, 4, 80, 60);
    assert_eq!(up.2, a.px);
    for (turns, rot) in ROTS.into_iter().enumerate() {
        let (g, r) = render_on(PrintDirection::Left, 96, 96, vec![el(rot)]);
        assert!(
            r.image_resampled.is_empty(),
            "{rot:?}: the local frame is upright"
        );
        let mut want = up.clone();
        for _ in 0..turns {
            want = rot_cw(&want);
        }
        assert_eq!(crop(&g.px, 96, 8, 4, want.0, want.1), want, "{rot:?}");
    }
}

/// The per-element halftone reaches the one-bit plane: a flat mid-grey
/// dithered comes out about half inked, where the page's threshold would ink
/// all of it.
#[test]
fn an_images_halftone_reaches_the_plane() {
    let grey = art(64, 64, |_, _| 128);
    let mut e = image(1, frame(0, 0, 64, 64), &grey, ImageFit::Contain);
    e.halftone = Some(ImageHalftone::FloydSteinberg);
    let p = plane(vec![Element::Image(e.clone())]);
    let inked = (0..64u16)
        .flat_map(|y| (0..64u16).map(move |x| (x, y)))
        .filter(|&(x, y)| p.get(x, y))
        .count();
    assert!((1843..=2253).contains(&inked), "{inked} of 4096 inked");

    e.halftone = None;
    let p = plane(vec![Element::Image(e)]);
    let inked = (0..64u16)
        .flat_map(|y| (0..64u16).map(move |x| (x, y)))
        .filter(|&(x, y)| p.get(x, y))
        .count();
    assert_eq!(
        inked, 4096,
        "the page's threshold of 128 inks a grey of 128"
    );
}

/// An image's zone is only the dots it inks, so its settings never reach
/// the anti-aliased text beneath its white.
#[test]
fn an_images_settings_stay_off_what_lies_beneath() {
    let text = || {
        let mut t = TextEl {
            common: Common::new(1, frame(0, 0, 320, 96)),
            text: "Widget 42".into(),
            family: String::new(),
            size: 40.0,
            line_height: 1.0,
            align: TextAlign::Start,
            weight: TextWeight::Regular,
            raster: GlyphRaster::Smooth,
        };
        t.common.name = "beneath".into();
        Element::Text(t)
    };
    let dot = art(100, 96, |x, y| {
        if (45..55).contains(&x) && (43..53).contains(&y) {
            0
        } else {
            255
        }
    });
    let mut e = image(2, frame(0, 0, 100, 96), &dot, ImageFit::Contain);
    e.halftone = Some(ImageHalftone::Threshold);
    e.threshold = Some(250);
    let alone = plane(vec![text()]);
    let under = plane(vec![text(), Element::Image(e)]);
    let mut grey_dots = 0;
    let (g, _) = render(vec![text()]);
    for y in 0..96u16 {
        for x in 0..320u16 {
            if (45..55).contains(&x) && (43..53).contains(&y) {
                continue;
            }
            assert_eq!(alone.get(x, y), under.get(x, y), "({x}, {y})");
            if x < 100 && (129..250).contains(&g.at(x, y)) {
                grey_dots += 1;
            }
        }
    }
    assert!(
        grey_dots > 20,
        "the test needs grey text under the image: {grey_dots}"
    );
}

/// The zone table has the image's settings at the image's id, and the page's
/// everywhere else.
#[test]
fn the_zone_table_matches_the_painted_ids() {
    let a = art(16, 16, |_, _| 0);
    let mut e = image(20, frame(40, 0, 16, 16), &a, ImageFit::Contain);
    e.halftone = Some(ImageHalftone::Atkinson);
    e.threshold = Some(99);
    let els = vec![
        rect(10, frame(0, 0, 16, 16), 0),
        Element::Image(e),
        rect(30, frame(80, 0, 16, 16), 0),
    ];
    let l = label_on(320, 96, els.clone());
    let zones = zone_table(&l, Zone::PAGE);
    assert_eq!(zones.len(), 4);
    assert_eq!(zones[1], Zone::PAGE);
    assert_eq!(zones[2].halftone, relabel_render::Halftone::Atkinson);
    assert_eq!(zones[2].threshold, 99);
    assert_eq!(zones[3], Zone::PAGE);
    let (g, _) = render(els);
    assert_eq!(g.zone[8 * 320 + 48], 2, "the image paints its own id");
    assert_eq!(g.zone[8 * 320 + 8], 1);
}

/// Past 254 elements ids are shared. An image whose override then cannot be
/// honoured is reported, and the shared entry keeps the page's settings, so
/// the elements after it are not binarised with the image's.
#[test]
fn an_override_on_a_shared_zone_is_reported() {
    let a = art(1, 1, |_, _| 0);
    let mut els: Vec<Element> = (1..=253).map(|i| rect(i, frame(0, 0, 1, 1), 0)).collect();
    let mut e = image(900, frame(2, 2, 1, 1), &a, ImageFit::Contain);
    e.halftone = Some(ImageHalftone::FloydSteinberg);
    e.threshold = Some(40);
    els.push(Element::Image(e));
    els.extend((1..=3).map(|i| rect(1000 + i, frame(4, 4, 1, 1), 0)));
    let l = label_on(320, 96, els.clone());
    assert_eq!(zone_table(&l, Zone::PAGE).len(), 255);
    assert_eq!(zone_table(&l, Zone::PAGE)[254], Zone::PAGE);
    let (_, r) = render(els);
    assert_eq!(r.zone_shared, vec![900]);
    assert!(!r.is_clean());
}

/// A value from a later build fails that element only, and survives a save.
#[test]
fn unknown_values_fail_only_their_element() {
    let a = art(8, 8, |_, _| 0);
    let mut fit = image(1, frame(0, 0, 8, 8), &a, ImageFit::Unknown("tile".into()));
    fit.common.name = "fit".into();
    let mut ht = image(2, frame(10, 0, 8, 8), &a, ImageFit::Contain);
    ht.halftone = Some(ImageHalftone::Unknown("blue_noise".into()));
    let ok = image(3, frame(20, 0, 8, 8), &a, ImageFit::Contain);
    let els = vec![Element::Image(fit), Element::Image(ht), Element::Image(ok)];
    let (g, r) = render(els.clone());
    let failed: Vec<(u32, bool)> = r
        .failed
        .iter()
        .map(|(id, why)| {
            (
                *id,
                why.to_string().contains("tile") || why.to_string().contains("blue_noise"),
            )
        })
        .collect();
    assert_eq!(failed, vec![(1, true), (2, true)], "{r:?}");
    assert_eq!(g.at(24, 4), 0, "the third is drawn");
    let l = label_on(320, 96, els);
    let back = Label::from_json(&l.to_json().unwrap()).unwrap();
    assert_eq!(back, l);
}

/// Damaged artwork fails its element, and is kept exactly as it was.
#[test]
fn damaged_artwork_fails_only_its_element() {
    let a = art(8, 8, |_, _| 0);
    let mut bad64 = image(1, frame(0, 0, 8, 8), &a, ImageFit::Contain);
    bad64.png = "not base64 at all!".into();
    let mut not_png = image(2, frame(10, 0, 8, 8), &a, ImageFit::Contain);
    not_png.png = "aGVsbG8=".into();
    let mut cut = image(3, frame(20, 0, 8, 8), &a, ImageFit::Contain);
    let whole =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cut.png).unwrap();
    cut.png = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &whole[..whole.len() / 2],
    );
    let mut long = image(5, frame(40, 0, 8, 8), &a, ImageFit::Contain);
    long.png = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        png_bomb(9000, 1, 8, 0),
    );
    let ok = image(4, frame(30, 0, 8, 8), &a, ImageFit::Contain);
    let els: Vec<Element> = [bad64, not_png, cut, long, ok]
        .into_iter()
        .map(Element::Image)
        .collect();
    let (g, r) = render(els.clone());
    let ids: Vec<u32> = r.failed.iter().map(|f| f.0).collect();
    assert_eq!(ids, vec![1, 2, 3, 5], "{r:?}");
    assert!(
        r.failed[3].1.to_string().contains("9000x1"),
        "the refusal names the size: {r:?}"
    );
    assert!(r.failed[0].1.to_string().contains("base64"), "{r:?}");
    assert!(r.failed[1].1.to_string().contains("not a PNG"), "{r:?}");
    assert_eq!(g.at(34, 4), 0);
    let l = label_on(320, 96, els);
    assert_eq!(Label::from_json(&l.to_json().unwrap()).unwrap(), l);
}

/// A stored PNG that is not 8-bit grey - a later build's, or an editor's -
/// is drawn as import would have drawn the same file.
#[test]
fn any_stored_png_draws_as_import_would() {
    let rgba = png_rgba(32, 32, |x, y| {
        [
            (x * 8) as u8,
            (y * 8) as u8,
            128,
            if x < 16 { 255 } else { 100 },
        ]
    });
    let as_stored = ImageEl {
        common: Common::new(1, frame(0, 0, 32, 32)),
        path: String::new(),
        png: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &rgba),
        fit: ImageFit::Contain,
        halftone: None,
        threshold: None,
    };
    let imported = artwork::import(&rgba, (32, 32), &ImageFit::Contain, Dpmm::DPI_203)
        .unwrap()
        .into_element(
            Common::new(1, frame(0, 0, 32, 32)),
            String::new(),
            ImageFit::Contain,
        )
        .unwrap();
    let (a, ra) = render(vec![Element::Image(as_stored)]);
    let (b, _) = render(vec![Element::Image(imported)]);
    assert!(ra.is_clean(), "{ra:?}");
    assert_eq!(a.px, b.px);
}

/// An image's own settings reach every dot of its box, white included: a
/// threshold of 255 inks the whole box, and nothing outside it.
#[test]
fn an_images_settings_reach_its_white() {
    let white = art(16, 16, |_, _| 255);
    let mut e = image(1, frame(0, 0, 32, 16), &white, ImageFit::Contain);
    e.halftone = Some(ImageHalftone::Threshold);
    e.threshold = Some(255);
    let p = plane(vec![Element::Image(e)]);
    for y in 0..16u16 {
        for x in 0..40u16 {
            assert_eq!(p.get(x, y), (8..24).contains(&x), "({x}, {y})");
        }
    }
}

/// Where the artwork is within two levels of white, anything already beneath
/// keeps its own zone: the faint surround of a scanned or JPEG logo does not
/// hand the text beneath it to the image's threshold.
#[test]
fn a_near_white_surround_leaves_the_zone_beneath() {
    let text = Element::Text(TextEl {
        common: Common::new(1, frame(0, 0, 320, 96)),
        text: "Widget 42".into(),
        family: String::new(),
        size: 40.0,
        line_height: 1.0,
        align: TextAlign::Start,
        weight: TextWeight::Regular,
        raster: GlyphRaster::Smooth,
    });
    let faint = art(100, 96, |x, y| {
        if (45..55).contains(&x) && (43..53).contains(&y) {
            0
        } else {
            254
        }
    });
    let mut e = image(2, frame(0, 0, 100, 96), &faint, ImageFit::Contain);
    e.threshold = Some(250);
    let (alone, _) = render(vec![text.clone()]);
    let (both, _) = render(vec![text, Element::Image(e)]);
    let mut checked = 0;
    for y in 0..96u16 {
        for x in 0..100u16 {
            let i = usize::from(y) * 320 + usize::from(x);
            if (45..55).contains(&x) && (43..53).contains(&y) {
                assert_eq!(both.zone[i], 2, "({x}, {y}): the image's own ink");
            } else if alone.zone[i] == 1 {
                assert_eq!(both.zone[i], 1, "({x}, {y}): the text keeps its zone");
                checked += 1;
            } else {
                assert_eq!(both.zone[i], 2, "({x}, {y}): bare paper joins the image");
            }
        }
    }
    assert!(checked > 100, "{checked}");
}

/// An odd overflow is cropped the same way at import and when drawing, so
/// importing again for a frame keeps the dots a stale draw showed.
#[test]
fn odd_overflow_crops_alike_at_import_and_draw() {
    let codes = |x: u32, y: u32| ((x * 7 + y * 13) % 251) as u8;
    let big = art(65, 65, codes);
    let (g, _) = render(vec![Element::Image(image(
        1,
        frame(0, 0, 32, 32),
        &big,
        ImageFit::Dots,
    ))]);
    let file = png_l8(65, 65, codes);
    let i = artwork::import(&file, (32, 32), &ImageFit::Dots, Dpmm::DPI_203).unwrap();
    assert_eq!(crop(&g.px, 320, 0, 0, 32, 32).2, i.art.px);
}

/// A frame too large to draw fails its element, whatever the artwork.
#[test]
fn a_frame_too_large_to_draw_fails_only_its_element() {
    let a = art(1, 1, |_, _| 0);
    let els = vec![
        Element::Image(image(1, frame(0, 0, 8192, 8), &a, ImageFit::Contain)),
        Element::Image(image(2, frame(0, 0, 3000, 2000), &a, ImageFit::Contain)),
        Element::Image(image(3, frame(0, 0, 8, 8), &a, ImageFit::Contain)),
    ];
    let (_, r) = render(els);
    let failed: Vec<(u32, bool)> = r
        .failed
        .iter()
        .map(|(id, why)| (*id, why.to_string().contains("too large")))
        .collect();
    assert_eq!(failed, vec![(1, true), (2, true)], "{r:?}");
}
