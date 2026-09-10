//! Golden images for every halftone, compared byte for byte.
//!
//! The files under `tests/golden/` are PBM (`P4`): a short text header, then
//! rows of one bit per dot, most significant bit first, 1 meaning black, each
//! row padded to a whole byte. That is exactly the layout of `BitPlane` itself,
//! so a golden file is a header followed by the plane's own bytes, readable in
//! any image viewer, and comparable without an image codec in the test path.
//!
//! The fixtures use integer arithmetic only. A zone plate is the classic test
//! pattern for halftoning, but it needs a cosine, and floating-point
//! transcendentals are not guaranteed bit-identical across platforms - a golden
//! built from one would fail on whichever CI runner's `libm` rounds
//! differently. The quadratic bowl gives the same kind of curved,
//! frequency-sweeping content without that hazard.
//!
//! To regenerate after a deliberate change to a kernel:
//! `RELABEL_BLESS=1 cargo test -p relabel-render --test halftone_golden`, then
//! look at the images before committing them. A golden that was blessed
//! without being looked at only proves the code agrees with itself.

use relabel_proto::BitPlane;
use relabel_render::{BinariseOpts, GreyMap, Halftone, Zone, binarise};

const W: u16 = 320;
const H: u16 = 96;

const KERNELS: [(&str, Halftone); 5] = [
    ("threshold", Halftone::Threshold),
    ("floyd", Halftone::FloydSteinberg),
    ("atkinson", Halftone::Atkinson),
    ("sierra3", Halftone::Sierra3),
    ("bayer8", Halftone::Bayer8),
];

/// Black on the left, white on the right, the length of a 40 mm label.
fn ramp() -> GreyMap {
    let mut g = GreyMap::new(W, H);
    for y in 0..usize::from(H) {
        for x in 0..usize::from(W) {
            g.px[y * usize::from(W) + x] = (x * 255 / (usize::from(W) - 1)) as u8;
        }
    }
    g
}

/// Grey rising with the square of the distance from the centre: curved edges
/// at every angle, and a spatial frequency that sweeps across the label.
fn bowl() -> GreyMap {
    let mut g = GreyMap::new(W, H);
    let (cx, cy) = (i64::from(W) / 2, i64::from(H) / 2);
    let max = cx * cx + cy * cy;
    for y in 0..i64::from(H) {
        for x in 0..i64::from(W) {
            let d = (x - cx) * (x - cx) + (y - cy) * (y - cy);
            // Wrapped every quarter of the range, so the bands repeat and tighten
            // towards the corners the way a zone plate's rings do.
            let v = (d * 4 * 255 / max) % 256;
            g.px[(y * i64::from(W) + x) as usize] = v as u8;
        }
    }
    g
}

mod common;
use common::{check, pbm};

#[test]
fn every_kernel_matches_its_golden() {
    for (fixture, grey) in [("ramp", ramp()), ("bowl", bowl())] {
        for (kname, h) in KERNELS {
            let zones = [Zone::with(h)];
            let plane = binarise(&grey, &BinariseOpts::with_zones(&zones));
            check(&format!("{fixture}-{kname}"), &plane);
        }
    }
}

/// The PBM layout really is the plane's own layout, so a golden file is not an
/// encoding of the plane but the plane itself. Checked on a width that is not
/// a multiple of eight, where the row padding would show any mistake.
#[test]
fn the_golden_format_is_the_plane_layout() {
    let mut p = BitPlane::new(10, 2);
    p.set(0, 0, true);
    p.set(9, 1, true);
    let bytes = pbm(&p);
    let header = b"P4\n10 2\n";
    assert_eq!(&bytes[..header.len()], header);
    assert_eq!(&bytes[header.len()..], &[0x80, 0x00, 0x00, 0x40]);
}
