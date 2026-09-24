//! The `label` command, run the way a person runs it. Nothing here reaches a
//! printer: every case stops at `--check` or `--dry-run`, or before either.

use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-09-11T16:05:09+02:00";

fn corpus(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../relabel-render/tests/corpus/v1")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

/// A directory of the test's own under the build's scratch space.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = std::process::Command::new(env!("CARGO_BIN_EXE_relabel-cli"))
        .args(args)
        .output()
        .unwrap();
    (
        status.code(),
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

/// The frozen fixture: its third row's SKU is too long for the barcode, and
/// the run is refused before any connection, with the row, its line in the
/// CSV, and the element named.
#[test]
fn the_over_long_row_stops_the_run_before_it_connects() {
    let (code, out, err) = run(&[
        "label",
        &corpus("variables.relabel"),
        "--check",
        "--now",
        NOW,
    ]);
    assert_eq!(code, Some(1), "{out}{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].starts_with("row 3 (line 4), element 2 \"SKU barcode\": ")
            && lines[0].contains("528")
            && lines[0].ends_with("[refused]"),
        "{out}"
    );
    assert!(
        lines[1].starts_with("row 5 (line 6): $times is 0") && lines[1].ends_with("[note]"),
        "{out}"
    );
    assert!(err.contains("nothing was printed"), "{err}");
}

/// The label prints the date, and a time zone is not something this command
/// can find out, so it asks rather than guess UTC.
#[test]
fn the_time_is_asked_for_when_the_label_prints_it() {
    let (code, out, err) = run(&["label", &corpus("variables.relabel"), "--check"]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("--now \"$(date -Iseconds)\""), "{err}");
    assert!(out.is_empty(), "{out}");

    let (code, _, err) = run(&[
        "label",
        &corpus("variables.relabel"),
        "--check",
        "--now",
        "2026-09-11T16:05:09",
    ]);
    assert_eq!(code, Some(1));
    assert!(err.contains("RFC 3339"), "{err}");
}

/// `--csv` replaces the label's rows, and a dry run draws and encodes every
/// label that would print: `$times` for each row, times `--copies`.
#[test]
fn a_dry_run_counts_every_label() {
    let csv = scratch("a_dry_run_counts_every_label").join("rows.csv");
    std::fs::write(
        &csv,
        "sku,name,$times\nA1,Widget,2\nB2,Gadget,\nC3,Gizmo,0\n",
    )
    .unwrap();
    let (code, out, err) = run(&[
        "label",
        &corpus("variables.relabel"),
        "--csv",
        csv.to_str().unwrap(),
        "--copies",
        "2",
        "--dry-run",
        "--now",
        NOW,
    ]);
    assert_eq!(code, Some(0), "{out}{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("row 3 (line 4): $times is 0"), "{out}");
    assert_eq!(lines[1], "6 labels from 3 rows; nothing refused.");
    assert!(lines[2].ends_with("bytes on the wire in all."), "{out}");
}

/// A label with no data and no clock prints once, and needs no `--now`.
#[test]
fn a_label_with_no_data_prints_once() {
    let file = scratch("a_label_with_no_data_prints_once").join("plain.relabel");
    std::fs::write(
        &file,
        r#"{ "format": "relabel-label", "version": 1,
            "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
            "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
            "elements": [ { "type": "text", "id": 1,
                "frame": { "feed": 8, "head": 8, "feed_len": 200, "head_len": 32 },
                "text": "Hello", "family": "Noto Sans", "size": 20.0 } ] }"#,
    )
    .unwrap();
    let (code, out, err) = run(&["label", file.to_str().unwrap(), "--check"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(out, "1 label; nothing refused.\n");
    let (code, out, err) = run(&["label", file.to_str().unwrap(), "--check", "--head", "384"]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("the printer takes 320 by 384"), "{out}");
}

/// A CSV the reader refuses names its file and the line to look at.
#[test]
fn a_broken_csv_names_its_line() {
    let csv = scratch("a_broken_csv_names_its_line").join("rows.csv");
    std::fs::write(&csv, "sku,name,$times\nA1,Widget,2\n\"B2,Gadget,1\n").unwrap();
    let (code, out, err) = run(&[
        "label",
        &corpus("variables.relabel"),
        "--csv",
        csv.to_str().unwrap(),
        "--check",
        "--now",
        NOW,
    ]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("rows.csv"), "{err}");
    assert!(
        err.contains("line 3: a quote is opened and never closed"),
        "{err}"
    );
}

/// A run of no labels must not reach the printer: nothing would come out of
/// it, and connecting alone interrupts whatever the printer is doing.
#[test]
fn a_run_of_no_labels_does_not_connect() {
    let csv = scratch("a_run_of_no_labels_does_not_connect").join("rows.csv");
    std::fs::write(&csv, "sku,name,$times\nA1,Widget,0\nB2,Gadget,0\n").unwrap();
    let label = corpus("variables.relabel");
    let csv = csv.to_str().unwrap().to_string();
    let (code, out, err) = run(&["label", &label, "--csv", &csv, "--now", NOW]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("nothing would print"), "{err}");
    assert!(!err.to_lowercase().contains("bluetooth"), "{err}");

    let (code, out, err) = run(&[
        "label", &label, "--csv", &csv, "--copies", "0", "--now", NOW,
    ]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("--copies 0"), "{err}");
}

/// A label's data file travels with the label, so one naming a file outside
/// the label's own directory is refused: opening a label someone sent would
/// otherwise read a file of their choosing.
#[test]
fn a_data_file_outside_the_labels_directory_is_refused() {
    let dir = scratch("a_data_file_outside_the_labels_directory_is_refused");
    std::fs::write(dir.join("rows.csv"), "sku,name\nA1,Widget\n").unwrap();
    for path in ["../rows.csv", "/etc/passwd", "sub/../rows.csv"] {
        let file = dir.join("escape.relabel");
        std::fs::write(
            &file,
            format!(
                r#"{{ "format": "relabel-label", "version": 1,
                  "media": {{ "feed_tenths_mm": 400, "head_tenths_mm": 120 }},
                  "canvas": {{ "feed": 320, "head": 96, "dpmm": {{ "num": 8, "den": 1 }} }},
                  "data": {{ "kind": "csv_file", "path": "{path}" }},
                  "elements": [ {{ "type": "text", "id": 1,
                    "frame": {{ "feed": 8, "head": 8, "feed_len": 200, "head_len": 32 }},
                    "text": "{{name}}", "family": "Noto Sans", "size": 20.0 }} ] }}"#
            ),
        )
        .unwrap();
        let (code, out, err) = run(&["label", file.to_str().unwrap(), "--check"]);
        assert_eq!(code, Some(1), "{path}: {out}{err}");
        assert!(err.contains("not beside the label"), "{path}: {err}");
    }
}

/// `--csv` names the rows' file anywhere, as the refusal above says, so a label
/// whose own data file is not beside it still runs with it.
#[test]
fn csv_gives_rows_to_a_label_whose_data_file_is_not_beside_it() {
    let dir = scratch("csv_gives_rows_to_a_label_whose_data_file_is_not_beside_it");
    let rows = dir.join("rows.csv");
    std::fs::write(&rows, "sku,name\nA1,Widget\n").unwrap();
    let file = dir.join("up.relabel");
    std::fs::write(
        &file,
        r#"{ "format": "relabel-label", "version": 1,
          "media": { "feed_tenths_mm": 400, "head_tenths_mm": 120 },
          "canvas": { "feed": 320, "head": 96, "dpmm": { "num": 8, "den": 1 } },
          "data": { "kind": "csv_file", "path": "../rows.csv" },
          "elements": [ { "type": "text", "id": 1,
            "frame": { "feed": 8, "head": 8, "feed_len": 200, "head_len": 32 },
            "text": "{name}", "family": "Noto Sans", "size": 20.0 } ] }"#,
    )
    .unwrap();
    let (code, out, err) = run(&[
        "label",
        file.to_str().unwrap(),
        "--check",
        "--csv",
        rows.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("nothing refused"), "{out}");
}

/// The size cap counts the bytes as they arrive: a character device reports no
/// length to ask for.
#[cfg(unix)]
#[test]
fn a_file_without_end_is_refused() {
    let (code, out, err) = run(&[
        "label",
        &corpus("variables.relabel"),
        "--csv",
        "/dev/zero",
        "--check",
        "--now",
        NOW,
    ]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("longer than"), "{err}");
}
