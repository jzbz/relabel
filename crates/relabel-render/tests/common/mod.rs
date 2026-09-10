//! Golden-image helpers shared by the integration tests.
//!
//! A golden is a PBM (`P4`) file, whose bit layout is exactly `BitPlane`'s own:
//! a short header, then rows of one bit per dot, most significant bit first,
//! padded to a whole byte. Regenerate with `RELABEL_BLESS=1`, and look at the
//! images before committing them - a golden blessed unseen only proves the code
//! agrees with itself. For a barcode, looking is not enough either: read it back
//! with an independent decoder such as `zbarimg`, which is how a wrong Codabar
//! table in a dependency was found.

use relabel_proto::BitPlane;
use std::path::PathBuf;

pub fn pbm(p: &BitPlane) -> Vec<u8> {
    let mut out = format!("P4\n{} {}\n", p.width(), p.height()).into_bytes();
    out.extend_from_slice(p.bits());
    out
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.pbm"))
}

/// Compare against the golden byte for byte, or write it when blessing.
pub fn check(name: &str, plane: &BitPlane) {
    let path = golden_path(name);
    let got = pbm(plane);
    if std::env::var_os("RELABEL_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "no golden at {}. Generate it with RELABEL_BLESS=1 and look at it before committing.",
            path.display()
        )
    });
    // Deterministic integer rendering: any difference at all is a change.
    assert!(want == got, "{name} no longer matches its golden image");
}
