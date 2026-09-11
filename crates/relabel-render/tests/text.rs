//! Text on the label: the engine's refusals, placement under rotation and on
//! both kinds of printer, clipping, hinting, and golden images.

mod common;

use parley::PositionedLayoutItem;
use relabel_proto::{BitPlane, PrintDirection};
use relabel_render::canvas::{Canvas, blend};
use relabel_render::coverage::Coverage;
use relabel_render::document::*;
use relabel_render::text::{self, MONO, SANS, TextEngine, TextError, TextStyle};
use relabel_render::{Dpmm, GreyMap, RenderReport, Target, rasterise};
use skrifa::MetadataProvider;
use tiny_skia::{Color, FillRule, Mask, Pixmap, Transform};

const TEXT: &str = "Shelf B4 Widget 0O1lI8B gypsy QUIZ";
const ROTS: [Rot; 4] = [Rot::D0, Rot::D90, Rot::D180, Rot::D270];
const RASTERS: [GlyphRaster; 2] = [GlyphRaster::Hinted, GlyphRaster::Smooth];
const LIMIT: u64 = text::MAX_LOCAL_DOTS;

fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

fn text_el(id: u32, s: &str, size: f32, f: RectDots) -> TextEl {
    TextEl {
        common: Common::new(id, f),
        text: s.into(),
        family: String::new(),
        size,
        line_height: 1.0,
        align: TextAlign::Start,
        weight: TextWeight::Regular,
        raster: GlyphRaster::Hinted,
    }
}

fn panel(id: u32, f: RectDots) -> Element {
    Element::Rect(RectEl {
        common: Common::new(id, f),
        stroke_dots: 0,
        radius_dots: 0,
    })
}

/// A label `feed` x `head` on a target of either direction.
fn render_on(
    dir: PrintDirection,
    feed: i32,
    head: i32,
    els: Vec<Element>,
) -> (GreyMap, RenderReport) {
    let mut l = Label::new(feed, head, Dpmm::DPI_203);
    l.elements = els.into_iter().map(ElementOrRaw::Known).collect();
    let t = Target::for_label(head as u16, feed as u16, 203, dir).unwrap();
    rasterise(&l, &t).unwrap()
}

fn render(els: Vec<Element>) -> (GreyMap, RenderReport) {
    render_on(PrintDirection::Left, 320, 96, els)
}

/// (w, h, bytes) cut out of a row-major buffer `stride` wide.
type Buf = (u16, u16, Vec<u8>);

fn crop(src: &[u8], stride: u16, x0: u16, y0: u16, w: u16, h: u16) -> Buf {
    let mut out = Vec::with_capacity(usize::from(w) * usize::from(h));
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            out.push(src[usize::from(y) * usize::from(stride) + usize::from(x)]);
        }
    }
    (w, h, out)
}

/// Quarter turn clockwise by direct index math, written independently of the
/// crate's own: `dst(x, y) = src(y, h - 1 - x)`.
fn rot_cw(b: &Buf) -> Buf {
    let (w, h, src) = (usize::from(b.0), usize::from(b.1), &b.2);
    let (nw, nh) = (h, w);
    let mut out = vec![0u8; nw * nh];
    for y in 0..nh {
        for x in 0..nw {
            out[y * nw + x] = src[(h - 1 - x) * w + y];
        }
    }
    (b.1, b.0, out)
}

fn plane(b: &Buf) -> BitPlane {
    GreyMap::from_px(b.0, b.1, b.2.clone()).threshold(128)
}

fn quarters(r: Rot) -> usize {
    match r {
        Rot::D90 => 1,
        Rot::D180 => 2,
        Rot::D270 => 3,
        _ => 0,
    }
}

/// The inked bounding box of a plane, (x0, y0, x1, y1) exclusive.
fn ink_box(p: &BitPlane) -> Option<(u16, u16, u16, u16)> {
    let mut b: Option<(u16, u16, u16, u16)> = None;
    for y in 0..p.height() {
        for x in 0..p.width() {
            if p.get(x, y) {
                b = Some(match b {
                    None => (x, y, x + 1, y + 1),
                    Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x + 1), e.max(y + 1)),
                });
            }
        }
    }
    b
}

fn style(ppem: f32) -> TextStyle<'static> {
    TextStyle {
        family: "",
        ppem,
        line_height: 1.0,
        weight: TextWeight::Regular,
        align: TextAlign::Start,
        raster: GlyphRaster::Hinted,
    }
}

fn failed_with(r: &RenderReport, what: &str) -> bool {
    r.failed.len() == 1 && r.failed[0].1.contains(what)
}

/// The embedded faces are the upstream files `THIRD-PARTY.md` names: Noto
/// Sans 2.015 and Noto Sans Mono 2.014, as released.
#[test]
fn the_embedded_faces_are_the_recorded_upstream_files() {
    for (bytes, len, version) in [
        (
            text::SANS_REGULAR,
            617_784,
            "Version 2.015; ttfautohint (v1.8.4.7-5d5b)",
        ),
        (
            text::SANS_SEMIBOLD,
            620_208,
            "Version 2.015; ttfautohint (v1.8.4.7-5d5b)",
        ),
        (
            text::MONO_REGULAR,
            596_712,
            "Version 2.014; ttfautohint (v1.8.4.7-5d5b)",
        ),
    ] {
        assert_eq!(bytes.len(), len);
        let f = skrifa::FontRef::new(bytes).unwrap();
        let v = f
            .localized_strings(skrifa::string::StringId::VERSION_STRING)
            .english_or_first()
            .unwrap()
            .to_string();
        assert_eq!(v, version);
    }
}

/// Parley itself never errors: with no font it lays text out in nothing at
/// all. The engine turns each such case into an error before any layout.
#[test]
fn an_engine_without_usable_fonts_is_an_error() {
    assert_eq!(
        TextEngine::with_faces(&[], &[text::MONO_REGULAR]).err(),
        Some(TextError::GenericUnresolved("sans-serif"))
    );
    assert_eq!(
        TextEngine::with_faces(&[text::SANS_REGULAR], &[]).err(),
        Some(TextError::GenericUnresolved("monospace"))
    );
    let junk: &'static [u8] = b"this is not a font file at all";
    assert_eq!(
        TextEngine::with_faces(&[junk], &[text::MONO_REGULAR]).err(),
        Some(TextError::FontRejected { index: 0 })
    );
    assert_eq!(
        TextEngine::with_faces(&[text::SANS_REGULAR], &[junk]).err(),
        Some(TextError::FontRejected { index: 1 }),
        "mono faces are numbered after the sans ones"
    );
    let truncated: &'static [u8] = &text::SANS_REGULAR[..1024];
    assert!(TextEngine::with_faces(&[truncated], &[text::MONO_REGULAR]).is_err());
    assert!(TextEngine::new().is_ok());
}

/// A copy of a face that declares itself normal width in OS/2 `usWidthClass`,
/// its outlines untouched.
fn normal_width(bytes: &[u8]) -> &'static [u8] {
    let mut v = bytes.to_vec();
    let tables = usize::from(u16::from_be_bytes([v[4], v[5]]));
    let os2 = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&r| &v[r..r + 4] == b"OS/2")
        .map(|r| u32::from_be_bytes([v[r + 8], v[r + 9], v[r + 10], v[r + 11]]) as usize)
        .unwrap();
    assert_eq!(
        u16::from_be_bytes([v[os2 + 6], v[os2 + 7]]),
        3,
        "condensed to begin with"
    );
    v[os2 + 6..os2 + 8].copy_from_slice(&5u16.to_be_bytes());
    Box::leak(v.into_boxed_slice())
}

/// Where the bytes of each face that drew a layout live.
fn faces_used(eng: &mut TextEngine, s: &str, st: &TextStyle<'_>) -> Vec<usize> {
    let layout = eng.layout(s, st, 320.0).unwrap();
    let mut out = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(run) = item {
                let at = run.run().font().data.data().as_ptr() as usize;
                if !out.contains(&at) {
                    out.push(at);
                }
            }
        }
    }
    out
}

/// Faces are asked for by width, so a normal-width "Noto Sans" registered
/// beside the condensed one - an imported font, one day - does not take over,
/// whichever was registered first.
#[test]
fn the_condensed_face_is_chosen_over_a_normal_width_one() {
    let wide = normal_width(text::SANS_REGULAR);
    let condensed = text::SANS_REGULAR.as_ptr() as usize;
    for sans in [[wide, text::SANS_REGULAR], [text::SANS_REGULAR, wide]] {
        let mut eng = TextEngine::with_faces(&sans, &[text::MONO_REGULAR]).unwrap();
        assert_eq!(
            faces_used(&mut eng, "Widget", &style(20.0)),
            vec![condensed]
        );
    }
    // The copy registers, and is used when it is all there is.
    let mut eng = TextEngine::with_faces(&[wide], &[text::MONO_REGULAR]).unwrap();
    assert_eq!(
        faces_used(&mut eng, "Widget", &style(20.0)),
        vec![wide.as_ptr() as usize]
    );
}

/// Advances are the font's own, measured once and pinned: a change here is a
/// change in the font or the shaper, and every label's line breaks move.
#[test]
fn a_known_string_has_its_measured_width() {
    let mut eng = TextEngine::new().unwrap();
    let layout = eng.layout("Widget 12mm", &style(20.0), 320.0).unwrap();
    let w = layout.width();
    assert!((w - 108.86).abs() < 0.05, "{w}");
}

/// A character no embedded face has is named and refused, not drawn as the
/// missing-glyph box.
#[test]
fn characters_the_fonts_lack_are_refused_by_name() {
    let (g, r) = render(vec![Element::Text(text_el(
        1,
        "Price 日本",
        16.0,
        frame(0, 0, 320, 96),
    ))]);
    assert!(failed_with(&r, "U+65E5"), "{r:?}");
    assert!(g.px.iter().all(|&p| p == 255), "nothing drawn");
    // Greek and Cyrillic are in the faces.
    let (_, r) = render(vec![Element::Text(text_el(
        1,
        "Ωμ Привет €5",
        16.0,
        frame(0, 0, 320, 96),
    ))]);
    assert!(r.is_clean(), "{r:?}");
}

/// Coverage is checked against the faces the style can reach: a symbol only
/// the mono face has is refused in sans, by name, and drawn in mono.
#[test]
fn coverage_follows_the_family() {
    let sans: Vec<_> = [text::SANS_REGULAR, text::SANS_SEMIBOLD]
        .iter()
        .map(|b| skrifa::FontRef::new(b).unwrap())
        .collect();
    let mono = skrifa::FontRef::new(text::MONO_REGULAR).unwrap();
    let only_mono = mono
        .charmap()
        .mappings()
        .map(|(c, _)| c)
        .filter(|c| (0x2070..=0x2BFF).contains(c))
        .filter_map(char::from_u32)
        .find(|&c| sans.iter().all(|f| f.charmap().map(c).is_none()))
        .expect("the mono face has a symbol the sans faces lack");
    let s = format!("A{only_mono}B");
    let named = format!("U+{:04X}", u32::from(only_mono));
    let mut eng = TextEngine::new().unwrap();
    let err = eng.layout(&s, &style(16.0), 320.0).unwrap_err();
    assert!(
        matches!(&err, TextError::MissingGlyphs { family: SANS, chars } if chars.contains(&named)),
        "{err:?}"
    );
    let m = TextStyle {
        family: MONO,
        ..style(16.0)
    };
    eng.layout(&s, &m, 320.0).unwrap();
}

/// A control character has a glyph in no face, so it is named rather than
/// drawn as a box. Every kind of line separator is a line break, and the
/// invisible characters a keyboard inserts are left to the shaper, which
/// hides them.
#[test]
fn controls_separators_and_invisible_characters() {
    let mut eng = TextEngine::new().unwrap();
    let err = eng.layout("a\u{7}b", &style(16.0), 320.0).unwrap_err();
    assert!(
        matches!(&err, TextError::MissingGlyphs { chars, .. } if chars.contains("U+0007")),
        "{err:?}"
    );
    let f = frame(0, 0, 320, 96);
    let draw = |s: &str| {
        let (g, r) = render(vec![Element::Text(text_el(1, s, 16.0, f))]);
        assert!(r.is_clean(), "{s:?}: {r:?}");
        g.px
    };
    let broken = draw("a\nb");
    for sep in ['\u{B}', '\u{C}', '\u{85}', '\u{2028}', '\u{2029}'] {
        assert_eq!(draw(&format!("a{sep}b")), broken, "{sep:?}");
    }
    assert_eq!(
        draw("\u{A9}\u{FE0F} 2026"),
        draw("\u{A9} 2026"),
        "a variation selector"
    );
    assert_eq!(draw("a\u{AD}b"), draw("ab"), "a soft hyphen");
}

#[test]
fn tabs_and_line_endings_are_normalised() {
    let f = frame(0, 0, 320, 96);
    let draw = |s: &str| render(vec![Element::Text(text_el(1, s, 16.0, f))]).0;
    assert_eq!(draw("a\tb").px, draw("a b").px, "a tab is a space");
    assert_eq!(draw("a\r\nb").px, draw("a\nb").px, "CRLF is one break");
    assert_eq!(draw("a\rb").px, draw("a\nb").px, "and so is a lone CR");
    assert_ne!(draw("a\nb").px, draw("a b").px);
}

#[test]
fn empty_text_draws_nothing_and_is_clean() {
    for s in ["", "\n", "   "] {
        let (g, r) = render(vec![Element::Text(text_el(
            1,
            s,
            16.0,
            frame(0, 0, 320, 96),
        ))]);
        assert!(r.is_clean(), "{s:?}: {r:?}");
        assert!(g.px.iter().all(|&p| p == 255), "{s:?}");
    }
}

/// Only lines with something to draw are counted: a trailing newline in a
/// frame one line deep is not text lost. A leading one is.
#[test]
fn trailing_blank_lines_are_not_clipped_text() {
    let f = frame(0, 0, 320, 16);
    for s in ["WIDE", "WIDE\n", "WIDE\n\n\n"] {
        let (_, r) = render(vec![Element::Text(text_el(1, s, 16.0, f))]);
        assert!(r.is_clean(), "{s:?}: {r:?}");
    }
    let (_, r) = render(vec![Element::Text(text_el(1, "\nWIDE", 16.0, f))]);
    let c = r.text_clipped[0].1;
    assert_eq!((c.lines, c.lines_whole), (2, 1), "{c:?}");
}

/// A frame with no area loses any text put in it, so that is an error rather
/// than a clean report over a blank label. With nothing to draw it is fine.
#[test]
fn a_frame_with_no_area_is_an_error_unless_the_text_is_blank() {
    for f in [frame(0, 0, 0, 20), frame(0, 0, 100, 0)] {
        let (_, r) = render(vec![Element::Text(text_el(1, "Widget", 16.0, f))]);
        assert!(failed_with(&r, "no area"), "{f:?}: {r:?}");
        let (_, r) = render(vec![Element::Text(text_el(1, " \n", 16.0, f))]);
        assert!(
            r.failed.is_empty() && r.text_clipped.is_empty(),
            "{f:?}: {r:?}"
        );
    }
}

#[test]
fn families_weights_and_values_are_resolved_or_refused() {
    let mut eng = TextEngine::new().unwrap();
    let mut width = |family: &'static str, weight| {
        let s = TextStyle {
            family,
            weight,
            ..style(20.0)
        };
        eng.layout("Widget", &s, 320.0).map(|l| l.width())
    };
    let regular = width("", TextWeight::Regular).unwrap();
    assert_eq!(
        width(SANS, TextWeight::Regular).unwrap(),
        regular,
        "\"Noto Sans\" is the default"
    );
    let semibold = width(SANS, TextWeight::SemiBold).unwrap();
    let mono = width(MONO, TextWeight::Regular).unwrap();
    assert!(semibold > regular, "{semibold} > {regular}");
    assert!(mono > regular, "{mono} > {regular}");
    assert!(matches!(
        width("Arial", TextWeight::Regular),
        Err(TextError::UnknownFamily(_))
    ));
    assert!(matches!(
        width(MONO, TextWeight::SemiBold),
        Err(TextError::NoSuchWeight { .. })
    ));

    // A value from a later build: this element is not drawn, and says why.
    let f = frame(0, 0, 320, 96);
    let mut a = text_el(1, "Widget", 16.0, f);
    a.align = TextAlign::Unknown("justify".into());
    let mut w = text_el(1, "Widget", 16.0, f);
    w.weight = TextWeight::Unknown("black".into());
    let mut g = text_el(1, "Widget", 16.0, f);
    g.raster = GlyphRaster::Unknown("dither".into());
    for (e, value) in [(a, "justify"), (w, "black"), (g, "dither")] {
        let (px, r) = render(vec![Element::Text(e)]);
        assert!(failed_with(&r, value), "{r:?}");
        assert!(px.px.iter().all(|&p| p == 255), "{value}: nothing drawn");
    }
}

#[test]
fn alignment_places_lines_across_the_frame() {
    let f = frame(20, 0, 200, 96);
    let x_span = |align: TextAlign| {
        let mut e = text_el(1, "Hi", 20.0, f);
        e.align = align.clone();
        let (g, r) = render(vec![Element::Text(e)]);
        assert!(r.is_clean(), "{align:?}: {r:?}");
        let b = ink_box(&g.threshold(128)).unwrap();
        (b.0, b.2)
    };
    let (s0, _) = x_span(TextAlign::Start);
    let (c0, c1) = x_span(TextAlign::Center);
    let (e0, e1) = x_span(TextAlign::End);
    assert!(s0 < c0 && c0 < e0, "{s0} {c0} {e0}");
    assert!(s0 <= 22, "start sits at the frame's start: {s0}");
    assert!(
        (214..=220).contains(&e1),
        "end sits at the frame's end: {e1}"
    );
    let mid = f64::from(c0 + c1) / 2.0;
    assert!((mid - 120.0).abs() <= 2.0, "centred on the frame: {mid}");
}

/// Parley aligns by advance, but ink can reach past a glyph's advance - a
/// hinted `f` by more than a dot, a `j` before its origin. In a frame with
/// room for it, none of that ink is cut.
#[test]
fn ink_past_the_advance_is_kept_inside_a_frame_with_room() {
    for raster in RASTERS {
        for size in [12.0f32, 16.0, 20.0, 24.0, 32.0] {
            for (s, align) in [
                ("staff", TextAlign::End),
                ("Tf", TextAlign::End),
                ("f", TextAlign::End),
                ("jaw", TextAlign::Start),
                ("j", TextAlign::Start),
                ("fjord", TextAlign::Center),
            ] {
                let mut e = text_el(1, s, size, frame(10, 0, 100, 60));
                e.align = align.clone();
                e.raster = raster.clone();
                let (_, r) = render(vec![Element::Text(e)]);
                assert!(r.is_clean(), "{s:?} {align:?} {size} {raster:?}: {r:?}");
            }
        }
    }
}

/// A turned element on a `Left` target is the upright one turned, byte for
/// byte - grey levels, zones, and the thresholded plane against
/// `BitPlane::rotate_cw`, which is independent code in another crate.
#[test]
fn quarter_turns_are_the_upright_rendering_turned() {
    for (size, raster) in [
        (14.0f32, GlyphRaster::Hinted),
        (18.0, GlyphRaster::Hinted),
        (28.0, GlyphRaster::Smooth),
    ] {
        let f = frame(8, 4, 80, 60);
        let el = |rot| {
            let mut e = text_el(1, TEXT, size, f);
            e.common.rot = rot;
            e.raster = raster.clone();
            Element::Text(e)
        };
        let (g0, r0) = render_on(PrintDirection::Left, 96, 96, vec![el(Rot::D0)]);
        assert!(r0.failed.is_empty(), "{r0:?}");
        let up = crop(&g0.px, 96, 8, 4, 80, 60);
        let up_zone = crop(&g0.zone, 96, 8, 4, 80, 60);
        assert!(
            up.2.iter().filter(|&&p| p < 255).count() > 200,
            "{size}: something drawn"
        );
        let grey = up.2.iter().filter(|&&p| p > 0 && p < 255).count();
        if raster == GlyphRaster::Hinted {
            assert_eq!(grey, 0, "{size}: hinted text is 0 or 255 only");
        } else {
            assert!(grey > 0, "{size}: smooth text has partial dots");
        }
        for rot in ROTS {
            let (g, r) = render_on(PrintDirection::Left, 96, 96, vec![el(rot)]);
            assert_eq!(
                r.text_clipped, r0.text_clipped,
                "{size} {rot:?}: the report turns too"
            );
            let (mut want, mut want_zone, mut want_plane) =
                (up.clone(), up_zone.clone(), plane(&up));
            for _ in 0..quarters(rot) {
                want = rot_cw(&want);
                want_zone = rot_cw(&want_zone);
                want_plane = want_plane.rotate_cw();
            }
            let got = crop(&g.px, 96, 8, 4, want.0, want.1);
            assert_eq!(got, want, "{size} {rot:?}: grey levels");
            assert_eq!(
                crop(&g.zone, 96, 8, 4, want.0, want.1),
                want_zone,
                "{size} {rot:?}: zones"
            );
            assert_eq!(
                plane(&got),
                want_plane,
                "{size} {rot:?}: against BitPlane::rotate_cw"
            );
            for y in 0..96u16 {
                for x in 0..96u16 {
                    let inside = (8..8 + want.0).contains(&x) && (4..4 + want.1).contains(&y);
                    if !inside {
                        assert_eq!(
                            g.at(x, y),
                            255,
                            "{size} {rot:?} ({x}, {y}) outside the frame"
                        );
                    }
                }
            }
        }
    }
}

/// On a `Top` target the document's axes swap onto the canvas. The same canvas
/// rectangle, described in each target's own document coordinates, comes out
/// identical in every rotation - over paper, and knocked out of a panel.
#[test]
fn a_top_target_draws_the_same_pixels_as_a_left_one() {
    for size in [16.0f32, 28.0] {
        for rot in ROTS {
            for ink in [Ink::Black, Ink::White] {
                let els = |dir| {
                    let f = match dir {
                        PrintDirection::Left => frame(8, 4, 80, 60),
                        PrintDirection::Top => frame(4, 8, 60, 80),
                    };
                    let mut e = text_el(1, TEXT, size, f);
                    e.common.rot = rot;
                    e.common.ink = ink;
                    let mut v = Vec::new();
                    if ink == Ink::White {
                        v.push(panel(9, frame(0, 0, 96, 96)));
                    }
                    v.push(Element::Text(e));
                    v
                };
                let (left, rl) = render_on(PrintDirection::Left, 96, 96, els(PrintDirection::Left));
                let (top, rt) = render_on(PrintDirection::Top, 96, 96, els(PrintDirection::Top));
                assert_eq!(rl, rt, "{size} {rot:?} {ink:?}");
                assert_eq!(left.px, top.px, "{size} {rot:?} {ink:?}: pixels");
                assert_eq!(left.zone, top.zone, "{size} {rot:?} {ink:?}: zones");
                let ground = if ink == Ink::White { 0 } else { 255 };
                assert!(
                    left.px.iter().any(|&p| p != ground),
                    "{size} {rot:?} {ink:?}: drawn"
                );
            }
        }
    }
}

/// An angle between quarter turns is drawn at the nearest one, on either kind
/// of printer, and reported as approximated.
#[test]
fn an_unmodelled_angle_is_drawn_at_the_nearest_quarter_turn() {
    for (angle, near) in [
        (20u16, Rot::D0),
        (80, Rot::D90),
        (200, Rot::D180),
        (300, Rot::D270),
    ] {
        for dir in [PrintDirection::Left, PrintDirection::Top] {
            let f = match dir {
                PrintDirection::Left => frame(8, 4, 80, 60),
                PrintDirection::Top => frame(4, 8, 60, 80),
            };
            let els = |rot| {
                let mut e = text_el(1, TEXT, 16.0, f);
                e.common.rot = rot;
                vec![Element::Text(e)]
            };
            let (g, r) = render_on(dir, 96, 96, els(Rot::Other(angle)));
            let (want, rw) = render_on(dir, 96, 96, els(near));
            assert_eq!(g.px, want.px, "{angle} {dir:?}");
            assert_eq!(g.zone, want.zone, "{angle} {dir:?}");
            assert_eq!(r.rotation_approximated, vec![1], "{angle} {dir:?}");
            assert_eq!(r.text_clipped, rw.text_clipped, "{angle} {dir:?}");
        }
    }
}

/// The dots a text element claims for its zone are exactly the dots it inks,
/// so halftoning error stops at the glyphs, not at the frame.
#[test]
fn the_zone_is_the_glyphs_not_the_frame() {
    let mut e = text_el(1, TEXT, 28.0, frame(0, 0, 320, 96));
    e.raster = GlyphRaster::Smooth;
    let (g, _) = render(vec![Element::Text(e)]);
    for i in 0..g.px.len() {
        assert_eq!(g.zone[i] != 0, g.px[i] != 255, "dot {i}");
    }
}

/// A named background: luminance for each dot.
type Ground = (&'static str, fn(u32, u32) -> u8);

/// The coverage blend is tiny-skia's own: a glyph path filled into a mask and
/// blended equals the same path filled straight into a pixmap, over white,
/// black and every grey between, for both inks and both rasters.
#[test]
fn blending_coverage_matches_filling_the_path() {
    let mut eng = TextEngine::new().unwrap();
    let (w, h) = (256u32, 72u32);
    let grounds: [Ground; 3] = [
        ("white", |_, _| 255),
        ("ramp", |x, _| x as u8),
        ("hash", |x, y| {
            (x.wrapping_mul(97) ^ y.wrapping_mul(193)) as u8
        }),
    ];
    let mut partial = 0usize;
    for size in [14.0f32, 31.0] {
        for raster in RASTERS {
            let s = TextStyle {
                raster: raster.clone(),
                ..style(size)
            };
            let layout = eng.layout(TEXT, &s, w as f32).unwrap();
            let path = eng.glyph_path(&layout, &raster).unwrap().unwrap();
            let aa = raster == GlyphRaster::Smooth;
            let mut mask = Mask::new(w, h).unwrap();
            mask.fill_path(&path, FillRule::Winding, aa, Transform::identity());
            for ink in [Ink::Black, Ink::White] {
                for (name, ground) in &grounds {
                    let mut direct = Pixmap::new(w, h).unwrap();
                    direct.fill(Color::WHITE);
                    for y in 0..h {
                        for x in 0..w {
                            let i = ((y * w + x) * 4) as usize;
                            let l = ground(x, y);
                            direct.data_mut()[i..i + 4].copy_from_slice(&[l, l, l, 255]);
                        }
                    }
                    let paint = if aa {
                        Canvas::soft(ink)
                    } else {
                        Canvas::hard(ink)
                    };
                    direct.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                    for y in 0..h {
                        for x in 0..w {
                            let i = (y * w + x) as usize;
                            let (l, c) = (ground(x, y), mask.data()[i]);
                            let want = if c == 0 { l } else { blend(l, c, ink) };
                            let px = &direct.data()[i * 4..i * 4 + 4];
                            assert_eq!(
                                px,
                                &[want, want, want, 255],
                                "{size} {raster:?} {ink:?} {name} ({x}, {y})"
                            );
                            if c > 0 && c < 255 {
                                partial += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        partial > 1000,
        "the smooth cases exercised partial coverage: {partial}"
    );
}

/// Overflow is drawn clipped - never outside the frame - and reported with
/// numbers, not a string.
#[test]
fn text_that_does_not_fit_is_drawn_clipped_and_reported() {
    let (_, r) = render(vec![Element::Text(text_el(
        1,
        "Widget",
        16.0,
        frame(0, 0, 320, 96),
    ))]);
    assert!(r.is_clean(), "{r:?}");

    let para = "Shelf B4 Widget twelve millimetres by forty, batch 0042, lot 7, use by 2027-01-31, keep dry";
    for raster in RASTERS {
        let mut e = text_el(2, para, 24.0, frame(0, 0, 160, 48));
        e.raster = raster.clone();
        let (g, r) = render(vec![Element::Text(e)]);
        assert_eq!(r.text_clipped.len(), 1, "{r:?}");
        let c = r.text_clipped[0].1;
        assert!(
            c.lines_whole < c.lines && c.ink_clipped_dots > 0,
            "{raster:?}: {c:?}"
        );
        for y in 0..96u16 {
            for x in 0..320u16 {
                if x >= 160 || y >= 48 {
                    assert_eq!(g.at(x, y), 255, "{raster:?} ({x}, {y}) outside the frame");
                }
            }
        }
    }

    // A frame deep enough for one and a half lines holds one whole line.
    let (_, r) = render(vec![Element::Text(text_el(
        5,
        "Ab\nCd",
        20.0,
        frame(0, 0, 320, 30),
    ))]);
    let c = r.text_clipped[0].1;
    assert_eq!((c.lines, c.lines_whole), (2, 1), "{c:?}");

    // One word wider than its frame overhangs by exactly its excess advance.
    let word = "SUPERCALIFRAGILISTIC";
    let advance = TextEngine::new()
        .unwrap()
        .layout(word, &style(24.0), 1000.0)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .metrics()
        .advance;
    let (_, r) = render(vec![Element::Text(text_el(
        3,
        word,
        24.0,
        frame(0, 0, 100, 96),
    ))]);
    let c = r.text_clipped[0].1;
    assert_eq!(c.overhang_dots, (advance - 100.0).ceil() as u32, "{c:?}");
    assert!(c.ink_clipped_dots > 0, "{c:?}");
    let mut turned = text_el(3, word, 24.0, frame(0, 0, 100, 96));
    turned.common.rot = Rot::D90;
    let (_, r90) = render_on(PrintDirection::Left, 320, 320, vec![Element::Text(turned)]);
    assert_eq!(
        r90.text_clipped, r.text_clipped,
        "the report is about the text, not its turn"
    );

    // Four lines at 24 ppem fill 96 dots of line boxes, but the last line's
    // descenders hang past them: ink clipped, no line lost.
    let (_, r) = render(vec![Element::Text(text_el(
        4,
        "Hgjpqy Hgjpqy Hgjpqy Hgjpqy",
        24.0,
        frame(0, 0, 90, 96),
    ))]);
    let c = r.text_clipped[0].1;
    assert_eq!(
        (c.lines, c.lines_whole, c.overhang_dots),
        (4, 4, 0),
        "{c:?}"
    );
    assert!(c.ink_clipped_dots > 0, "{c:?}");

    // A hostile frame is refused, not allocated.
    let (_, r) = render(vec![Element::Text(text_el(
        6,
        "x",
        16.0,
        frame(0, 0, i32::MAX, i32::MAX),
    ))]);
    assert_eq!(r.failed.len(), 1, "{r:?}");
}

/// Whitespace at the end of a line hangs past the frame without counting as
/// overhang: it draws nothing.
#[test]
fn trailing_whitespace_is_not_overhang() {
    let mut eng = TextEngine::new().unwrap();
    let word = eng.layout("Widget", &style(20.0), 1000.0).unwrap();
    let w = word.lines().next().unwrap().metrics().advance.ceil() as u32 + 2;
    let local = eng
        .render_local("Widget   ", &style(20.0), w, 24, LIMIT)
        .unwrap();
    assert_eq!(local.clip.lines, 1, "{:?}", local.clip);
    assert!(!local.clip.is_clipped(), "{:?}", local.clip);
}

/// Cutting text at its frame changes nothing inside the frame: tiny-skia's
/// clipper, which moves dots near a mask's edge, never runs on text.
#[test]
fn clipped_text_is_the_same_dots_as_unclipped() {
    let mut eng = TextEngine::new().unwrap();
    for raster in RASTERS {
        let st = TextStyle {
            raster: raster.clone(),
            ..style(24.0)
        };
        // Cut across the foot of the text: the same width, so the same lines.
        let para = "Shelf B4 Widget twelve millimetres by forty, batch 0042";
        let deep = eng.render_local(para, &st, 160, 400, LIMIT).unwrap();
        for h in [20, 30, 41, 48] {
            let cut = eng.render_local(para, &st, 160, h, LIMIT).unwrap();
            assert!(cut.clip.ink_clipped_dots > 0, "{raster:?} {h}");
            assert_eq!(
                cut.cov,
                deep.cov.crop(0, 0, 160, h),
                "{raster:?} cut at {h}"
            );
        }
        // Cut across the side: a word that fits none of these frames is not
        // moved to fit any of them.
        let word = "SUPERCALIFRAGILISTIC";
        let wide = eng.render_local(word, &st, 600, 60, LIMIT).unwrap();
        for w in [60, 100, 151] {
            let cut = eng.render_local(word, &st, w, 60, LIMIT).unwrap();
            assert_eq!(cut.cov, wide.cov.crop(0, 0, w, 60), "{raster:?} cut at {w}");
        }
    }
}

/// Ink above the first line box and before the frame's start - an accent at
/// a tight line height, a `j` - is cut where the outline puts it.
#[test]
fn ink_above_and_before_the_frame_is_cut_where_it_falls() {
    let mut eng = TextEngine::new().unwrap();
    for raster in RASTERS {
        let st = TextStyle {
            raster: raster.clone(),
            line_height: 0.7,
            ..style(24.0)
        };
        // Too wide for the frame, so not moved to fit it.
        let s = "jÅSUPERCALIFRAGILISTIC";
        let layout = eng.layout(s, &st, 100.0).unwrap();
        let path = eng.glyph_path(&layout, &raster).unwrap().unwrap();
        let b = path.compute_tight_bounds().unwrap();
        assert!(
            b.left() < -0.5 && b.top() < -0.5,
            "the test needs ink on both sides: {b:?}"
        );
        let (ox, oy) = (64u32, 64u32);
        let mut mask = Mask::new(800, 200).unwrap();
        let aa = raster == GlyphRaster::Smooth;
        let at = Transform::from_translate(ox as f32, oy as f32);
        mask.fill_path(&path, FillRule::Winding, aa, at);
        let whole = Coverage {
            w: 800,
            h: 200,
            data: mask.data().to_vec(),
        };
        let want = whole.crop(ox.into(), oy.into(), 100, 40);
        let got = eng.render_local(s, &st, 100, 40, LIMIT).unwrap();
        assert_eq!(got.cov, want, "{raster:?}");
        assert_eq!(
            got.clip.ink_clipped_dots as usize,
            whole.inked() - want.inked(),
            "{raster:?}"
        );
    }
}

/// Lines stack at a whole number of dots, the size times the line height
/// rounded, so every line's glyphs are the first line's moved down. Ten lines
/// down, an unrounded pitch would be a whole dot out.
#[test]
fn the_line_pitch_is_a_whole_number_of_dots() {
    let mut eng = TextEngine::new().unwrap();
    for (ppem, lh, pitch) in [(13.0f32, 1.3f32, 17u32), (11.0, 1.1, 12), (13.0, 1.5, 20)] {
        let st = TextStyle {
            line_height: lh,
            ..style(ppem)
        };
        let h = 12 * pitch;
        let one = eng.render_local("H", &st, 60, h, LIMIT).unwrap().cov;
        assert!(one.inked() > 0);
        for k in [1u32, 2, 10] {
            let down = format!("{}H", "\n".repeat(k as usize));
            let moved = eng.render_local(&down, &st, 60, h, LIMIT).unwrap().cov;
            assert_eq!(
                moved.crop(0, i64::from(k * pitch), 60, h - k * pitch),
                one.crop(0, 0, 60, h - k * pitch),
                "{ppem} x {lh}, {k} lines down"
            );
        }
    }
}

/// Sizes, line heights and lengths from a file are refused when no label could
/// use them - never allowed to hang the layout, overflow the raster arithmetic
/// or allocate without bound.
#[test]
fn hostile_parameters_are_refused_not_drawn() {
    for (size, lh) in [
        (16.0f32, 3e37f32),
        (16.0, f32::INFINITY),
        (16.0, 0.0),
        (16.0, -1.0),
        (16.0, f32::NAN),
        (16.0, 101.0),
        (1e30, 1.0),
        (f32::MAX, 1.0),
        (5000.0, 1.0),
        (0.0, 1.0),
        (-12.0, 1.0),
        (f32::NAN, 1.0),
    ] {
        let mut e = text_el(1, "Widget", size, frame(0, 0, 320, 96));
        e.line_height = lh;
        let (g, r) = render(vec![Element::Text(e)]);
        assert_eq!(r.failed.len(), 1, "{size} x {lh}: {r:?}");
        assert!(g.px.iter().all(|&p| p == 255), "{size} x {lh}");
    }
    // The largest accepted size and line height together, in a small frame.
    let mut e = text_el(1, "Widget\nWidget", text::MAX_PPEM, frame(0, 0, 320, 96));
    e.line_height = text::MAX_LINE_HEIGHT;
    let (_, r) = render(vec![Element::Text(e)]);
    assert!(failed_with(&r, "too large"), "{r:?}");

    let long = "x".repeat(text::MAX_TEXT_CHARS + 1);
    let (_, r) = render(vec![Element::Text(text_el(
        1,
        &long,
        8.0,
        frame(0, 0, 320, 96),
    ))]);
    assert!(failed_with(&r, "longer than"), "{r:?}");
}

/// The cap on a local raster's area follows the canvas: a frame as large as
/// the label it sits on is drawn, however large the label. A frame longer
/// than tiny-skia can fill in one piece is refused.
#[test]
fn a_frame_as_large_as_its_label_is_drawn() {
    let (feed, head) = (8000, 600);
    assert!((feed * head) as u64 > text::MAX_LOCAL_DOTS);
    let (g, r) = render_on(
        PrintDirection::Left,
        feed,
        head,
        vec![Element::Text(text_el(
            1,
            "Widget",
            24.0,
            frame(0, 0, feed, head),
        ))],
    );
    assert!(r.is_clean(), "{r:?}");
    assert!(g.px.iter().any(|&p| p < 255));

    let feed = text::MAX_SIDE as i32 + 1;
    let (_, r) = render_on(
        PrintDirection::Left,
        feed,
        96,
        vec![Element::Text(text_el(
            1,
            "Widget",
            24.0,
            frame(0, 0, feed, 96),
        ))],
    );
    assert!(failed_with(&r, "too large"), "{r:?}");
}

/// Stem widths per row of a glyph's middle band, as (first, last) ink runs.
fn stem_pairs(p: &BitPlane) -> Vec<(u16, u16)> {
    let (x0, y0, x1, y1) = ink_box(p).unwrap();
    let h = y1 - y0;
    let mut out = Vec::new();
    for y in y0 + h * 2 / 5..y0 + h * 4 / 5 {
        let mut runs = Vec::new();
        let mut x = x0;
        while x < x1 {
            if p.get(x, y) {
                let start = x;
                while x < x1 && p.get(x, y) {
                    x += 1;
                }
                runs.push(x - start);
            } else {
                x += 1;
            }
        }
        if runs.len() >= 2 {
            out.push((runs[0], runs[runs.len() - 1]));
        }
    }
    out
}

/// What the hinting is for: both stems of an `n`, and the outer stems of an
/// `m`, come out the same whole number of dots at every size a 96-dot label
/// uses, and wherever the glyph starts.
#[test]
fn hinted_stems_are_the_same_width_on_both_sides() {
    for ppem in [12.0f32, 14.0, 16.0, 18.0, 20.0, 24.0] {
        for glyph in ["n", "m", "u"] {
            for feed in [3, 4, 5, 6] {
                let (g, r) = render(vec![Element::Text(text_el(
                    1,
                    glyph,
                    ppem,
                    frame(feed, 10, 80, 60),
                ))]);
                assert!(r.failed.is_empty(), "{r:?}");
                let pairs = stem_pairs(&g.threshold(128));
                assert!(!pairs.is_empty(), "{glyph} at {ppem}");
                for (a, b) in pairs {
                    assert_eq!(
                        a, b,
                        "{glyph} at {ppem} ppem, feed {feed}: stems {a} and {b}"
                    );
                }
            }
        }
    }
}

/// The two rasters and three faces, at the sizes a 12 mm label uses. Look at
/// these before blessing: they are the text a printer will produce. Every
/// line fits its frame, so a golden never records clipping.
#[test]
fn every_text_raster_matches_its_golden() {
    let s = "0O 1lI nm ae B4 - 12mm";
    for (family, weight, fname) in [
        ("", TextWeight::Regular, "regular"),
        (SANS, TextWeight::SemiBold, "semibold"),
        (MONO, TextWeight::Regular, "mono"),
    ] {
        for (raster, rname) in [
            (GlyphRaster::Hinted, "hinted"),
            (GlyphRaster::Smooth, "smooth"),
        ] {
            let mut els = Vec::new();
            for (i, (head, size)) in [(0, 12.0f32), (14, 16.0), (32, 20.0), (54, 24.0)]
                .into_iter()
                .enumerate()
            {
                let mut e = text_el(i as u32 + 1, s, size, frame(0, head, 320, size as i32 + 4));
                e.family = family.into();
                e.weight = weight.clone();
                e.raster = raster.clone();
                els.push(Element::Text(e));
            }
            let (g, r) = render(els);
            assert!(r.is_clean(), "{fname} {rname}: {r:?}");
            common::check(&format!("text-{fname}-{rname}"), &g.threshold(128));
        }
    }
}
