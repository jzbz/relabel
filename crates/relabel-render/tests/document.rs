//! The document format, and the promise it makes to future versions.
//!
//! The forward-compatibility tests are the point of this file. A saved label is
//! the one artefact a user cannot regenerate, so a build that silently drops
//! what it does not understand destroys work. Everything here is about not
//! doing that.

use relabel_render::document::*;
use relabel_render::{Dpmm, Label, Target, rasterise};

fn label() -> Label {
    Label::new(320, 96, Dpmm::DPI_203)
}

#[test]
fn a_new_label_matches_the_reference_roll() {
    let l = label();
    assert_eq!(l.canvas.feed, 320);
    assert_eq!(l.canvas.head, 96);
    assert_eq!(l.media.feed_tenths_mm, 400, "40.0 mm");
    assert_eq!(l.media.head_tenths_mm, 120, "12.0 mm");
    assert_eq!(l.format, FORMAT_TAG);
}

#[test]
fn a_label_round_trips_through_json() {
    let mut l = label();
    l.meta.title = "Shelf tag".into();
    l.elements.push(ElementOrRaw::Known(Element::Rect(RectEl {
        common: Common::new(
            1,
            RectDots {
                feed: 4,
                head: 4,
                feed_len: 100,
                head_len: 40,
            },
        ),
        stroke_dots: 2,
        radius_dots: 0,
    })));
    let json = l.to_json().unwrap();
    assert!(
        json.ends_with('\n'),
        "trailing newline, so labels diff in git"
    );
    assert_eq!(Label::from_json(&json).unwrap(), l);
}

/// The promise: a field written by a later version comes back out unharmed.
#[test]
fn unknown_top_level_fields_survive_a_round_trip() {
    let json = r#"{
      "format": "relabel-label",
      "version": 1,
      "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
      "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
      "elements": [],
      "colour_profile": { "name": "something from 2027" }
    }"#;
    let l = Label::from_json(json).unwrap();
    assert!(l.unknown.contains_key("colour_profile"));
    let back = l.to_json().unwrap();
    assert!(
        back.contains("something from 2027"),
        "a field this build does not understand must not be deleted"
    );
}

/// And the harder half: an element *type* that did not exist when this build
/// was compiled. `#[serde(other)]` cannot do this - it is only allowed on a
/// unit variant, so it would discard the payload.
#[test]
fn unknown_element_types_survive_a_round_trip() {
    let json = r#"{
      "format": "relabel-label",
      "version": 1,
      "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
      "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
      "elements": [
        { "type": "rect", "id": 1, "frame": { "feed": 0, "head": 0, "feed_len": 10, "head_len": 10 } },
        { "type": "hologram", "id": 2, "wavelength_nm": 632 }
      ]
    }"#;
    let l = Label::from_json(json).unwrap();
    assert_eq!(l.elements.len(), 2);
    assert_eq!(l.drawable().count(), 1, "only the rect can be drawn");
    assert_eq!(l.unrecognised().collect::<Vec<_>>(), vec!["hologram"]);

    let back = l.to_json().unwrap();
    assert!(back.contains("hologram"));
    assert!(back.contains("632"), "its payload survives too");
    assert_eq!(Label::from_json(&back).unwrap(), l, "stable across saves");
}

/// An untagged enum accepts any object, so genuinely corrupt input would
/// otherwise become a silent placeholder rather than an error.
#[test]
fn an_element_with_no_type_is_rejected() {
    let json = r#"{
      "format": "relabel-label", "version": 1,
      "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
      "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
      "elements": [ { "id": 7, "colour": "beige" } ]
    }"#;
    assert!(matches!(
        Label::from_json(json),
        Err(LoadError::UntypedElement)
    ));
}

#[test]
fn foreign_json_fails_on_the_format_tag() {
    let json = r#"{"format":"some-other-tool","version":1,
      "media":{"feed_tenths_mm":1,"head_tenths_mm":1},
      "canvas":{"feed":1,"head":1,"dpmm":{"num":8,"den":1}},"elements":[]}"#;
    assert!(matches!(
        Label::from_json(json),
        Err(LoadError::NotALabel(_))
    ));
}

/// A future *major* version is the one case that is refused rather than
/// preserved, because by definition this build would read it wrongly.
#[test]
fn a_future_major_version_is_refused_clearly() {
    let json = r#"{"format":"relabel-label","version":99,
      "media":{"feed_tenths_mm":1,"head_tenths_mm":1},
      "canvas":{"feed":1,"head":1,"dpmm":{"num":8,"den":1}},"elements":[]}"#;
    assert!(matches!(
        Label::from_json(json),
        Err(LoadError::TooNew { found: 99, .. })
    ));
}

#[test]
fn rotation_serialises_as_a_degree() {
    assert_eq!(serde_json::to_string(&Rot::D90).unwrap(), "90");
    assert_eq!(serde_json::from_str::<Rot>("270").unwrap(), Rot::D270);
    // A negative angle is normalised rather than rejected.
    assert_eq!(serde_json::from_str::<Rot>("-90").unwrap(), Rot::D270);
}

/// An angle this renderer cannot draw must survive the file even though the
/// drawing is approximated. Quantising on load would write the approximation
/// back and destroy what the user asked for.
#[test]
fn an_unmodelled_angle_survives_but_is_approximated() {
    let r: Rot = serde_json::from_str("45").unwrap();
    assert_eq!(r, Rot::Other(45));
    assert!(!r.is_exact());
    assert_eq!(
        serde_json::to_string(&r).unwrap(),
        "45",
        "written back as 45"
    );
    assert_eq!(
        r.quantised(),
        Rot::D90,
        "but drawn as the nearest quarter turn"
    );
    assert_eq!(Rot::Other(44).quantised(), Rot::D0);
    assert_eq!(
        Rot::Other(359).quantised(),
        Rot::D0,
        "wraps rather than reaching 360"
    );
}

#[test]
fn a_filled_rect_and_an_outline_differ() {
    let t = Target::new(320, 96, 203).unwrap();
    let mut l = label();
    l.elements.push(ElementOrRaw::Known(Element::Rect(RectEl {
        common: Common::new(
            1,
            RectDots {
                feed: 10,
                head: 10,
                feed_len: 20,
                head_len: 20,
            },
        ),
        stroke_dots: 0,
        radius_dots: 0,
    })));
    let (filled, report) = rasterise(&l, &t).unwrap();
    assert!(report.is_clean());
    assert_eq!(filled.at(20, 20), 0, "the middle is inked when filled");

    if let ElementOrRaw::Known(Element::Rect(e)) = &mut l.elements[0] {
        e.stroke_dots = 2;
    }
    let (outlined, _) = rasterise(&l, &t).unwrap();
    assert_eq!(outlined.at(20, 20), 255, "and clear when outlined");
    assert_eq!(outlined.at(10, 20), 0, "but the edge is still there");
}

#[test]
fn hidden_elements_are_not_drawn() {
    let t = Target::new(320, 96, 203).unwrap();
    let mut l = label();
    let mut common = Common::new(
        1,
        RectDots {
            feed: 0,
            head: 0,
            feed_len: 50,
            head_len: 50,
        },
    );
    common.hidden = true;
    l.elements.push(ElementOrRaw::Known(Element::Rect(RectEl {
        common,
        stroke_dots: 0,
        radius_dots: 0,
    })));
    let (g, _) = rasterise(&l, &t).unwrap();
    assert!(g.px.iter().all(|p| *p == 255));
}

/// Every element in the frozen kitchen sink is drawn - it inks its frame -
/// or fails with a reason of its own. None is silently skipped.
#[test]
fn every_known_kind_is_drawn() {
    let t = Target::new(320, 96, 203).unwrap();
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/v1/kitchen-sink.relabel"),
    )
    .unwrap();
    let l = Label::from_json(&text).unwrap();
    for el in l.drawable() {
        let mut one = l.clone();
        one.elements = vec![ElementOrRaw::Known(el.clone())];
        let (g, report) = rasterise(&one, &t).unwrap();
        let inked = g.px.iter().any(|&p| p < 255);
        let failed = report
            .failed
            .iter()
            .any(|(id, why)| *id == el.common().id && !why.to_string().is_empty());
        assert!(
            inked != failed,
            "{}: inked {inked}, failed {failed}: {report:?}",
            el.kind()
        );
    }
}

/// A placeholder in text is stored as written, so it survives a save and is
/// resolved at print time rather than at authoring time.
#[test]
fn a_placeholder_is_stored_not_resolved() {
    let mut l = label();
    l.elements.push(ElementOrRaw::Known(Element::Text(TextEl {
        common: Common::new(1, RectDots::default()),
        text: "{sku}".into(),
        family: String::new(),
        size: 16.0,
        line_height: 1.0,
        align: TextAlign::Start,
        weight: TextWeight::Regular,
        raster: GlyphRaster::Hinted,
    })));
    let back = Label::from_json(&l.to_json().unwrap()).unwrap();
    match back.drawable().next().unwrap() {
        Element::Text(t) => assert_eq!(t.text, "{sku}"),
        other => panic!("expected text, got {other:?}"),
    }
}

// --- Defects found by an adversarial review of this format, each with the
// --- reproducer that found it. These are the tests that matter most: every one
// --- of them was a way to lose a user's work.

/// An untagged enum falls through on *any* deserialisation error, so a known
/// element type with one bad field silently became an opaque blob: the file
/// loaded, the element vanished from the label, and the report blamed a future
/// version for a type this build understands.
#[test]
fn a_malformed_known_element_is_an_error_not_a_silent_blob() {
    let wrap = |el: &str| {
        format!(
            r#"{{"format":"relabel-label","version":1,
               "media":{{"feed_tenths_mm":400,"head_tenths_mm":120}},
               "canvas":{{"feed":320,"head":96,"dpmm":{{"num":8,"den":1}}}},
               "elements":[{el}]}}"#
        )
    };
    let frame = r#""frame":{"feed":0,"head":0,"feed_len":10,"head_len":10}"#;
    for bad in [
        format!(r#"{{"type":"rect","id":1,{frame},"stroke_dots":"2"}}"#),
        r#"{"type":"rect","id":1}"#.to_string(),
        format!(r#"{{"type":"rect","id":1,{frame},"ink":"red"}}"#),
    ] {
        match Label::from_json(&wrap(&bad)) {
            Err(LoadError::MalformedElement { kind, .. }) => assert_eq!(kind, "rect"),
            other => panic!("expected a malformed-element error, got {other:?} for {bad}"),
        }
    }
    // And a genuinely unknown type still passes through untouched.
    assert!(Label::from_json(&wrap(r#"{"type":"hologram","id":2}"#)).is_ok());
}

/// Adding a shape or a repeat mode is additive and does not bump the version,
/// so an older build meets an unrecognised value sooner or later. Refusing the
/// whole file over a drawing guide would be absurd; defaulting it silently
/// would lose it on the next save.
#[test]
fn an_unknown_enum_value_does_not_make_a_label_unopenable() {
    let json = r#"{"format":"relabel-label","version":1,
      "media":{"feed_tenths_mm":400,"head_tenths_mm":120,"shape":{"kind":"oval"}},
      "canvas":{"feed":320,"head":96,"dpmm":{"num":8,"den":1}},
      "elements":[],"repeat":"mirror"}"#;
    let l = Label::from_json(json).unwrap();
    assert!(matches!(l.media.shape, MediaShape::Unknown(_)));
    assert_eq!(l.repeat, Repeat::Unknown("mirror".into()));
    let back = l.to_json().unwrap();
    assert!(back.contains("oval"), "the shape survives");
    assert!(back.contains("mirror"), "and so does the repeat mode");
}

/// One escape hatch per element, on `Common`. Two flattened maps in one struct
/// is not merely redundant - the outer absorbs the inner's fields too, and the
/// save emits duplicate keys the same build then refuses to load.
#[test]
fn a_future_field_on_any_element_survives() {
    let json = r#"{"format":"relabel-label","version":1,
      "media":{"feed_tenths_mm":400,"head_tenths_mm":120},
      "canvas":{"feed":320,"head":96,"dpmm":{"num":8,"den":1}},
      "elements":[{"type":"rect","id":1,
        "frame":{"feed":0,"head":0,"feed_len":10,"head_len":10},
        "stroke_dots":2,"dash_dots":4,"fill":true}]}"#;
    let l = Label::from_json(json).unwrap();
    let c = l.drawable().next().unwrap().common();
    assert_eq!(c.id, 1, "the known fields still land where they belong");
    assert!(c.unknown.contains_key("dash_dots"));
    assert!(c.unknown.contains_key("fill"));

    let back = l.to_json().unwrap();
    assert_eq!(
        back.matches(r#""type""#).count(),
        1,
        "no duplicate type key"
    );
    assert_eq!(Label::from_json(&back).unwrap(), l, "and it reloads");
}

/// `serde_json` writes a non-finite float as `null`, so a size that became NaN
/// through a shrink-to-fit division would save clean and then fail to load,
/// putting the text in the file but not on the label.
#[test]
fn a_save_that_could_not_be_reloaded_is_refused() {
    let mut l = label();
    l.elements.push(ElementOrRaw::Known(Element::Text(TextEl {
        common: Common::new(3, RectDots::default()),
        text: "x".into(),
        family: String::new(),
        size: f32::NAN,
        line_height: 1.0,
        align: TextAlign::Start,
        weight: TextWeight::Regular,
        raster: GlyphRaster::Hinted,
    })));
    assert!(matches!(
        l.to_json(),
        Err(LoadError::BadFloat {
            what: "size",
            id: 3,
            ..
        })
    ));
}

/// Not reachable by deserialising - serde consumes known names first - but
/// entirely reachable by code writing into these public maps, which is exactly
/// what an importer would do.
#[test]
fn a_reserved_key_in_an_escape_hatch_is_refused() {
    let mut l = label();
    l.unknown.insert("version".into(), serde_json::json!(99));
    assert!(matches!(l.to_json(), Err(LoadError::ReservedKey(k)) if k == "version"));
}

#[test]
fn the_known_kind_list_matches_the_element_enum() {
    // These two must not drift: `from_json` uses the list to decide whether a
    // `Raw` element is a stranger or a defect.
    let rect = Element::Rect(RectEl {
        common: Common::new(1, RectDots::default()),
        stroke_dots: 0,
        radius_dots: 0,
    });
    assert!(KNOWN_KINDS.contains(&rect.kind()));
    assert_eq!(KNOWN_KINDS.len(), 7);
}

/// A DataMatrix size is written back exactly as it was read, including one this
/// build cannot parse and a non-canonical spelling of one it can.
#[test]
fn a_datamatrix_size_round_trips_as_written() {
    for (s, want) in [
        ("square", DmSize::Square),
        ("rectangle", DmSize::Rectangle),
        ("any", DmSize::Any),
        ("16x48", DmSize::Fixed { rows: 16, cols: 48 }),
        ("016x48", DmSize::Unknown("016x48".into())),
        ("hexagon", DmSize::Unknown("hexagon".into())),
    ] {
        let v: DmSize = serde_json::from_value(serde_json::json!(s)).unwrap();
        assert_eq!(v, want, "{s}");
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!(s),
            "{s}"
        );
    }
}

/// Error correction decides a QR code's size, so it is written out even at its
/// default: a later build choosing a different default must not change what an
/// existing label prints. An unpinned version is simply absent.
#[test]
fn qr_error_correction_is_always_written() {
    let json = r#"{ "type": "qr", "id": 1, "data": "x", "module_dots": 3,
                   "frame": { "feed": 0, "head": 0, "feed_len": 96, "head_len": 96 } }"#;
    let el: Element = serde_json::from_str(json).unwrap();
    let Element::Qr(q) = &el else {
        panic!("not a QR element")
    };
    assert_eq!((q.ec, q.version, q.out_of_spec), (QrEc::M, None, false));
    let back = serde_json::to_string(&el).unwrap();
    assert!(back.contains(r#""ec":"m""#), "{back}");
    assert!(!back.contains("version"), "{back}");
    assert!(!back.contains("out_of_spec"), "{back}");
}

/// The settings a batch depends on - a pinned QR version, a pinned DataMatrix
/// size - and the out-of-spec override survive a real save. Checked in the
/// saved text, because comparing two values would pass if both lost them.
#[test]
fn pinned_code_settings_survive_a_save() {
    let frame = RectDots {
        feed: 0,
        head: 0,
        feed_len: 96,
        head_len: 96,
    };
    let mut l = Label::new(320, 96, relabel_render::Dpmm::DPI_203);
    l.elements.push(ElementOrRaw::Known(Element::Qr(QrEl {
        common: Common::new(1, frame),
        data: "RELABEL".into(),
        module_dots: 3,
        ec: QrEc::H,
        version: Some(3),
        out_of_spec: true,
    })));
    l.elements
        .push(ElementOrRaw::Known(Element::DataMatrix(DataMatrixEl {
            common: Common::new(2, frame),
            data: "RELABEL".into(),
            module_dots: 4,
            size: DmSize::Fixed { rows: 16, cols: 48 },
            out_of_spec: true,
        })));
    l.elements
        .push(ElementOrRaw::Known(Element::DataMatrix(DataMatrixEl {
            common: Common::new(3, frame),
            data: "RELABEL".into(),
            module_dots: 4,
            size: DmSize::default(),
            out_of_spec: false,
        })));
    let saved = l.to_json().unwrap();
    let v: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let els = &v["elements"];
    assert_eq!(els[0]["ec"], "h");
    assert_eq!(els[0]["version"], 3);
    assert_eq!(els[0]["out_of_spec"], true);
    assert_eq!(els[1]["size"], "16x48");
    assert_eq!(els[1]["out_of_spec"], true);
    assert_eq!(els[2]["size"], "square", "the default is written too");
    assert!(els[2].get("out_of_spec").is_none());
    assert_eq!(Label::from_json(&saved).unwrap(), l);
}

/// A text element's style is always written, even at its defaults, for the
/// same reason as a QR code's error correction: a later build choosing other
/// defaults must not change what an existing label prints. Absent, it reads
/// as the defaults; a value this build does not know survives a save.
#[test]
fn text_style_is_always_written_and_open_to_new_values() {
    let frame = serde_json::json!({ "feed": 0, "head": 0, "feed_len": 96, "head_len": 24 });
    let el: Element = serde_json::from_value(serde_json::json!({
        "type": "text", "id": 1, "text": "x", "size": 16, "frame": frame
    }))
    .unwrap();
    let Element::Text(t) = &el else {
        panic!("not a text element")
    };
    assert_eq!(
        (&t.align, &t.weight, &t.raster),
        (
            &TextAlign::Start,
            &TextWeight::Regular,
            &GlyphRaster::Hinted
        )
    );
    let back = serde_json::to_string(&el).unwrap();
    for field in [
        r#""align":"start""#,
        r#""weight":"regular""#,
        r#""raster":"hinted""#,
    ] {
        assert!(back.contains(field), "{back}");
    }

    for (align, weight, raster, want) in [
        (
            "center",
            "semibold",
            "smooth",
            (TextAlign::Center, TextWeight::SemiBold, GlyphRaster::Smooth),
        ),
        (
            "end",
            "regular",
            "hinted",
            (TextAlign::End, TextWeight::Regular, GlyphRaster::Hinted),
        ),
        (
            "justify",
            "black",
            "dither",
            (
                TextAlign::Unknown("justify".into()),
                TextWeight::Unknown("black".into()),
                GlyphRaster::Unknown("dither".into()),
            ),
        ),
    ] {
        let el: Element = serde_json::from_value(serde_json::json!({
            "type": "text", "id": 1, "text": "x", "size": 16, "frame": frame,
            "align": align, "weight": weight, "raster": raster
        }))
        .unwrap();
        let Element::Text(t) = &el else {
            panic!("not a text element")
        };
        assert_eq!((t.align.clone(), t.weight.clone(), t.raster.clone()), want);
        let back = serde_json::to_value(&el).unwrap();
        assert_eq!(
            (&back["align"], &back["weight"], &back["raster"]),
            (
                &serde_json::json!(align),
                &serde_json::json!(weight),
                &serde_json::json!(raster)
            )
        );
    }
}

/// Each kind's own keys, as written, are exactly the list `to_json` checks an
/// element's escape hatch against: a future field under one of those names
/// would be written twice, and the reload would keep only one.
#[test]
fn each_element_kind_lists_the_keys_it_writes() {
    let f = RectDots::default();
    let els = [
        Element::Text(TextEl {
            common: Common::new(1, f),
            text: "x".into(),
            family: String::new(),
            size: 16.0,
            line_height: 1.0,
            align: TextAlign::Start,
            weight: TextWeight::Regular,
            raster: GlyphRaster::Hinted,
        }),
        Element::Barcode(BarcodeEl {
            common: Common::new(2, f),
            symbology: "code128".into(),
            data: "x".into(),
            module_dots: 2,
            bar_dots: 40,
            out_of_spec: true,
        }),
        Element::Qr(QrEl {
            common: Common::new(3, f),
            data: "x".into(),
            module_dots: 3,
            ec: QrEc::M,
            version: Some(2),
            out_of_spec: true,
        }),
        Element::DataMatrix(DataMatrixEl {
            common: Common::new(4, f),
            data: "x".into(),
            module_dots: 3,
            size: DmSize::Square,
            out_of_spec: true,
        }),
        Element::Image(ImageEl {
            common: Common::new(5, f),
            path: "a.png".into(),
            png: "iVBORw0KGgo=".into(),
            fit: ImageFit::Contain,
            halftone: Some(ImageHalftone::Threshold),
            threshold: Some(128),
        }),
        Element::Rect(RectEl {
            common: Common::new(6, f),
            stroke_dots: 1,
            radius_dots: 1,
        }),
        Element::Line(LineEl {
            common: Common::new(7, f),
            thickness_dots: 1,
        }),
    ];
    assert_eq!(els.len(), KNOWN_KINDS.len());
    let common = [
        "type", "id", "name", "frame", "rot", "ink", "hidden", "locked",
    ];
    for el in &els {
        let v = serde_json::to_value(el).unwrap();
        let mut written: Vec<&str> = v
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|k| !common.contains(k))
            .collect();
        let mut listed = el.own_keys().to_vec();
        written.sort_unstable();
        listed.sort_unstable();
        assert_eq!(written, listed, "{}", el.kind());
    }

    let mut t = TextEl {
        common: Common::new(1, f),
        text: "x".into(),
        family: String::new(),
        size: 16.0,
        line_height: 1.0,
        align: TextAlign::Start,
        weight: TextWeight::Regular,
        raster: GlyphRaster::Hinted,
    };
    t.common
        .unknown
        .insert("raster".into(), serde_json::json!("smooth"));
    let mut l = label();
    l.elements.push(ElementOrRaw::Known(Element::Text(t)));
    assert!(matches!(l.to_json(), Err(LoadError::ReservedKey(k)) if k == "raster"));
}

/// The image element as a build from before artwork was embedded knew it: a
/// path, and nothing else.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct OldImage {
    #[serde(flatten)]
    common: Common,
    path: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OldElement {
    Image(OldImage),
}

/// An older build reading a newer image element keeps it: `path` is there, so
/// the element loads, and the artwork and its settings ride through a save in
/// the unknown-field map.
#[test]
fn an_older_build_keeps_a_newer_image() {
    let new = Element::Image(ImageEl {
        common: Common::new(1, RectDots::default()),
        path: String::new(),
        png: "iVBORw0KGgo=".into(),
        fit: ImageFit::Cover,
        halftone: Some(ImageHalftone::FloydSteinberg),
        threshold: Some(187),
    });
    let old: OldElement = serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
    let OldElement::Image(o) = &old;
    for key in ["png", "fit", "halftone", "threshold"] {
        assert!(o.common.unknown.contains_key(key), "{key}");
    }
    let back: Element = serde_json::from_value(serde_json::to_value(&old).unwrap()).unwrap();
    assert_eq!(back, new);
}

/// `path` is written even when empty: an older build requires it, and
/// refuses the whole label without it.
#[test]
fn an_images_path_is_always_written() {
    let e = Element::Image(ImageEl {
        common: Common::new(1, RectDots::default()),
        path: String::new(),
        png: String::new(),
        fit: ImageFit::default(),
        halftone: None,
        threshold: None,
    });
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(v["path"], "");
    assert_eq!(v["fit"], "contain", "the fit is always written");
    assert!(v.get("png").is_none() && v.get("halftone").is_none());
}

/// The names in the file are the format: each fit and halftone is written as
/// exactly these strings, and a rename would turn this build's labels into
/// unknown values for the next.
#[test]
fn image_value_names_are_the_format() {
    for (fit, name) in [
        (ImageFit::Contain, "contain"),
        (ImageFit::Cover, "cover"),
        (ImageFit::Stretch, "stretch"),
        (ImageFit::Dots, "dots"),
    ] {
        assert_eq!(serde_json::to_value(&fit).unwrap(), name);
    }
    for (h, name) in [
        (ImageHalftone::Threshold, "threshold"),
        (ImageHalftone::FloydSteinberg, "floyd_steinberg"),
        (ImageHalftone::Atkinson, "atkinson"),
        (ImageHalftone::Sierra3, "sierra3"),
        (ImageHalftone::Bayer8, "bayer8"),
    ] {
        assert_eq!(serde_json::to_value(&h).unwrap(), name);
    }
}

fn with_data(data: serde_json::Value) -> String {
    serde_json::json!({
        "format": "relabel-label", "version": 1,
        "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
        "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
        "elements": [], "data": data
    })
    .to_string()
}

/// A CSV binding survives a save, a field a later version added included.
#[test]
fn a_csv_binding_round_trips_with_a_later_field() {
    let l = Label::from_json(&with_data(serde_json::json!({
        "kind": "csv_file", "path": "rows.csv", "delimiter": ";", "has_header": false,
        "encoding": "latin1"
    })))
    .unwrap();
    match &l.data {
        Some(DataBinding::Known(KnownBinding::CsvFile(f))) => {
            assert_eq!(
                (f.path.as_str(), f.delimiter, f.has_header),
                ("rows.csv", ';', false)
            );
            assert!(f.unknown.contains_key("encoding"));
        }
        other => panic!("{other:?}"),
    }
    let saved = l.to_json().unwrap();
    assert!(saved.contains("latin1"));
    assert_eq!(Label::from_json(&saved).unwrap(), l);
    assert!(
        !label().to_json().unwrap().contains("\"data\""),
        "a label with no binding writes none"
    );
}

/// A kind of binding a later build adds is kept exactly as it was written.
#[test]
fn a_later_binding_kind_survives() {
    let v = serde_json::json!({ "kind": "inline", "columns": ["a"], "rows": [["1"]] });
    let l = Label::from_json(&with_data(v.clone())).unwrap();
    assert_eq!(l.data, Some(DataBinding::Unknown(v.clone())));
    let saved: serde_json::Value = serde_json::from_str(&l.to_json().unwrap()).unwrap();
    assert_eq!(saved["data"], v);
}

/// A binding of a kind this build knows, damaged, is an error - not a
/// stranger's data to carry along unread.
#[test]
fn a_damaged_binding_is_refused() {
    for bad in [
        serde_json::json!({ "kind": "csv_file", "path": 5 }),
        serde_json::json!({ "kind": "csv_file", "path": "a", "delimiter": "ab" }),
        serde_json::json!({ "path": "rows.csv" }),
        serde_json::json!("rows.csv"),
    ] {
        assert!(
            matches!(
                Label::from_json(&with_data(bad.clone())),
                Err(LoadError::MalformedBinding { .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn a_binding_takes_its_defaults() {
    let l = Label::from_json(&with_data(serde_json::json!({
        "kind": "csv_file", "path": "rows.csv"
    })))
    .unwrap();
    match l.data {
        Some(DataBinding::Known(KnownBinding::CsvFile(f))) => {
            assert_eq!((f.delimiter, f.has_header), (',', true));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn data_is_a_reserved_key() {
    let mut l = label();
    l.unknown.insert("data".into(), serde_json::json!(1));
    assert!(matches!(l.to_json(), Err(LoadError::ReservedKey(k)) if k == "data"));
}

/// A binding holding one of its own keys among its unknown fields would write
/// that key twice, and reload as a different binding.
#[test]
fn a_binding_cannot_hold_its_own_keys_as_unknown_fields() {
    let mut f = CsvFile {
        path: "rows.csv".into(),
        delimiter: ',',
        has_header: true,
        unknown: Default::default(),
    };
    f.unknown
        .insert("path".into(), serde_json::json!("other.csv"));
    let mut l = label();
    l.data = Some(DataBinding::Known(KnownBinding::CsvFile(f)));
    assert!(matches!(l.to_json(), Err(LoadError::ReservedKey(k)) if k == "path"));
}

/// What loading refuses, saving refuses too: a label that saves and then will
/// not load again is worse than one that was never written.
#[test]
fn a_damaged_binding_cannot_be_written_either() {
    for bad in [
        serde_json::json!("rows.csv"),
        serde_json::json!({}),
        serde_json::json!({ "kind": "csv_file", "path": 5 }),
        serde_json::Value::Null,
    ] {
        let mut l = label();
        l.data = Some(DataBinding::Unknown(bad.clone()));
        assert!(
            matches!(l.to_json(), Err(LoadError::MalformedBinding { .. })),
            "{bad}"
        );
    }
    let mut l = label();
    l.data = Some(DataBinding::Unknown(
        serde_json::json!({ "kind": "inline", "rows": [] }),
    ));
    let saved = l.to_json().unwrap();
    assert_eq!(Label::from_json(&saved).unwrap(), l);
}

/// The frozen fixture that holds a key a later version wrote at every nested
/// place, loaded and saved once.
fn nested_unknowns() -> (Label, String) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/v1/nested-unknowns.relabel");
    let text = std::fs::read_to_string(&path).unwrap();
    let label = Label::from_json(&text).unwrap();
    let saved = label.to_json().unwrap();
    (label, saved)
}

/// A save that has to put strangers back goes through a `Value`, which would
/// write an `f32` as the `f64` it widens to and sort every key. It re-reads the
/// plain serialiser's own text instead, so the floats are the digits that
/// serialiser wrote, and `preserve_order` keeps the document's order.
#[test]
fn a_save_with_strangers_keeps_floats_and_key_order() {
    let (_, saved) = nested_unknowns();
    assert!(saved.contains("\"size\": 12.1,"), "{saved}");
    assert!(saved.contains("\"line_height\": 1.2,"), "{saved}");
    assert!(!saved.contains("12.100000381469727"), "{saved}");
    assert!(
        saved.starts_with("{\n  \"format\": \"relabel-label\",\n  \"version\": 1,\n  \"meta\": {"),
        "{saved}"
    );
    // The stranger's element in the order its file wrote it, not alphabetical.
    let at = |key: &str| {
        saved
            .find(key)
            .unwrap_or_else(|| panic!("{key} is not saved"))
    };
    let hologram = [
        "\"type\": \"hologram\"",
        "\"id\": 3",
        "\"wavelength_nm\"",
        "\"depth\"",
    ]
    .map(at);
    assert!(hologram.is_sorted(), "{saved}");
}

/// With nothing to put back, a save is the plain serialiser's text, byte for
/// byte: the graft never touches a label that does not need it, and strangers
/// at the top level, on an element or in an open enum ride the structs' own
/// catch-alls as they always did.
#[test]
fn a_label_without_strangers_saves_through_the_plain_serialiser() {
    let mut plain = label();
    plain
        .elements
        .push(ElementOrRaw::Known(Element::Rect(RectEl {
            common: Common::new(
                1,
                RectDots {
                    feed: 4,
                    head: 4,
                    feed_len: 100,
                    head_len: 40,
                },
            ),
            stroke_dots: 2,
            radius_dots: 0,
        })));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/v1/from-the-future.relabel");
    let future = Label::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    for l in [plain, future] {
        assert_eq!(
            l.to_json().unwrap(),
            serde_json::to_string_pretty(&l).unwrap() + "\n"
        );
    }
}

/// A stranger is a key outside a struct's list of the keys it knows, not a key
/// its save leaves out. A zero inset, a rectangle and an empty title are all
/// omitted when written; spelled out in a file they are known keys, and the
/// save leaves them out as it would have anyway.
#[test]
fn an_explicit_default_is_not_a_stranger() {
    let json = r#"{
      "format": "relabel-label",
      "version": 1,
      "meta": { "title": "" },
      "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120, "shape": { "kind": "rect" } },
      "canvas": {
        "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 },
        "inset": { "feed_start": 0, "feed_end": 0, "head_start": 0, "head_end": 0 }
      },
      "elements": []
    }"#;
    let mut l = Label::from_json(json).unwrap();
    let saved = l.to_json().unwrap();
    for gone in ["inset", "shape", "title"] {
        assert!(!saved.contains(gone), "{gone} in {saved}");
    }
    l.canvas.inset.feed_start = 8;
    let back = Label::from_json(&l.to_json().unwrap()).unwrap();
    assert_eq!(back.canvas.inset.feed_start, 8);
}

/// A later version's numbers are its own: a save writes back the same value it
/// read, in a raw element, a top-level unknown key and a key kept inside a
/// nested object alike. serde_json's default parser is not correctly rounded
/// at extreme exponents, and without `float_roundtrip` each of these came back
/// one step away.
#[test]
fn a_later_versions_numbers_come_back_as_the_same_values() {
    let numbers = [
        "1.0715660391465826e-75",
        "-1.603964615428183e+143",
        "2.2250738585072014e-308",
        "9.109383701528e-31",
    ];
    let json = format!(
        r#"{{
      "format": "relabel-label",
      "version": 1,
      "meta": {{ "title": "t", "x_meta": {n2} }},
      "media": {{ "feed_tenths_mm": 400, "head_tenths_mm": 120 }},
      "canvas": {{ "feed": 320, "head": 96, "dpmm": {{ "num": 8, "den": 1 }} }},
      "elements": [
        {{ "type": "hologram", "id": 1, "k": {n0}, "frame": {{ "feed": 0, "head": 0, "feed_len": 8, "head_len": 8, "x_frame": {n3} }} }}
      ],
      "x_top": {n1}
    }}"#,
        n0 = numbers[0],
        n1 = numbers[1],
        n2 = numbers[2],
        n3 = numbers[3],
    );
    let saved = Label::from_json(&json).unwrap().to_json().unwrap();
    for n in numbers {
        // Written the way serde_json writes the value itself: shortest, with its
        // own exponent style. std's parse is correctly rounded.
        let written = serde_json::to_string(&n.parse::<f64>().unwrap()).unwrap();
        assert!(saved.contains(&written), "{n} as {written} in {saved}");
    }
}

/// Only a rounded rectangle has a radius. On a rectangle or a circle `radius` is
/// a later version's key like any other, and a save keeps it; on a rounded
/// rectangle it is the struct's own, and the label's value is the one written.
#[test]
fn a_radius_on_a_shape_that_has_none_is_kept() {
    let with_shape = |shape: serde_json::Value| {
        let mut v = serde_json::json!({
            "format": "relabel-label",
            "version": 1,
            "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
            "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
            "elements": []
        });
        v["media"]["shape"] = shape;
        Label::from_json(&v.to_string()).unwrap()
    };
    let saved_shape = |l: &Label| {
        let v: serde_json::Value = serde_json::from_str(&l.to_json().unwrap()).unwrap();
        v["media"]["shape"].clone()
    };
    for shape in [
        serde_json::json!({ "kind": "circle", "radius": 7, "future": 1 }),
        serde_json::json!({ "kind": "rect", "radius": 7 }),
        serde_json::json!({ "kind": "rounded_rect", "radius": 7, "future": 1 }),
    ] {
        let l = with_shape(shape.clone());
        assert_eq!(saved_shape(&l), shape);
        assert_eq!(Label::from_json(&l.to_json().unwrap()).unwrap(), l);
    }
    let mut l = with_shape(serde_json::json!({ "kind": "rounded_rect", "radius": 7, "future": 1 }));
    l.media.shape = MediaShape::Known(KnownShape::RoundedRect { radius: 9 });
    assert_eq!(
        saved_shape(&l),
        serde_json::json!({ "kind": "rounded_rect", "radius": 9, "future": 1 })
    );
}

/// A zero inset is left out of a save, but one that carries a stranger's key
/// cannot be: it is written whole, zeros and all, so the key has somewhere to
/// live.
#[test]
fn a_zero_inset_that_carries_a_stranger_is_written_whole() {
    let (label, saved) = nested_unknowns();
    let v: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        v["canvas"]["inset"],
        serde_json::json!({
            "feed_start": 0,
            "feed_end": 0,
            "head_start": 0,
            "head_end": 0,
            "x_inset": { "from": "blind zones" },
        })
    );
    assert_eq!(Label::from_json(&saved).unwrap(), label);
}

/// A frame's strangers belong to its element: deleting the element takes them
/// out of the next save, and the label that saved compares equal to the one
/// that reloads.
#[test]
fn a_deleted_element_takes_its_frame_strangers_with_it() {
    let (mut label, _) = nested_unknowns();
    label
        .elements
        .retain(|e| e.known().is_none_or(|e| e.common().id != 2));
    let saved = label.to_json().unwrap();
    assert!(!saved.contains("x_frame_2"), "{saved}");
    assert!(saved.contains("x_frame"), "{saved}");
    assert_eq!(Label::from_json(&saved).unwrap(), label);
}

/// Likewise a tail: removed, it takes its strangers with it, and the fold's stay.
#[test]
fn a_removed_tail_takes_its_strangers_with_it() {
    let (mut label, _) = nested_unknowns();
    label.media.tail = None;
    let saved = label.to_json().unwrap();
    assert!(!saved.contains("x_tail"), "{saved}");
    assert!(saved.contains("x_fold"), "{saved}");
    assert_eq!(Label::from_json(&saved).unwrap(), label);
}

/// What `to_json` would refuse, asked before anything is written: the editor
/// refuses a save up front with this, and says so when a label opens.
#[test]
fn writable_names_what_would_stop_a_save() {
    let mut l = label();
    l.elements.push(ElementOrRaw::Known(Element::Text(TextEl {
        common: Common::new(3, RectDots::default()),
        text: "x".into(),
        family: String::new(),
        size: 0.0,
        line_height: 1.0,
        align: TextAlign::Start,
        weight: TextWeight::Regular,
        raster: GlyphRaster::Hinted,
    })));
    assert!(matches!(
        l.writable(),
        Err(LoadError::BadFloat {
            what: "size",
            id: 3,
            ..
        })
    ));
    assert!(l.to_json().is_err());
    assert!(label().writable().is_ok());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/v1/kitchen-sink.relabel");
    let reference = Label::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(reference.writable().is_ok());
}
