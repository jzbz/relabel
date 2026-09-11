//! QR and DataMatrix, rendered onto a label and checked dot by dot.

mod common;

use relabel_proto::{BitPlane, PrintDirection};
use relabel_render::code2d::{self, ModuleGrid};
use relabel_render::document::*;
use relabel_render::{Dpmm, Label, RenderReport, Target, rasterise};

fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

fn qr(data: &str, module: u16, f: RectDots) -> QrEl {
    QrEl {
        common: Common::new(1, f),
        data: data.into(),
        module_dots: module,
        ec: QrEc::M,
        version: None,
        out_of_spec: false,
    }
}

fn dm(data: &str, module: u16, size: DmSize, f: RectDots) -> DataMatrixEl {
    DataMatrixEl {
        common: Common::new(1, f),
        data: data.into(),
        module_dots: module,
        size,
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

fn render(e: Element) -> (BitPlane, RenderReport) {
    render_all(vec![e])
}

/// A square 96-dot label, so one frame fits either way round, on a target of
/// either direction. On `Top` the canvas has the head across and the feed down.
fn render_on(direction: PrintDirection, e: Element) -> (BitPlane, RenderReport) {
    let mut l = Label::new(96, 96, Dpmm::DPI_203);
    l.elements = vec![ElementOrRaw::Known(e)];
    let t = Target::with_direction(96, 96, 203, direction).unwrap();
    let (grey, report) = rasterise(&l, &t).unwrap();
    (grey.threshold(128), report)
}

/// Cut a rectangle out of a plane.
fn crop(p: &BitPlane, x0: u16, y0: u16, w: u16, h: u16) -> BitPlane {
    let mut out = BitPlane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            out.set(x, y, p.get(x0 + x, y0 + y));
        }
    }
    out
}

/// The inked bounding box, as (x0, y0, x1, y1) exclusive.
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

/// Read the plane back into modules and check every module is uniform: no
/// module is partly inked, so every edge falls on a whole multiple of the
/// module size from the symbol's origin.
fn modules_are_whole(p: &BitPlane, x0: u16, y0: u16, m: u16, cols: u16, rows: u16) {
    for r in 0..rows {
        for c in 0..cols {
            let first = p.get(x0 + c * m, y0 + r * m);
            for dy in 0..m {
                for dx in 0..m {
                    assert_eq!(
                        p.get(x0 + c * m + dx, y0 + r * m + dy),
                        first,
                        "module ({c}, {r}) is not uniform at +({dx}, {dy})"
                    );
                }
            }
        }
    }
}

#[test]
fn a_qr_code_lands_on_whole_module_boundaries() {
    for m in [2u16, 3] {
        let (p, report) = render(Element::Qr(qr("RELABEL", m, frame(10, 0, 100, 96))));
        assert!(report.is_clean(), "{m}: {report:?}");
        let (x0, y0, x1, y1) = ink_box(&p).unwrap();
        // Version 1: 21 modules, placed after a 4-module quiet zone.
        assert_eq!((x0, y0), (10 + 4 * m, 4 * m), "{m}");
        assert_eq!((x1 - x0, y1 - y0), (21 * m, 21 * m), "{m}");
        modules_are_whole(&p, x0, y0, m, 21, 21);
    }
}

#[test]
fn a_datamatrix_lands_on_whole_module_boundaries() {
    let size = DmSize::Fixed { rows: 16, cols: 48 };
    let (p, report) = render(Element::DataMatrix(dm(
        "RELABEL",
        5,
        size,
        frame(0, 0, 320, 96),
    )));
    assert!(report.is_clean(), "{report:?}");
    let (x0, y0, x1, y1) = ink_box(&p).unwrap();
    // A one-module quiet zone, then 48 columns by 16 rows.
    assert_eq!((x0, y0, x1 - x0, y1 - y0), (5, 5, 48 * 5, 16 * 5));
    modules_are_whole(&p, x0, y0, 5, 48, 16);
}

/// The plan's rotation check: a quarter-turned symbol is the upright one run
/// through `BitPlane::rotate_cw`, which is independent code in another crate.
#[test]
fn a_quarter_turn_matches_the_protocol_crates_rotation() {
    let m = 3u16;
    let side = (21 + 8) * m; // version 1 with its quiet zone: 87 dots
    let f = frame(0, 0, i32::from(side), i32::from(side));
    let (upright, a) = render(Element::Qr(qr("RELABEL", m, f)));
    let mut e = qr("RELABEL", m, f);
    e.common.rot = Rot::D90;
    let (turned, b) = render(Element::Qr(e));
    assert!(a.is_clean() && b.is_clean(), "{a:?} {b:?}");
    let want = crop(&upright, 0, 0, side, side).rotate_cw();
    assert_eq!(crop(&turned, 0, 0, side, side), want);
    // And four turns come back to the start.
    let mut back = crop(&upright, 0, 0, side, side);
    for _ in 0..4 {
        back = back.rotate_cw();
    }
    assert_eq!(back, crop(&upright, 0, 0, side, side));
}

#[test]
fn a_symbol_too_big_for_its_frame_is_refused_not_shrunk() {
    // Version 1 at 4 dots needs 116 dots; the label is 96 deep.
    let (p, report) = render(Element::Qr(qr("RELABEL", 4, frame(0, 0, 320, 96))));
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(
        report.failed[0].1.contains("refused"),
        "{}",
        report.failed[0].1
    );
    assert!(ink_box(&p).is_none(), "nothing drawn");
}

#[test]
fn the_module_floor_holds_unless_overridden() {
    let f = frame(0, 0, 320, 96);
    let (_, report) = render(Element::Qr(qr("RELABEL", 1, f)));
    assert!(report.failed[0].1.contains("2-dot floor"), "{report:?}");
    let mut e = qr("RELABEL", 1, f);
    e.out_of_spec = true;
    assert!(render(Element::Qr(e)).1.is_clean());
    let mut e = qr("RELABEL", 0, f);
    e.out_of_spec = true;
    assert_eq!(
        render(Element::Qr(e)).1.failed.len(),
        1,
        "zero is never a size"
    );
}

/// The quiet zone is clear even over a filled panel.
#[test]
fn the_quiet_zone_is_knocked_out_of_anything_beneath() {
    let panel = Element::Rect(RectEl {
        common: Common::new(9, frame(0, 0, 320, 96)),
        stroke_dots: 0,
        radius_dots: 0,
    });
    let code = || Element::Qr(qr("RELABEL", 3, frame(100, 0, 100, 96)));
    let (alone, _) = render(code());
    let (p, report) = render_all(vec![panel, code()]);
    assert!(report.is_clean(), "{report:?}");
    // The whole footprint - quiet zone and light modules alike - is exactly as
    // it is on bare paper.
    for y in 0..87 {
        for x in 100..187 {
            assert_eq!(p.get(x, y), alone.get(x, y), "({x}, {y})");
        }
    }
    assert!(
        p.get(99, 40) && p.get(187, 40) && p.get(150, 87),
        "the panel around it is untouched"
    );
}

/// Hostile positions from a file are reported, never an overflow.
#[test]
fn a_symbol_at_the_edge_of_the_coordinate_range_does_not_overflow() {
    for (feed, head) in [(i32::MAX - 5, 0), (0, i32::MAX - 5), (i32::MIN, i32::MIN)] {
        for rot in [Rot::D0, Rot::D90] {
            let mut e = qr("RELABEL", 3, frame(feed, head, 96, 96));
            e.common.rot = rot;
            let (p, report) = render(Element::Qr(e));
            assert_eq!(report.off_canvas, vec![1], "({feed}, {head})");
            assert!(ink_box(&p).is_none(), "({feed}, {head})");
        }
    }
}

/// The module size `fit` offers is accepted, and one dot less room is refused.
#[test]
fn fit_matches_what_the_rasteriser_accepts() {
    let g = ModuleGrid::new(21, 21, 4, vec![false; 21 * 21]);
    let m = g.fit(87, 96).unwrap();
    assert_eq!(m, 3);
    let (_, ok) = render(Element::Qr(qr("RELABEL", m, frame(0, 0, 87, 96))));
    assert!(ok.is_clean(), "{ok:?}");
    let (p, short) = render(Element::Qr(qr("RELABEL", m, frame(0, 0, 86, 96))));
    assert!(short.failed[0].1.contains("refused"), "{short:?}");
    assert!(ink_box(&p).is_none(), "and nothing drawn");
}

/// A reversed symbol is the plain one inverted, exactly over its footprint.
#[test]
fn a_reversed_code_is_the_inverse_within_its_footprint() {
    let f = frame(10, 0, 100, 96);
    let (plain, _) = render(Element::Qr(qr("RELABEL", 3, f)));
    let mut e = qr("RELABEL", 3, f);
    e.common.ink = Ink::White;
    let (rev, report) = render(Element::Qr(e));
    assert!(report.is_clean(), "{report:?}");
    for y in 0..96u16 {
        for x in 0..320u16 {
            let inside = (10..97).contains(&x) && y < 87;
            let want = inside && !plain.get(x, y);
            assert_eq!(rev.get(x, y), want, "({x}, {y})");
        }
    }
}

/// On a `Top` target the document's axes are swapped onto the canvas, which
/// is a reflection; a 2D symbol must still come out unmirrored. Compared
/// module by module, because a decoder may read a mirror image without
/// complaint - rqrr does.
#[test]
fn a_2d_code_reads_the_same_on_a_top_target() {
    let m = 3u16;
    let side = (21 + 8) * m;
    let f = frame(0, 0, 96, 96);
    for rot in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
        let mut e = qr("RELABEL", m, f);
        e.common.rot = rot;
        let (left, a) = render_on(PrintDirection::Left, Element::Qr(e.clone()));
        let (top, b) = render_on(PrintDirection::Top, Element::Qr(e));
        assert!(a.is_clean() && b.is_clean(), "{rot:?}: {a:?} {b:?}");
        assert_eq!(
            crop(&top, 0, 0, side, side),
            crop(&left, 0, 0, side, side),
            "{rot:?}"
        );
    }
    // Upright, it is the encoder's own symbol, module for module.
    let (grid, _) = code2d::qr("RELABEL", QrEc::M, None).unwrap();
    let (top, _) = render_on(PrintDirection::Top, Element::Qr(qr("RELABEL", m, f)));
    for y in 0..21u16 {
        for x in 0..21u16 {
            let here = top.get((4 + x) * m + 1, (4 + y) * m + 1);
            assert_eq!(here, grid.is_dark(x.into(), y.into()), "module ({x}, {y})");
        }
    }
    let e = dm("RELABEL", 4, DmSize::Square, f);
    let (top, r) = render_on(PrintDirection::Top, Element::DataMatrix(e.clone()));
    let (left, _) = render_on(PrintDirection::Left, Element::DataMatrix(e));
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(top, left);
}

/// The canvas check follows the frame as drawn: a quarter turn swaps its
/// sides, and a turned symbol that runs off the canvas is reported rather than
/// silently clipped - while one that fits only once turned is not reported.
#[test]
fn the_canvas_check_follows_a_turned_frame() {
    let size = DmSize::Fixed { rows: 16, cols: 48 };
    let mut e = dm("RELABEL", 4, size, frame(0, 0, 200, 72));
    e.common.rot = Rot::D90;
    let (_, report) = render(Element::DataMatrix(e));
    assert_eq!(report.off_canvas, vec![1], "{report:?}");
    let mut e = dm("RELABEL", 4, DmSize::Square, frame(0, 0, 96, 300));
    e.common.rot = Rot::D90;
    let (_, report) = render(Element::DataMatrix(e));
    assert!(report.is_clean(), "{report:?}");
}

/// The frame steers the size: the smallest rectangle is too long for this
/// frame at four dots a module, so a shorter one is drawn.
#[test]
fn the_frame_steers_the_datamatrix_size() {
    let data = "RELABEL-0042";
    let (g, _) = code2d::data_matrix(data, &DmSize::Rectangle, |_| true).unwrap();
    assert_eq!(
        (g.w, g.h),
        (32, 8),
        "136 dots long at 4 dots, with its quiet zone"
    );
    let (p, report) = render(Element::DataMatrix(dm(
        data,
        4,
        DmSize::Rectangle,
        frame(0, 0, 120, 96),
    )));
    assert!(report.is_clean(), "{report:?}");
    let (x0, _, x1, _) = ink_box(&p).unwrap();
    assert!(x1 - x0 <= 120 - 8, "{x0}..{x1}");
    assert_eq!(read_dm(&p).as_deref(), Ok(data));
}

/// Every QR code on a plane, read back through `rqrr` - a port of quirc that
/// shares no code with the encoder - as (version, error correction, payload
/// bytes). The level is rqrr's raw format bits, which do not run in L, M, Q,
/// H order: see [`raw_ec`].
fn read_qr(p: &BitPlane) -> Vec<(usize, u16, Vec<u8>)> {
    let (w, h) = (usize::from(p.width()), usize::from(p.height()));
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        if p.get(x as u16, y as u16) { 0 } else { 255 }
    });
    img.detect_grids()
        .iter()
        .map(|g| {
            let mut buf = Vec::new();
            let meta = g
                .decode_to(&mut buf)
                .expect("a grid was found but did not decode");
            (meta.version.0, meta.ecc_level, buf)
        })
        .collect()
}

/// The two-bit error-correction field as ISO/IEC 18004 encodes it, which is
/// what rqrr 0.11 reports.
fn raw_ec(ec: QrEc) -> u16 {
    match ec {
        QrEc::M => 0,
        QrEc::L => 1,
        QrEc::H => 2,
        QrEc::Q => 3,
    }
}

/// A QR code read back through `rxing`, which, unlike rqrr, decodes a
/// Kanji-mode segment as the Shift JIS the standard says it is - so text sent
/// in the wrong mode comes back wrong instead of coincidentally right.
fn read_qr_rx(p: &BitPlane) -> Result<String, String> {
    let luma = (0..p.height())
        .flat_map(|y| (0..p.width()).map(move |x| if p.get(x, y) { 0 } else { 255 }))
        .collect();
    let mut hints = rxing::DecodeHints::default();
    rxing::helpers::detect_in_luma_with_hints(
        luma,
        u32::from(p.width()),
        u32::from(p.height()),
        Some(rxing::BarcodeFormat::QR_CODE),
        &mut hints,
    )
    .map(|r| r.getText().to_string())
    .map_err(|e| e.to_string())
}

/// A DataMatrix on a plane, read back through `rxing` - a port of ZXing that
/// shares no code with the encoder.
fn read_dm(p: &BitPlane) -> Result<String, String> {
    let luma = (0..p.height())
        .flat_map(|y| (0..p.width()).map(move |x| if p.get(x, y) { 0 } else { 255 }))
        .collect();
    let mut hints = rxing::DecodeHints::default();
    rxing::helpers::detect_in_luma_with_hints(
        luma,
        u32::from(p.width()),
        u32::from(p.height()),
        Some(rxing::BarcodeFormat::DATA_MATRIX),
        &mut hints,
    )
    .map(|r| r.getText().to_string())
    .map_err(|e| e.to_string())
}

#[test]
fn rendered_qr_codes_decode_back_to_their_data() {
    let mut decoded = 0;
    for data in [
        "1",
        "RELABEL",
        "https://example.invalid/r/42",
        "SKU-00412 lot 7",
        "Größe 12 mm",
        "a\tb",
    ] {
        for ec in [QrEc::L, QrEc::M, QrEc::Q, QrEc::H] {
            for m in [2u16, 3] {
                let mut e = qr(data, m, frame(0, 0, 320, 96));
                e.ec = ec;
                let (p, report) = render(Element::Qr(e));
                if !report.is_clean() {
                    // Too big for this module size on a 96-dot label, which
                    // is a refusal, never a clipped symbol.
                    assert!(
                        report.failed[0].1.contains("refused"),
                        "{data} {ec:?} {m}: {report:?}"
                    );
                    assert!(read_qr(&p).is_empty());
                    continue;
                }
                let got = read_qr(&p);
                assert_eq!(got.len(), 1, "{data} {ec:?} at {m} dots");
                assert_eq!(got[0].1, raw_ec(ec), "{data} {ec:?} at {m} dots");
                assert_eq!(got[0].2, data.as_bytes(), "{data} {ec:?} at {m} dots");
                decoded += 1;
            }
        }
    }
    assert!(
        decoded >= 35,
        "only {decoded} symbols were small enough to test"
    );
}

#[test]
fn a_pinned_qr_version_is_the_version_printed() {
    for v in 1..=5u8 {
        let mut e = qr("RELABEL", 2, frame(0, 0, 320, 96));
        e.version = Some(v);
        let (p, report) = render(Element::Qr(e));
        assert!(report.is_clean(), "{v}: {report:?}");
        assert_eq!(read_qr(&p), vec![(usize::from(v), 0, b"RELABEL".to_vec())]);
    }
    let mut e = qr(&"A".repeat(30), 2, frame(0, 0, 320, 96));
    e.version = Some(1);
    let (_, report) = render(Element::Qr(e));
    assert!(report.failed[0].1.contains("version 1"), "{report:?}");
}

/// UTF-8 the QR crate's own segmenter would send in Kanji mode, where a
/// decoder that follows the standard reads Shift JIS: "ああああ" came back as
/// "縺ゅ≠縺ゅ≠". rqrr cannot catch it - it hands a Kanji segment back as raw
/// bytes, which here are the UTF-8 that went in - so rxing reads these.
#[test]
fn text_that_looks_like_shift_jis_stays_utf8() {
    for data in [
        "ああああ",
        "ありがとう",
        "あいうえお",
        "品番: あ-12",
        "こんにちは",
        "Größe",
        "€100",
    ] {
        for ec in [QrEc::L, QrEc::M, QrEc::H] {
            let mut e = qr(data, 2, frame(0, 0, 320, 96));
            e.ec = ec;
            let (p, report) = render(Element::Qr(e));
            assert!(report.is_clean(), "{data} {ec:?}: {report:?}");
            assert_eq!(read_qr_rx(&p).as_deref(), Ok(data), "{data} {ec:?}");
        }
    }
}

#[test]
fn a_turned_qr_code_still_reads() {
    for rot in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
        let mut e = qr("RELABEL", 3, frame(40, 0, 96, 96));
        e.common.rot = rot;
        let (p, report) = render(Element::Qr(e));
        assert!(report.is_clean(), "{rot:?}: {report:?}");
        assert_eq!(read_qr(&p), vec![(1, 0, b"RELABEL".to_vec())], "{rot:?}");
    }
}

#[test]
fn rendered_datamatrix_symbols_decode_back_to_their_data() {
    let long_digits = "0123456789".repeat(9);
    for (data, size, m) in [
        ("RELABEL", DmSize::Fixed { rows: 16, cols: 48 }, 5u16),
        ("SKU-00412 lot 7", DmSize::Rectangle, 4),
        (long_digits.as_str(), DmSize::Rectangle, 3),
        ("RELABEL-DMRE-0042", DmSize::Fixed { rows: 12, cols: 64 }, 4),
        ("RELABEL-DMRE-0042", DmSize::Fixed { rows: 20, cols: 64 }, 4),
        ("Größe 12 mm", DmSize::Square, 4),
        // Control characters. That they go in with no ECI is checked module by
        // module in the unit tests; a decoder returns the same text either way.
        ("a\tb\nc", DmSize::Any, 4),
        // An envelope header, which the crate's default macro handling mangles.
        ("[)>\u{1e}05\u{1d}ABCDE", DmSize::Any, 3),
    ] {
        let (p, report) = render(Element::DataMatrix(dm(
            data,
            m,
            size.clone(),
            frame(0, 0, 320, 96),
        )));
        assert!(report.is_clean(), "{data:?} {size:?}: {report:?}");
        assert_eq!(
            read_dm(&p).as_deref(),
            Ok(data),
            "{data:?} {size:?} at {m} dots"
        );
    }
}

/// Beyond Latin-1 the symbol carries a UTF-8 ECI, and rxing cannot check this
/// case. Its Base256 decoding appends the bytes as UTF-8 text behind a second
/// ECI of its own (`ECIStringBuilder::append_string`), so it returns the text
/// mis-decoded and twice; upstream ZXing appends the raw bytes instead.
///
/// So the ECI is checked against the standard's own arithmetic, and the
/// rendered symbol is read back through the encoding crate's decoder - which
/// proves the drawing, not the encoder. Whether a given handheld scanner
/// honours the ECI is not known.
#[test]
fn text_beyond_latin1_carries_a_utf8_eci() {
    let data = "€5 日本";
    // ISO/IEC 16022 5.4.1: codeword 241 announces an ECI, and ECI 000026,
    // UTF-8, is written as the single codeword 26 + 1.
    let sym = datamatrix::DataMatrixBuilder::new()
        .with_macros(false)
        .with_symbol_list(datamatrix::SymbolSize::Square18)
        .encode_str(data)
        .unwrap();
    assert_eq!(&sym.data_codewords()[..2], &[241, 27]);

    let m = 4u16;
    let size = DmSize::Fixed { rows: 18, cols: 18 };
    let (p, report) = render(Element::DataMatrix(dm(data, m, size, frame(0, 0, 320, 96))));
    assert!(report.is_clean(), "{report:?}");
    // The centre of every module, inside the one-module quiet zone.
    let bits: Vec<bool> = (0..18u16)
        .flat_map(|y| (0..18u16).map(move |x| (x, y)))
        .map(|(x, y)| p.get((1 + x) * m + m / 2, (1 + y) * m + m / 2))
        .collect();
    let (map, size) = datamatrix::placement::MatrixMap::try_from_bits(&bits, 18).unwrap();
    let mut codewords = map.codewords();
    datamatrix::errorcode::decode_error(&mut codewords, size).unwrap();
    let n = sym.data_codewords().len();
    assert_eq!(datamatrix::data::decode_str(&codewords[..n]).unwrap(), data);

    // rxing's present answer, pinned so that a fixed rxing is noticed and
    // made the check here instead.
    assert_ne!(
        read_dm(&p).as_deref(),
        Ok(data),
        "rxing now reads UTF-8 ECI data correctly; use it for this test"
    );
}

#[test]
fn a_turned_datamatrix_still_reads() {
    for rot in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
        let mut e = dm("RELABEL", 4, DmSize::Square, frame(40, 0, 96, 96));
        e.common.rot = rot;
        let (p, report) = render(Element::DataMatrix(e));
        assert!(report.is_clean(), "{rot:?}: {report:?}");
        assert_eq!(read_dm(&p).as_deref(), Ok("RELABEL"), "{rot:?}");
        // And a rectangle, whose quarter turn trades its long side for its
        // short one - in a frame it fits exactly, either way round.
        let size = DmSize::Fixed { rows: 8, cols: 18 };
        let mut e = dm("RL-42", 4, size, frame(0, 0, 80, 40));
        e.common.rot = rot;
        let (p, report) = render(Element::DataMatrix(e));
        assert!(report.is_clean(), "{rot:?} rectangle: {report:?}");
        assert_eq!(read_dm(&p).as_deref(), Ok("RL-42"), "{rot:?} rectangle");
    }
}

#[test]
fn datamatrix_size_problems_are_reported() {
    let f = frame(0, 0, 320, 96);
    let (_, r) = render(Element::DataMatrix(dm(
        "x",
        4,
        DmSize::Unknown("hexagon".into()),
        f,
    )));
    assert!(r.failed[0].1.contains("hexagon"), "{r:?}");
    let (_, r) = render(Element::DataMatrix(dm(
        &"9".repeat(99),
        4,
        DmSize::Fixed { rows: 16, cols: 48 },
        f,
    )));
    assert!(r.failed[0].1.contains("16x48"), "{r:?}");
    let (_, r) = render(Element::DataMatrix(dm("x", 4, DmSize::Square, f)));
    assert!(r.is_clean());
    let (_, r) = render(Element::DataMatrix(dm("", 4, DmSize::Square, f)));
    assert!(r.failed[0].1.contains("needs some data"), "{r:?}");
    // When nothing fits the frame, the smallest symbol that holds the data is
    // the one reported: 14x14 and its quiet zone is 64 dots at 4 a module.
    let (p, r) = render(Element::DataMatrix(dm(
        "RELABEL",
        4,
        DmSize::Square,
        frame(0, 0, 40, 40),
    )));
    assert!(
        r.failed[0].1.contains("64x64") && r.failed[0].1.contains("40x40"),
        "{r:?}"
    );
    assert!(ink_box(&p).is_none());
}

#[test]
fn every_code_matches_its_golden() {
    let f = frame(0, 0, 320, 96);
    let mut pinned = qr("https://example.invalid/r/42", 2, f);
    pinned.version = Some(3);
    for (name, el) in [
        ("code2d-qr-v1-3dots", Element::Qr(qr("RELABEL", 3, f))),
        ("code2d-qr-v3-2dots", Element::Qr(pinned)),
        (
            "code2d-dm-16x48-5dots",
            Element::DataMatrix(dm(
                "RELABEL-0042",
                5,
                DmSize::Fixed { rows: 16, cols: 48 },
                f,
            )),
        ),
        (
            "code2d-dm-20x64-4dots",
            Element::DataMatrix(dm(
                "RELABEL-DMRE-0042",
                4,
                DmSize::Fixed { rows: 20, cols: 64 },
                f,
            )),
        ),
        (
            "code2d-dm-square-4dots",
            Element::DataMatrix(dm("RELABEL", 4, DmSize::Square, f)),
        ),
    ] {
        let (p, report) = render(el);
        assert!(report.is_clean(), "{name}: {report:?}");
        common::check(name, &p);
    }
}

/// The DataMatrix size choice makes the same orientation test the drawing
/// does. "RELABEL-0042" is smallest as an 8x32 strip; in a frame whose long
/// side runs the other way, the strip fits only once it is oriented for the
/// target - by the transpose on a `Top` target, by the turn on a turned
/// element - and a choice that forgot either would settle on a square.
#[test]
fn the_datamatrix_size_choice_follows_the_orientation() {
    let data = "RELABEL-0042";
    let e = dm(data, 2, DmSize::Any, frame(0, 0, 40, 96));
    let (p, r) = render_on(PrintDirection::Top, Element::DataMatrix(e));
    assert!(r.is_clean(), "{r:?}");
    let (x0, y0, x1, y1) = ink_box(&p).unwrap();
    assert_eq!((x1 - x0, y1 - y0), (64, 16), "an upright 8x32 strip");
    assert_eq!(read_dm(&p).as_deref(), Ok(data));

    let mut e = dm(data, 2, DmSize::Any, frame(0, 0, 96, 40));
    e.common.rot = Rot::D90;
    let (p, r) = render_on(PrintDirection::Left, Element::DataMatrix(e));
    assert!(r.is_clean(), "{r:?}");
    let (x0, y0, x1, y1) = ink_box(&p).unwrap();
    assert_eq!((x1 - x0, y1 - y0), (16, 64), "the strip on its side");
    assert_eq!(read_dm(&p).as_deref(), Ok(data));
}

/// A turned rectangle knocks out its own footprint - the turned one - and
/// nothing else: the panel where the unturned footprint would have reached
/// keeps its ink.
#[test]
fn a_turned_rectangle_knocks_out_its_turned_footprint() {
    let panel = || {
        Element::Rect(RectEl {
            common: Common::new(9, frame(0, 0, 320, 96)),
            stroke_dots: 0,
            radius_dots: 0,
        })
    };
    let code = || {
        let size = DmSize::Fixed { rows: 8, cols: 18 };
        let mut e = dm("RL-42", 4, size, frame(0, 0, 80, 40));
        e.common.rot = Rot::D90;
        Element::DataMatrix(e)
    };
    let (alone, _) = render(code());
    let (p, report) = render_all(vec![panel(), code()]);
    assert!(report.is_clean(), "{report:?}");
    // Turned, the footprint is 40 dots along the feed and 80 across the head.
    for y in 0..80 {
        for x in 0..40 {
            assert_eq!(p.get(x, y), alone.get(x, y), "({x}, {y})");
        }
    }
    assert!(
        p.get(60, 10) && p.get(40, 10) && p.get(10, 85),
        "the panel outside the turned footprint keeps its ink"
    );
}

/// The same at a pinned version, which is the path a batch takes: at the
/// version the automatic path picks, and one above it, which it never picks
/// and so can only be checked by reading it back.
#[test]
fn a_pinned_version_keeps_text_out_of_kanji_mode() {
    let mut checked = 0;
    for data in ["ああああ", "ありがとう", "あいうえお", "品番: あ-12"] {
        for ec in [QrEc::L, QrEc::M, QrEc::H] {
            let (_, auto) = code2d::qr(data, ec, None).unwrap();
            for v in [auto, auto + 1] {
                // At two dots a module, a 96-dot label holds version 5.
                if (17 + 4 * u16::from(v) + 8) * 2 > 96 {
                    continue;
                }
                let mut e = qr(data, 2, frame(0, 0, 320, 96));
                e.ec = ec;
                e.version = Some(v);
                let (p, report) = render(Element::Qr(e));
                assert!(report.is_clean(), "{data} {ec:?} v{v}: {report:?}");
                assert_eq!(read_qr_rx(&p).as_deref(), Ok(data), "{data} {ec:?} v{v}");
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 20,
        "only {checked} pinned symbols were small enough"
    );
}
