//! The rendered label as pixels, for a preview pane.
//!
//! Two surfaces: the 1-bit plane exactly as it will print, and the grey map it
//! was thresholded from, which is what an operator judges a dither by. Both
//! write RGB8 into a buffer the caller owns, so no toolkit type appears in any
//! signature here and nothing here allocates the picture.
//!
//! # Why three bytes a pixel for a monochrome label
//!
//! There is no greyscale pixel buffer in Slint - `SharedImageBuffer` has RGB8,
//! RGBA8 and RGBA8-premultiplied and nothing else - so three bytes a pixel is
//! the floor rather than a choice. RGB8 is the smallest of the three and a
//! printer plane has no alpha.
//!
//! # Why the scaling happens here
//!
//! Letting the toolkit scale is not the cheap option, it is the wrong one.
//! `ImageFit::Preserve` preserves a size in *logical* pixels, which is then
//! multiplied by the display's scale factor, so at the 1.25x or 1.5x that
//! Windows and GNOME default to, nearest-neighbour gives one printer dot a
//! single physical pixel and the dot beside it two. An operator judging a
//! dither would be looking at an artefact of the display. Scaling by a whole
//! number here makes source and target the same size in physical pixels, so no
//! filtering runs at all - which is why [`Extent`] is in physical pixels, to be
//! stated in `phx` rather than in logical units.
//!
//! # Why the scale goes both ways
//!
//! Only the 96-dot family fits a pane at one pixel to the dot. The largest page
//! in the registry is B4 Pro's 1229 x 4134, which has to come down by five or
//! more to be seen whole, and a renderer asked to do that samples one dot in
//! five and drops the other four: a one-dot rule disappears, and a dither turns
//! into a moire of the sampling grid rather than of the label. So a reduction
//! is an area average over the block of dots behind each pixel, rounded so that
//! any ink at all darkens the pixel it lands in. What that pane shows is tone,
//! honestly: whether an individual dot is where it should be is a question for
//! [`Scale::ONE`] or a magnification, and the GUI should always offer 1:1.
//!
//! # What a caller does
//!
//! Take [`Extent`] first and read all three numbers out of that one value - the
//! two the toolkit is given and the length that must be filled. Allocate,
//! fill, hand over, and never keep the buffer: Slint's `SharedVector::detach`
//! is a no-op only at refcount 1, so a buffer already given to an `Image` deep
//! copies on the next write, about two seconds for a 6500 x 6500 one. The
//! buffer it hands back is *black*, because `Rgb8Pixel::default()` is black, so
//! every byte inside the extent is written here, including where a source
//! buffer ran out.

use crate::greymap::GreyMap;
use relabel_proto::BitPlane;

/// Bare paper: white.
const PAPER: u8 = 0xFF;

/// The largest expanded preview this module will describe, in bytes.
///
/// Every page in the vendor registry fits at two pixels to the dot, each model
/// at the longest label it takes. The largest by area is the B4 Pro's 1229 by
/// 4134 - 14.5 MiB of RGB8 at 1:1 and 58.1 MiB at 2x - and the widest head, the
/// ET10's 1600 dots, stops at a 200 mm label, so its 1600 by 1598 is 7.3 MiB
/// and 29.3 MiB. Above this a caller is asking for a buffer no pane can show
/// and no compositor wants to upload, and reducing is the honest answer rather
/// than a larger allocation.
///
/// The cap is also what keeps the arithmetic in range: a length that passes it
/// fits a `usize` on a 32-bit target, so [`Extent`] can compute in `u64` and
/// narrow afterwards.
pub const MAX_BYTES: usize = 64 << 20;

/// How many pixels a printer dot gets, or how many dots share a pixel.
///
/// Whole numbers in both directions, never a ratio: a fractional scale is
/// exactly the artefact this module exists to keep off the screen. Exactly one
/// of [`pixels_per_dot`](Self::pixels_per_dot) and
/// [`dots_per_pixel`](Self::dots_per_pixel) is greater than one.
///
/// A type rather than a bare `u32` so that the clamp below exists once: the
/// size [`Extent`] reports and the picture the expansions draw come from the
/// same number, and no caller can arrive at a buffer that disagrees with what
/// is written into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    up: u32,
    down: u32,
}

impl Scale {
    /// One pixel to the dot.
    pub const ONE: Scale = Scale { up: 1, down: 1 };

    /// The bound on either direction.
    ///
    /// Past 64 pixels to the dot [`MAX_BYTES`] has refused anything larger than
    /// a 96-dot label long before, and past 64 dots to the pixel the longest
    /// page in the registry is already under 65 pixels. The bound is here to
    /// keep the arithmetic and this module's own table in range rather than to
    /// express a policy: it holds the expanded length inside a `u64` and the
    /// table inside 384 KiB.
    pub const MAX: u32 = 64;

    /// Magnify, clamped into `1..=MAX` rather than refused.
    ///
    /// Zero reads as one. A zoom of zero is an uninitialised control or a
    /// fit-to-pane division that rounded down, never a request for a pane with
    /// no pixels in it, and a blank pane is the harder of the two faults to
    /// chase. A GUI must size its image element from
    /// [`Extent`] rather than from what it asked for, or a clamped scale
    /// reintroduces the fractional ratio this module exists to avoid.
    pub fn up(pixels_per_dot: u32) -> Scale {
        Scale {
            up: pixels_per_dot.clamp(1, Scale::MAX),
            down: 1,
        }
    }

    /// Reduce, clamped into `1..=MAX` rather than refused. Zero reads as one,
    /// for the reason [`up`](Self::up) gives.
    pub fn down(dots_per_pixel: u32) -> Scale {
        Scale {
            up: 1,
            down: dots_per_pixel.clamp(1, Scale::MAX),
        }
    }

    /// The largest scale at which a `w` x `h` label fits a pane of `pane_w` x
    /// `pane_h` *physical* pixels.
    ///
    /// This is the question a pane actually asks, and the reason the byte cap
    /// is not a public search: a budget in bytes cannot answer it. The result
    /// always has an [`Extent`], because a label reduced by 64 is at most
    /// 1024 x 1024 dots' worth of pixels whatever it started as.
    ///
    /// A pane smaller than one pixel per dot in one axis only still reduces on
    /// both, because the dot grid has to stay square.
    pub fn for_pane(w: u16, h: u16, pane_w: u32, pane_h: u32) -> Scale {
        if w == 0 || h == 0 {
            return Scale::ONE;
        }
        let (dots_w, dots_h) = (u32::from(w), u32::from(h));
        if dots_w <= pane_w && dots_h <= pane_h {
            let mut s = Scale::up((pane_w / dots_w).min(pane_h / dots_h));
            while s.up > 1 && Extent::new(w, h, s).is_none() {
                s = Scale::up(s.up - 1);
            }
            // A label can fit the pane and still be past the cap at one pixel
            // to the dot, if the pane itself is that large. Then the answer is
            // below 1:1 rather than at it, and the search below finds it.
            if Extent::new(w, h, s).is_some() {
                return s;
            }
        }
        let by_pane = dots_w
            .div_ceil(pane_w.max(1))
            .max(dots_h.div_ceil(pane_h.max(1)));
        let mut s = Scale::down(by_pane);
        while s.down < Scale::MAX && Extent::new(w, h, s).is_none() {
            s = Scale::down(s.down + 1);
        }
        s
    }

    /// Pixels to a dot, at least one.
    pub fn pixels_per_dot(self) -> u32 {
        self.up
    }

    /// Dots to a pixel, at least one.
    pub fn dots_per_pixel(self) -> u32 {
        self.down
    }

    /// The dot a pane pixel came from, for hit testing.
    ///
    /// Here rather than in the GUI so that the division that places a click is
    /// the same one that placed the pixel. The pixel is a *physical* one,
    /// measured from the pane's own origin; a toolkit that reports a click in
    /// logical units has to multiply by the scale factor first.
    pub fn dot_of(self, pixel: u32) -> u32 {
        pixel / self.up * self.down
    }

    /// Where a dot starts in the pane, the inverse of [`dot_of`](Self::dot_of),
    /// for drawing a selection outline around dots the operator picked.
    pub fn pixel_of(self, dot: u16) -> u32 {
        u32::from(dot) / self.down * self.up
    }

    /// Pixels along an axis of `dots` dots.
    ///
    /// A reduction rounds up, so the last pixel of a label whose size is not a
    /// whole number of blocks covers the dots that are left rather than
    /// dropping them.
    fn across(self, dots: u16) -> u32 {
        if self.down > 1 {
            u32::from(dots).div_ceil(self.down)
        } else {
            u32::from(dots) * self.up
        }
    }
}

impl std::fmt::Display for Scale {
    /// `4x` or `1/4`, the two spellings a zoom control needs, in one place.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.down > 1 {
            write!(f, "1/{}", self.down)
        } else {
            write!(f, "{}x", self.up)
        }
    }
}

/// The size of one preview pane: what the toolkit is told, and what must be
/// filled.
///
/// All three numbers come from one value on purpose. A caller that derives the
/// width and height separately from the length can size a buffer that is the
/// right length and the wrong shape - `SharedPixelBuffer::new(h, w)` for a
/// square-ish label is a sheared picture that no byte count can detect - so the
/// two dimensions handed to the toolkit and the length handed to the fill must
/// be read out of the same [`Extent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    /// Physical pixels across.
    pub px_w: u32,
    /// Physical pixels down.
    pub px_h: u32,
    /// Bytes of RGB8: `px_w * px_h * 3`, tightly packed. A Slint pixel buffer
    /// has no row stride, which is why the plane's own right-padded rows cannot
    /// simply be copied across.
    pub len: usize,
}

impl Extent {
    /// The pane for a `w` x `h` label, or `None` past [`MAX_BYTES`].
    ///
    /// Scalars because a pane has to answer "can this be shown at all" from a
    /// [`Target`](crate::target::Target) before anything has been rasterised.
    /// `None` is the signal to reduce further, and
    /// [`Scale::for_pane`] never produces one.
    pub fn new(w: u16, h: u16, scale: Scale) -> Option<Extent> {
        let px_w = scale.across(w);
        let px_h = scale.across(h);
        let len = u64::from(px_w) * u64::from(px_h) * 3;
        (len <= MAX_BYTES as u64).then_some(Extent {
            px_w,
            px_h,
            len: len as usize,
        })
    }

    /// The pane [`plane_to_rgb8`] will fill for this plane.
    pub fn for_plane(plane: &BitPlane, scale: Scale) -> Option<Extent> {
        Extent::new(plane.width(), plane.height(), scale)
    }

    /// The pane [`grey_to_rgb8`] will fill for this grey map.
    pub fn for_grey(grey: &GreyMap, scale: Scale) -> Option<Extent> {
        Extent::new(grey.w, grey.h, scale)
    }
}

/// Draw the 1-bit plane into RGB8: a set bit is black, a clear bit white.
///
/// The plane must be the one [`rasterise_plane`](crate::rasterise::rasterise_plane)
/// returns, in canvas orientation, never one already turned for the head by
/// [`BitPlane::oriented`]. A turned plane draws a pane at right angles to the
/// grey pane beside it, and nothing about a buffer of bytes can notice.
///
/// `out` should be [`Extent::for_plane`] bytes long. A shorter one is filled as
/// far as it reaches, as an exact prefix of what a right-sized buffer would
/// have held, and a longer one keeps its tail: the crate normalises a buffer
/// whose length disagrees with its stated size rather than refusing it -
/// [`GreyMap::from_px`] pads, `binarise` reads missing dots as paper - and here
/// a refusal has nowhere to go, because this runs under a paint callback where
/// a panic closes the window and an error has no recovery beyond not painting.
/// A short fill shows as a black band, since the toolkit's buffer arrives
/// black, which is a fault an operator can see and report.
///
/// Magnifying allocates a table of the 256 packed source bytes expanded to the
/// scale, up to 384 KiB and nothing else; the output buffer is always the
/// caller's.
pub fn plane_to_rgb8(plane: &BitPlane, scale: Scale, out: &mut [u8]) {
    if out.is_empty() || plane.width() == 0 || plane.height() == 0 {
        return;
    }
    if scale.down > 1 {
        reduce_plane(plane, scale.down as usize, out);
    } else {
        magnify_plane(plane, scale.up as usize, out);
    }
}

/// Draw the pre-threshold grey map into RGB8, for the pane that shows the label
/// as it was before it became dots.
///
/// The byte is written through unchanged, into all three channels. [`GreyMap`]
/// is already 0 for fully inked and 255 for bare paper, which is luminance and
/// not ink coverage, so there is nothing to invert - the buffer that runs the
/// other way is [`Coverage`](crate::coverage::Coverage), which never leaves the
/// rasteriser. A byte missing from a short `px` reads as bare paper, the same
/// rule [`GreyMap::at`] gives it. Same buffer rule as [`plane_to_rgb8`].
pub fn grey_to_rgb8(grey: &GreyMap, scale: Scale, out: &mut [u8]) {
    if out.is_empty() || grey.w == 0 || grey.h == 0 {
        return;
    }
    if scale.down > 1 {
        reduce_grey(grey, scale.down as usize, out);
    } else {
        magnify_grey(grey, scale.up as usize, out);
    }
}

/// A fingerprint of the pixels [`plane_to_rgb8`] would draw.
///
/// Equal stamps mean equal pixels; unequal stamps mean nothing, because the
/// padding bits at the end of a row move a stamp without moving a pixel. That
/// one direction is all a caller needs, and it is the direction that must not
/// be wrong.
///
/// Here because the toolkit cannot answer it and the bytes are here. Slint
/// compares image buffers by data pointer rather than by content, so a freshly
/// allocated buffer holding identical pixels always looks new, marks the
/// property dirty and forces a texture upload - and a fresh buffer is what it
/// has to be handed. Stamping is a read of the source at roughly a tenth of the
/// cost of expanding it, and what it buys is the expansion and the upload
/// behind it, on every re-render that leaves the dots where they were: a clock
/// that ticked inside the same minute, a selection, a property that changed
/// nothing. It is deliberately not part of [`Extent`], so a layout pass that
/// only wants the size pays nothing.
///
/// Not a checksum and not asked to be: a collision leaves a pane stale until
/// the next edit, so nothing but the display may depend on it.
pub fn plane_stamp(plane: &BitPlane, scale: Scale) -> u64 {
    stamp(PLANE, plane.width(), plane.height(), scale, plane.bits())
}

/// A fingerprint of the pixels [`grey_to_rgb8`] would draw, with the contract
/// [`plane_stamp`] states. The zone plane is not read, because the grey pane
/// does not draw it.
pub fn grey_stamp(grey: &GreyMap, scale: Scale) -> u64 {
    stamp(GREY, grey.w, grey.h, scale, &grey.px)
}

/// One seed per surface, so a caller that keeps one stamp per pane cannot key
/// one pane on another's and silently skip an upload.
const PLANE: u64 = 0x5052_4c4e_0000_0001;
const GREY: u64 = 0x4752_4559_0000_0002;

/// Magnify the plane: one output row through the table, then copied down.
///
/// The packed byte is the unit of work, for the same reason it is in the PBM
/// goldens. Measured in release against the same rows built a dot at a time
/// through [`BitPlane::get`], table build included, worst of three runs into a
/// warm buffer: a 320 x 96 label is 12 us at 1x against 62 us, 24 us at 4x
/// against 73 us, and 69 us at 8x against 117 us, where the buffer is 5.9 MB
/// and memory is most of the cost either way. The case the table really earns
/// is the large one: the largest page in the registry at 1:1 is 1.2 ms against
/// 7.3 ms. For scale, that same 320 x 96 label rasterises in 216 us, and a
/// freshly allocated buffer - which Slint's refcount rule forces - adds about a
/// third at 1x and doubles it at 8x.
fn magnify_plane(plane: &BitPlane, z: usize, out: &mut [u8]) {
    let row_len = usize::from(plane.width()) * z * 3;
    let table = DotTable::new(z);
    let mut out = out;
    for y in 0..plane.height() {
        if out.is_empty() {
            return;
        }
        let (band, rest) = out.split_at_mut((row_len * z).min(out.len()));
        let (first, copies) = band.split_at_mut(row_len.min(band.len()));
        // `row` is the one accessor on `BitPlane` that panics out of range, and
        // the bound is the plane's own height, so it cannot be reached here. A
        // later edit that takes the height from anywhere else reintroduces it.
        table.fill_row(first, plane.row(y));
        for copy in copies.chunks_mut(row_len) {
            let n = copy.len();
            copy.copy_from_slice(&first[..n]);
        }
        out = rest;
    }
}

/// Magnify the grey map: one run a dot, then the row copied down.
///
/// No table. Grey is monochrome, so all three channels carry the same byte and
/// a dot is a `fill` rather than a lookup and a copy; a 256-entry table of runs
/// was measured against this and is no faster, and slower at 1x. 15 us for a
/// 320 x 96 label at 1x, 58 us at 4x, 102 us at 8x, and 2.1 ms for the largest
/// page in the registry at 1:1.
fn magnify_grey(grey: &GreyMap, z: usize, out: &mut [u8]) {
    let w = usize::from(grey.w);
    let row_len = w * z * 3;
    let mut out = out;
    for y in 0..usize::from(grey.h) {
        if out.is_empty() {
            return;
        }
        let (band, rest) = out.split_at_mut((row_len * z).min(out.len()));
        let (first, copies) = band.split_at_mut(row_len.min(band.len()));
        let src = row_of(&grey.px, w, y);
        let dot = z * 3;
        let whole = src.len().min(first.len() / dot);
        let (full, tail) = first.split_at_mut(whole * dot);
        if z == 1 {
            // Written out rather than left to the `fill` below with a run of
            // three: the compiler does not know the run is one pixel long
            // there, and on the largest page in the registry that difference is
            // 2.0 ms against 6.7 ms.
            for (&v, px) in src.iter().zip(full.as_chunks_mut::<3>().0) {
                px[0] = v;
                px[1] = v;
                px[2] = v;
            }
        } else {
            for (&v, run) in src.iter().zip(full.chunks_exact_mut(dot)) {
                run.fill(v);
            }
        }
        // A dot the buffer had only part of the room for, and then the dots the
        // source did not reach, which are paper. Left unwritten they would be
        // black, because that is the colour a Slint buffer arrives in.
        match src.get(whole) {
            Some(&v) => {
                let n = tail.len().min(dot);
                tail[..n].fill(v);
                tail[n..].fill(PAPER);
            }
            None => tail.fill(PAPER),
        }
        for copy in copies.chunks_mut(row_len) {
            let n = copy.len();
            copy.copy_from_slice(&first[..n]);
        }
        out = rest;
    }
}

/// Reduce the plane: the fraction of the block that is ink, as grey.
///
/// Rounded away from paper, so a block holding a single dot is never written as
/// bare paper - a one-dot rule and a hairline stem come out light rather than
/// disappearing, which is the whole difference between this and the
/// nearest-neighbour a renderer would do. The skip on an empty source byte is
/// what makes it cheap on a label, which is mostly paper: the largest page in
/// the registry costs 1.2 ms at 1/5 and 1.0 ms at 1/6, the scale a 900 x 700
/// pane asks for.
fn reduce_plane(plane: &BitPlane, den: usize, out: &mut [u8]) {
    let (w, h) = (usize::from(plane.width()), usize::from(plane.height()));
    let px_w = w.div_ceil(den);
    let row_len = px_w * 3;
    let mut ink = vec![0u32; px_w];
    // Which block a dot falls in does not depend on the row, so the division
    // that answers it is done once for the whole call rather than once per set
    // dot: on the largest page in the registry that is 1229 divisions instead
    // of 1.7 million.
    let block: Vec<u32> = (0..w).map(|x| (x / den) as u32).collect();
    let mut out = out;
    for top in (0..h).step_by(den) {
        if out.is_empty() {
            return;
        }
        ink.fill(0);
        let rows = den.min(h - top);
        for y in top..top + rows {
            for (i, &b) in plane.row(y as u16).iter().enumerate() {
                if b == 0 {
                    continue;
                }
                for bit in 0..8 {
                    let x = i * 8 + bit;
                    if x >= w {
                        // The row is padded on the right to a whole byte and
                        // those bits are not part of the label.
                        break;
                    }
                    if b & (0x80 >> bit) != 0 {
                        ink[block[x] as usize] += 1;
                    }
                }
            }
        }
        let (row, rest) = out.split_at_mut(row_len.min(out.len()));
        for (x, px) in row.chunks_mut(3).enumerate() {
            let dots = (den.min(w - x * den) * rows) as u32;
            px.fill(PAPER - ((255 * ink[x]).div_ceil(dots)) as u8);
        }
        out = rest;
    }
}

/// Reduce the grey map: the mean coverage of the block.
///
/// Rounded down, towards ink, for the same reason [`reduce_plane`] rounds away
/// from paper: a block with any ink in it must come out darker than bare paper
/// rather than rounding back to it.
fn reduce_grey(grey: &GreyMap, den: usize, out: &mut [u8]) {
    let (w, h) = (usize::from(grey.w), usize::from(grey.h));
    let px_w = w.div_ceil(den);
    let row_len = px_w * 3;
    let mut sum = vec![0u32; px_w];
    let mut out = out;
    for top in (0..h).step_by(den) {
        if out.is_empty() {
            return;
        }
        sum.fill(0);
        let rows = den.min(h - top);
        for y in top..top + rows {
            let src = row_of(&grey.px, w, y);
            // By the block rather than by the dot: `chunks` is the same
            // division done once, and the sum inside one vectorises.
            for (bx, run) in src.chunks(den).enumerate() {
                sum[bx] += run.iter().map(|&v| u32::from(v)).sum::<u32>();
            }
            for x in src.len()..w {
                sum[x / den] += u32::from(PAPER);
            }
        }
        let (row, rest) = out.split_at_mut(row_len.min(out.len()));
        for (x, px) in row.chunks_mut(3).enumerate() {
            let dots = (den.min(w - x * den) * rows) as u32;
            px.fill((sum[x] / dots) as u8);
        }
        out = rest;
    }
}

/// Row `y` of a `w`-wide row-major buffer, cut short where the buffer is.
///
/// A [`GreyMap`] whose `px` is shorter than its own size is not an error - an
/// image decoded to a slightly different size must not become a panic - so the
/// dots it does not carry read as paper.
fn row_of(buf: &[u8], w: usize, y: usize) -> &[u8] {
    let start = y * w;
    let end = start.saturating_add(w).min(buf.len());
    buf.get(start..end).unwrap_or(&[])
}

/// Every value a packed source byte can take, expanded to its eight dots at the
/// scale: `256 * 8 * zoom * 3` bytes, 6 KiB at 1x and 384 KiB at the bound.
///
/// Built per call rather than cached. It costs about 1 us at 1x and 7 us at 8x,
/// under a tenth of the work it saves on a small label and a fiftieth of it on
/// a page, and a cached table buys an invariant that fails silently, since a
/// table built for another scale writes a row of the wrong width, plus a public
/// type with state, a lifetime and a thread-safety question, in a module that is
/// otherwise free functions.
struct DotTable {
    /// Bytes per entry: `8 * zoom * 3`.
    entry: usize,
    data: Vec<u8>,
}

impl DotTable {
    fn new(z: usize) -> DotTable {
        let entry = 8 * z * 3;
        let mut data = Vec::with_capacity(256 * entry);
        for b in 0..256u32 {
            for bit in 0..8 {
                // A set bit is ink, and ink is the same byte three times.
                let v = if b & (0x80 >> bit) != 0 { 0x00 } else { PAPER };
                data.resize(data.len() + z * 3, v);
            }
        }
        DotTable { entry, data }
    }

    /// One output row from one packed source row.
    ///
    /// The last source byte is cut to what is left of the row rather than
    /// copied whole: a width that is not a multiple of eight leaves up to seven
    /// padding bits in that byte, and the plane's rows are padded on the right,
    /// so its entry is wider than the row has room for.
    fn fill_row(&self, dst: &mut [u8], src: &[u8]) {
        for (i, b) in src.iter().enumerate() {
            let at = i * self.entry;
            if at >= dst.len() {
                return;
            }
            let n = self.entry.min(dst.len() - at);
            let from = usize::from(*b) * self.entry;
            dst[at..at + n].copy_from_slice(&self.data[from..from + n]);
        }
    }
}

/// A fingerprint of everything that decides a pane's pixels.
///
/// A word at a time, because it runs over the same bytes the expansion does and
/// must not cost a share of them: on the largest page in the registry a plane
/// stamps in 0.1 ms against 1.3 ms to expand it at 1:1, and its 5.1 MB of
/// coverage in 0.8 ms against 2.1 ms.
fn stamp(seed: u64, w: u16, h: u16, scale: Scale, src: &[u8]) -> u64 {
    let shape =
        u64::from(w) << 48 | u64::from(h) << 32 | u64::from(scale.up) << 16 | u64::from(scale.down);
    let mut s = mix(seed, shape);
    let (words, rest) = src.as_chunks::<8>();
    for &word in words {
        s = mix(s, u64::from_le_bytes(word));
    }
    let mut word = [0u8; 8];
    word[..rest.len()].copy_from_slice(rest);
    mix(mix(s, u64::from_le_bytes(word)), src.len() as u64)
}

#[inline]
fn mix(s: u64, v: u64) -> u64 {
    let s = (s ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    s ^ (s >> 29)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::Dpmm;

    /// A plane with a repeatable mixture of ink and paper, including a dot in
    /// every corner, so an off-by-one in either axis shows.
    fn plane(w: u16, h: u16) -> BitPlane {
        let mut p = BitPlane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let edge = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                if edge || (x * 7 + y * 13) % 5 == 0 {
                    p.set(x, y, true);
                }
            }
        }
        p
    }

    /// A grey map with every level the same dots would have taken.
    fn grey(w: u16, h: u16) -> GreyMap {
        let n = usize::from(w) * usize::from(h);
        let px = (0..n).map(|i| (i * 37 % 256) as u8).collect();
        GreyMap::from_px(w, h, px)
    }

    /// The obvious expansion of a plane, a dot at a time through
    /// [`BitPlane::get`]. The table and the block loop are pinned to this
    /// rather than to written-down bytes, so the agreement survives a rewrite.
    fn reference_plane(plane: &BitPlane, scale: Scale) -> Vec<u8> {
        let e = Extent::for_plane(plane, scale).expect("fixture inside the cap");
        let (z, den) = (scale.pixels_per_dot(), scale.dots_per_pixel());
        let (w, h) = (u32::from(plane.width()), u32::from(plane.height()));
        let mut out = Vec::with_capacity(e.len);
        for py in 0..e.px_h {
            for px in 0..e.px_w {
                let v = if den > 1 {
                    let (mut ink, mut dots) = (0u32, 0u32);
                    for y in py * den..(py * den + den).min(h) {
                        for x in px * den..(px * den + den).min(w) {
                            dots += 1;
                            if plane.get(x as u16, y as u16) {
                                ink += 1;
                            }
                        }
                    }
                    PAPER - ((255 * ink).div_ceil(dots)) as u8
                } else if plane.get((px / z) as u16, (py / z) as u16) {
                    0
                } else {
                    PAPER
                };
                out.extend_from_slice(&[v, v, v]);
            }
        }
        out
    }

    /// The same, for the grey map, through [`GreyMap::at`].
    fn reference_grey(grey: &GreyMap, scale: Scale) -> Vec<u8> {
        let e = Extent::for_grey(grey, scale).expect("fixture inside the cap");
        let (z, den) = (scale.pixels_per_dot(), scale.dots_per_pixel());
        let (w, h) = (u32::from(grey.w), u32::from(grey.h));
        let mut out = Vec::with_capacity(e.len);
        for py in 0..e.px_h {
            for px in 0..e.px_w {
                let v = if den > 1 {
                    let (mut sum, mut dots) = (0u32, 0u32);
                    for y in py * den..(py * den + den).min(h) {
                        for x in px * den..(px * den + den).min(w) {
                            dots += 1;
                            sum += u32::from(grey.at(x as u16, y as u16));
                        }
                    }
                    (sum / dots) as u8
                } else {
                    grey.at((px / z) as u16, (py / z) as u16)
                };
                out.extend_from_slice(&[v, v, v]);
            }
        }
        out
    }

    fn drawn_plane(p: &BitPlane, scale: Scale) -> Vec<u8> {
        let e = Extent::for_plane(p, scale).expect("fixture inside the cap");
        let mut out = vec![0; e.len];
        plane_to_rgb8(p, scale, &mut out);
        out
    }

    fn drawn_grey(g: &GreyMap, scale: Scale) -> Vec<u8> {
        let e = Extent::for_grey(g, scale).expect("fixture inside the cap");
        let mut out = vec![0; e.len];
        grey_to_rgb8(g, scale, &mut out);
        out
    }

    const SIZES: [(u16, u16); 7] = [
        (13, 5),
        (8, 1),
        (1, 8),
        (17, 3),
        (12, 2),
        (64, 9),
        (320, 96),
    ];

    #[test]
    fn a_zoomed_pixel_is_the_dot_it_came_from() {
        for z in [1, 2, 3, 4] {
            let s = Scale::up(z);
            let p = plane(13, 5);
            let g = grey(13, 5);
            assert_eq!(drawn_plane(&p, s), reference_plane(&p, s), "plane at {s}");
            assert_eq!(drawn_grey(&g, s), reference_grey(&g, s), "grey at {s}");
        }
    }

    #[test]
    fn the_table_agrees_with_one_dot_at_a_time() {
        for (w, h) in SIZES {
            for z in [1, 2, 3, 4, 8] {
                let s = Scale::up(z);
                let p = plane(w, h);
                assert_eq!(drawn_plane(&p, s), reference_plane(&p, s), "{w}x{h} at {s}");
            }
        }
    }

    #[test]
    fn a_magnified_grey_map_agrees_with_one_dot_at_a_time() {
        for (w, h) in SIZES {
            for z in [1, 3, 4, 8] {
                let s = Scale::up(z);
                let g = grey(w, h);
                assert_eq!(drawn_grey(&g, s), reference_grey(&g, s), "{w}x{h} at {s}");
            }
        }
    }

    #[test]
    fn a_reduced_pane_agrees_with_a_block_at_a_time() {
        for (w, h) in SIZES {
            for d in [2, 3, 5, 7, 8, 64] {
                let s = Scale::down(d);
                let p = plane(w, h);
                let g = grey(w, h);
                assert_eq!(drawn_plane(&p, s), reference_plane(&p, s), "{w}x{h} at {s}");
                assert_eq!(drawn_grey(&g, s), reference_grey(&g, s), "{w}x{h} at {s}");
            }
        }
    }

    #[test]
    fn ink_is_black_and_paper_is_white_in_both_panes() {
        let mut p = BitPlane::new(2, 1);
        p.set(0, 0, true);
        assert_eq!(
            drawn_plane(&p, Scale::ONE),
            vec![0, 0, 0, 255, 255, 255],
            "a set bit is ink and ink is black"
        );
        let g = GreyMap::from_px(3, 1, vec![0, 128, 255]);
        assert_eq!(
            drawn_grey(&g, Scale::ONE),
            vec![0, 0, 0, 128, 128, 128, 255, 255, 255],
            "a GreyMap byte is luminance already: 0 is ink, and it is not inverted"
        );
    }

    /// The trap RENDER-DESIGN 7.1 sets by asking for `255 - coverage` under the
    /// name `InkMap`. That is right for [`Coverage`] and inverts [`GreyMap`],
    /// and an inverted preview compiles, passes every length assertion and is
    /// wrong only to the eye.
    #[test]
    fn a_grey_map_is_not_inverted_on_the_way_to_the_pane() {
        let g = GreyMap::from_px(2, 1, vec![10, 245]);
        let out = drawn_grey(&g, Scale::up(2));
        assert_eq!(out[0], 10, "the darkest dot stays dark");
        assert_eq!(out[out.len() - 1], 245, "the lightest dot stays light");
    }

    /// Without the cut in `fill_row` the four padding bits of a 12-dot row are
    /// drawn as four more dots, which land at the start of the row below.
    #[test]
    fn set_padding_bits_past_the_width_are_not_drawn() {
        let p = BitPlane::from_bits(12, 2, vec![0x00, 0x1F, 0x00, 0x00])
            .expect("two bytes a row, two rows");
        for z in [1, 2, 3] {
            let s = Scale::up(z);
            let e = Extent::for_plane(&p, s).expect("a 12 x 2 pane");
            let out = drawn_plane(&p, s);
            assert_eq!(e.px_w, 12 * z, "twelve dots wide, not sixteen");
            let last = (e.px_w as usize - 1) * 3;
            assert_eq!(out[last], 0, "the dot at x = 11 is ink");
            let below = (e.px_w * z) as usize * 3;
            assert_eq!(out[below], PAPER, "the row below is paper, not pad bits");
        }
    }

    /// The pane is larger than the label, and the label is larger than the cap:
    /// one pixel to the dot fits the pane and cannot be drawn, so the answer
    /// has to come from below 1:1.
    #[test]
    fn a_label_the_cap_refuses_at_one_to_one_is_reduced_anyway() {
        let (w, h) = (4096u16, 5462u16);
        assert!(
            Extent::new(w, h, Scale::ONE).is_none(),
            "the fixture has to be past the cap at 1:1 to test anything"
        );
        let s = Scale::for_pane(w, h, u32::from(w), u32::from(h));
        assert!(
            Extent::new(w, h, s).is_some(),
            "a pane asked for {s}, which has no extent to draw into"
        );
    }

    #[test]
    fn a_reduction_never_loses_a_dot_to_paper() {
        let mut p = BitPlane::new(64, 64);
        p.set(33, 47, true);
        let out = drawn_plane(&p, Scale::down(64));
        assert_eq!(out.len(), 3, "one pixel");
        assert!(out[0] < PAPER, "one dot in four thousand still darkens");

        let mut g = GreyMap::new(64, 64);
        g.px[47 * 64 + 33] = 0;
        let out = drawn_grey(&g, Scale::down(64));
        assert!(out[0] < PAPER, "one inked dot still darkens the mean");
    }

    #[test]
    fn a_reduction_is_the_ink_in_the_block() {
        let mut p = BitPlane::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                p.set(x, y, x < 2);
            }
        }
        let out = drawn_plane(&p, Scale::down(4));
        assert_eq!(out, vec![127, 127, 127], "half the block is ink");

        let mut all = BitPlane::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                all.set(x, y, true);
            }
        }
        assert_eq!(drawn_plane(&all, Scale::down(4)), vec![0, 0, 0], "all ink");
        assert_eq!(
            drawn_plane(&BitPlane::new(4, 4), Scale::down(4)),
            vec![PAPER; 3],
            "all paper"
        );
    }

    #[test]
    fn the_two_panes_agree_where_the_threshold_does() {
        let (w, h) = (13, 5);
        let g = GreyMap::from_px(
            w,
            h,
            (0..usize::from(w) * usize::from(h))
                .map(|i| if i % 3 == 0 { 0 } else { PAPER })
                .collect(),
        );
        let mut p = BitPlane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                p.set(x, y, g.at(x, y) <= 128);
            }
        }
        for s in [Scale::ONE, Scale::up(3), Scale::down(2)] {
            assert_eq!(
                drawn_plane(&p, s),
                drawn_grey(&g, s),
                "a grey map of only ink and paper draws as its own plane at {s}"
            );
        }
    }

    #[test]
    fn every_cut_of_the_buffer_is_a_prefix() {
        for (w, h) in [(13u16, 5u16), (12, 2), (9, 4), (1, 1)] {
            for s in [Scale::ONE, Scale::up(3), Scale::down(2)] {
                let p = plane(w, h);
                let g = grey(w, h);
                let full_p = drawn_plane(&p, s);
                let full_g = drawn_grey(&g, s);
                for cut in 0..=full_p.len() {
                    let mut out = vec![0; cut];
                    plane_to_rgb8(&p, s, &mut out);
                    assert_eq!(out[..], full_p[..cut], "plane {w}x{h} {s} cut to {cut}");
                    let mut out = vec![0; cut];
                    grey_to_rgb8(&g, s, &mut out);
                    assert_eq!(out[..], full_g[..cut], "grey {w}x{h} {s} cut to {cut}");
                }
            }
        }
    }

    #[test]
    fn a_buffer_longer_than_the_pane_keeps_its_tail() {
        let p = plane(13, 5);
        let s = Scale::up(3);
        let e = Extent::for_plane(&p, s).expect("a 13 x 5 pane");
        let mut out = vec![7u8; e.len + 7];
        plane_to_rgb8(&p, s, &mut out);
        assert_eq!(&out[e.len..], &[7; 7], "past the extent is the caller's");
        assert_eq!(
            &out[..e.len],
            &drawn_plane(&p, s)[..],
            "and the pane is whole"
        );
    }

    #[test]
    fn the_extent_is_the_buffer_the_fills_write() {
        for (w, h) in SIZES {
            for s in [Scale::ONE, Scale::up(3), Scale::up(4), Scale::down(3)] {
                let e = Extent::new(w, h, s).expect("fixture inside the cap");
                assert_eq!(
                    e.len,
                    e.px_w as usize * e.px_h as usize * 3,
                    "three bytes a pixel, no row padding"
                );
                let p = plane(w, h);
                let mut out = vec![0; e.len + 1];
                out[e.len] = 0xA5;
                plane_to_rgb8(&p, s, &mut out);
                assert_eq!(out[e.len], 0xA5, "nothing is written past the extent");
            }
        }
    }

    #[test]
    fn a_scale_of_zero_is_one_pixel_to_the_dot() {
        assert_eq!(Scale::up(0), Scale::ONE);
        assert_eq!(Scale::down(0), Scale::ONE);
        let p = plane(13, 5);
        assert_eq!(drawn_plane(&p, Scale::up(0)), drawn_plane(&p, Scale::ONE));
    }

    #[test]
    fn a_scale_past_the_bound_is_clamped_and_reads_back_clamped() {
        assert_eq!(Scale::up(u32::MAX).pixels_per_dot(), Scale::MAX);
        assert_eq!(Scale::down(u32::MAX).dots_per_pixel(), Scale::MAX);
        let e = Extent::new(8, 1, Scale::up(u32::MAX)).expect("a small pane");
        assert_eq!(e.px_w, 8 * Scale::MAX, "the extent uses the clamped scale");
    }

    /// The table is sized by the scale alone, so it must not be built before
    /// the call knows it has somewhere to put the pixels.
    #[test]
    fn a_scale_nobody_meant_allocates_nothing() {
        let p = plane(320, 96);
        plane_to_rgb8(&p, Scale::up(u32::MAX), &mut []);
        grey_to_rgb8(&grey(320, 96), Scale::up(u32::MAX), &mut []);
    }

    #[test]
    fn a_label_with_no_area_asks_for_no_pixels() {
        for (w, h) in [(0u16, 8u16), (8, 0), (0, 0)] {
            for s in [Scale::ONE, Scale::up(8), Scale::down(8)] {
                let e = Extent::new(w, h, s).expect("nothing is inside the cap");
                assert_eq!(e.len, 0, "{w}x{h} at {s}");
                let mut out = [0xA5u8; 8];
                plane_to_rgb8(&BitPlane::new(w, h), s, &mut out);
                grey_to_rgb8(&GreyMap::new(w, h), s, &mut out);
                assert_eq!(out, [0xA5; 8], "{w}x{h} at {s} writes nothing");
            }
        }
    }

    #[test]
    fn a_short_grey_buffer_reads_as_paper_rather_than_panicking() {
        let mut g = grey(13, 5);
        g.px.truncate(20);
        for s in [Scale::ONE, Scale::up(3), Scale::down(2)] {
            assert_eq!(drawn_grey(&g, s), reference_grey(&g, s), "at {s}");
        }
        let out = drawn_grey(&g, Scale::ONE);
        assert_eq!(
            out[out.len() - 1],
            PAPER,
            "the dots that are missing are paper"
        );
    }

    #[test]
    fn a_page_past_the_cap_has_no_extent() {
        assert!(Extent::new(2000, 4134, Scale::up(3)).is_none(), "74 MiB");
        assert!(Extent::new(1229, 4134, Scale::up(2)).is_some(), "58.2 MiB");
        assert!(Extent::new(u16::MAX, u16::MAX, Scale::ONE).is_none());
    }

    /// Where [`MAX_BYTES`]'s number is written down, from the registry itself
    /// rather than from a page size copied into this file by hand.
    #[test]
    fn every_page_in_the_registry_fits_at_two_pixels_to_the_dot() {
        let mut worst = (0usize, "");
        for m in relabel_proto::models::MODELS {
            let head = m.printhead_px;
            let feed = Dpmm::for_dpi(m.dpi).dots_from_tenths_mm(i32::from(m.max_label_mm) * 10);
            let feed = u16::try_from(feed).unwrap_or(u16::MAX);
            let e = Extent::new(head, feed, Scale::up(2))
                .unwrap_or_else(|| panic!("{} is {head} x {feed} and does not fit", m.name));
            if e.len > worst.0 {
                worst = (e.len, m.name);
            }
        }
        assert_eq!(worst.1, "B4 Pro", "the largest page in the registry");
        assert_eq!(worst.0, 60_968_232, "58.2 MiB at two pixels to the dot");
    }

    #[test]
    fn the_scale_a_pane_asks_for_can_always_be_drawn() {
        for (w, h) in [
            (320u16, 96u16),
            (1229, 4134),
            (1600, 1600),
            // Past the cap at one pixel to the dot, so a pane large enough to
            // hold it whole must still be answered with a reduction.
            (4096, 5462),
            (u16::MAX, u16::MAX),
        ] {
            for (pw, ph) in [
                (900u32, 700u32),
                (100, 100),
                (4000, 4000),
                (6000, 6000),
                (1, 1),
            ] {
                let s = Scale::for_pane(w, h, pw, ph);
                let e = Extent::new(w, h, s)
                    .unwrap_or_else(|| panic!("{w}x{h} in {pw}x{ph} came back as {s}"));
                assert!(
                    e.px_w <= pw.max(1) || s.dots_per_pixel() == Scale::MAX,
                    "{w}x{h} in {pw}x{ph} at {s} is {} across",
                    e.px_w
                );
            }
        }
    }

    #[test]
    fn a_pane_larger_than_the_label_magnifies_to_fill_it() {
        let s = Scale::for_pane(320, 96, 900, 700);
        assert_eq!(s.pixels_per_dot(), 2, "two fits, three does not");
        let s = Scale::for_pane(1229, 4134, 900, 700);
        assert_eq!(s.dots_per_pixel(), 6, "the feed axis is what decides it");
    }

    #[test]
    fn a_pane_pixel_maps_back_to_the_dot_it_came_from() {
        let s = Scale::up(4);
        assert_eq!(s.dot_of(0), 0);
        assert_eq!(s.dot_of(7), 1);
        assert_eq!(s.pixel_of(3), 12);
        let s = Scale::down(4);
        assert_eq!(s.dot_of(3), 12);
        assert_eq!(s.pixel_of(13), 3);
    }

    #[test]
    fn a_scale_says_which_way_it_goes() {
        assert_eq!(Scale::ONE.to_string(), "1x");
        assert_eq!(Scale::up(4).to_string(), "4x");
        assert_eq!(Scale::down(4).to_string(), "1/4");
    }

    #[test]
    fn the_stamp_moves_when_one_dot_does() {
        let p = plane(320, 96);
        let mut q = p.clone();
        q.set(319, 95, !q.get(319, 95));
        assert_eq!(plane_stamp(&p, Scale::ONE), plane_stamp(&p, Scale::ONE));
        assert_ne!(plane_stamp(&p, Scale::ONE), plane_stamp(&q, Scale::ONE));

        let g = grey(13, 5);
        let mut h = g.clone();
        h.px[64] = h.px[64].wrapping_add(1);
        assert_eq!(grey_stamp(&g, Scale::ONE), grey_stamp(&g, Scale::ONE));
        assert_ne!(grey_stamp(&g, Scale::ONE), grey_stamp(&h, Scale::ONE));
    }

    #[test]
    fn a_stamp_belongs_to_one_surface_at_one_scale() {
        let p = plane(8, 8);
        let g = GreyMap::from_px(8, 8, vec![0; 64]);
        let mut seen = vec![
            plane_stamp(&p, Scale::ONE),
            plane_stamp(&p, Scale::up(2)),
            plane_stamp(&p, Scale::down(2)),
            grey_stamp(&g, Scale::ONE),
            grey_stamp(&g, Scale::up(2)),
            grey_stamp(&g, Scale::down(2)),
        ];
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 6, "six panes, six stamps");
    }

    #[test]
    fn two_planes_of_the_same_bytes_and_different_widths_stamp_apart() {
        let a = BitPlane::from_bits(12, 1, vec![0xFF, 0xF0]).expect("two bytes");
        let b = BitPlane::from_bits(13, 1, vec![0xFF, 0xF0]).expect("two bytes");
        assert_ne!(plane_stamp(&a, Scale::ONE), plane_stamp(&b, Scale::ONE));
    }
}
