//! Turning a 1-bit page into row packets.
//!
//! # Orientation and polarity
//!
//! Row-major, **MSB first**: pixel `x = 0` is bit 7 (`0x80`) of byte 0, `x = 7`
//! is bit 0 of byte 0, `x = 8` is bit 7 of byte 1. **A set bit is black** and
//! fires the thermal element. Rows are padded on the **right** with zero bits to
//! a whole byte.
//!
//! The right-padding matters. Both Python reimplementations build the row with
//! `int(bits, 2).to_bytes(..., "big")`, which *right-aligns* the value and so
//! shifts the whole image right by `(8 - w % 8) % 8` pixels whenever the width
//! is not a multiple of eight.
//!
//! # Thresholding is the caller's job
//!
//! [`BitPlane`] is strictly one bit per pixel. `relabel-render` decides how a
//! grey pixel becomes black or white, because that decision wants a dither
//! kernel and a preview, neither of which belong in a protocol crate. (The
//! reference TypeScript treats any pixel that is not exactly `0xFFFFFF` as
//! black, which turns anti-aliased text into blobs.)

use crate::error::ProtoError;
use crate::frame::Frame;
use crate::models::PrintDirection;

/// A one-bit-per-pixel page, MSB-first, rows padded to a whole byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitPlane {
    w: u16,
    h: u16,
    stride: usize,
    bits: Vec<u8>,
}

impl BitPlane {
    /// An all-white plane.
    pub fn new(w: u16, h: u16) -> BitPlane {
        let stride = (w as usize).div_ceil(8);
        BitPlane {
            w,
            h,
            stride,
            bits: vec![0u8; stride * h as usize],
        }
    }

    /// Wrap existing packed rows.
    pub fn from_bits(w: u16, h: u16, bits: Vec<u8>) -> Result<BitPlane, ProtoError> {
        let stride = (w as usize).div_ceil(8);
        let want = stride * h as usize;
        if bits.len() != want {
            return Err(ProtoError::ShortPayload {
                what: "bit plane",
                need: want,
                have: bits.len(),
            });
        }
        Ok(BitPlane { w, h, stride, bits })
    }

    pub fn width(&self) -> u16 {
        self.w
    }
    pub fn height(&self) -> u16 {
        self.h
    }
    pub fn stride(&self) -> usize {
        self.stride
    }
    pub fn bits(&self) -> &[u8] {
        &self.bits
    }

    #[inline]
    pub fn get(&self, x: u16, y: u16) -> bool {
        if x >= self.w || y >= self.h {
            return false;
        }
        let byte = y as usize * self.stride + (x as usize >> 3);
        self.bits[byte] & (0x80 >> (x & 7)) != 0
    }

    #[inline]
    pub fn set(&mut self, x: u16, y: u16, black: bool) {
        if x >= self.w || y >= self.h {
            return;
        }
        let byte = y as usize * self.stride + (x as usize >> 3);
        let mask = 0x80 >> (x & 7);
        if black {
            self.bits[byte] |= mask;
        } else {
            self.bits[byte] &= !mask;
        }
    }

    #[inline]
    pub fn row(&self, y: u16) -> &[u8] {
        let start = y as usize * self.stride;
        &self.bits[start..start + self.stride]
    }

    /// Rotate 90° clockwise, which is what a [`PrintDirection::Left`] model needs.
    ///
    /// `dst(r, c) = src(x = r, y = H - 1 - c)`, so the source *height* becomes
    /// the printhead axis and a black pixel at the source's top-left lands at
    /// the output's top-right.
    ///
    /// The user never does this. relabel's canvas is always authored in the
    /// orientation the printed label reads - for the 96-dot D11 family that is a
    /// landscape strip whose height is the 12 mm label width - and the encoder
    /// applies the transform.
    pub fn rotate_cw(&self) -> BitPlane {
        let mut out = BitPlane::new(self.h, self.w);
        for r in 0..self.w {
            for c in 0..self.h {
                if self.get(r, self.h - 1 - c) {
                    out.set(c, r, true);
                }
            }
        }
        out
    }

    /// Apply whatever rotation the model's print direction calls for.
    pub fn oriented(&self, direction: PrintDirection) -> BitPlane {
        match direction {
            PrintDirection::Top => self.clone(),
            PrintDirection::Left => self.rotate_cw(),
        }
    }
}

/// How the three counter bytes in a row packet are computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CountsMode {
    /// Split or total, chosen per row by the rule the firmware is believed to
    /// use. The default, and what every reference implementation that computes
    /// them at all does.
    #[default]
    Auto,
    /// Always the little-endian total, never the thirds.
    ForceTotal,
    /// All three zero.
    ///
    /// Three independent implementations hardcode zeros and report successful
    /// prints, and one hardware writeup claims a B21S firmware validates them.
    /// The claim is unresolved, so relabel computes them properly by default -
    /// three popcounts against a firmware that might check is the cheapest
    /// possible insurance - and this exists so a user with a misbehaving model
    /// can bisect it.
    ForceZero,
}

/// What the encoder needs to know about the destination printer.
#[derive(Debug, Clone, Copy)]
pub struct HeadSpec {
    /// Printhead width in dots. **Zero means unknown**, which forces total
    /// counting mode - the safe branch.
    pub printhead_px: u16,
    pub direction: PrintDirection,
    pub counts: CountsMode,
    /// Whether to use the `0x83` indexed-row packet for very sparse rows.
    ///
    /// Off by default. The saving is negligible, one driver never uses it and
    /// prints correctly, and the reference implementation's own source carries
    /// the comment that the printer *powers off* if a `0x83` row has more than
    /// six black pixels. A negligible gain against that hazard is not a trade
    /// worth making by default.
    pub indexed_rows: bool,
    /// Largest run of identical rows to collapse into one packet.
    ///
    /// The repeat field is one byte. The reference implementation never caps it,
    /// so a 300-row blank label silently emits `repeat = 44`. Whether firmware
    /// handles a full 255-row run is untested, so this is a constant that can be
    /// lowered if a model misbehaves.
    pub max_run: u8,
}

impl HeadSpec {
    pub fn new(printhead_px: u16, direction: PrintDirection) -> HeadSpec {
        HeadSpec {
            printhead_px,
            direction,
            counts: CountsMode::Auto,
            indexed_rows: false,
            max_run: 255,
        }
    }
}

/// A page ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPage {
    /// Rows along the feed axis - the first `SetPageSize` field.
    pub rows: u16,
    /// Dots across the printhead axis - the second `SetPageSize` field.
    pub cols: u16,
    pub frames: Vec<Frame>,
}

impl EncodedPage {
    /// Total bytes on the wire, for the time estimate the print pane shows.
    pub fn wire_bytes(&self) -> usize {
        self.frames
            .iter()
            .map(|f| f.data.len() + crate::frame::STANDARD_OVERHEAD)
            .sum()
    }
}

/// Encode a page into row packets.
///
/// The plane is rotated first if the model needs it, so `plane` is always in
/// the orientation the label reads.
pub fn encode_page(plane: &BitPlane, head: &HeadSpec) -> Result<EncodedPage, ProtoError> {
    let plane = plane.oriented(head.direction);
    let stride = plane.stride();
    if stride == 0 || plane.height() == 0 {
        return Err(ProtoError::InvalidValue {
            what: "empty page",
            value: 0,
        });
    }
    // The printhead-axis dimension the printer is told about is the image width
    // clamped to the head: the vendor's own client sends
    // `min(image_width, printhead_px)` with a per-model constant, and columns
    // beyond the head are simply not fired.
    //
    // That clamp is what reconciles the two contradictory readings in the
    // reverse-engineering literature - one client sends the label width, another
    // sends the printhead width, and both are right for the cases they were
    // tested on. Here it is an error rather than a clamp: relabel-render knows
    // the head width and is expected to fit the canvas to it, so an oversized
    // plane is a caller bug, and silently truncating a label is exactly the
    // failure this crate exists to avoid.
    let cols = (stride * 8) as u16;
    if head.printhead_px != 0 && cols > head.printhead_px {
        return Err(ProtoError::PageTooWide {
            cols,
            printhead_px: head.printhead_px,
        });
    }
    // The row packet is six header bytes plus the packed row, and the length
    // field is one byte. Nothing in the catalogue reaches this - the widest head
    // is ET10 at 1600 dots, giving 200 + 6 - but a caller can hand us any plane.
    if stride + 6 > crate::frame::MAX_PAYLOAD {
        return Err(ProtoError::PayloadTooLong(stride + 6));
    }

    let mut frames = Vec::new();
    let mut y = 0u16;
    while y < plane.height() {
        let row = plane.row(y);
        // Collapse a run of byte-identical rows into one packet.
        let mut run = 1u16;
        while y + run < plane.height() && u16::from(head.max_run) > run && plane.row(y + run) == row
        {
            run += 1;
        }
        let repeat = run as u8;

        if row.iter().all(|b| *b == 0) {
            // 0x84: [row u16 BE][repeat u8]
            let mut data = Vec::with_capacity(3);
            data.extend_from_slice(&y.to_be_bytes());
            data.push(repeat);
            frames.push(Frame::new(0x84, data));
        } else if head.indexed_rows && popcount(row) <= 6 {
            // 0x83: the same six header bytes, then each black pixel's x as a
            // big-endian u16.
            let mut data = Vec::new();
            data.extend_from_slice(&y.to_be_bytes());
            let (c1, c2, c3) = counters(row, head.printhead_px, head.counts);
            data.extend_from_slice(&[c1, c2, c3, repeat]);
            for x in 0..plane.width() {
                if plane.get(x, y) {
                    data.extend_from_slice(&x.to_be_bytes());
                }
            }
            frames.push(Frame::new(0x83, data));
        } else {
            // 0x85: [row u16 BE][C1][C2][C3][repeat u8][packed row]
            let mut data = Vec::with_capacity(6 + stride);
            data.extend_from_slice(&y.to_be_bytes());
            let (c1, c2, c3) = counters(row, head.printhead_px, head.counts);
            data.extend_from_slice(&[c1, c2, c3, repeat]);
            data.extend_from_slice(row);
            frames.push(Frame::new(0x85, data));
        }
        y += run;
    }

    Ok(EncodedPage {
        rows: plane.height(),
        cols,
        frames,
    })
}

#[inline]
fn popcount(bytes: &[u8]) -> u32 {
    bytes.iter().map(|b| b.count_ones()).sum()
}

/// The three counter bytes.
///
/// Two mutually exclusive encodings, selected by comparing the row's byte width
/// against a third of the **printhead** - not against the row itself.
///
/// In split mode each byte counts the black pixels in one third of the head. In
/// total mode `C1` is zero and the 16-bit total sits **little-endian** across
/// `C2` and `C3` - the one place in the whole protocol where a multi-byte field
/// is not big-endian.
///
/// Because `chunk` is a floor division by 24, any model whose printhead is not a
/// multiple of 24 always lands in total mode at full width.
fn counters(row: &[u8], printhead_px: u16, mode: CountsMode) -> (u8, u8, u8) {
    if mode == CountsMode::ForceZero {
        return (0, 0, 0);
    }
    let stride = row.len();
    let chunk = (printhead_px as usize) / 8 / 3;
    let split = mode != CountsMode::ForceTotal && chunk > 0 && stride <= 3 * chunk;
    if split {
        let a = chunk.min(stride);
        let b = (2 * chunk).min(stride);
        let c = (3 * chunk).min(stride);
        // Saturating because a very wide head in split mode could in principle
        // exceed 255 black pixels in one third. No catalogue model does - the
        // widest that stays in split mode is 384 dots, giving at most 128 - but
        // wrapping would be a silent corruption where clamping is merely wrong
        // in a way the printer already tolerates.
        (
            popcount(&row[..a]).min(255) as u8,
            popcount(&row[a..b]).min(255) as u8,
            popcount(&row[b..c]).min(255) as u8,
        )
    } else {
        let total = popcount(row).min(u32::from(u16::MAX)) as u16;
        (0x00, (total & 0xFF) as u8, (total >> 8) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u16, h: u16) -> BitPlane {
        let mut p = BitPlane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                p.set(x, y, true);
            }
        }
        p
    }

    #[test]
    fn msb_first_and_right_padded() {
        let mut p = BitPlane::new(12, 1);
        p.set(0, 0, true);
        assert_eq!(p.row(0), &[0x80, 0x00], "x=0 is bit 7 of byte 0");
        p.set(0, 0, false);
        p.set(8, 0, true);
        assert_eq!(p.row(0), &[0x00, 0x80], "x=8 is bit 7 of byte 1");
        p.set(8, 0, false);
        p.set(11, 0, true);
        // The four pad bits are on the right and stay clear - the bug both
        // Python ports have is that this ends up as 0x00,0x01.
        assert_eq!(p.row(0), &[0x00, 0x10]);
    }

    /// The worked example from the protocol notes: a 240-px full-black row on a
    /// 384-px head gives 128/112/0, not 80/80/80. The thirds are thirds of the
    /// *head*, not of the row.
    #[test]
    fn split_counters_are_thirds_of_the_head() {
        let row = vec![0xFFu8; 30]; // 240 px
        assert_eq!(counters(&row, 384, CountsMode::Auto), (0x80, 0x70, 0x00));
    }

    #[test]
    fn total_mode_puts_the_count_little_endian_in_c2_c3() {
        // 1600-dot head: chunk = 66, 3*66 = 198 < stride 200, so total mode.
        let row = vec![0xFFu8; 200];
        let (c1, c2, c3) = counters(&row, 1600, CountsMode::Auto);
        assert_eq!(c1, 0x00);
        assert_eq!(u16::from(c2) | (u16::from(c3) << 8), 1600);
    }

    #[test]
    fn unknown_head_takes_the_safe_branch() {
        let row = vec![0xFFu8; 12];
        let (c1, c2, c3) = counters(&row, 0, CountsMode::Auto);
        assert_eq!(
            (c1, c2, c3),
            (0x00, 96, 0),
            "96 black pixels, little-endian"
        );
    }

    #[test]
    fn blank_rows_collapse_into_one_empty_packet() {
        let plane = BitPlane::new(96, 200);
        let page = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Top)).unwrap();
        assert_eq!(page.rows, 200);
        assert_eq!(page.cols, 96);
        assert_eq!(page.frames.len(), 1, "one run of 200 identical blank rows");
        assert_eq!(page.frames[0].id, 0x84);
        assert_eq!(page.frames[0].data, vec![0x00, 0x00, 200]);
    }

    #[test]
    fn runs_are_capped_so_the_repeat_byte_cannot_wrap() {
        let plane = BitPlane::new(96, 300);
        let page = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Top)).unwrap();
        // Without a cap this is one packet claiming repeat = 44.
        assert_eq!(page.frames.len(), 2);
        assert_eq!(page.frames[0].data[2], 255);
        assert_eq!(page.frames[1].data[2], 45);
        let total: u32 = page.frames.iter().map(|f| u32::from(f.data[2])).sum();
        assert_eq!(total, 300);
    }

    #[test]
    fn row_index_is_the_first_row_of_the_run() {
        let mut plane = BitPlane::new(96, 10);
        for x in 0..96 {
            plane.set(x, 7, true);
        }
        let page = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Top)).unwrap();
        // rows 0..6 blank, row 7 black, rows 8..9 blank
        assert_eq!(page.frames.len(), 3);
        assert_eq!(page.frames[0].data[..2], [0x00, 0x00]);
        assert_eq!(page.frames[0].data[2], 7);
        assert_eq!(page.frames[1].id, 0x85);
        assert_eq!(page.frames[1].data[..2], [0x00, 0x07], "row 7, big-endian");
        assert_eq!(page.frames[2].data[..2], [0x00, 0x08]);
        assert_eq!(page.frames[2].data[2], 2);
    }

    #[test]
    fn rotation_sends_the_top_left_pixel_to_the_top_right() {
        let mut src = BitPlane::new(4, 3);
        src.set(0, 0, true);
        let out = src.rotate_cw();
        assert_eq!(out.width(), 3);
        assert_eq!(out.height(), 4);
        assert!(out.get(2, 0), "top-left becomes top-right");
        assert!(!out.get(0, 0));
    }

    #[test]
    fn left_direction_swaps_the_page_axes() {
        // A D11 label authored as a 200-dot-long, 96-dot-tall strip.
        let plane = solid(200, 96);
        let page = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Left)).unwrap();
        assert_eq!(page.cols, 96, "the 96-dot head is the short axis");
        assert_eq!(page.rows, 200, "the long axis runs along the feed");
    }

    #[test]
    fn row_packet_layout_is_six_header_bytes_then_the_row() {
        let plane = solid(96, 1);
        let page = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Top)).unwrap();
        let f = &page.frames[0];
        assert_eq!(f.id, 0x85);
        assert_eq!(f.data.len(), 6 + 12);
        assert_eq!(&f.data[..2], &[0x00, 0x00]);
        assert_eq!(f.data[5], 1, "repeat of 1 means one row");
        assert_eq!(&f.data[6..], &[0xFFu8; 12]);
        // A 96-dot head has chunk = 4, so the thirds are 32 px each.
        assert_eq!(&f.data[2..5], &[32, 32, 32]);
    }

    #[test]
    fn indexed_rows_are_off_by_default() {
        let mut plane = BitPlane::new(96, 1);
        plane.set(3, 0, true);
        let mut head = HeadSpec::new(96, PrintDirection::Top);
        assert_eq!(encode_page(&plane, &head).unwrap().frames[0].id, 0x85);
        head.indexed_rows = true;
        let page = encode_page(&plane, &head).unwrap();
        assert_eq!(page.frames[0].id, 0x83);
        assert_eq!(&page.frames[0].data[6..], &[0x00, 0x03], "x position, BE");
    }

    #[test]
    fn every_frame_fits_the_length_byte() {
        // The widest head in the catalogue.
        let plane = solid(1600, 4);
        let page = encode_page(&plane, &HeadSpec::new(1600, PrintDirection::Top)).unwrap();
        for f in &page.frames {
            assert!(f.encode().is_ok(), "{} bytes", f.data.len());
        }
    }

    #[test]
    fn a_page_wider_than_the_head_is_refused_not_truncated() {
        let plane = solid(200, 4);
        let err = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Top)).unwrap_err();
        assert_eq!(
            err,
            ProtoError::PageTooWide {
                cols: 200,
                printhead_px: 96
            }
        );
        // An unknown head cannot clamp, so it must not refuse either.
        assert!(encode_page(&plane, &HeadSpec::new(0, PrintDirection::Top)).is_ok());
    }

    #[test]
    fn cols_is_the_image_width_not_the_head_width() {
        // A 64-dot-wide label on a 96-dot head reports 64, not 96.
        let plane = solid(64, 4);
        let page = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Top)).unwrap();
        assert_eq!(page.cols, 64);
    }

    #[test]
    fn force_zero_is_available_for_bisecting() {
        let plane = solid(96, 1);
        let mut head = HeadSpec::new(96, PrintDirection::Top);
        head.counts = CountsMode::ForceZero;
        let page = encode_page(&plane, &head).unwrap();
        assert_eq!(&page.frames[0].data[2..5], &[0, 0, 0]);
    }
}
