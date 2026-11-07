//! What the About pane says about relabel: its version, copyright, licence and
//! repository, where the third-party notices are, and that relabel is not the
//! vendor's.
//!
//! Toolkit-free, for `filing`'s reason: the words are reached by an ordinary unit
//! test. The version, the licence and the repository are Cargo's, read at
//! compile time from the workspace's `[workspace.package]`, so the pane cannot
//! drift from the manifest. Slint's own attribution is not here: it is the
//! `AboutSlint` widget, which the pane shows as Slint ships it (about.slint).
//!
//! # The notices are pointed to, not embedded
//!
//! The notice file is written by `packaging/third-party-notices.sh` at release
//! time, and is not in the binary. It cannot be built into it honestly: the
//! script needs the network for two crates' licences, so neither `cargo build`
//! nor an offline test can make it; a copy kept in the repository goes stale with
//! every change to `Cargo.lock`, which `THIRD-PARTY.md` rules out; it is several
//! hundred kilobytes, which compressed would need a new dependency; and thousands
//! of lines in one popup are no way to read anything with a screen reader. A
//! release is to ship the file beside the binary, which is what the MIT and BSD
//! notices ask, and the pane says so and names the repository's account of how
//! the file is made; no release step does so yet, and where in a release the
//! file is to sit is not decided, so the pane names no path (THIRD-PARTY.md).

/// The version line: Cargo's version of this crate, which the workspace sets.
pub fn version_says() -> String {
    format!("Version {}", env!("CARGO_PKG_VERSION"))
}

/// The copyright line, in the words rPGP's and Azzurro's About screens use. The
/// year and the holder are LICENSE's, which a test holds it to; Cargo has no
/// field for either.
pub const COPYRIGHT: &str = "Copyright \u{a9} 2026 Jonathan Zeppettini";

/// The licence line, from Cargo's licence field.
pub fn licence_says() -> String {
    format!("Released under the {} licence", env!("CARGO_PKG_LICENSE"))
}

/// The repository, as Cargo has it.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Where the licences of everything relabel is built from are.
pub const NOTICES: &str = "The licences of the code relabel is built from, Slint's \
     among them, are in the third-party notices file that each release ships with. \
     How that file is made is in THIRD-PARTY.md, in the repository.";

/// The README's not-affiliated line, which DESIGN section 9 asks relabel to
/// carry.
pub const TRADEMARK: &str = "Not affiliated with, endorsed by, or connected to \
     NIIMBOT or Gengcon. \"NIIMBOT\" is their trademark and is used here only to say \
     which printers this talks to.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The workspace's root, where the README and THIRD-PARTY.md are.
    fn root() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    #[test]
    fn the_version_licence_and_repository_are_cargos() {
        assert_eq!(
            version_says(),
            format!("Version {}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(licence_says(), "Released under the MIT licence");
        assert_eq!(REPOSITORY, "https://github.com/jzbz/relabel");
    }

    /// The copyright line names LICENSE's year and holder, so the pane and the
    /// licence the code ships under cannot disagree.
    #[test]
    fn the_copyright_line_is_licenses() {
        assert_eq!(COPYRIGHT, "Copyright \u{a9} 2026 Jonathan Zeppettini");
        let licence = std::fs::read_to_string(root().join("LICENSE")).expect("the LICENSE");
        let holder = COPYRIGHT.trim_start_matches("Copyright \u{a9} ");
        assert!(
            licence.contains(&format!("Copyright (c) {holder}")),
            "LICENSE names {holder:?}"
        );
    }

    /// The pane names a file the repository has, and the README carries the same
    /// trademark line, so neither says something the other does not.
    #[test]
    fn the_notices_and_the_trademark_line_match_the_repository() {
        assert!(NOTICES.contains("THIRD-PARTY.md"));
        assert!(root().join("THIRD-PARTY.md").is_file());
        assert!(root().join("packaging/third-party-notices.sh").is_file());
        let readme = std::fs::read_to_string(root().join("README.md")).expect("the README");
        let readme = readme.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(readme.contains(TRADEMARK), "the README says {TRADEMARK:?}");
    }
}
