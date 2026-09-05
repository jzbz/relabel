//! Turning a byte stream back into frames.
//!
//! Neither transport delivers frame-aligned reads. A single BLE notification can
//! carry several whole frames back to back - `55554a01044faaaa5555f60101f6aaaa`
//! is two - and a single frame can equally arrive split across two
//! notifications. Serial has no framing at all. So every inbound byte goes
//! through here.
//!
//! Because the payload may contain `55 55` and `AA AA` freely (see
//! [`crate::frame`]), this cannot work by scanning for the magic bytes. It reads
//! the length field, takes exactly that many bytes, and verifies the tail and
//! checksum. Scanning happens only on the recovery path, when a frame has
//! already failed to verify and the stream is known to be desynchronised.
//!
//! The reference implementations all get some part of this wrong: the two Python
//! clients spin forever on a partial frame (`while len(buf) > 4` with no
//! `break`), and one BLE client assumes one notification equals one packet.

use crate::error::ProtoError;
use crate::frame::{Frame, FrameShape, HEAD, MAX_FRAME, Probe, is_unparseable};

/// How much unparsed input to hold before discarding the oldest.
///
/// A frame is at most 262 bytes, so this is room for well over a hundred
/// bundled frames. The cap exists only so a wedged or hostile stream cannot
/// grow the buffer without bound; in normal operation it is never approached.
const DEFAULT_LIMIT: usize = 64 * 1024;

/// Something the accumulator recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A complete, checksum-verified frame.
    Frame(Frame),
    /// A firmware-transfer frame was recognised and stepped over.
    ///
    /// relabel does not implement firmware upgrade, but the decoder still has
    /// to know these opcodes: their length field is at a different offset and
    /// their trailer is a 4-byte CRC, so without recognising them the stream
    /// would desynchronise on the first one.
    SkippedFirmware { id: u8, bytes: usize },
    /// Bytes that could not be interpreted were dropped to resynchronise.
    ///
    /// Surfaced rather than swallowed because it is the signature of a real
    /// problem - a transport dropping writes, or a printer speaking a framing
    /// this decoder does not know - and it belongs in the protocol log.
    Desync { bytes: usize, reason: DesyncReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesyncReason {
    /// The stream did not begin with `55 55`.
    NoHead,
    /// The length field pointed somewhere that was not `AA AA`.
    BadTail,
    /// The frame verified structurally but its XOR was wrong.
    BadChecksum,
    /// A compression opcode, whose framing is undocumented (see
    /// [`crate::frame::is_unparseable`]).
    UnparseableOpcode(u8),
    /// The buffer hit its cap with no complete frame in it.
    Overflow,
}

/// A streaming frame reassembler.
///
/// Feed it whatever the transport hands over with [`push`](Self::push), then
/// drain with [`next`](Self::next) until it returns `None`.
#[derive(Debug)]
pub struct Accumulator {
    buf: Vec<u8>,
    limit: usize,
    /// Bytes dropped since the last `Desync` was emitted, so a run of garbage
    /// becomes one event rather than one per byte.
    pending_drop: usize,
    pending_reason: Option<DesyncReason>,
    /// A decoded event held back so the `Desync` that preceded it in the byte
    /// stream is reported first. At most one, for at most one call to `next`.
    pending_event: Option<Event>,
}

impl Default for Accumulator {
    fn default() -> Self {
        Accumulator::new()
    }
}

impl Accumulator {
    pub fn new() -> Self {
        Accumulator::with_limit(DEFAULT_LIMIT)
    }

    pub fn with_limit(limit: usize) -> Self {
        Accumulator {
            // One frame's worth up front covers the overwhelmingly common case
            // of a read that holds exactly one reply.
            buf: Vec::with_capacity(MAX_FRAME),
            limit: limit.max(MAX_FRAME),
            pending_drop: 0,
            pending_reason: None,
            pending_event: None,
        }
    }

    /// Bytes currently held but not yet forming a complete frame.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Discard everything held. Called on reconnect, so a half-frame from the
    /// previous session cannot corrupt the first frame of the new one.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.pending_drop = 0;
        self.pending_reason = None;
        self.pending_event = None;
    }

    /// Add raw bytes from the transport.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > self.limit {
            // Drop from the front: the newest bytes are the ones most likely to
            // form a usable frame. This is unreachable in normal operation.
            let excess = self.buf.len() - self.limit;
            self.buf.drain(..excess);
            self.note_drop(excess, DesyncReason::Overflow);
        }
    }

    /// Take the next recognised item, if one is complete.
    ///
    /// Returns `None` when the buffer holds only a partial frame - that is the
    /// signal to wait for more input, not an error.
    ///
    /// Named `pop` rather than `next` because this is not an iterator: `None`
    /// means "not yet", not "finished", and wrapping it in `Iterator` would
    /// invite a `for` loop that stops permanently on the first partial read.
    pub fn pop(&mut self) -> Option<Event> {
        // A frame decoded on the previous call, held back behind its desync.
        if let Some(ev) = self.pending_event.take() {
            return Some(ev);
        }
        loop {
            // A run of dropped bytes is reported once, at the point the stream
            // resynchronises or runs out.
            if self.buf.len() < 2 {
                return self.flush_drop();
            }
            if self.buf[0..2] != HEAD {
                self.resync(DesyncReason::NoHead);
                continue;
            }
            if self.buf.len() >= 3 && is_unparseable(self.buf[2]) {
                let id = self.buf[2];
                self.resync(DesyncReason::UnparseableOpcode(id));
                continue;
            }
            match FrameShape::probe(&self.buf) {
                Ok(Probe::NeedMore(_)) => return self.flush_drop(),
                Ok(Probe::Frame { shape, total }) => {
                    let ev = match Frame::decode(&self.buf[..total]) {
                        Ok((frame, used)) => {
                            debug_assert_eq!(used, total);
                            match shape {
                                FrameShape::Standard => Event::Frame(frame),
                                FrameShape::Firmware => Event::SkippedFirmware {
                                    id: frame.id,
                                    bytes: total,
                                },
                            }
                        }
                        Err(ProtoError::BadTail(_)) => {
                            self.resync(DesyncReason::BadTail);
                            continue;
                        }
                        Err(ProtoError::BadChecksum { .. }) => {
                            self.resync(DesyncReason::BadChecksum);
                            continue;
                        }
                        // probe() already established the head is right and the
                        // whole frame is present, so no other variant is
                        // reachable. Resynchronise rather than panic: this code
                        // is fed by a device we do not control.
                        Err(_) => {
                            self.resync(DesyncReason::BadTail);
                            continue;
                        }
                    };
                    self.buf.drain(..total);
                    // A pending drop run is reported before the good frame, so
                    // the log reads in the order the bytes arrived.
                    if let Some(drop) = self.flush_drop() {
                        self.pending_event = Some(ev);
                        return Some(drop);
                    }
                    return Some(ev);
                }
                Err(_) => {
                    self.resync(DesyncReason::NoHead);
                    continue;
                }
            }
        }
    }

    /// Step over one byte and record why.
    fn resync(&mut self, reason: DesyncReason) {
        self.buf.drain(..1);
        self.note_drop(1, reason);
    }

    fn note_drop(&mut self, bytes: usize, reason: DesyncReason) {
        self.pending_drop += bytes;
        // The first reason in a run is the informative one: later bytes are
        // dropped because the stream is already lost, not for a new reason.
        self.pending_reason.get_or_insert(reason);
    }

    fn flush_drop(&mut self) -> Option<Event> {
        let bytes = core::mem::take(&mut self.pending_drop);
        let reason = self.pending_reason.take()?;
        if bytes == 0 {
            return None;
        }
        Some(Event::Desync { bytes, reason })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(acc: &mut Accumulator) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(e) = acc.pop() {
            out.push(e);
        }
        out
    }

    #[test]
    fn two_frames_in_one_read() {
        // The documented example: 55554a01044faaaa5555f60101f6aaaa.
        let bytes = [
            0x55, 0x55, 0x4a, 0x01, 0x04, 0x4f, 0xaa, 0xaa, 0x55, 0x55, 0xf6, 0x01, 0x01, 0xf6,
            0xaa, 0xaa,
        ];
        let mut acc = Accumulator::new();
        acc.push(&bytes);
        let got = frames(&mut acc);
        assert_eq!(
            got,
            vec![
                Event::Frame(Frame::new(0x4a, [0x04])),
                Event::Frame(Frame::new(0xf6, [0x01])),
            ]
        );
    }

    #[test]
    fn one_frame_split_across_reads() {
        let bytes = Frame::new(0x1A, [0x01]).encode().unwrap();
        let mut acc = Accumulator::new();
        for chunk in bytes.chunks(1) {
            // Every prefix but the last must yield nothing at all.
            acc.push(chunk);
        }
        assert_eq!(
            frames(&mut acc),
            vec![Event::Frame(Frame::new(0x1A, [0x01]))]
        );
    }

    #[test]
    fn partial_frame_yields_nothing_and_does_not_spin() {
        let bytes = Frame::new(0x1A, [0x01]).encode().unwrap();
        let mut acc = Accumulator::new();
        acc.push(&bytes[..5]);
        assert_eq!(acc.pop(), None);
        assert_eq!(acc.buffered(), 5);
    }

    #[test]
    fn payload_full_of_magic_bytes_is_not_split() {
        // The case that breaks every scan-for-magic decoder.
        let f = Frame::new(0x85, vec![0x55, 0x55, 0xAA, 0xAA, 0x55, 0x55, 0xAA, 0xAA]);
        let mut acc = Accumulator::new();
        acc.push(&f.encode().unwrap());
        assert_eq!(frames(&mut acc), vec![Event::Frame(f)]);
    }

    #[test]
    fn resynchronises_after_garbage() {
        let good = Frame::new(0x1A, [0x01]).encode().unwrap();
        let mut acc = Accumulator::new();
        acc.push(&[0x00, 0x11, 0x22]);
        acc.push(&good);
        let got = frames(&mut acc);
        assert_eq!(
            got,
            vec![
                Event::Desync {
                    bytes: 3,
                    reason: DesyncReason::NoHead
                },
                Event::Frame(Frame::new(0x1A, [0x01])),
            ]
        );
    }

    #[test]
    fn bad_checksum_desyncs_rather_than_yielding_a_frame() {
        let mut bytes = Frame::new(0x1A, [0x01]).encode().unwrap();
        bytes[5] ^= 0xFF;
        bytes.extend_from_slice(&Frame::new(0xF4, [0x01]).encode().unwrap());
        let mut acc = Accumulator::new();
        acc.push(&bytes);
        let got = frames(&mut acc);
        // The corrupt frame is dropped byte by byte until the good one is found.
        assert!(matches!(got[0], Event::Desync { .. }));
        assert_eq!(got[1], Event::Frame(Frame::new(0xF4, [0x01])));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn firmware_frame_is_stepped_over_not_misparsed() {
        // 12 bytes of overhead, length at offset 5.
        let mut fw = vec![0x55, 0x55, 0x9A, 0x00, 0x01, 0x02, 0xDE, 0xAD];
        fw.extend_from_slice(&[0, 0, 0, 0]); // CRC-32, not verified
        fw.extend_from_slice(&[0xAA, 0xAA]);
        assert_eq!(fw.len(), 14);
        let mut acc = Accumulator::new();
        acc.push(&fw);
        acc.push(&Frame::new(0xF4, [0x01]).encode().unwrap());
        let got = frames(&mut acc);
        assert_eq!(
            got,
            vec![
                Event::SkippedFirmware {
                    id: 0x9A,
                    bytes: 14
                },
                Event::Frame(Frame::new(0xF4, [0x01])),
            ]
        );
    }

    #[test]
    fn compression_opcode_desyncs_by_design() {
        let mut acc = Accumulator::new();
        acc.push(&[0x55, 0x55, 0x87, 0x00, 0x04]);
        acc.push(&Frame::new(0xF4, [0x01]).encode().unwrap());
        let got = frames(&mut acc);
        assert!(matches!(
            got[0],
            Event::Desync {
                reason: DesyncReason::UnparseableOpcode(0x87),
                ..
            }
        ));
        assert_eq!(got.last(), Some(&Event::Frame(Frame::new(0xF4, [0x01]))));
    }

    #[test]
    fn buffer_is_bounded() {
        let mut acc = Accumulator::with_limit(1024);
        for _ in 0..100 {
            acc.push(&[0x00; 512]);
        }
        assert!(acc.buffered() <= 1024);
    }

    #[test]
    fn reset_clears_a_half_frame() {
        let bytes = Frame::new(0x1A, [0x01]).encode().unwrap();
        let mut acc = Accumulator::new();
        acc.push(&bytes[..4]);
        acc.reset();
        acc.push(&bytes);
        assert_eq!(
            frames(&mut acc),
            vec![Event::Frame(Frame::new(0x1A, [0x01]))]
        );
    }
}
