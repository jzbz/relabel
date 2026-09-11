//! Hostile headers are refused before anything is allocated for them.
//!
//! Its own test binary, with one test and a counting allocator, so that no
//! other test running in parallel can disturb the peak. Each case is a few
//! dozen bytes claiming an enormous image; `image` 0.25.10 left to itself
//! allocates what such a header claims, and for a PNG claiming 100000 x 100000
//! that aborted the process. Each of the first three cases is refused by a
//! different check, so removing any one of them fails its case.

#[path = "common/fixtures.rs"]
mod fixtures;

use fixtures::*;
use relabel_render::artwork::{self, ArtworkError};
use relabel_render::document::*;
use relabel_render::{Dpmm, Target, rasterise};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = NOW.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        NOW.fetch_sub(l.size(), Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Run `f`, returning its result and the most it allocated above what was
/// already live.
fn peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = NOW.load(Relaxed);
    PEAK.store(base, Relaxed);
    let out = f();
    (out, PEAK.load(Relaxed).saturating_sub(base))
}

const MIB: usize = 1 << 20;

#[test]
fn hostile_headers_are_refused_without_allocating() {
    let import = |bytes: &[u8]| artwork::import(bytes, (96, 96), &ImageFit::Contain, Dpmm::DPI_203);
    for (name, bytes, want) in [
        (
            "PNG 100000 x 100000: a side over the limit",
            png_bomb(100_000, 100_000, 8, 2),
            ArtworkError::SideTooLong,
        ),
        (
            "PNG 8000 x 8000: over the pixel budget",
            png_bomb(8000, 8000, 8, 2),
            ArtworkError::TooManyPixels { w: 8000, h: 8000 },
        ),
        (
            "PNG 6000 x 6000 at 16 bits RGBA: under the pixel budget, over the memory one",
            png_bomb(6000, 6000, 16, 6),
            ArtworkError::TooMuchMemory,
        ),
        (
            "BMP 12000 x 12000",
            bmp_bomb(12_000, 12_000),
            ArtworkError::TooManyPixels {
                w: 12_000,
                h: 12_000,
            },
        ),
        (
            "GIF screen 65535 x 65535",
            gif_bomb(65_535, 65_535),
            ArtworkError::SideTooLong,
        ),
        (
            "progressive JPEG 6000 x 6000: its coefficients, over the memory budget",
            progressive_jpeg_bomb(6000, 6000),
            ArtworkError::TooMuchMemory,
        ),
        (
            "JPEG claiming 65535 x 65535",
            jpeg_bomb(65_535, 65_535),
            ArtworkError::SideTooLong,
        ),
    ] {
        let (r, used) = peak(|| import(&bytes));
        assert_eq!(r.err(), Some(want), "{name}");
        assert!(used < MIB, "{name}: {} KiB allocated", used / 1024);
    }

    // A source 16384 pixels wide stretched into a frame 8191 dots tall: the
    // resize's working buffer, run the obvious way round, would be 2 GiB.
    let wide = png_l8(16_384, 1, |x, _| (x % 256) as u8);
    let (r, used) = peak(|| artwork::import(&wide, (96, 8191), &ImageFit::Stretch, Dpmm::DPI_203));
    assert_eq!(r.map(|i| (i.art.w, i.art.h)).ok(), Some((96, 8191)));
    assert!(
        used < 64 * MIB,
        "wide into tall: {} MiB allocated",
        used / MIB
    );

    // A label carrying such a header as its artwork: drawing refuses it from
    // the header, and only that element fails.
    let mut l = Label::new(320, 96, Dpmm::DPI_203);
    l.elements.push(ElementOrRaw::Known(Element::Image(ImageEl {
        common: Common::new(
            1,
            RectDots {
                feed: 0,
                head: 0,
                feed_len: 96,
                head_len: 96,
            },
        ),
        path: String::new(),
        png: base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            png_bomb(8000, 8000, 8, 0),
        ),
        fit: ImageFit::Contain,
        halftone: None,
        threshold: None,
    })));
    let t = Target::new(320, 96, 203).unwrap();
    let (r, used) = peak(|| rasterise(&l, &t).unwrap().1);
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(r.failed[0].1.to_string().contains("8000x8000"), "{r:?}");
    assert!(used < MIB, "drawing: {} KiB allocated", used / 1024);
}
