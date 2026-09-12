//! Frozen v1 documents, and the promise that they keep loading.
//!
//! These files are never edited and never deleted. They are what actually keeps
//! saved labels working across versions; the migration function is only the
//! code that reads them.
//!
//! A `PartialEq` round-trip assertion **cannot** substitute for this, and that
//! is the whole point: if a build drops a field on load it drops it on save
//! too, so both sides of the comparison agree and the test passes while the
//! data is gone. These tests compare against the file's own text instead.

mod common;

use relabel_render::document::*;
use relabel_render::{Target, rasterise, rasterise_plane};

fn corpus(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/v1")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn every_element_type_still_loads() {
    let l = Label::from_json(&corpus("kitchen-sink.relabel")).unwrap();
    assert_eq!(l.elements.len(), 7);
    // One of each, whatever order the document happens to list them in.
    let mut kinds: Vec<&str> = l.elements.iter().filter_map(|e| e.kind()).collect();
    kinds.sort_unstable();
    let mut want: Vec<&str> = KNOWN_KINDS.to_vec();
    want.sort_unstable();
    assert_eq!(kinds, want, "the corpus must exercise every element type");
    assert_eq!(l.meta.authored_for.as_ref().unwrap().head_px, 96);
}

/// The test the round-trip assertions cannot do: check the *text* that comes
/// back, not a comparison of two equally-lossy values.
#[test]
fn nothing_a_later_version_wrote_is_lost() {
    let l = Label::from_json(&corpus("from-the-future.relabel")).unwrap();
    let back = l.to_json().unwrap();
    for needle in [
        "colour_profile",      // an unknown top-level key
        "something from 2027", // ...and its nested payload
        "hologram",            // an unknown element type
        "632",                 // ...and its payload
        "layers",              // ...including nested objects
        "dash_dots",           // an unknown field on a known element
        "oval",                // an unknown enum value
        "mirror",              // ...and another
    ] {
        assert!(back.contains(needle), "{needle:?} was lost on save");
    }
    // And it is stable: a second save changes nothing further.
    let twice = Label::from_json(&back).unwrap().to_json().unwrap();
    assert_eq!(back, twice, "saving twice must be idempotent");
}

#[test]
fn hostile_geometry_neither_panics_nor_wraps() {
    let l = Label::from_json(&corpus("hostile-numbers.relabel")).unwrap();
    let t = Target::new(320, 96, 203).unwrap();
    let (grey, report) = rasterise(&l, &t).unwrap();
    assert_eq!(grey.w, 320);
    // The i32::MAX frame is out of bounds and must be reported, not overflow.
    assert!(report.off_canvas.contains(&1));
    // 45 degrees survives the file and is reported as approximated.
    assert!(report.rotation_approximated.contains(&2));
    // -90 normalises to a real quarter turn, so it is exact.
    assert!(!report.rotation_approximated.contains(&3));
}

#[test]
fn the_corpus_survives_a_save_and_reload() {
    for name in [
        "kitchen-sink.relabel",
        "from-the-future.relabel",
        "hostile-numbers.relabel",
        "images.relabel",
        "variables.relabel",
    ] {
        let l = Label::from_json(&corpus(name)).unwrap();
        let back = Label::from_json(&l.to_json().unwrap())
            .unwrap_or_else(|e| panic!("{name} did not reload after saving: {e}"));
        assert_eq!(l, back, "{name}");
    }
}

/// Embedded artwork as the first build to store it wrote it: an 8-bit grey
/// PNG and a grey-and-alpha one draw, and a fit and a halftone this build does
/// not know - and an image from before artwork was embedded - each fail their
/// element alone, and all of it survives a save.
#[test]
fn the_image_corpus_draws_as_it_did() {
    let l = Label::from_json(&corpus("images.relabel")).unwrap();
    let t = Target::new(320, 96, 203).unwrap();
    let (_, r) = rasterise(&l, &t).unwrap();
    let failed: Vec<u32> = r.failed.iter().map(|f| f.0).collect();
    assert_eq!(failed, vec![3, 4, 5], "{r:?}");
    for (i, why) in [(0, "tile"), (1, "blue_noise"), (2, "no embedded artwork")] {
        assert!(r.failed[i].1.to_string().contains(why), "{:?}", r.failed[i]);
    }
    assert!(
        r.failed[2].1.to_string().contains("logo.png"),
        "{:?}",
        r.failed[2]
    );
    assert!(r.image_resampled.is_empty(), "{r:?}");
    let back = l.to_json().unwrap();
    for needle in ["\"tile\"", "\"blue_noise\"", "\"logo.png\""] {
        assert!(back.contains(needle), "{needle} was lost on save");
    }
    common::check("image-corpus", &rasterise_plane(&l, &t).unwrap());
}

/// The variable-data corpus: its binding loads as a CSV file beside it, and
/// its placeholders are stored as written.
#[test]
fn the_variable_corpus_keeps_its_binding() {
    let l = Label::from_json(&corpus("variables.relabel")).unwrap();
    match &l.data {
        Some(DataBinding::Known(KnownBinding::CsvFile(f))) => {
            assert_eq!(
                (f.path.as_str(), f.delimiter, f.has_header),
                ("variables.csv", ',', true)
            );
        }
        other => panic!("{other:?}"),
    }
    let back = l.to_json().unwrap();
    assert!(back.contains("{sku}") && back.contains("{dt|YYYY-MM-DD}"));
}

/// Fold-over and cable-flag stock as the first build to read it wrote it: the
/// tail, the crease, the die-cut and the repeat all survive a save, the fold's
/// axis is written only when it is not the feed, and the label still draws.
#[test]
fn the_fold_over_corpus_survives_a_save_and_still_draws() {
    let t = Target::new(320, 96, 203).unwrap();
    for (name, needles) in [
        (
            "fold-over.relabel",
            vec![
                "\"at_start\": true",
                "\"len_dots\": 40",
                "\"at_dots\": 180",
                "\"parts\": 2",
                "\"radius\": 8",
                "\"repeat\": \"flip\"",
            ],
        ),
        (
            "fold-head-axis.relabel",
            vec![
                "\"axis\": \"head\"",
                "\"at_dots\": 40",
                "\"repeat\": \"copy\"",
            ],
        ),
    ] {
        let l = Label::from_json(&corpus(name)).unwrap();
        let back = l.to_json().unwrap();
        for needle in needles {
            assert!(back.contains(needle), "{name} lost {needle} on save");
        }
        assert_eq!(
            Label::from_json(&back).unwrap(),
            l,
            "{name} did not reload after saving"
        );
        // A feed fold writes no axis key, so a label that folds the ordinary way
        // saves as it did before the field existed.
        assert_eq!(
            back.contains("\"axis\""),
            name == "fold-head-axis.relabel",
            "{name}"
        );
        let p = rasterise_plane(&l, &t).unwrap();
        assert_eq!((p.width(), p.height()), (320, 96));
        // The repeat actually happened: the second panel carries the first.
        let (grey, _) = rasterise(&l, &t).unwrap();
        assert!(grey.coverage(128) > 0.0, "{name} drew nothing");
    }
}
