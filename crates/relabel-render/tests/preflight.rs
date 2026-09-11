//! Preflight: every row, every element, checked before any stock is used -
//! and each problem reported once, at the row and element it belongs to.

mod common;

use relabel_render::bind::{BindError, Table, TimesError, bind};
use relabel_render::code2d::CodeError;
use relabel_render::datetime::CivilTime;
use relabel_render::document::*;
use relabel_render::preflight::{Issue, IssueKind, Preflight, Severity, preflight};
use relabel_render::{BarcodeError, Dpmm, Failure, Target, rasterise_plane};

fn n1() -> CivilTime {
    CivilTime::new(2026, 9, 11, 16, 5, 9, 42, 120).unwrap()
}

fn target() -> Target {
    Target::new(320, 96, 203).unwrap()
}

fn corpus(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/v1")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn variables() -> Label {
    Label::from_json(&corpus("variables.relabel")).unwrap()
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

/// `tests/corpus/v1/variables.csv`, as the editor or command line hands it
/// over.
fn variables_csv() -> Table {
    table(
        &["sku", "name", "$times"],
        &[
            &["SKU-0412", "Widget", "2"],
            &["SKU-0413", "Gadget", ""],
            &["SKU-0414-EXTRA-LONG", "Gizmo", "1"],
            &["SKU-0415", "Doohickey", "3"],
            &["SKU-0416", "Spare", "0"],
        ],
    )
}

fn kinds(p: &Preflight, element: Option<u32>) -> Vec<&IssueKind> {
    p.issues
        .iter()
        .filter(|i| i.element == element)
        .map(|i| &i.kind)
        .collect()
}

/// The literal is the frozen CSV. The reader that produces a `Table` from it
/// lives in relabel-core and has its own test against the same file.
#[test]
fn the_table_is_the_frozen_csv() {
    let csv = corpus("variables.csv");
    let mut lines = csv
        .lines()
        .map(|l| l.split(',').map(String::from).collect::<Vec<_>>());
    let t = variables_csv();
    assert_eq!(lines.next().unwrap(), t.columns);
    assert_eq!(lines.collect::<Vec<_>>(), t.rows);
}

/// What preflight exists for: of five rows, the one whose SKU is too long for
/// the barcode's frame is named - its row, its element, and by how much - and
/// the row that asks for no copies is noted. Nothing else is said.
#[test]
fn the_over_long_row_is_named() {
    let p = preflight(&variables(), Some(&variables_csv()), &target(), n1()).unwrap();
    assert_eq!(p.times, vec![2, 1, 1, 3, 0]);
    assert_eq!(p.pages(), 7);
    assert_eq!(
        p.issues,
        vec![
            Issue {
                row: Some(2),
                element: Some(2),
                name: "SKU barcode".into(),
                kind: IssueKind::Failed(Failure::Barcode(BarcodeError::TooWide {
                    needed: 528,
                    available: 304
                })),
                severity: Severity::Refuse,
            },
            Issue {
                row: Some(4),
                element: None,
                name: String::new(),
                kind: IssueKind::Skipped,
                severity: Severity::Note,
            },
        ]
    );
    assert!(p.refused());
}

/// Eight letters of Code 128 fit the 304-dot frame at two dots a module; nine
/// do not, by four dots.
#[test]
fn the_code128_boundary() {
    let t = table(
        &["sku", "name", "$times"],
        &[&["ABCDEFGH", "Eight", "1"], &["ABCDEFGHI", "Nine", "1"]],
    );
    let p = preflight(&variables(), Some(&t), &target(), n1()).unwrap();
    assert_eq!(
        p.issues
            .iter()
            .map(|i| (i.row, i.element, i.kind.clone()))
            .collect::<Vec<_>>(),
        vec![(
            Some(1),
            Some(2),
            IssueKind::Failed(Failure::Barcode(BarcodeError::TooWide {
                needed: 308,
                available: 304
            }))
        )]
    );
}

/// A frame hanging off the canvas is the same on every row, so it is reported
/// once - even for an element whose data changes by row - and for a barcode
/// it refuses, since the canvas would clip its bars.
#[test]
fn a_frame_problem_is_reported_once_for_every_row() {
    let mut l = variables();
    for el in &mut l.elements {
        if let ElementOrRaw::Known(Element::Barcode(b)) = el {
            b.common.frame.feed_len = 322;
        }
        if let ElementOrRaw::Known(Element::Text(t)) = el
            && t.common.id == 1
        {
            t.common.rot = Rot::Other(10);
        }
    }
    let rows = table(
        &["sku", "name", "$times"],
        &[&["A1", "x", "1"], &["B2", "y", "1"], &["C3", "z", "1"]],
    );
    let p = preflight(&l, Some(&rows), &target(), n1()).unwrap();
    assert_eq!(
        p.issues
            .iter()
            .map(|i| (i.row, i.element, i.kind.clone(), i.severity))
            .collect::<Vec<_>>(),
        vec![
            (
                None,
                Some(1),
                IssueKind::RotationApproximated,
                Severity::Warn
            ),
            (None, Some(2), IssueKind::OffCanvas, Severity::Refuse),
        ]
    );
}

/// An element with no data in it fails the same way on every row, so it is
/// reported once, not fifty times.
#[test]
fn a_problem_no_row_changes_is_reported_once() {
    let l = Label::from_json(&corpus("kitchen-sink.relabel")).unwrap();
    let rows: Vec<Vec<String>> = (0..50).map(|i| vec![format!("A{i}")]).collect();
    let data = Table {
        columns: vec!["sku".into()],
        rows,
    };
    let p = preflight(&l, Some(&data), &target(), n1()).unwrap();
    let qr = kinds(&p, Some(5));
    assert_eq!(
        qr,
        vec![&IssueKind::Failed(Failure::Code(CodeError::TooLarge {
            needed: (99, 99),
            available: (80, 80)
        }))]
    );
    let image = kinds(&p, Some(7));
    assert_eq!(image.len(), 1);
    assert!(matches!(image[0], IssueKind::Failed(Failure::Artwork(_))));
    assert!(
        p.issues
            .iter()
            .filter(|i| i.element == Some(5))
            .all(|i| i.row.is_none())
    );
}

/// A placeholder no column answers prints as written: in text that is a
/// visible mistake, in a barcode it is a scannable wrong value.
#[test]
fn an_unresolved_key_refuses_in_a_symbol_and_warns_in_text() {
    let l = Label::from_json(&corpus("kitchen-sink.relabel")).unwrap();
    let p = preflight(&l, None, &target(), n1()).unwrap();
    let unresolved = |id| {
        p.issues
            .iter()
            .find(|i| i.element == Some(id) && matches!(i.kind, IssueKind::UnresolvedKey { .. }))
            .map(|i| (i.kind.clone(), i.severity))
    };
    let key = IssueKind::UnresolvedKey {
        key: "sku".into(),
        near: None,
    };
    assert_eq!(unresolved(4), Some((key.clone(), Severity::Refuse)));
    assert_eq!(unresolved(3), Some((key, Severity::Warn)));
    assert_eq!(p.times, vec![1]);
}

/// A column that differs only in case, spacing or a byte-order mark is
/// suggested; one no placeholder could ever name is noted.
#[test]
fn a_near_column_is_suggested_and_an_unreachable_one_noted() {
    let l = variables();
    let data = table(
        &["SKU", "\u{feff}sku", "name", "$times"],
        &[&["A1", "A1", "x", "1"]],
    );
    let p = preflight(&l, Some(&data), &target(), n1()).unwrap();
    assert!(p.issues.contains(&Issue {
        row: None,
        element: Some(2),
        name: "SKU barcode".into(),
        kind: IssueKind::UnresolvedKey {
            key: "sku".into(),
            near: Some("SKU".into())
        },
        severity: Severity::Refuse,
    }));
    let unreachable: Vec<&IssueKind> = p
        .issues
        .iter()
        .filter(|i| matches!(i.kind, IssueKind::UnreachableColumn { .. }))
        .map(|i| &i.kind)
        .collect();
    assert_eq!(
        unreachable,
        vec![&IssueKind::UnreachableColumn {
            column: "\u{feff}sku".into()
        }]
    );
}

#[test]
fn a_label_for_another_head_is_refused() {
    let l = Label::new(320, 92, Dpmm::DPI_203);
    let p = preflight(&l, None, &target(), n1()).unwrap();
    assert_eq!(
        p.issues[0].kind,
        IssueKind::TargetMismatch {
            canvas: (320, 92),
            target: (320, 96)
        }
    );
    assert!(p.refused());
}

#[test]
fn data_with_no_rows_is_refused() {
    let data = table(&["sku", "name", "$times"], &[]);
    let p = preflight(&variables(), Some(&data), &target(), n1()).unwrap();
    assert_eq!(kinds(&p, None), vec![&IssueKind::NoRows]);
    assert_eq!(p.pages(), 0);
    assert!(p.refused());
}

#[test]
fn duplicate_ids_are_refused() {
    let mut l = variables();
    let copy = l.elements[0].clone();
    l.elements.push(copy);
    let p = preflight(&l, None, &target(), n1()).unwrap();
    assert!(
        p.issues
            .iter()
            .any(|i| i.kind == IssueKind::DuplicateId && i.element == Some(1))
    );
}

/// A QR code whose data changes by row and whose version is not pinned can
/// change size from one label to the next.
#[test]
fn an_unpinned_symbol_that_varies_warns() {
    let qr = |version| {
        let mut l = Label::new(320, 96, Dpmm::DPI_203);
        l.elements.push(ElementOrRaw::Known(Element::Qr(QrEl {
            common: Common::new(
                1,
                RectDots {
                    feed: 0,
                    head: 0,
                    feed_len: 96,
                    head_len: 96,
                },
            ),
            data: "{sku}".into(),
            module_dots: 2,
            ec: QrEc::M,
            version,
            out_of_spec: false,
        })));
        l
    };
    let data = table(&["sku"], &[&["A1"], &["B2"]]);
    let p = preflight(&qr(None), Some(&data), &target(), n1()).unwrap();
    assert_eq!(kinds(&p, Some(1)), vec![&IssueKind::SizeUnpinned]);
    let p = preflight(&qr(Some(3)), Some(&data), &target(), n1()).unwrap();
    assert!(kinds(&p, Some(1)).is_empty(), "{:?}", p.issues);
}

/// A filter after a column does nothing, and a `dt` column replaces the
/// clock: both said, neither refused.
#[test]
fn an_ignored_filter_and_a_shadowed_clock_are_noted() {
    let mut l = variables();
    if let ElementOrRaw::Known(Element::Text(t)) = &mut l.elements[0] {
        t.text = "{name|upper} {name|upper} {dt|YYYY}".into();
    }
    let data = table(&["sku", "name", "dt"], &[&["A1", "x", "today"]]);
    let p = preflight(&l, Some(&data), &target(), n1()).unwrap();
    assert_eq!(
        kinds(&p, Some(1)),
        vec![
            &IssueKind::FilterIgnored {
                key: "name".into(),
                filter: "upper".into()
            },
            &IssueKind::ClockShadowed,
        ]
    );
    assert!(!p.refused(), "{:?}", p.issues);
}

/// A bad `$times`, or a row of the wrong length, refuses that row and no
/// other; the rows around it are still checked and counted.
#[test]
fn a_bad_row_refuses_itself_only() {
    let data = table(
        &["sku", "name", "$times"],
        &[
            &["A1", "x", "abc"],
            &["B2", "y", "2"],
            &["C3", "z"],
            &["D4", "w", ""],
        ],
    );
    let p = preflight(&variables(), Some(&data), &target(), n1()).unwrap();
    assert_eq!(p.times, vec![0, 2, 0, 1]);
    assert_eq!(
        p.issues
            .iter()
            .map(|i| (i.row, i.kind.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                Some(0),
                IssueKind::BadTimes(TimesError {
                    value: "abc".into()
                })
            ),
            (
                Some(2),
                IssueKind::Bind(BindError::RowLength {
                    cells: 2,
                    columns: 3
                })
            ),
        ]
    );
}

#[test]
fn preflight_is_deterministic() {
    let a = preflight(&variables(), Some(&variables_csv()), &target(), n1()).unwrap();
    let b = preflight(&variables(), Some(&variables_csv()), &target(), n1()).unwrap();
    assert_eq!(a, b);
}

/// The first row of the frozen data, bound and drawn: the label a person
/// would see on the first sheet of the run.
#[test]
fn the_first_row_matches_its_golden() {
    let bound = bind(&variables(), &variables_csv().bindings(0, n1()).unwrap()).unwrap();
    common::check(
        "variables-row0",
        &rasterise_plane(&bound, &target()).unwrap(),
    );
}

/// Every issue has a sentence a person can act on.
#[test]
fn every_issue_reads_as_a_sentence() {
    let p = preflight(&variables(), Some(&variables_csv()), &target(), n1()).unwrap();
    let text: Vec<String> = p.issues.iter().map(|i| i.kind.to_string()).collect();
    assert!(text[0].contains("528 dots long"), "{text:?}");
    assert!(text[1].contains("prints nothing"), "{text:?}");
}

/// A row that prints nothing is not drawn, so what would be wrong with it is
/// no reason to stop the run.
#[test]
fn a_row_that_prints_nothing_is_not_checked() {
    let t = table(
        &["sku", "name", "$times"],
        &[&["A1", "x", "1"], &["SKU-0414-EXTRA-LONG", "y", "0"]],
    );
    let p = preflight(&variables(), Some(&t), &target(), n1()).unwrap();
    assert_eq!(
        p.issues
            .iter()
            .map(|i| (i.row, &i.kind))
            .collect::<Vec<_>>(),
        vec![(Some(1), &IssueKind::Skipped)]
    );
    assert!(!p.refused());
}

/// A hidden element prints nothing, so a placeholder in it that no column
/// answers is not reported.
#[test]
fn a_hidden_element_is_not_checked_for_keys() {
    let mut l = Label::from_json(&corpus("kitchen-sink.relabel")).unwrap();
    for el in &mut l.elements {
        if let ElementOrRaw::Known(Element::Barcode(b)) = el
            && b.common.id == 4
        {
            b.common.hidden = true;
        }
    }
    let p = preflight(&l, None, &target(), n1()).unwrap();
    assert!(kinds(&p, Some(4)).is_empty(), "{:?}", p.issues);
    assert!(
        kinds(&p, Some(3)).contains(&&IssueKind::UnresolvedKey {
            key: "sku".into(),
            near: None
        }),
        "{:?}",
        p.issues
    );
}

/// With one row printing, nothing changes from label to label, so an
/// unpinned size is not worth a warning.
#[test]
fn an_unpinned_symbol_on_one_row_is_quiet() {
    let mut l = Label::new(320, 96, Dpmm::DPI_203);
    l.elements.push(ElementOrRaw::Known(Element::Qr(QrEl {
        common: Common::new(
            1,
            RectDots {
                feed: 0,
                head: 0,
                feed_len: 96,
                head_len: 96,
            },
        ),
        data: "{sku}".into(),
        module_dots: 2,
        ec: QrEc::M,
        version: None,
        out_of_spec: false,
    })));
    let one = table(&["sku", "$times"], &[&["A1", "3"], &["B2", "0"]]);
    let p = preflight(&l, Some(&one), &target(), n1()).unwrap();
    assert_eq!(
        p.issues.iter().map(|i| &i.kind).collect::<Vec<_>>(),
        vec![&IssueKind::Skipped]
    );
}

/// A placeholder is usually longer than the value that replaces it, so a
/// template that would not fit its frame is not a problem with the run. Only
/// the bound rows are judged.
#[test]
fn what_does_not_fit_unbound_is_judged_bound() {
    let mut l = variables();
    for el in &mut l.elements {
        match el {
            ElementOrRaw::Known(Element::Text(t)) if t.common.id == 1 => {
                t.text = "{a_column_name_so_long_that_no_frame_on_this_label_could_hold_it}".into();
            }
            ElementOrRaw::Known(Element::Barcode(b)) => {
                b.data = "{a_column_name_so_long_that_no_frame_on_this_label_could_hold_it}".into();
            }
            _ => {}
        }
    }
    let t = table(
        &["a_column_name_so_long_that_no_frame_on_this_label_could_hold_it"],
        &[&["A1"], &["B2"]],
    );
    let p = preflight(&l, Some(&t), &target(), n1()).unwrap();
    assert!(p.issues.is_empty(), "{:?}", p.issues);
    assert_eq!(p.times, vec![1, 1]);
}

/// A hidden element is bound like any other, so a row it cannot bind stops
/// the run - even though nothing of it would print. Without this the run dies
/// part way through, which is the stock preflight exists to save.
#[test]
fn a_hidden_element_that_cannot_bind_refuses_its_row() {
    let mut l = variables();
    for el in &mut l.elements {
        match el {
            ElementOrRaw::Known(Element::Text(t)) if t.common.id == 1 => {
                t.common.hidden = true;
                t.text = "{name}{name}".into();
            }
            ElementOrRaw::Known(Element::Text(t)) => t.text = "Packed".into(),
            ElementOrRaw::Known(Element::Barcode(b)) => b.data = "SKU-0001".into(),
            _ => {}
        }
    }
    let big = "x".repeat(40_000);
    let rows = table(&["sku", "name"], &[&["A1", "small"], &["A2", &big]]);
    let p = preflight(&l, Some(&rows), &target(), n1()).unwrap();
    assert_eq!(
        p.issues
            .iter()
            .map(|i| (i.row, i.kind.clone()))
            .collect::<Vec<_>>(),
        vec![(
            Some(1),
            IssueKind::Bind(BindError::FieldTooLong {
                element: 1,
                limit: 65_536
            })
        )]
    );
    assert!(p.refused());
}
