//! Reading and writing label files: a save replaces the file whole or leaves it
//! as it was, and a read says what is wrong with a file in words.

#[path = "support/mod.rs"]
mod support;

use relabel_gui::disk::{self, ReadError, SaveError};
use relabel_render::document::{
    Common, Element, ElementOrRaw, GlyphRaster, Label, RectDots, TextAlign, TextEl, TextWeight,
};
use std::fs;
use std::path::{Path, PathBuf};
use support::reference_label;

/// A directory of the test's own under the build's scratch space, emptied first
/// so a run never sees what an earlier one left.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("file_disk")
        .join(test);
    if dir.exists() {
        make_writable(&dir);
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A folder or file a test set read-only, set back so it can be cleared. A link
/// is left alone: it may point nowhere, and removing it needs nothing of it.
fn make_writable(p: &Path) {
    let meta = fs::symlink_metadata(p).unwrap();
    if meta.file_type().is_symlink() {
        return;
    }
    let mut perms = meta.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(p, perms).unwrap();
    if meta.is_dir() {
        for e in fs::read_dir(p).unwrap() {
            make_writable(&e.unwrap().path());
        }
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut n: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    n.sort();
    n
}

#[test]
fn a_save_replaces_the_file_whole_and_leaves_nothing_beside_it() {
    let dir = scratch("replace");
    let a = dir.join("a.relabel");
    fs::write(&a, "old").unwrap();
    disk::write_atomic(&a, b"new").expect("the save");
    assert_eq!(fs::read(&a).unwrap(), b"new");
    assert_eq!(names(&dir), ["a.relabel"]);
}

/// Replaced, not written into: a hard link to the old file keeps the old text.
/// The one test that tells a rename from a write in place on every platform, and
/// whoever runs it.
#[test]
fn a_save_replaces_the_file_rather_than_writing_into_it() {
    let dir = scratch("replaced");
    let a = dir.join("a.relabel");
    let kept = dir.join("kept.relabel");
    fs::write(&a, "old").unwrap();
    fs::hard_link(&a, &kept).unwrap();
    disk::write_atomic(&a, b"new").expect("the save");
    assert_eq!(fs::read(&a).unwrap(), b"new");
    assert_eq!(fs::read(&kept).unwrap(), b"old");
    assert_eq!(names(&dir), ["a.relabel", "kept.relabel"]);
}

/// A file the operator may not write is refused even when other write bits are
/// set: in a folder shared with a group, the rename would otherwise replace a
/// colleague's label that an ordinary editor could not have changed. Root may
/// write it, so for root the save goes ahead.
#[cfg(unix)]
#[test]
fn a_file_its_owner_may_not_write_is_refused_whatever_its_other_bits_say() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = scratch("not_mine_to_write");
    let a = dir.join("a.relabel");
    fs::write(&a, "old").unwrap();
    let root = fs::metadata(&a).unwrap().uid() == 0;
    fs::set_permissions(&a, fs::Permissions::from_mode(0o466)).unwrap();

    let result = disk::write_atomic(&a, b"new");
    fs::set_permissions(&a, fs::Permissions::from_mode(0o644)).unwrap();
    if root {
        assert!(result.is_ok(), "{result:?}");
        return;
    }
    assert!(matches!(result, Err(SaveError::ReadOnly)), "{result:?}");
    assert_eq!(fs::read(&a).unwrap(), b"old");
    assert_eq!(names(&dir), ["a.relabel"]);
}

/// Not merely a failed rename: in a folder the operator can write to, the rename
/// would succeed and replace a file marked not to be written.
#[test]
fn a_read_only_file_is_refused_and_left_as_it_was() {
    let dir = scratch("read_only");
    let a = dir.join("a.relabel");
    fs::write(&a, "old").unwrap();
    let mut perms = fs::metadata(&a).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&a, perms).unwrap();

    let e = disk::write_atomic(&a, b"new").expect_err("a read-only file");
    assert!(matches!(e, SaveError::ReadOnly));
    assert!(e.to_string().contains("read-only"), "{e}");
    assert_eq!(fs::read(&a).unwrap(), b"old");
    assert!(!names(&dir).iter().any(|n| n.ends_with(".part")));
    make_writable(&a);
}

#[cfg(unix)]
#[test]
fn a_save_keeps_the_files_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("mode");
    let a = dir.join("a.relabel");
    fs::write(&a, "old").unwrap();
    fs::set_permissions(&a, fs::Permissions::from_mode(0o640)).unwrap();
    disk::write_atomic(&a, b"new").expect("the save");
    assert_eq!(
        fs::metadata(&a).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[cfg(unix)]
#[test]
fn a_save_through_a_symlink_replaces_what_it_points_at() {
    let dir = scratch("symlink");
    let real = dir.join("real.relabel");
    let link = dir.join("link.relabel");
    fs::write(&real, "old").unwrap();
    std::os::unix::fs::symlink("real.relabel", &link).unwrap();

    disk::write_atomic(&link, b"new").expect("the save");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&real).unwrap(), b"new");
    assert_eq!(names(&dir), ["link.relabel", "real.relabel"]);
}

#[cfg(unix)]
#[test]
fn a_link_that_points_nowhere_is_refused_by_name() {
    let dir = scratch("dangling");
    let link = dir.join("link.relabel");
    std::os::unix::fs::symlink("gone.relabel", &link).unwrap();
    let e = disk::write_atomic(&link, b"new").expect_err("a dangling link");
    assert_eq!(e.to_string(), "it is a link that points nowhere");
    assert_eq!(names(&dir), ["link.relabel"]);
}

/// A `.part` file may be another program's, or all that is left of a save a
/// crash interrupted: never removed, only stepped round.
#[test]
fn a_part_file_left_behind_is_stepped_round_and_left_alone() {
    let dir = scratch("part");
    let a = dir.join("a.relabel");
    let part = dir.join("a.relabel.part");
    fs::write(&a, "old").unwrap();
    fs::write(&part, "someone else's").unwrap();

    disk::write_atomic(&a, b"new").expect("the save");
    assert_eq!(fs::read(&a).unwrap(), b"new");
    assert_eq!(fs::read(&part).unwrap(), b"someone else's");
    assert_eq!(names(&dir), ["a.relabel", "a.relabel.part"]);
}

#[cfg(unix)]
#[test]
fn a_failed_write_leaves_the_old_file() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = scratch("failed");
    let a = dir.join("a.relabel");
    fs::write(&a, "old").unwrap();
    // Root writes into a folder whatever its mode says, so the failure this
    // needs cannot be made.
    if fs::metadata(&a).unwrap().uid() == 0 {
        eprintln!("skipped: running as root");
        return;
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = disk::write_atomic(&a, b"new");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(matches!(result, Err(SaveError::Io(_))), "{result:?}");
    assert_eq!(fs::read(&a).unwrap(), b"old");
    assert_eq!(names(&dir), ["a.relabel"]);
}

#[test]
fn reading_accepts_a_bom_and_names_what_is_wrong() {
    let dir = scratch("read");
    let json = reference_label().to_json().unwrap();

    let bom = dir.join("bom.relabel");
    fs::write(&bom, [b"\xef\xbb\xbf".as_slice(), json.as_bytes()].concat()).unwrap();
    let label = disk::read_label(&bom).expect("a BOM is dropped");
    assert_eq!(label.elements.len(), 3);

    let said = |name: &str, bytes: &[u8]| {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        disk::read_label(&p).expect_err(name).to_string()
    };
    assert_eq!(said("utf16.relabel", &[0xff, 0xfe]), "it is not UTF-8 text");
    assert_eq!(said("empty.relabel", b""), "it is empty");
    assert_eq!(said("only-bom.relabel", b"\xef\xbb\xbf \n"), "it is empty");
    assert!(
        said("braces.relabel", b"{}").contains("not a relabel label"),
        "{}",
        said("braces.relabel", b"{}")
    );

    let big = dir.join("big.relabel");
    fs::File::create(&big).unwrap().set_len(65 << 20).unwrap();
    let e = disk::read_label(&big).expect_err("65 MiB").to_string();
    assert_eq!(e, "it is 68 MB, far larger than any label");

    let missing = disk::read_label(&dir.join("absent.relabel"));
    assert!(matches!(missing, Err(ReadError::Missing)), "{missing:?}");
    assert_eq!(missing.unwrap_err().to_string(), "it does not exist");
}

#[test]
fn a_label_that_cannot_be_written_writes_nothing() {
    let dir = scratch("unwritable");
    let x = dir.join("x.relabel");
    let mut label = reference_label();
    label
        .elements
        .push(ElementOrRaw::Known(Element::Text(TextEl {
            common: Common::new(4, RectDots::default()),
            text: "x".into(),
            family: String::new(),
            size: 0.0,
            line_height: 1.0,
            align: TextAlign::Start,
            weight: TextWeight::Regular,
            raster: GlyphRaster::Hinted,
        })));

    let e = disk::save(&x, &label).expect_err("a size of 0 does not load back");
    assert!(e.to_string().contains("size"), "{e}");
    assert!(names(&dir).is_empty(), "{:?}", names(&dir));
}

#[test]
fn a_saved_label_reads_back_equal() {
    let dir = scratch("round_trip");
    let a = dir.join("a.relabel");
    let label: Label = reference_label();
    disk::save(&a, &label).expect("the save");
    assert_eq!(disk::read_label(&a).expect("the read"), label);
    assert_eq!(names(&dir), ["a.relabel"]);
}
