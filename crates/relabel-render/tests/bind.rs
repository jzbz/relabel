//! Binding a label to a row: niimblue's grammar and precedence, checked
//! against the vectors the niimblue research ran in its own code, and the
//! three places relabel deliberately parts from it.

use relabel_render::Dpmm;
use relabel_render::bind::{
    BindError, Bindings, MAX_FIELD_BYTES, MAX_TIMES, Table, TimesError, bind, parse_times,
};
use relabel_render::datetime::CivilTime;
use relabel_render::document::*;

/// Fri 2026-09-11 16:05:09.042 at +02:00: the instant the research used.
fn n1() -> CivilTime {
    CivilTime::new(2026, 9, 11, 16, 5, 9, 42, 120).unwrap()
}

fn frame() -> RectDots {
    RectDots {
        feed: 0,
        head: 0,
        feed_len: 96,
        head_len: 96,
    }
}

fn text(id: u32, s: &str) -> Element {
    Element::Text(TextEl {
        common: Common::new(id, frame()),
        text: s.into(),
        family: String::new(),
        size: 16.0,
        line_height: 1.0,
        align: TextAlign::Start,
        weight: TextWeight::Regular,
        raster: GlyphRaster::Hinted,
    })
}

fn label(els: Vec<Element>) -> Label {
    let mut l = Label::new(320, 96, Dpmm::DPI_203);
    l.elements = els.into_iter().map(ElementOrRaw::Known).collect();
    l
}

fn table(columns: &[&str], rows: &[&[&str]]) -> Table {
    Table {
        columns: columns.iter().map(|s| s.to_string()).collect(),
        rows: rows
            .iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect(),
    }
}

/// The research's variables: `{sku:"A1", dt:"ROWDT", $times:"3", empty:"",
/// v:"$&$1$$ {sku}"}`.
fn v() -> Table {
    table(
        &["sku", "dt", "$times", "empty", "v"],
        &[&["A1", "ROWDT", "3", "", "$&$1$$ {sku}"]],
    )
}

/// The first field of the one element, after binding.
fn field(l: &Label) -> String {
    match l.elements[0].known().unwrap() {
        Element::Text(t) => t.text.clone(),
        Element::Barcode(b) => b.data.clone(),
        Element::Qr(q) => q.data.clone(),
        Element::DataMatrix(d) => d.data.clone(),
        other => panic!("{other:?}"),
    }
}

/// One template, bound against a table's first row - or with `None`, against
/// the clock alone, which is niimblue with its CSV switched off.
fn resolve(template: &str, t: Option<&Table>) -> String {
    let l = label(vec![text(1, template)]);
    let b = match t {
        Some(t) => t.bindings(0, n1()).unwrap(),
        None => Bindings::clock_only(n1()),
    };
    field(&bind(&l, &b).unwrap())
}

#[test]
fn the_grammar_matches_niimblue() {
    let v = v();
    let with_v = |s: &str| resolve(s, Some(&v));
    let without = |s: &str| resolve(s, None);
    let same = |s: &str| {
        assert_eq!(with_v(s), s, "{s:?} with the row");
        assert_eq!(without(s), s, "{s:?} without it");
    };
    // G1-G5: a column, however it is spaced, with any filter ignored.
    assert_eq!(with_v("{sku}"), "A1");
    assert_eq!(without("{sku}"), "{sku}", "unresolved stays as written");
    for s in ["{ sku }", "{\tsku\n}", "{\u{a0}sku\u{a0}}", "{\u{feff}sku}"] {
        assert_eq!(with_v(s), "A1", "{s:?}");
    }
    same("{\u{200b}sku}");
    same("{\u{85}sku}");
    assert_eq!(with_v("{sku|upper}"), "A1");
    assert_eq!(with_v("{sku|}"), "A1");
    // G6, G7, G10, G27: not placeholders, or not columns: as written.
    for s in [
        "{missing}",
        "{ missing | f }",
        "{}",
        "{ }",
        "{a b}",
        "{a-b}",
        "{$}",
        "{a$}",
        "{$ times}",
        "{$$times}",
        "{SKU}",
        "{DT}",
        "{constructor}",
        "{__proto__}",
    ] {
        assert_eq!(with_v(s), s, "{s:?}");
    }
    // G8, G9: keys are ASCII word characters, digits and underscores included.
    let accented = table(&["skú"], &[&["X"]]);
    assert_eq!(resolve("{skú}", Some(&accented)), "{skú}");
    let odd = table(&["1", "_", "$1"], &[&["one", "u", "d"]]);
    assert_eq!(resolve("{1}{_}{$1}", Some(&odd)), "oneud");
    // G11: `$times` is an ordinary column to a template.
    assert_eq!(with_v("{$times}"), "3");
    assert_eq!(with_v("{ $times | x }"), "3");
    assert_eq!(without("{$times}"), "{$times}");
    // G12-G20: the clock, and a `dt` column over it.
    for s in ["{dt}", "{dt|}", "{dt| }"] {
        assert_eq!(without(s), "2026-09-11 16:05:09", "{s:?}");
    }
    assert_eq!(with_v("{dt}"), "ROWDT");
    assert_eq!(
        with_v("{dt|YYYY}"),
        "ROWDT",
        "the column wins, filter and all"
    );
    for s in ["{ dt | YYYY }", "{dt|YYYY\n}", "{dt|YYYY\u{2028}}"] {
        assert_eq!(without(s), "2026", "{s:?}");
    }
    assert_eq!(without("{dt|  HH  }"), "16");
    assert_eq!(without("{dt|HH mm}"), "16 05");
    assert_eq!(without("{dt|b|c}"), "b|c");
    assert_eq!(without("{dt|{YYYY}"), "{2026");
    assert_eq!(without("{dt|[}]}"), "[]}");
    assert_eq!(with_v("{dt|[}]}"), "ROWDT]}");
    assert_eq!(without("{dt|{dt}}"), "{5t}");
    assert_eq!(with_v("{dt|{dt}}"), "ROWDT}");
    for s in ["{dt|YYYY\nMM}", "{dt|YYYY\u{2028}MM}", "{dt|a\rb}"] {
        assert_eq!(without(s), s, "{s:?}: a filter cannot cross a line");
    }
    assert_eq!(without("{dt|Today}"), "To5pmy");
    assert_eq!(without("{dt|[Today]}"), "Today");
    assert_eq!(without("{dt|YYYY\u{85}}"), "2026\u{85}");
    // G21-G26: braces around braces, repeats, and nothing scanned twice.
    assert_eq!(with_v("{{sku}}"), "{A1}");
    assert_eq!(with_v("{a{sku}"), "{aA1");
    assert_eq!(with_v("{sku}}"), "A1}");
    assert_eq!(with_v("{sku}{sku}"), "A1A1");
    assert_eq!(with_v("{sku"), "{sku");
    assert_eq!(with_v("sku}"), "sku}");
    assert_eq!(with_v("{sku|{dt}}"), "A1}");
    assert_eq!(with_v("{v}"), "$&$1$$ {sku}", "no re-scan, no $ expansion");
    assert_eq!(with_v("{empty}"), "", "a present, empty cell blanks it");
    assert_eq!(with_v("line1\n{sku}\nline3"), "line1\nA1\nline3");
}

#[test]
fn bind_is_pure() {
    let l = label(vec![text(1, "{sku}"), text(2, "{dt|YYYY}")]);
    let before = l.clone();
    let t = table(&["sku"], &[&["A1"], &["B2"]]);
    for r in 0..2 {
        let bound = bind(&l, &t.bindings(r, n1()).unwrap()).unwrap();
        assert_ne!(bound, l);
    }
    assert_eq!(l, before, "the label bound from is unchanged");
}

/// Text, barcode, QR and DataMatrix data are bound; names, titles, image
/// paths, a later version's elements and its unknown fields are not.
#[test]
fn exactly_four_fields_are_bound() {
    let mut t = text(1, "{sku}");
    if let Element::Text(e) = &mut t {
        e.common.name = "{sku}".into();
        e.common
            .unknown
            .insert("note".into(), serde_json::json!("{sku}"));
    }
    let barcode = Element::Barcode(BarcodeEl {
        common: Common::new(2, frame()),
        symbology: "code128".into(),
        data: "{sku}".into(),
        module_dots: 2,
        bar_dots: 0,
        out_of_spec: false,
    });
    let qr = Element::Qr(QrEl {
        common: Common::new(3, frame()),
        data: "{sku}".into(),
        module_dots: 2,
        ec: QrEc::M,
        version: None,
        out_of_spec: false,
    });
    let dm = Element::DataMatrix(DataMatrixEl {
        common: Common::new(4, frame()),
        data: "{sku}".into(),
        module_dots: 2,
        size: DmSize::Square,
        out_of_spec: false,
    });
    let image = Element::Image(ImageEl {
        common: Common::new(5, frame()),
        path: "{sku}".into(),
        png: String::new(),
        fit: ImageFit::Contain,
        halftone: None,
        threshold: None,
    });
    let mut l = label(vec![t, barcode, qr, dm, image]);
    l.meta.title = "{sku}".into();
    l.elements.push(ElementOrRaw::Raw(serde_json::json!({
        "type": "hologram", "id": 9, "text": "{sku}"
    })));
    let data = table(&["sku"], &[&["A1"]]);
    let bound = bind(&l, &data.bindings(0, n1()).unwrap()).unwrap();
    let json = serde_json::to_value(&bound).unwrap();
    let els = json["elements"].as_array().unwrap();
    assert_eq!(els[0]["text"], "A1");
    assert_eq!(els[1]["data"], "A1");
    assert_eq!(els[2]["data"], "A1");
    assert_eq!(els[3]["data"], "A1");
    assert_eq!(els[0]["name"], "{sku}");
    assert_eq!(els[0]["note"], "{sku}");
    assert_eq!(els[4]["path"], "{sku}");
    assert_eq!(els[5]["text"], "{sku}");
    assert_eq!(json["meta"]["title"], "{sku}");
}

#[test]
fn hidden_elements_are_bound_too() {
    let mut t = text(1, "{sku}");
    if let Element::Text(e) = &mut t {
        e.common.hidden = true;
    }
    let data = table(&["sku"], &[&["A1"]]);
    assert_eq!(
        field(&bind(&label(vec![t]), &data.bindings(0, n1()).unwrap()).unwrap()),
        "A1"
    );
}

/// The frozen kitchen sink has `{sku}` and no `{dt}`: bound to the clock
/// alone, nothing in it changes.
#[test]
fn a_label_with_nothing_to_resolve_binds_to_itself() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/v1/kitchen-sink.relabel");
    let l = Label::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(bind(&l, &Bindings::clock_only(n1())).unwrap(), l);
}

#[test]
fn a_row_must_match_its_header() {
    let l = label(vec![text(1, "{a}")]);
    let t = table(&["a", "b"], &[&["1"], &["1", "2", "3"]]);
    assert_eq!(
        bind(&l, &t.bindings(0, n1()).unwrap()),
        Err(BindError::RowLength {
            cells: 1,
            columns: 2
        })
    );
    assert_eq!(
        bind(&l, &t.bindings(1, n1()).unwrap()),
        Err(BindError::RowLength {
            cells: 3,
            columns: 2
        })
    );
}

/// Two columns of one name are an error only where a placeholder asks for it.
#[test]
fn a_duplicate_column_matters_only_when_used() {
    let t = table(&["a", "a", "b"], &[&["1", "2", "3"]]);
    let b = t.bindings(0, n1()).unwrap();
    assert_eq!(field(&bind(&label(vec![text(1, "{b}")]), &b).unwrap()), "3");
    assert_eq!(
        bind(&label(vec![text(1, "{a}")]), &b),
        Err(BindError::AmbiguousColumn("a".into()))
    );
}

#[test]
fn a_dt_column_replaces_the_clock_even_when_empty() {
    let t = table(&["dt"], &[&[""]]);
    assert_eq!(resolve("[{dt|YY}]", Some(&t)), "[]");
}

#[test]
fn a_cell_that_names_itself_is_not_expanded_again() {
    let t = table(&["v"], &[&["{v}"]]);
    assert_eq!(resolve("{v}{v}", Some(&t)), "{v}{v}");
}

#[test]
fn a_field_too_long_once_bound_names_its_element() {
    let big = "x".repeat(40_000);
    let t = table(&["v"], &[&[&big]]);
    let l = label(vec![text(7, "{v}"), text(8, "{v}{v}")]);
    assert_eq!(
        bind(&l, &t.bindings(0, n1()).unwrap()),
        Err(BindError::FieldTooLong {
            element: 8,
            limit: MAX_FIELD_BYTES
        })
    );
    let exactly = "y".repeat(MAX_FIELD_BYTES);
    let t = table(&["v"], &[&[&exactly]]);
    assert_eq!(
        resolve("{v}", Some(&t)).len(),
        MAX_FIELD_BYTES,
        "the limit itself fits"
    );
}

/// `$times` is strict where niimblue's `parseInt` silently printed the wrong
/// number of labels, or none.
#[test]
fn times_is_a_plain_whole_number() {
    for (cell, want) in [
        ("", 1),
        (" ", 1),
        ("3", 3),
        (" 3", 3),
        ("\u{a0}3", 3),
        ("\u{feff}3", 3),
        ("3 ", 3),
        ("08", 8),
        ("0", 0),
        ("10000", 10_000),
    ] {
        assert_eq!(parse_times(cell), Ok(want), "{cell:?}");
    }
    assert_eq!(MAX_TIMES, 10_000);
    for cell in [
        "10001",
        "3abc",
        "3.9",
        "1e3",
        "1_000",
        "0x10",
        "0X1a",
        "0b11",
        "+2",
        "-1",
        "-0",
        "\u{2212}2",
        "Infinity",
        "abc",
        "\u{663}",
        "\u{85}3",
        "12345678901234567890",
    ] {
        assert_eq!(
            parse_times(cell),
            Err(TimesError {
                value: cell.to_string()
            }),
            "{cell:?}"
        );
    }
}

/// The time is asked for only when a shown element prints it: not when a
/// column called `dt` answers every `{dt}`, and not for a hidden element.
#[test]
fn the_clock_is_needed_only_when_it_prints() {
    use relabel_render::bind::uses_clock;
    use relabel_render::document::{Element, ElementOrRaw, Label};
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/v1/variables.relabel");
    let mut l = Label::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cols = |c: &[&str]| c.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(uses_clock(&l, &cols(&["sku", "name", "$times"])));
    assert!(uses_clock(&l, &[]));
    assert!(!uses_clock(&l, &cols(&["sku", "dt"])));
    for el in &mut l.elements {
        if let ElementOrRaw::Known(Element::Text(t)) = el
            && t.text.contains("{dt")
        {
            t.common.hidden = true;
        }
    }
    assert!(!uses_clock(&l, &[]));
}
