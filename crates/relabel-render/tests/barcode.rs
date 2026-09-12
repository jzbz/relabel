//! Linear barcodes, rendered and read back.
//!
//! The central test does not trust the encoder. It renders a Code 128 onto a
//! label, reads one row of the resulting dots back into bars, and hands those
//! to `code128`'s own ISO/IEC 15417 decoder. A bar one dot out of place, or a
//! module width that drifted, and the decoder refuses it.

mod common;

use relabel_proto::BitPlane;
use relabel_render::barcode::{self, MIN_MODULE_DOTS, Symbology};
use relabel_render::document::*;
use relabel_render::{BarcodeError, Dpmm, Label, RenderReport, Target, rasterise};

fn el(sym: &str, data: &str, module: u16, bar: u16) -> BarcodeEl {
    BarcodeEl {
        common: Common::new(
            1,
            RectDots {
                feed: 0,
                head: 8,
                feed_len: 320,
                head_len: 80,
            },
        ),
        symbology: sym.into(),
        data: data.into(),
        module_dots: module,
        bar_dots: bar,
        out_of_spec: false,
    }
}

fn render_all(els: Vec<Element>) -> (BitPlane, RenderReport) {
    let mut l = Label::new(320, 96, Dpmm::DPI_203);
    l.elements = els.into_iter().map(ElementOrRaw::Known).collect();
    let t = Target::new(320, 96, 203).unwrap();
    let (grey, report) = rasterise(&l, &t).unwrap();
    (grey.threshold(128), report)
}

fn render(e: BarcodeEl) -> (BitPlane, RenderReport) {
    render_all(vec![Element::Barcode(e)])
}

/// Read a line of dots back into bars, asserting on the way that every bar and
/// every space is a whole number of modules. The white after the last bar is
/// the trailing quiet zone, not a space, so the last bar's space is zero - the
/// shape `code128` itself produces for its stop pattern.
fn bars(dots: &[bool], module: usize) -> Vec<code128::Bar> {
    let mut i = dots.iter().position(|d| *d).expect("no bars at all");
    let mut out = Vec::new();
    while i < dots.len() {
        let start = i;
        while i < dots.len() && dots[i] {
            i += 1;
        }
        let black = i - start;
        let gap = i;
        while i < dots.len() && !dots[i] {
            i += 1;
        }
        let white = if i < dots.len() { i - gap } else { 0 };
        assert_eq!(
            black % module,
            0,
            "bar at {start} is {black} dots, not whole {module}-dot modules"
        );
        assert_eq!(
            white % module,
            0,
            "space at {gap} is {white} dots, not whole {module}-dot modules"
        );
        out.push(code128::Bar {
            width: (black / module) as u8,
            space: (white / module) as u8,
        });
    }
    out
}

fn row(p: &BitPlane, y: u16) -> Vec<bool> {
    (0..p.width()).map(|x| p.get(x, y)).collect()
}

fn column(p: &BitPlane, x: u16) -> Vec<bool> {
    (0..p.height()).map(|y| p.get(x, y)).collect()
}

#[test]
fn a_rendered_code128_decodes_back_to_its_data() {
    for data in [
        "12345678",
        "ABC-1234",
        "A1B2C3D4E5",
        "relabel",
        "Hello, world",
        "0042",
        "x",
        "aB3$%&",
    ] {
        for module in [2u16, 3] {
            let width = barcode::width_dots(Symbology::Code128, data, module).unwrap();
            let (p, report) = render(el("code128", data, module, 40));
            if width > 320 {
                assert_eq!(
                    report.failed.len(),
                    1,
                    "{data} at {module} should be refused"
                );
                continue;
            }
            assert!(report.is_clean(), "{data} at {module}: {report:?}");
            let decoded = code128::decode(&bars(&row(&p, 20), usize::from(module)))
                .unwrap_or_else(|e| panic!("{data} at {module} dots did not decode: {e}"));
            assert_eq!(decoded, data.as_bytes(), "{data} at {module} dots");
        }
    }
}

/// The capacity readout must equal what is drawn, to the dot.
#[test]
fn the_measured_width_is_the_rendered_width() {
    let module = 2u16;
    let m = barcode::encode(Symbology::Code128, "relabel").unwrap();
    let (p, _) = render(el("code128", "relabel", module, 40));
    let dots = row(&p, 20);
    let first = dots.iter().position(|d| *d).unwrap();
    let last = dots.iter().rposition(|d| *d).unwrap() + 1;
    assert_eq!(
        first,
        10 * usize::from(module),
        "a 10-module quiet zone first"
    );
    assert_eq!(
        last,
        (m.len as usize - 10) * usize::from(module),
        "and one after"
    );
    assert_eq!(
        m.width_dots(module) as usize,
        last + 10 * usize::from(module)
    );
}

/// The design's own capacity table: eighteen digits fit across a 320-dot label
/// at two dots a module, twenty do not.
#[test]
fn eighteen_digits_fit_at_two_dots_and_twenty_do_not() {
    let w = |s: &str| barcode::width_dots(Symbology::Code128, s, 2).unwrap();
    assert!(w("123456789012345678") <= 320);
    assert!(w("12345678901234567890") > 320);
}

#[test]
fn a_symbol_too_long_for_its_frame_is_refused_not_clipped() {
    let data = "THIS IS FAR TOO LONG FOR A FORTY MILLIMETRE LABEL";
    let (p, report) = render(el("code128", data, 3, 40));
    assert_eq!(report.failed.len(), 1);
    assert!(
        report.failed[0].1.to_string().contains("clipped"),
        "{}",
        report.failed[0].1
    );
    assert!(row(&p, 20).iter().all(|d| !d), "nothing drawn at all");
}

/// Nothing else would make the rule visible: on a 12 mm label no EAN or UPC
/// can be printed to its specification at any width.
#[test]
fn retail_symbols_are_refused_on_a_twelve_millimetre_label() {
    for (sym, data) in [
        ("ean13", "590123412345"),
        ("ean8", "9638507"),
        ("upca", "03600029145"),
    ] {
        for module in [2u16, 3, 4] {
            let (_, report) = render(el(sym, data, module, 80));
            assert_eq!(report.failed.len(), 1, "{sym} at {module} dots");
            assert!(
                report.failed[0].1.to_string().contains("Code 128"),
                "{}",
                report.failed[0].1
            );
        }
    }
    let mut anyway = el("ean13", "590123412345", 2, 80);
    anyway.out_of_spec = true;
    assert!(
        render(anyway).1.is_clean(),
        "unless explicitly marked out of spec"
    );
}

/// The crate's alias encodes a twelve-digit UPC-A as a different number. The
/// wrapper takes the UPC as written and produces the EAN-13 it really is.
#[test]
fn upc_a_encodes_the_number_it_was_given() {
    let full = barcode::encode(Symbology::UpcA, "036000291452").unwrap();
    let short = barcode::encode(Symbology::UpcA, "03600029145").unwrap();
    let as_ean = barcode::encode(Symbology::Ean13, "0036000291452").unwrap();
    assert_eq!(full.bars, short.bars, "with or without its check digit");
    // The same bars as the EAN-13 it is. Only the quiet zones differ - UPC-A
    // asks for 9 modules each side, EAN-13 for 11 before and 7 after - so
    // compare the symbols from their first bar.
    let from_first = |m: &barcode::Modules| {
        let lead = m.bars[0].0;
        m.bars
            .iter()
            .map(|&(x, w)| (x - lead, w))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        from_first(&full),
        from_first(&as_ean),
        "and the EAN-13 it is"
    );
    assert_eq!((full.bars[0].0, as_ean.bars[0].0), (9, 11));
    assert_eq!(full.len, as_ean.len, "18 modules of quiet zone either way");

    let trap = barcoders::sym::ean13::UPCA::new("036000291452")
        .unwrap()
        .encode();
    let honest = barcoders::sym::ean13::EAN13::new("0036000291452")
        .unwrap()
        .encode();
    assert_ne!(trap, honest, "the alias this wrapper exists to avoid");

    assert!(matches!(
        barcode::encode(Symbology::UpcA, "036000291453"),
        Err(BarcodeError::Checksum { .. })
    ));
}

#[test]
fn codabar_needs_its_start_and_stop_letters() {
    assert_eq!(
        barcode::encode(Symbology::Codabar, "12345"),
        Err(BarcodeError::CodabarGuards)
    );
    assert_eq!(
        barcode::encode(Symbology::Codabar, "A12B45C"),
        Err(BarcodeError::CodabarGuards),
        "a guard letter in the middle"
    );
    assert!(barcode::encode(Symbology::Codabar, "A40156B").is_ok());
}

/// The crate would append a check digit and print a different value.
#[test]
fn odd_length_itf_is_refused_rather_than_altered() {
    assert_eq!(
        barcode::encode(Symbology::Itf, "12345"),
        Err(BarcodeError::OddLength { len: 5 })
    );
    assert!(barcode::encode(Symbology::Itf, "123456").is_ok());
}

#[test]
fn data_that_cannot_be_encoded_is_an_error() {
    assert!(matches!(
        barcode::encode(Symbology::Code128, "日本"),
        Err(BarcodeError::Character { .. })
    ));
    assert!(matches!(
        barcode::encode(Symbology::Code39, "lower"),
        Err(BarcodeError::Character { .. })
    ));
    for s in Symbology::ALL {
        assert_eq!(barcode::encode(s, ""), Err(BarcodeError::Empty), "{s:?}");
    }
}

/// Code 93 is deliberately absent; a document naming it is reported, not a
/// crash.
#[test]
fn an_unknown_symbology_is_reported() {
    let (_, report) = render(el("code93", "RELABEL", 2, 40));
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].1.to_string().contains("code93"));
}

#[test]
fn the_module_floor_holds_unless_overridden() {
    assert_eq!(MIN_MODULE_DOTS, 2);
    let (_, report) = render(el("code128", "1234", 1, 40));
    assert!(report.failed[0].1.to_string().contains("2-dot floor"));

    let mut narrow = el("code128", "1234", 1, 40);
    narrow.out_of_spec = true;
    assert!(render(narrow).1.is_clean());

    let mut zero = el("code128", "1234", 0, 40);
    zero.out_of_spec = true;
    assert_eq!(render(zero).1.failed.len(), 1, "zero is never a width");
}

/// A quarter turn runs the symbol down the head axis, and it still decodes -
/// read from a column this time instead of a row.
#[test]
fn a_quarter_turn_runs_the_symbol_across_the_head() {
    let mut e = el("code128", "1", 1, 30);
    e.out_of_spec = true;
    e.common.frame = RectDots {
        feed: 100,
        head: 0,
        feed_len: 96,
        head_len: 40,
    };
    e.common.rot = Rot::D90;
    let (p, report) = render(e);
    assert!(report.is_clean(), "{report:?}");
    let decoded = code128::decode(&bars(&column(&p, 110), 1)).unwrap();
    assert_eq!(decoded, b"1");
}

#[test]
fn a_half_turn_mirrors_the_modules() {
    let (upright, _) = render(el("code128", "relabel", 2, 40));
    let mut e = el("code128", "relabel", 2, 40);
    e.common.rot = Rot::D180;
    let (turned, _) = render(e);
    let w = barcode::width_dots(Symbology::Code128, "relabel", 2).unwrap() as usize;
    let (a, b) = (row(&upright, 20), row(&turned, 20));
    for x in 0..w {
        assert_eq!(a[x], b[w - 1 - x], "dot {x}");
    }
}

/// A symbol knocks out whatever is beneath its footprint, so its quiet zones
/// are clear even over a filled panel.
#[test]
fn the_quiet_zone_is_knocked_out_of_anything_beneath() {
    let panel = Element::Rect(RectEl {
        common: Common::new(
            9,
            RectDots {
                feed: 0,
                head: 0,
                feed_len: 320,
                head_len: 96,
            },
        ),
        stroke_dots: 0,
        radius_dots: 0,
    });
    let (p, _) = render_all(vec![panel, Element::Barcode(el("code128", "1234", 2, 40))]);
    let dots = row(&p, 20);
    assert!(
        dots[..20].iter().all(|d| !d),
        "the 10-module quiet zone is paper"
    );
    assert!(
        p.get(0, 4),
        "while the panel outside the symbol is untouched"
    );
}

#[test]
fn every_symbology_matches_its_golden() {
    for (sym, data, out_of_spec) in [
        // Letters then a digit run, so the golden pins the switch into set C.
        ("code128", "AB-12345678", false),
        ("code39", "RELABEL", false),
        ("itf", "12345678", false),
        ("codabar", "A40156B", false),
        // Retail symbols cannot be compliant on this canvas, so the goldens
        // exercise the geometry with the override on.
        ("ean13", "590123412345", true),
        ("ean8", "9638507", true),
        ("upca", "03600029145", true),
    ] {
        let mut e = el(sym, data, 2, 40);
        e.out_of_spec = out_of_spec;
        let (p, report) = render(e);
        assert!(report.is_clean(), "{sym}: {report:?}");
        common::check(&format!("barcode-{sym}"), &p);
    }
}

/// Frame positions come straight out of a file. A barcode placed near the end
/// of the `i32` range must be reported as off the canvas, not overflow while
/// its bars are being laid out.
#[test]
fn a_barcode_at_the_edge_of_the_coordinate_range_does_not_overflow() {
    for (feed, head) in [(i32::MAX - 5, 8), (8, i32::MAX - 5), (i32::MIN, i32::MIN)] {
        let mut e = el("code128", "1234", 2, 40);
        e.common.frame.feed = feed;
        e.common.frame.head = head;
        // A frame is stored unrotated, so the same frame serves the turned
        // symbol; only the placement arithmetic differs.
        let mut turned = e.clone();
        turned.common.rot = Rot::D90;
        for e in [e, turned] {
            let (p, report) = render(e);
            assert_eq!(report.off_canvas, vec![1], "({feed}, {head})");
            assert!(report.failed.is_empty(), "({feed}, {head}): {report:?}");
            assert!(
                (0..p.height()).all(|y| (0..p.width()).all(|x| !p.get(x, y))),
                "nothing lands on the canvas"
            );
        }
    }
}

/// The same picture on a printer whose head runs the other way is the same
/// picture. An element's reading direction follows the canvas, not the
/// document's own feed axis: `local_size` measures a frame through the target,
/// so text has always done this, and a barcode that did not ran down a `Top`
/// canvas while the text beside it ran across - and refused a landscape frame
/// as `TooWide` on the 53 models of the registry whose head runs that way.
#[test]
fn a_barcode_reads_the_way_text_does_on_both_printers() {
    use relabel_proto::PrintDirection;
    use relabel_render::rasterise;

    // One screen rectangle, written in each printer's document axes.
    let label = |dir, el: &str| {
        let frame = match dir {
            PrintDirection::Left => r#""feed": 8, "head": 4, "feed_len": 170, "head_len": 60"#,
            PrintDirection::Top => r#""feed": 4, "head": 8, "feed_len": 60, "head_len": 170"#,
        };
        let s = format!(
            r#"{{ "format": "relabel-label", "version": 1,
                 "media": {{ "feed_tenths_mm": 250, "head_tenths_mm": 250 }},
                 "canvas": {{ "feed": 200, "head": 200, "dpmm": {{ "num": 8, "den": 1 }} }},
                 "elements": [ {{ "id": 1, "frame": {{ {frame} }}, {el} }} ] }}"#
        );
        Label::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
    };
    let kinds = [
        (
            "text",
            r#""type": "text", "text": "AB", "family": "Noto Sans", "size": 24.0"#,
        ),
        (
            "barcode",
            r#""type": "barcode", "symbology": "code128", "data": "AB", "module_dots": 2, "bar_dots": 40"#,
        ),
    ];
    for (name, el) in kinds {
        let mut drawn = Vec::new();
        for dir in [PrintDirection::Left, PrintDirection::Top] {
            let t = Target::with_direction(200, 200, 203, dir).unwrap();
            let (g, r) = rasterise(&label(dir, el), &t).unwrap();
            assert!(r.failed.is_empty(), "{name} on {dir:?}: {:?}", r.failed);
            assert!(g.px.iter().any(|&p| p != 255), "{name} on {dir:?}: drawn");
            drawn.push(g.px);
        }
        assert_eq!(
            drawn[0], drawn[1],
            "{name}: the same picture, dot for dot, on both printers"
        );
    }
}
