//! Grey to one bit.
//!
//! A thermal head fires a dot or it does not, so everything on a label is
//! reduced to one bit per dot before it is sent. How that happens decides
//! whether a photograph comes out as a picture or as a black rectangle, and
//! whether the barcode beside it still scans.
//!
//! # Zones
//!
//! Error diffusion carries each dot's rounding error on to its neighbours. On
//! a page holding one photograph that is the whole point; on a label holding a
//! photograph, a line of text and a barcode it is a hazard, because the error
//! does not know where one element ends and the next begins. So every element
//! is painted into its own *zone*, and error is never carried from one zone
//! into another. Each zone also carries its own settings, which is how a
//! photograph can be diffused while the text beside it is hard-thresholded.
//!
//! What the fence actually protects was measured rather than assumed, and it
//! is narrower than the obvious story. A barcode beside a photograph is *not*
//! at risk: across three photograph types, three diffusion kernels and
//! thresholds from 64 to 192, leaked error flipped no dots in the bars or the
//! quiet zone - even with the bars pushed flush against the photograph and no
//! quiet zone at all. Pure black and white absorb a great deal of error before
//! they flip, with a margin of the threshold on one side and 255 minus it on
//! the other. What *is* at risk is anything holding intermediate values -
//! above all the anti-aliased edge of a glyph, where a dot sitting near the
//! threshold flips on the slightest push - and the per-element settings, which
//! a shared zone could not express at all.
//!
//! # The tone curve is identity until a print says otherwise
//!
//! Thermal paper has heavy dot gain - a fired dot prints larger than its cell -
//! and the received wisdom is to lighten the image first to compensate. The
//! design work behind this module simulated the printed page and found the
//! opposite: a lightening curve made tone reproduction worse at every dot-gain
//! level modelled, and a D110_M has already printed correctly with no curve at
//! all. So the curve is a 256-entry table that stays the identity until a
//! printed step wedge produces a measured one. Guessing a gamma would regress a
//! path that works.
//!
//! # Why this is hand-rolled
//!
//! `image::imageops::colorops::dither` panics on anything narrower than two
//! pixels - it indexes the pixel to the right before checking there is one -
//! and a one-dot-wide element on a 96-dot head is ordinary. It is also fixed to
//! one kernel with no threshold and no serpentine scan. The loop here is the
//! one that has already printed correctly from the bring-up CLI, moved rather
//! than rewritten, with the zone fence added.

use crate::greymap::GreyMap;
use relabel_proto::BitPlane;

/// How a zone's grey dots become black or white.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Halftone {
    /// Hard cut at the threshold. Right for line art, text and every symbology,
    /// where a half-lit module is a scan failure.
    #[default]
    Threshold,
    /// The usual choice for a photograph, and the best of the set on local
    /// fidelity.
    FloydSteinberg,
    /// Diffuses only six eighths of the error. Kept for a reason other than
    /// tone: it almost never fires an isolated dot, and a lone dot is exactly
    /// the mark that direct-thermal dot gain distorts most.
    ///
    /// The discarded error raises contrast, and harshly at the ends. Measured
    /// on a flat field it matches Floyd-Steinberg at the midpoint, but prints
    /// nothing at all for greys of about 224 and lighter, and solid black for
    /// about 32 and darker. Right for a logo; wrong for a photograph with a
    /// pale background, which disappears.
    Atkinson,
    /// Three-row Sierra. A middle ground between the two above.
    Sierra3,
    /// Ordered dither. No diffusion at all, so no error can travel anywhere,
    /// and a flat grey comes out as a regular texture.
    Bayer8,
}

/// Settings for one zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub halftone: Halftone,
    /// A dot at or below this is ink. For `Bayer8` it shifts the whole ordered
    /// matrix instead, so it still darkens and lightens.
    pub threshold: u8,
    /// Scale on the diffused error, `0.0` to `1.0`. Zero reduces any diffusion
    /// kernel to a plain threshold; values between soften a texture that comes
    /// out too crunchy at 96 dots without changing kernel.
    pub strength: f32,
    /// Treat light as dark and dark as light before quantising.
    pub invert: bool,
}

impl Zone {
    /// Hard threshold at the midpoint. What the page gets unless told otherwise.
    pub const PAGE: Zone = Zone {
        halftone: Halftone::Threshold,
        threshold: 128,
        strength: 1.0,
        invert: false,
    };

    pub const fn with(halftone: Halftone) -> Zone {
        Zone {
            halftone,
            threshold: 128,
            strength: 1.0,
            invert: false,
        }
    }
}

impl Default for Zone {
    fn default() -> Zone {
        Zone::PAGE
    }
}

/// The tone curve that changes nothing.
pub const IDENTITY: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as u8;
        i += 1;
    }
    t
};

static PAGE_ONLY: [Zone; 1] = [Zone::PAGE];

#[derive(Debug, Clone, Copy)]
pub struct BinariseOpts<'a> {
    /// Applied to every dot before quantising. Dot gain, paper sensitivity and
    /// the printer's own density setting all collapse into this one curve.
    pub tone: &'a [u8; 256],
    /// Settings by zone id. Zone 0 is the page. An id with no entry here uses
    /// the page's settings - but it is still fenced off, because the fence
    /// compares zone ids, not settings.
    pub zones: &'a [Zone],
    /// Alternate the scan direction each row, which stops diffusion error
    /// building into diagonal streaks. Page-level rather than per-zone: scan
    /// direction is a property of a whole row, and two zones can share a row.
    pub serpentine: bool,
}

impl Default for BinariseOpts<'static> {
    fn default() -> Self {
        BinariseOpts {
            tone: &IDENTITY,
            zones: &PAGE_ONLY,
            serpentine: true,
        }
    }
}

impl<'a> BinariseOpts<'a> {
    /// Identity tone and serpentine scanning, with the given zones.
    pub fn with_zones(zones: &'a [Zone]) -> BinariseOpts<'a> {
        BinariseOpts {
            tone: &IDENTITY,
            zones,
            serpentine: true,
        }
    }
}

/// A diffusion kernel: where the error goes, and in what shares.
struct Kernel {
    /// `(dx, dy, share)`, with `dx` measured in the direction of the scan.
    taps: &'static [(isize, isize, i32)],
    den: i32,
}

const FLOYD: Kernel = Kernel {
    taps: &[(1, 0, 7), (-1, 1, 3), (0, 1, 5), (1, 1, 1)],
    den: 16,
};

/// Six taps of one eighth each: only six eighths of the error is diffused,
/// which is the point of the kernel.
const ATKINSON: Kernel = Kernel {
    taps: &[
        (1, 0, 1),
        (2, 0, 1),
        (-1, 1, 1),
        (0, 1, 1),
        (1, 1, 1),
        (0, 2, 1),
    ],
    den: 8,
};

const SIERRA3: Kernel = Kernel {
    taps: &[
        (1, 0, 5),
        (2, 0, 3),
        (-2, 1, 2),
        (-1, 1, 4),
        (0, 1, 5),
        (1, 1, 4),
        (2, 1, 2),
        (-1, 2, 2),
        (0, 2, 3),
        (1, 2, 2),
    ],
    den: 32,
};

fn kernel(h: Halftone) -> Option<&'static Kernel> {
    match h {
        Halftone::FloydSteinberg => Some(&FLOYD),
        Halftone::Atkinson => Some(&ATKINSON),
        Halftone::Sierra3 => Some(&SIERRA3),
        Halftone::Threshold | Halftone::Bayer8 => None,
    }
}

/// The standard 8x8 ordered-dither matrix, values 0 to 63.
const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// The ordered threshold for a dot, shifted by the zone's own threshold so
/// that 128 is the unmodified matrix and the knob still darkens and lightens.
fn bayer_threshold(x: usize, y: usize, threshold: u8) -> i32 {
    let m = i32::from(BAYER8[y % 8][x % 8]);
    // Centres each of the 64 levels in its band: 1 at the lightest cell up to
    // 253 at the darkest, so a flat 128 comes out exactly half inked.
    (2 * m + 1) * 255 / 128 + (i32::from(threshold) - 128)
}

/// Look up a zone's settings, falling back to the page's.
fn zone_for(zones: &[Zone], id: u8) -> Zone {
    zones
        .get(usize::from(id))
        .or_else(|| zones.first())
        .copied()
        .unwrap_or(Zone::PAGE)
}

/// Strength as a fixed-point factor over 1024.
///
/// Integer arithmetic keeps the result identical on every platform. At full
/// strength the factor is exactly 1024, which cancels out of the division and
/// reproduces the plain kernel arithmetic bit for bit.
fn strength_fixed(s: f32) -> i32 {
    // A NaN clamps to NaN and then casts to zero: no diffusion, which is the
    // safe reading of a strength nobody can interpret.
    (s.clamp(0.0, 1.0) * 1024.0).round() as i32
}

/// Reduce a grey map to one bit.
///
/// Never panics on the shape of its input, including maps one dot wide or
/// high, and a map whose buffers are shorter than its stated size - missing
/// dots read as paper, missing zone entries as the page.
pub fn binarise(g: &GreyMap, o: &BinariseOpts<'_>) -> BitPlane {
    let (w, h) = (usize::from(g.w), usize::from(g.h));
    let mut plane = BitPlane::new(g.w, g.h);
    if w == 0 || h == 0 {
        return plane;
    }
    let n = w * h;
    let zone_id = |i: usize| g.zone.get(i).copied().unwrap_or(0);

    // A signed working copy with headroom, because diffused error pushes values
    // below 0 and above 255 before they are quantised.
    let mut buf: Vec<i16> = (0..n)
        .map(|i| {
            let z = zone_for(o.zones, zone_id(i));
            let v = o.tone[usize::from(g.px.get(i).copied().unwrap_or(255))];
            i16::from(if z.invert { 255 - v } else { v })
        })
        .collect();

    for y in 0..h {
        let ltr = !o.serpentine || y % 2 == 0;
        for i in 0..w {
            let x = if ltr { i } else { w - 1 - i };
            let src = y * w + x;
            let zid = zone_id(src);
            let z = zone_for(o.zones, zid);
            let old = i32::from(buf[src]);

            let ink = match z.halftone {
                Halftone::Bayer8 => old <= bayer_threshold(x, y, z.threshold),
                _ => old <= i32::from(z.threshold),
            };
            if ink {
                plane.set(x as u16, y as u16, true);
            }

            let Some(k) = kernel(z.halftone) else {
                continue;
            };
            let s = strength_fixed(z.strength);
            if s == 0 {
                continue;
            }
            let err = old - if ink { 0 } else { 255 };
            for &(dx, dy, share) in k.taps {
                let nx = if ltr {
                    x as isize + dx
                } else {
                    x as isize - dx
                };
                let ny = y as isize + dy;
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let dst = ny as usize * w + nx as usize;
                // The fence. Error never leaves the zone it was generated in,
                // which is what stops a photograph's dither landing in the
                // anti-aliased edge of the text beside it.
                if zone_id(dst) != zid {
                    continue;
                }
                let add = err * share * s / (k.den * 1024);
                buf[dst] = (i32::from(buf[dst]) + add).clamp(-255, 510) as i16;
            }
        }
    }
    plane
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(w: u16, h: u16, px: impl Fn(u16, u16) -> u8) -> GreyMap {
        let mut g = GreyMap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                g.px[usize::from(y) * usize::from(w) + usize::from(x)] = px(x, y);
            }
        }
        g
    }

    fn run(g: &GreyMap, h: Halftone) -> BitPlane {
        let zones = [Zone::with(h)];
        binarise(g, &BinariseOpts::with_zones(&zones))
    }

    fn ink_fraction(p: &BitPlane) -> f32 {
        let mut n = 0u32;
        for y in 0..p.height() {
            for x in 0..p.width() {
                n += u32::from(p.get(x, y));
            }
        }
        n as f32 / (u32::from(p.width()) * u32::from(p.height())) as f32
    }

    const ALL: [Halftone; 5] = [
        Halftone::Threshold,
        Halftone::FloydSteinberg,
        Halftone::Atkinson,
        Halftone::Sierra3,
        Halftone::Bayer8,
    ];

    #[test]
    fn the_kernels_conserve_what_they_claim() {
        let sum = |k: &Kernel| k.taps.iter().map(|t| t.2).sum::<i32>();
        assert_eq!(sum(&FLOYD), FLOYD.den, "Floyd-Steinberg diffuses all of it");
        assert_eq!(sum(&SIERRA3), SIERRA3.den, "so does Sierra");
        assert_eq!(sum(&ATKINSON), 6, "Atkinson diffuses six eighths");
    }

    #[test]
    fn the_bayer_matrix_is_a_permutation() {
        let mut seen = [false; 64];
        for row in BAYER8 {
            for v in row {
                assert!(!seen[usize::from(v)], "{v} appears twice");
                seen[usize::from(v)] = true;
            }
        }
        assert!(seen.iter().all(|s| *s));
    }

    /// The regression against the `image` crate, which panics on a one-dot
    /// wide input. Every kernel, both thin shapes.
    #[test]
    fn one_dot_wide_or_high_inputs_do_not_panic() {
        for h in ALL {
            for (w, ht) in [(1, 8), (8, 1), (1, 1)] {
                let g = map(w, ht, |x, y| ((x + y) * 37 % 256) as u8);
                let p = run(&g, h);
                assert_eq!((p.width(), p.height()), (w, ht), "{h:?}");
            }
        }
    }

    #[test]
    fn a_short_buffer_reads_as_paper_rather_than_panicking() {
        let mut g = GreyMap::new(8, 8);
        g.px.truncate(10);
        g.zone.clear();
        for h in ALL {
            let _ = run(&g, h);
        }
    }

    /// One rule for "is this ink", in one place: the plain threshold on the grey
    /// map and the threshold halftone must agree on every value.
    #[test]
    fn the_threshold_rule_is_shared() {
        let g = map(256, 1, |x, _| x as u8);
        for t in [0u8, 1, 64, 127, 128, 129, 254, 255] {
            let zones = [Zone {
                threshold: t,
                ..Zone::PAGE
            }];
            assert_eq!(
                binarise(&g, &BinariseOpts::with_zones(&zones)),
                g.threshold(t),
                "threshold {t}"
            );
        }
    }

    /// Pure black and white come through every kernel untouched: they generate
    /// no error, so there is nothing to diffuse.
    #[test]
    fn black_and_white_are_fixed_points_of_every_kernel() {
        let g = map(
            32,
            16,
            |x, y| if (x / 3 + y / 2) % 2 == 0 { 0 } else { 255 },
        );
        let want = g.threshold(128);
        for h in ALL {
            assert_eq!(run(&g, h), want, "{h:?}");
        }
    }

    /// An error-conserving kernel reproduces tone: a flat field at a given
    /// grey comes out with about that much ink. An independent check that the
    /// golden images are not confidently wrong.
    #[test]
    fn diffusion_reproduces_flat_greys() {
        for h in [
            Halftone::FloydSteinberg,
            Halftone::Sierra3,
            Halftone::Bayer8,
        ] {
            for v in [32u8, 64, 128, 192, 224] {
                let p = run(&map(96, 96, |_, _| v), h);
                let want = (255.0 - f32::from(v)) / 255.0;
                let got = ink_fraction(&p);
                assert!(
                    (got - want).abs() < 0.03,
                    "{h:?} at {v}: {got:.3} ink, expected about {want:.3}"
                );
            }
        }
    }

    /// Atkinson throws away a quarter of the error, which raises contrast
    /// rather than shifting tone: dark greys fill in, light greys wash out,
    /// and the midpoint is untouched. The figures in the kernel's own
    /// documentation come from this measurement.
    #[test]
    fn atkinson_raises_contrast() {
        let ink = |v: u8, h| ink_fraction(&run(&map(96, 96, |_, _| v), h));
        let (fs, at) = (Halftone::FloydSteinberg, Halftone::Atkinson);
        assert!(ink(64, at) > ink(64, fs) + 0.05, "shadows fill in");
        assert!(
            (ink(128, at) - ink(128, fs)).abs() < 0.02,
            "the midpoint holds"
        );
        assert!(ink(192, at) < ink(192, fs) - 0.05, "highlights wash out");
        assert_eq!(ink(224, at), 0.0, "a pale grey vanishes entirely");
        assert_eq!(ink(32, at), 1.0, "a dark grey prints solid");
    }

    #[test]
    fn zero_strength_reduces_diffusion_to_a_threshold() {
        let g = map(64, 32, |x, y| ((x * 4 + y * 3) % 256) as u8);
        let zones = [Zone {
            halftone: Halftone::FloydSteinberg,
            strength: 0.0,
            ..Zone::PAGE
        }];
        assert_eq!(
            binarise(&g, &BinariseOpts::with_zones(&zones)),
            g.threshold(128)
        );
    }

    #[test]
    fn the_identity_curve_is_a_no_op_and_a_darkening_curve_is_not() {
        let g = map(64, 32, |x, _| (x * 4) as u8);
        let zones = [Zone::with(Halftone::FloydSteinberg)];
        let plain = binarise(&g, &BinariseOpts::with_zones(&zones));
        let explicit = binarise(
            &g,
            &BinariseOpts {
                tone: &IDENTITY,
                ..BinariseOpts::with_zones(&zones)
            },
        );
        assert_eq!(plain, explicit);

        let mut dark = IDENTITY;
        for (i, v) in dark.iter_mut().enumerate() {
            *v = (i / 2) as u8;
        }
        let darker = binarise(
            &g,
            &BinariseOpts {
                tone: &dark,
                ..BinariseOpts::with_zones(&zones)
            },
        );
        assert!(ink_fraction(&darker) > ink_fraction(&plain));
    }

    #[test]
    fn inversion_swaps_ink_and_paper() {
        let g = map(8, 1, |_, _| 0);
        let zones = [Zone {
            invert: true,
            ..Zone::PAGE
        }];
        let p = binarise(&g, &BinariseOpts::with_zones(&zones));
        assert!(!p.get(0, 0), "black, inverted, does not print");
    }

    #[test]
    fn serpentine_changes_the_texture_and_is_deterministic() {
        let g = map(96, 48, |x, _| (x * 255 / 95) as u8);
        let zones = [Zone::with(Halftone::FloydSteinberg)];
        let on = BinariseOpts::with_zones(&zones);
        let off = BinariseOpts {
            serpentine: false,
            ..on
        };
        assert_eq!(
            binarise(&g, &on),
            binarise(&g, &on),
            "same input, same output"
        );
        assert_ne!(binarise(&g, &on), binarise(&g, &off));
    }

    /// The fence, with teeth. Region B holds greys right at the threshold, so
    /// any error arriving from region A flips its bits. With A and B in the
    /// *same* zone, B's output depends on what A holds - that is the leak. Give
    /// them different zone ids and B comes out bit-identical whatever A holds.
    /// The settings are identical throughout; only the zone ids change.
    #[test]
    fn error_never_crosses_a_zone_boundary() {
        let build = |a_present: bool, separate: bool| {
            let mut g = map(64, 32, |x, y| {
                if x < 32 {
                    if a_present {
                        ((x * 7 + y * 13) % 200 + 28) as u8
                    } else {
                        255
                    }
                } else {
                    // Greys straddling the threshold: maximally sensitive.
                    if (x + y) % 2 == 0 { 126 } else { 130 }
                }
            });
            for y in 0..32usize {
                for x in 32..64usize {
                    g.zone[y * 64 + x] = if separate { 2 } else { 1 };
                }
                for x in 0..32usize {
                    g.zone[y * 64 + x] = 1;
                }
            }
            g
        };
        let region_b = |p: &BitPlane| -> Vec<bool> {
            (0..32u16)
                .flat_map(|y| (32..64u16).map(move |x| (x, y)))
                .map(|(x, y)| p.get(x, y))
                .collect()
        };
        let zones = [
            Zone::with(Halftone::FloydSteinberg),
            Zone::with(Halftone::FloydSteinberg),
            Zone::with(Halftone::FloydSteinberg),
        ];
        let o = BinariseOpts::with_zones(&zones);

        // The leak is real when the fence is not there...
        assert_ne!(
            region_b(&binarise(&build(true, false), &o)),
            region_b(&binarise(&build(false, false), &o)),
            "without separate zones, region B should depend on region A"
        );
        // ...and gone when it is.
        assert_eq!(
            region_b(&binarise(&build(true, true), &o)),
            region_b(&binarise(&build(false, true), &o)),
            "with separate zones, region B must not depend on region A"
        );
    }

    /// A zone with no settings of its own takes the page's, but is still fenced:
    /// the fence compares ids, not settings.
    #[test]
    fn an_unconfigured_zone_uses_the_page_settings() {
        let mut g = map(8, 1, |_, _| 200);
        g.zone = vec![7; 8];
        let zones = [Zone {
            threshold: 250,
            ..Zone::PAGE
        }];
        let p = binarise(&g, &BinariseOpts::with_zones(&zones));
        assert!(p.get(0, 0), "200 is ink at the page's threshold of 250");
    }
}
