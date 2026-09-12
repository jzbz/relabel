//! Media geometry: the fold, the tail, the die-cut, and the repeat.

mod common;

use relabel_proto::{BitPlane, PrintDirection};
use relabel_render::datetime::CivilTime;
use relabel_render::document::*;
use relabel_render::media::{self, Bounds, GeometryFault, Keepout, Outline};
use relabel_render::preflight::{IssueKind, Severity, preflight};
use relabel_render::{
    BinariseOpts, GreyMap, Halftone, Target, Zone, binarise, rasterise, rasterise_plane,
    rasterise_preview,
};

fn now() -> CivilTime {
    CivilTime::new(2026, 9, 11, 16, 5, 9, 42, 120).unwrap()
}

/// A label with one mark in the first panel, asymmetric on both axes so a
/// mirror cannot pass for a half turn.
fn label(media: &str, repeat: &str, elements: &str) -> Label {
    let s = format!(
        r#"{{
  "format": "relabel-label", "version": 1,
  "media": {{ "feed_tenths_mm": 400, "head_tenths_mm": 120 {media} }},
  "canvas": {{ "feed": 320, "head": 96, "dpmm": {{ "num": 8, "den": 1 }} }},
  "elements": [{elements}],
  "repeat": {repeat}
}}"#
    );
    Label::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn rect(id: u32, feed: i32, head: i32, fl: i32, hl: i32) -> String {
    format!(
        r#"{{ "type": "rect", "id": {id}, "frame": {{ "feed": {feed}, "head": {head}, "feed_len": {fl}, "head_len": {hl} }} }}"#
    )
}

/// An L, so a half turn, a feed mirror and a head mirror are three different
/// pictures.
fn ell() -> String {
    format!("{}, {}", rect(1, 8, 8, 40, 8), rect(2, 8, 8, 8, 60))
}

fn feed_fold(at: i32, parts: u8) -> String {
    format!(r#", "fold": {{ "at_dots": {at}, "parts": {parts} }}"#)
}

/// A dot of the plane, in document coordinates.
fn dot(p: &BitPlane, t: &Target, feed: i32, head: i32) -> bool {
    let (x, y) = t.place(feed, head);
    p.get(x as u16, y as u16)
}

fn targets() -> [Target; 2] {
    [
        Target::with_direction(320, 96, 203, PrintDirection::Left).unwrap(),
        Target::with_direction(96, 320, 203, PrintDirection::Top).unwrap(),
    ]
}

#[test]
fn a_copy_repeats_the_first_panel_dot_for_dot() {
    for t in targets() {
        let l = label(&feed_fold(160, 2), "\"copy\"", &ell());
        let p = rasterise_plane(&l, &t).unwrap();
        for feed in 0..160 {
            for head in 0..96 {
                assert_eq!(
                    dot(&p, &t, feed, head),
                    dot(&p, &t, feed + 160, head),
                    "({feed},{head}) on {:?}",
                    t.direction
                );
            }
        }
    }
}

#[test]
fn a_flip_repeats_the_first_panel_turned_through_half_a_turn() {
    for t in targets() {
        let l = label(&feed_fold(160, 2), "\"flip\"", &ell());
        let p = rasterise_plane(&l, &t).unwrap();
        for feed in 0..160 {
            for head in 0..96 {
                assert_eq!(
                    dot(&p, &t, feed, head),
                    dot(&p, &t, 160 + 159 - feed, 95 - head),
                    "({feed},{head}) on {:?}",
                    t.direction
                );
            }
        }
    }
}

#[test]
fn a_flip_is_a_turn_and_not_a_mirror() {
    let t = targets()[0];
    let l = label(&feed_fold(160, 2), "\"flip\"", &ell());
    let p = rasterise_plane(&l, &t).unwrap();
    // The arm of the L reaches the near edge of the second panel only if the
    // copy was turned rather than mirrored about the crease.
    assert!(dot(&p, &t, 319 - 8, 95 - 8));
    assert!(!dot(&p, &t, 160 + 8, 8));
}

#[test]
fn turning_the_repeat_on_moves_no_dot_of_the_first_panel() {
    let t = targets()[0];
    let none = rasterise_plane(&label(&feed_fold(160, 2), "\"none\"", &ell()), &t).unwrap();
    for repeat in ["\"copy\"", "\"flip\""] {
        let p = rasterise_plane(&label(&feed_fold(160, 2), repeat, &ell()), &t).unwrap();
        for feed in 0..160 {
            for head in 0..96 {
                assert_eq!(dot(&none, &t, feed, head), dot(&p, &t, feed, head));
            }
        }
    }
}

#[test]
fn a_dithered_panel_crosses_the_fold_unchanged() {
    // A photographic grey in the first panel, dithered with error diffusion:
    // the copy is bit-identical, which no reading before binarisation achieves.
    let (w, h) = (320u16, 96u16);
    let mut g = GreyMap::new(w, h);
    for y in 0..usize::from(h) {
        for x in 0..160 {
            g.px[y * usize::from(w) + x] = ((x * 7 + y * 3) % 256) as u8;
        }
    }
    let zones = [Zone {
        halftone: Halftone::FloydSteinberg,
        ..Zone::PAGE
    }];
    let before = binarise(&g, &BinariseOpts::with_zones(&zones));
    let t = targets()[0];
    let l = label(&feed_fold(160, 2), "\"copy\"", "");
    let plan = media::plan(&l).unwrap().unwrap();
    let after = media::repeat_plane(&before, &t, &plan);
    let mut ink = 0;
    for y in 0..h {
        for x in 0..160u16 {
            assert_eq!(before.get(x, y), after.get(x, y), "the first panel moved");
            assert_eq!(after.get(x, y), after.get(x + 160, y), "the copy differs");
            ink += u32::from(after.get(x, y));
        }
    }
    assert!(ink > 1000, "the fixture must actually have ink in it");
}

#[test]
fn every_panel_after_the_first_alternates_its_facing_under_flip() {
    let t = targets()[0];
    let l = label(&feed_fold(100, 3), "\"flip\"", &ell());
    let p = rasterise_plane(&l, &t).unwrap();
    for feed in 0..100 {
        for head in 0..96 {
            assert_eq!(
                dot(&p, &t, feed, head),
                dot(&p, &t, 100 + 99 - feed, 95 - head),
                "panel 1 is turned"
            );
            assert_eq!(
                dot(&p, &t, feed, head),
                dot(&p, &t, 200 + feed, head),
                "panel 2 faces the way panel 0 does"
            );
        }
    }
}

#[test]
fn the_paper_past_the_last_panel_is_left_exactly_as_it_was_drawn() {
    let t = targets()[0];
    let marked = format!("{}, {}", ell(), rect(3, 300, 4, 4, 4));
    let l = label(&feed_fold(100, 2), "\"copy\"", &marked);
    let p = rasterise_plane(&l, &t).unwrap();
    assert!(dot(&p, &t, 301, 5), "the mark past the panels still prints");
}

#[test]
fn a_fold_across_the_head_repeats_across_the_head() {
    let t = targets()[0];
    let media = r#", "fold": { "at_dots": 48, "parts": 2, "axis": "head" }"#;
    let l = label(media, "\"copy\"", &ell());
    let p = rasterise_plane(&l, &t).unwrap();
    for feed in 0..320 {
        for head in 0..48 {
            assert_eq!(dot(&p, &t, feed, head), dot(&p, &t, feed, head + 48));
        }
    }
}

#[test]
fn a_fold_measures_its_crease_from_the_start_of_the_content() {
    // The cable flag: 40 dots of tail, then two 140-dot panels either side of a
    // crease at 180. Lengthening the tail shortens the first panel and leaves
    // the crease where the paper folds.
    let media = format!(
        r#", "tail": {{ "at_start": true, "len_dots": 40 }}{}"#,
        feed_fold(180, 2)
    );
    let l = label(&media, "\"copy\"", "");
    let p = media::plan(&l).unwrap().unwrap();
    assert_eq!(p.source.feed, 40);
    assert_eq!(p.pitch, 140);
    assert_eq!(p.part(1).feed, 180);
    assert_eq!(p.copies, 1);
    assert_eq!(p.short, 0);
    assert_eq!(p.dropped, 0);
}

#[test]
fn a_copy_the_label_cuts_short_is_reported_and_the_rest_still_prints() {
    let t = targets()[0];
    let l = label(&feed_fold(200, 2), "\"copy\"", &ell());
    let p = media::plan(&l).unwrap().unwrap();
    assert_eq!((p.copies, p.short, p.dropped), (1, 80, 0));
    let faults = media::faults(&l);
    assert_eq!(
        faults,
        vec![GeometryFault::FoldDoesNotFit {
            printed: 1,
            short: 80,
            dropped: 0
        }]
    );
    assert_eq!(faults[0].severity(), Severity::Warn);
    // A copy keeps the start of the panel and loses the end.
    let plane = rasterise_plane(&l, &t).unwrap();
    assert!(dot(&plane, &t, 200 + 8 + 1, 9));
    assert_eq!(plane.width(), 320);
}

#[test]
fn a_flip_the_label_cuts_short_turns_about_the_crease_and_loses_the_far_end() {
    let t = targets()[0];
    let l = label(&feed_fold(200, 2), "\"flip\"", &ell());
    let p = rasterise_plane(&l, &t).unwrap();
    // 120 dots of room for a 200-dot panel: the destination dot next to the
    // crease comes from the source dot next to it.
    for d in 0..120 {
        assert_eq!(dot(&p, &t, 200 + d, 40), dot(&p, &t, 199 - d, 55));
    }
}

#[test]
fn a_fold_into_too_many_panels_drops_the_ones_with_no_room() {
    let l = label(&feed_fold(100, 200), "\"copy\"", "");
    let p = media::plan(&l).unwrap().unwrap();
    assert_eq!((p.copies, p.short, p.dropped), (3, 80, 196));
}

#[test]
fn a_repeat_with_no_fold_is_refused_rather_than_printed_short() {
    let l = label("", "\"copy\"", &ell());
    assert_eq!(media::plan(&l), Err(GeometryFault::RepeatWithoutFold));
    assert_eq!(
        media::faults(&l)[0].severity(),
        Severity::Refuse,
        "half a fold-over label is a missing mark"
    );
    let t = targets()[0];
    // It still renders: a document this build cannot carry out is reported, not
    // fatal.
    assert!(rasterise_plane(&l, &t).is_ok());
    let p = preflight(&l, None, &t, now()).unwrap();
    assert!(p.refused());
}

#[test]
fn an_unknown_repeat_is_refused_rather_than_treated_as_none() {
    for v in ["\"mirror\"", "{\"kind\":\"tile\",\"n\":3}", "3", "null"] {
        let l = label(&feed_fold(160, 2), v, &ell());
        let faults = media::faults(&l);
        assert!(
            matches!(faults[0], GeometryFault::RepeatUnknown { .. }),
            "{v}"
        );
        assert_eq!(faults[0].severity(), Severity::Refuse, "{v}");
        // ...and it survives the save.
        assert!(l.to_json().unwrap().contains("repeat"));
    }
}

#[test]
fn an_unknown_fold_axis_refuses_the_repeat_rather_than_guessing_the_feed() {
    let media = r#", "fold": { "at_dots": 160, "parts": 2, "axis": "diagonal" }"#;
    let l = label(media, "\"copy\"", &ell());
    assert_eq!(
        media::faults(&l),
        vec![GeometryFault::FoldAxisUnknown {
            value: "diagonal".into(),
            needed: true
        }]
    );
    assert_eq!(media::faults(&l)[0].severity(), Severity::Refuse);
    // Nothing is duplicated, and nothing panics.
    let t = targets()[0];
    assert!(rasterise_plane(&l, &t).is_ok());
}

#[test]
fn an_unknown_fold_axis_is_still_reported_when_the_repeat_is_unknown_too() {
    let media = r#", "fold": { "at_dots": 160, "parts": 2, "axis": {"along":"head"} }"#;
    let l = label(media, "\"mirror\"", "");
    let faults = media::faults(&l);
    assert!(
        faults
            .iter()
            .any(|f| matches!(f, GeometryFault::FoldAxisUnknown { needed: false, .. }))
    );
    assert!(
        faults
            .iter()
            .any(|f| matches!(f, GeometryFault::RepeatUnknown { .. }))
    );
    assert_eq!(
        faults.iter().map(GeometryFault::severity).max(),
        Some(Severity::Refuse)
    );
}

#[test]
fn a_fold_axis_survives_a_save_and_a_feed_fold_writes_no_axis_key() {
    let head = label(
        r#", "fold": { "at_dots": 48, "parts": 2, "axis": "head" }"#,
        "\"none\"",
        "",
    );
    let back = head.to_json().unwrap();
    assert!(back.contains("\"axis\": \"head\""));
    let strange = label(
        r#", "fold": { "at_dots": 48, "parts": 2, "axis": {"along":"head","from":"start"} }"#,
        "\"none\"",
        "",
    );
    let back = strange.to_json().unwrap();
    assert!(back.contains("\"along\": \"head\""), "{back}");
    assert!(back.contains("\"from\": \"start\""));
    let feed = label(&feed_fold(160, 2), "\"none\"", "");
    assert!(!feed.to_json().unwrap().contains("axis"));
}

#[test]
fn the_usable_area_is_the_canvas_less_the_tail_at_the_end_it_names() {
    for (at_start, feed, len) in [(true, 40, 280), (false, 0, 280)] {
        let media = format!(r#", "tail": {{ "at_start": {at_start}, "len_dots": 40 }}"#);
        let b = Bounds::of(&label(&media, "\"none\"", ""));
        assert_eq!(b.canvas.feed_len, 320, "the page still covers the tail");
        assert_eq!((b.content.feed, b.content.feed_len), (feed, len));
        assert_eq!(b.content.head_len, 96);
    }
}

#[test]
fn a_tail_across_the_head_takes_its_paper_off_the_head_axis() {
    let media = r#", "tail": { "at_start": true, "len_dots": 16, "axis": "head" }"#;
    let b = Bounds::of(&label(media, "\"none\"", ""));
    assert_eq!((b.content.head, b.content.head_len), (16, 80));
    assert_eq!(b.content.feed_len, 320);
}

#[test]
fn an_element_on_the_tail_is_reported_but_still_prints() {
    let t = targets()[0];
    let media = r#", "tail": { "at_start": true, "len_dots": 40 }"#;
    let l = label(media, "\"none\"", &rect(1, 8, 8, 16, 16));
    let (_, r) = rasterise(&l, &t).unwrap();
    assert_eq!(r.keepout, vec![(1, Keepout::Tail)]);
    assert!(r.off_canvas.is_empty(), "it is on the canvas");
    assert!(!r.is_clean());
    let p = rasterise_plane(&l, &t).unwrap();
    assert!(dot(&p, &t, 10, 10), "the dots still print");
    let pf = preflight(&l, None, &t, now()).unwrap();
    let i = pf
        .issues
        .iter()
        .find(|i| i.element == Some(1))
        .expect("reported");
    assert_eq!(i.kind, IssueKind::Keepout(Keepout::Tail));
    assert_eq!(i.severity, Severity::Warn);
    assert!(!pf.refused());
}

#[test]
fn a_barcode_on_the_tail_warns_where_one_past_the_die_cut_refuses() {
    let t = targets()[0];
    let code = |media: &str| {
        let el = r#"{ "type": "barcode", "id": 1, "frame": { "feed": 4, "head": 4, "feed_len": 220, "head_len": 40 }, "symbology": "code128", "data": "A1", "module_dots": 2, "bar_dots": 0 }"#;
        let l = label(media, "\"none\"", el);
        preflight(&l, None, &t, now()).unwrap()
    };
    let tail = code(r#", "tail": { "at_start": true, "len_dots": 40 }"#);
    assert!(!tail.refused(), "the tail prints and the symbol scans");
    let die = code(r#", "shape": { "kind": "circle" }"#);
    assert!(die.refused(), "a die-cut takes the paper away");
}

#[test]
fn an_element_a_repeat_overwrites_is_reported_because_it_does_not_print() {
    let t = targets()[0];
    let l = label(&feed_fold(160, 2), "\"copy\"", &rect(1, 200, 8, 16, 16));
    let (_, r) = rasterise(&l, &t).unwrap();
    assert_eq!(r.keepout, vec![(1, Keepout::Repeated)]);
    // And an element that straddles the crease is reported too.
    let l = label(&feed_fold(160, 2), "\"copy\"", &rect(1, 150, 8, 20, 16));
    let (_, r) = rasterise(&l, &t).unwrap();
    assert_eq!(r.keepout, vec![(1, Keepout::Repeated)]);
}

#[test]
fn an_element_outside_the_canvas_is_off_canvas_and_not_also_a_keepout() {
    let t = targets()[0];
    let media = r#", "tail": { "at_start": true, "len_dots": 40 }"#;
    let l = label(media, "\"none\"", &rect(1, -10, 8, 16, 16));
    let (_, r) = rasterise(&l, &t).unwrap();
    assert_eq!(r.off_canvas, vec![1]);
    assert!(r.keepout.is_empty());
}

#[test]
fn a_tail_longer_than_the_label_is_reported_and_clamped() {
    let media = r#", "tail": { "at_start": true, "len_dots": 400 }"#;
    let l = label(media, "\"none\"", "");
    assert_eq!(
        media::faults(&l),
        vec![GeometryFault::TailClamped {
            len_dots: 400,
            along: 320,
            clamped: 320
        }]
    );
    let b = Bounds::of(&l);
    assert_eq!(b.content.feed_len, 0, "no content fits");
    assert_eq!(b.content.feed, 320);
}

#[test]
fn a_negative_tail_length_is_reported_rather_than_growing_the_label() {
    let media = r#", "tail": { "at_start": false, "len_dots": -40 }"#;
    let l = label(media, "\"none\"", "");
    assert!(matches!(
        media::faults(&l)[0],
        GeometryFault::TailClamped { clamped: 0, .. }
    ));
    assert_eq!(Bounds::of(&l).content.feed_len, 320);
}

#[test]
fn a_label_with_no_media_geometry_bounds_exactly_as_it_did_before() {
    let l = label("", "\"none\"", "");
    let b = Bounds::of(&l);
    assert_eq!(b.canvas, b.content);
    assert_eq!(b.content, b.usable);
    assert_eq!(b.overwritten, None);
    assert!(media::faults(&l).is_empty());
    assert!(
        b.keepout(&RectDots {
            feed: 0,
            head: 0,
            feed_len: 320,
            head_len: 96
        })
        .is_none()
    );
}

#[test]
fn the_inset_comes_off_the_same_rectangle_the_tail_does() {
    let s = r#"{
  "format": "relabel-label", "version": 1,
  "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120, "tail": { "at_start": true, "len_dots": 40 } },
  "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 },
              "inset": { "feed_start": 8, "feed_end": 8, "head_start": 2, "head_end": 2 } },
  "elements": [{ "type": "rect", "id": 1, "frame": { "feed": 44, "head": 1, "feed_len": 8, "head_len": 8 } }]
}"#;
    let l = Label::from_json(s).unwrap();
    let b = Bounds::of(&l);
    assert_eq!((b.usable.feed, b.usable.feed_len), (48, 264));
    assert_eq!((b.usable.head, b.usable.head_len), (2, 92));
    let t = targets()[0];
    let (_, r) = rasterise(&l, &t).unwrap();
    assert_eq!(r.keepout, vec![(1, Keepout::Inset)]);
}

#[test]
fn an_inset_wider_than_the_stock_leaves_an_empty_usable_area_and_does_not_invert() {
    let s = r#"{
  "format": "relabel-label", "version": 1,
  "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
  "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 },
              "inset": { "feed_start": 400, "feed_end": 400, "head_start": -8, "head_end": 0 } },
  "elements": []
}"#;
    let b = Bounds::of(&Label::from_json(s).unwrap());
    assert_eq!(b.usable.feed_len, 0);
    assert_eq!(b.usable.head_len, 96, "a negative inset takes nothing");
}

#[test]
fn a_corner_radius_only_bites_at_the_corner() {
    let o = Outline::of(
        &MediaShape::Known(KnownShape::RoundedRect { radius: 16 }),
        RectDots {
            feed: 0,
            head: 0,
            feed_len: 320,
            head_len: 96,
        },
    );
    let f = |feed, head, fl, hl| {
        o.contains(&RectDots {
            feed,
            head,
            feed_len: fl,
            head_len: hl,
        })
    };
    assert!(f(16, 0, 288, 96), "the straight middle of both sides");
    assert!(!f(0, 0, 8, 8), "the corner");
    assert!(f(16, 16, 288, 64));
}

#[test]
fn a_radius_larger_than_the_label_is_read_as_the_largest_that_fits() {
    let canvas = RectDots {
        feed: 0,
        head: 0,
        feed_len: 320,
        head_len: 96,
    };
    let big = Outline::of(
        &MediaShape::Known(KnownShape::RoundedRect { radius: 9999 }),
        canvas,
    );
    assert_eq!(
        big,
        Outline::RoundedRect {
            rect: canvas,
            radius: 48
        }
    );
    let neg = Outline::of(
        &MediaShape::Known(KnownShape::RoundedRect { radius: -8 }),
        canvas,
    );
    assert_eq!(
        neg,
        Outline::RoundedRect {
            rect: canvas,
            radius: 0
        }
    );
}

#[test]
fn a_round_die_cut_is_the_largest_circle_that_fits_and_says_so() {
    let l = label(r#", "shape": { "kind": "circle" }"#, "\"none\"", "");
    assert_eq!(
        media::faults(&l),
        vec![GeometryFault::ShapeNotSquare {
            feed: 320,
            head: 96
        }]
    );
    assert_eq!(media::faults(&l)[0].severity(), Severity::Note);
    let b = Bounds::of(&l);
    let inside = |feed, head, fl, hl| {
        b.keepout(&RectDots {
            feed,
            head,
            feed_len: fl,
            head_len: hl,
        })
    };
    assert_eq!(inside(152, 40, 16, 16), None, "the middle");
    assert_eq!(inside(0, 0, 16, 16), Some(Keepout::Shape), "the end");
}

#[test]
fn a_die_cut_from_a_later_build_is_a_note_and_the_label_still_prints() {
    let l = label(r#", "shape": { "kind": "oval" }"#, "\"none\"", &ell());
    assert_eq!(
        media::faults(&l),
        vec![GeometryFault::ShapeUnknown {
            value: "{\"kind\":\"oval\"}".into()
        }]
    );
    assert_eq!(media::faults(&l)[0].severity(), Severity::Note);
    assert_eq!(Bounds::of(&l).outline, Outline::Rect(Bounds::of(&l).canvas));
    let t = targets()[0];
    assert!(!preflight(&l, None, &t, now()).unwrap().refused());
}

#[test]
fn a_damaged_die_cut_of_a_kind_this_build_knows_is_reported_not_mistaken_for_a_strangers() {
    let l = label(
        r#", "shape": { "kind": "rounded_rect", "radius": "eight" }"#,
        "\"none\"",
        "",
    );
    assert!(matches!(
        media::faults(&l)[0],
        GeometryFault::ShapeMalformed { .. }
    ));
    assert_eq!(media::faults(&l)[0].severity(), Severity::Warn);
    // And the file still loads and still saves, verbatim.
    assert!(l.to_json().unwrap().contains("eight"));
}

#[test]
fn a_shape_with_no_kind_at_all_is_still_reported() {
    let l = label(r#", "shape": "oval""#, "\"none\"", "");
    assert!(matches!(
        media::faults(&l)[0],
        GeometryFault::ShapeUnknown { .. }
    ));
}

#[test]
fn hostile_media_geometry_neither_panics_nor_wraps() {
    let t = targets()[0];
    for (media, repeat) in [
        (feed_fold(2147483647, 255), "\"flip\""),
        (feed_fold(-2147483648, 255), "\"copy\""),
        (feed_fold(0, 0), "\"copy\""),
        (feed_fold(1, 255), "\"flip\""),
        (feed_fold(319, 2), "\"flip\""),
        (
            format!(
                r#", "tail": {{ "at_start": true, "len_dots": 2147483647 }}{}"#,
                feed_fold(160, 2)
            ),
            "\"copy\"",
        ),
    ] {
        let l = label(&media, repeat, &ell());
        let p = rasterise_plane(&l, &t).unwrap();
        assert_eq!((p.width(), p.height()), (320, 96), "{media}");
        let _ = media::faults(&l);
        let _ = preflight(&l, None, &t, now()).unwrap();
    }
}

#[test]
fn a_canvas_a_file_wrote_backwards_is_read_as_empty_rather_than_panicking() {
    let s = r#"{
  "format": "relabel-label", "version": 1,
  "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120,
             "shape": { "kind": "rounded_rect", "radius": 8 },
             "tail": { "at_start": true, "len_dots": 40 },
             "fold": { "at_dots": 160, "parts": 2 } },
  "canvas": { "feed": -4, "head": -4, "dpmm": { "num": 8, "den": 1 } },
  "elements": [{ "type": "rect", "id": 1, "frame": { "feed": 0, "head": 0, "feed_len": 4, "head_len": 4 } }],
  "repeat": "flip"
}"#;
    let l = Label::from_json(s).unwrap();
    let t = targets()[0];
    // Whatever else it is, it is not a panic: this is `Outline::of`'s clamp.
    let _ = Bounds::of(&l);
    let _ = media::faults(&l);
    assert!(rasterise_plane(&l, &t).is_ok());
}

#[test]
fn a_canvas_larger_than_the_page_copies_no_dot_onto_a_row_it_does_not_own() {
    // The canvas and the target disagree - which preflight refuses - and the
    // repeat must still write only inside the page it was given.
    let s = r#"{
  "format": "relabel-label", "version": 1,
  "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120, "fold": { "at_dots": 65636, "parts": 2 } },
  "canvas": { "feed": 200000, "head": 96, "dpmm": { "num": 8, "den": 1 } },
  "elements": [{ "type": "rect", "id": 1, "frame": { "feed": 10, "head": 10, "feed_len": 20, "head_len": 20 } }],
  "repeat": "copy"
}"#;
    let l = Label::from_json(s).unwrap();
    let t = targets()[0];
    let p = rasterise_plane(&l, &t).unwrap();
    assert!(dot(&p, &t, 12, 12), "the first panel is drawn");
    // The crease is at 65636, far past the page, so nothing is copied into it.
    for feed in 100..320 {
        for head in 0..96 {
            assert!(!dot(&p, &t, feed, head), "({feed},{head}) was written");
        }
    }
}

#[test]
fn the_grey_pane_and_the_dot_pane_show_the_same_panels() {
    let t = targets()[0];
    let l = label(&feed_fold(160, 2), "\"copy\"", &ell());
    let (grey, plane, _) = rasterise_preview(&l, &t).unwrap();
    for x in 0..160u16 {
        for y in 0..96u16 {
            assert_eq!(grey.at(x, y), grey.at(x + 160, y), "the grey pane");
            assert_eq!(plane.get(x, y), plane.get(x + 160, y), "the dot pane");
        }
    }
    assert!(grey.coverage(128) > 0.0);
    let (plain, _) = rasterise(&l, &t).unwrap();
    assert!(
        grey.coverage(128) > plain.coverage(128),
        "the repeated map counts both panels"
    );
}

#[test]
fn a_label_drawn_for_another_head_says_so_without_stopping_the_run() {
    let s = r#"{
  "format": "relabel-label", "version": 1,
  "meta": { "authored_for": { "head_px": 192, "dpi": 300 } },
  "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
  "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
  "elements": []
}"#;
    let l = Label::from_json(s).unwrap();
    let t = targets()[0];
    let p = preflight(&l, None, &t, now()).unwrap();
    assert!(p.issues.iter().any(|i| matches!(
        i.kind,
        IssueKind::AuthoredElsewhere {
            head_px: 192,
            dpi: 300
        }
    )));
    assert!(!p.refused());
}

#[test]
fn a_half_split_label_copied_matches_its_golden() {
    let t = targets()[0];
    let l = label(&feed_fold(160, 2), "\"copy\"", &ell());
    common::check("fold-copy", &rasterise_plane(&l, &t).unwrap());
}

#[test]
fn a_half_split_label_flipped_matches_its_golden() {
    let t = targets()[0];
    let l = label(&feed_fold(160, 2), "\"flip\"", &ell());
    common::check("fold-flip", &rasterise_plane(&l, &t).unwrap());
}

#[test]
#[ignore = "eyeballing the goldens"]
fn show() {
    let t = targets()[0];
    for r in ["\"copy\"", "\"flip\"", "\"none\""] {
        let l = label(&feed_fold(160, 2), r, &ell());
        let p = rasterise_plane(&l, &t).unwrap();
        println!("--- {r}");
        for y in (0..96).step_by(4) {
            let row: String = (0..320)
                .step_by(4)
                .map(|x| if p.get(x, y) { '#' } else { '.' })
                .collect();
            println!("{row}");
        }
    }
}

/// A repeat writes into panels, and the paper past the last panel is not one.
/// A trailing tail is exactly that paper: the report calls a mark there hidden
/// by the tail, so the dots must still be the ones that were drawn.
#[test]
fn a_repeat_does_not_write_past_the_content_onto_a_trailing_tail() {
    let media = format!(
        r#", "tail": {{ "at_start": false, "len_dots": 40 }}{}"#,
        feed_fold(200, 2)
    );
    let l = label(
        &media,
        "\"copy\"",
        &format!("{}, {}", ell(), rect(3, 290, 40, 8, 8)),
    );
    let plan = media::plan(&l).unwrap().expect("a fold and a repeat");
    assert_eq!(
        (plan.pitch, plan.short),
        (200, 120),
        "content runs 0..280, so the second panel is 120 dots short"
    );
    for t in targets() {
        let p = rasterise_plane(&l, &t).unwrap();
        assert!(dot(&p, &t, 292, 42), "the mark on the tail is still drawn");
        assert!(
            !dot(&p, &t, 292, 90),
            "and nothing of panel 0 was copied over it"
        );
    }
}

/// `short` is a number about the plane, not only about the report: the dots it
/// says are cut are the dots that are missing.
#[test]
fn the_dots_a_short_panel_loses_are_the_dots_it_reports() {
    let media = format!(
        r#", "tail": {{ "at_start": false, "len_dots": 40 }}{}"#,
        feed_fold(200, 2)
    );
    let l = label(&media, "\"copy\"", &rect(1, 0, 0, 200, 96));
    let plan = media::plan(&l).unwrap().unwrap();
    let t = Target::with_direction(320, 96, 203, PrintDirection::Left).unwrap();
    let p = rasterise_plane(&l, &t).unwrap();
    let inked = |feed: i32| (0..96).filter(|&h| dot(&p, &t, feed, h)).count();
    assert_eq!(inked(279), 96, "the last dot of the content is written");
    assert_eq!(inked(280), 0, "and the first dot past it is not");
    assert_eq!(
        plan.short, 120,
        "which is the 120 dots of panel 0 that had no room"
    );
}
