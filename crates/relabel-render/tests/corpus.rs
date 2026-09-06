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

use relabel_render::document::*;
use relabel_render::{Target, rasterise};

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
    ] {
        let l = Label::from_json(&corpus(name)).unwrap();
        let back = Label::from_json(&l.to_json().unwrap())
            .unwrap_or_else(|e| panic!("{name} did not reload after saving: {e}"));
        assert_eq!(l, back, "{name}");
    }
}
