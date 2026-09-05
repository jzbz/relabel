//! The wire frame: encoding one, and recognising one in a slice.
//!
//! ```text
//!   0     1     2     3     4 .. 3+LEN     4+LEN   5+LEN 6+LEN
//! [0x55][0x55][CMD ][LEN ][DATA … LEN B ][ XOR ][0xAA][0xAA]
//! ```
//!
//! `LEN` counts `DATA` only. `XOR` covers `CMD`, `LEN` and `DATA`, and excludes
//! the head and the tail. Multi-byte fields inside `DATA` are big-endian, with
//! exactly one exception - the bitmap black-pixel total in [`crate::image`],
//! which is little-endian across two of three bytes.
//!
//! # There is no escaping
//!
//! Payload bytes are freely `0x55` and `0xAA`. A bitmap row of alternating
//! pixels is literally `55 55 55 …`, an RFID UUID can contain either byte, and
//! the checksum byte itself can be `0xAA`. **Framing is therefore driven by the
//! length field and never by scanning for the magic bytes.** A decoder that
//! splits its input on `55 55` corrupts every RFID response, every serial
//! number and every dense bitmap row. This is the single most consequential
//! fact about the protocol, and the reason [`crate::accumulator`] exists in the
//! shape it does.

use crate::error::ProtoError;

pub const HEAD: [u8; 2] = [0x55, 0x55];
pub const TAIL: [u8; 2] = [0xAA, 0xAA];

/// head(2) + cmd(1) + len(1) + xor(1) + tail(2).
pub const STANDARD_OVERHEAD: usize = 7;
/// head(2) + cmd(1) + index(2) + len(1) + crc32(4) + tail(2).
pub const FIRMWARE_OVERHEAD: usize = 12;

/// The largest `DATA` a frame can carry, because `LEN` is one byte.
pub const MAX_PAYLOAD: usize = u8::MAX as usize;
/// The largest complete frame, for sizing a transport's write buffer.
pub const MAX_FRAME: usize = MAX_PAYLOAD + STANDARD_OVERHEAD;

/// One protocol frame: a command byte and its payload.
///
/// Deliberately not split into request and response types. The same struct
/// crosses the wire both ways and the *interpretation* of `id` differs by
/// direction - see [`crate::command::RequestId`] and
/// [`crate::command::ResponseId`], which are two distinct enums precisely
/// because several response ids reuse request opcode values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: u8,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(id: u8, data: impl Into<Vec<u8>>) -> Self {
        Frame {
            id,
            data: data.into(),
        }
    }

    /// The one-byte payload that most commands use to mean "do it".
    ///
    /// Worth knowing when reading hex by eye: any frame of this shape has a
    /// checksum equal to its own command byte, because `n ^ 1 ^ 1 == n`.
    pub fn simple(id: u8) -> Self {
        Frame::new(id, [0x01])
    }

    /// Serialise to bytes.
    ///
    /// Fails rather than truncating when the payload will not fit the length
    /// byte. The JavaScript implementations assign `data.length` into a
    /// `Uint8Array` element, which wraps mod 256 and produces a frame the
    /// printer mis-parses with no diagnostic at either end.
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        if self.data.len() > MAX_PAYLOAD {
            return Err(ProtoError::PayloadTooLong(self.data.len()));
        }
        let len = self.data.len() as u8;
        let mut out = Vec::with_capacity(self.data.len() + STANDARD_OVERHEAD);
        out.extend_from_slice(&HEAD);
        out.push(self.id);
        out.push(len);
        out.extend_from_slice(&self.data);
        out.push(checksum(self.id, len, &self.data));
        out.extend_from_slice(&TAIL);
        Ok(out)
    }

    /// Serialise several frames back to back into one buffer.
    ///
    /// Bundling whole frames into a single transport write is documented as
    /// safe and is worth roughly 4x on BLE throughput. Splitting a *single*
    /// frame across writes is not documented and is never done - see
    /// `relabel-core`'s transport layer, which refuses to print rather than
    /// fragment.
    pub fn encode_all(frames: &[Frame]) -> Result<Vec<u8>, ProtoError> {
        let mut out = Vec::new();
        for f in frames {
            out.extend_from_slice(&f.encode()?);
        }
        Ok(out)
    }

    /// Decode exactly one frame from the front of `buf`, returning it and the
    /// number of bytes consumed.
    ///
    /// Used by the accumulator once it has established that a whole frame is
    /// present. Callers holding a slice that should contain precisely one frame
    /// can check the returned length against `buf.len()`.
    pub fn decode(buf: &[u8]) -> Result<(Frame, usize), ProtoError> {
        match FrameShape::probe(buf)? {
            Probe::NeedMore(need) => Err(ProtoError::Truncated {
                need,
                have: buf.len(),
            }),
            Probe::Frame { shape, total } => {
                let tail = &buf[total - 2..total];
                if tail != TAIL {
                    return Err(ProtoError::BadTail([tail[0], tail[1]]));
                }
                match shape {
                    FrameShape::Standard => {
                        let id = buf[2];
                        let len = buf[3] as usize;
                        let data = &buf[4..4 + len];
                        let found = buf[4 + len];
                        let computed = checksum(id, len as u8, data);
                        if found != computed {
                            return Err(ProtoError::BadChecksum { found, computed });
                        }
                        Ok((Frame::new(id, data), total))
                    }
                    // Recognised only so the boundary can be computed and the
                    // stream stays in sync. relabel does not implement firmware
                    // upgrade, so the contents are deliberately discarded - see
                    // `Accumulator`, which reports these as skipped.
                    FrameShape::Firmware => Ok((Frame::new(buf[2], &buf[6..total - 6]), total)),
                }
            }
        }
    }
}

/// `CMD ^ LEN ^ DATA[0] ^ … ^ DATA[LEN-1]`, starting from zero.
///
/// The head and tail are outside it, and so is the `0x03` byte that precedes a
/// Connect frame.
#[inline]
pub fn checksum(id: u8, len: u8, data: &[u8]) -> u8 {
    data.iter().fold(id ^ len, |acc, b| acc ^ b)
}

/// Which of the two framings a command byte selects.
///
/// Selected purely by the command byte, never by any negotiated state, which is
/// what makes a stateless decoder possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameShape {
    /// The ordinary `LEN`-plus-`XOR` frame that everything on a print path uses.
    Standard,
    /// The firmware-transfer frame: a 16-bit chunk index and a CRC-32 instead of
    /// the XOR. Twelve bytes of overhead rather than seven.
    Firmware,
}

impl FrameShape {
    /// Outbound firmware opcodes, plus the inbound ones the printer initiates.
    pub fn of(id: u8) -> FrameShape {
        match id {
            0x91 | 0x92 | 0x9B | 0x9C | 0x90 | 0x9A | 0x9D | 0x9E => FrameShape::Firmware,
            _ => FrameShape::Standard,
        }
    }

    /// Inspect the front of `buf` without consuming it.
    ///
    /// Public because the accumulator needs to know a frame's total length
    /// before it has all of it, so it can tell "wait for more" from "this
    /// stream is desynchronised".
    pub fn probe(buf: &[u8]) -> Result<Probe, ProtoError> {
        if buf.len() < 2 {
            return Ok(Probe::NeedMore(2));
        }
        if buf[0..2] != HEAD {
            return Err(ProtoError::BadHead([buf[0], buf[1]]));
        }
        if buf.len() < 3 {
            return Ok(Probe::NeedMore(3));
        }
        let shape = FrameShape::of(buf[2]);
        // The length field sits at a different offset in each shape, so the
        // minimum bytes needed before the total can be computed differs too.
        let len_at = match shape {
            FrameShape::Standard => 3,
            FrameShape::Firmware => 5,
        };
        if buf.len() <= len_at {
            return Ok(Probe::NeedMore(len_at + 1));
        }
        let overhead = match shape {
            FrameShape::Standard => STANDARD_OVERHEAD,
            FrameShape::Firmware => FIRMWARE_OVERHEAD,
        };
        let total = buf[len_at] as usize + overhead;
        if buf.len() < total {
            return Ok(Probe::NeedMore(total));
        }
        Ok(Probe::Frame { shape, total })
    }
}

/// What [`FrameShape::probe`] could tell about the front of a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// A whole frame is not present yet; this many bytes are needed in total.
    ///
    /// The figure grows as more of the header arrives: before the length byte
    /// is in hand the true total is not yet knowable, so this reports the next
    /// checkpoint rather than the final answer.
    NeedMore(usize),
    /// A whole frame of `total` bytes is present.
    Frame { shape: FrameShape, total: usize },
}

/// Whether this opcode uses a framing relabel cannot parse.
///
/// `0x87 CompressCommand` carries a 16-bit length and `0xA7 CompressImage` a
/// 32-bit one, and the checksum width for both is unspecified anywhere. The
/// compression codec itself is documented nowhere and no open-source project
/// emits either. relabel never sends them, and on receipt the accumulator
/// resynchronises rather than guessing at a length - a deliberate, documented
/// gap rather than an oversight.
pub fn is_unparseable(id: u8) -> bool {
    matches!(id, 0x87 | 0xA7)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four hex examples that verify. Two further examples circulating in
    /// the community documentation do not: the `TubeTypeAndWidth` get example
    /// prints checksum 0x0f where 0x0e computes, and the `PrintBitmapRow`
    /// example's prose contradicts its own hex. Neither is reproduced here.
    #[test]
    fn known_good_frames() {
        assert_eq!(
            Frame::new(0x1A, [0x01]).encode().unwrap(),
            vec![0x55, 0x55, 0x1a, 0x01, 0x01, 0x1a, 0xaa, 0xaa]
        );
        assert_eq!(
            Frame::new(0xDC, [0x04]).encode().unwrap(),
            vec![0x55, 0x55, 0xdc, 0x01, 0x04, 0xd9, 0xaa, 0xaa]
        );
        assert_eq!(
            Frame::new(0xC1, [0x01]).encode().unwrap(),
            vec![0x55, 0x55, 0xc1, 0x01, 0x01, 0xc1, 0xaa, 0xaa]
        );
        assert_eq!(
            Frame::new(0x84, [0x00, 0x00, 0x0a]).encode().unwrap(),
            vec![0x55, 0x55, 0x84, 0x03, 0x00, 0x00, 0x0a, 0x8d, 0xaa, 0xaa]
        );
    }

    #[test]
    fn simple_frame_checksum_equals_command() {
        for id in [0x01u8, 0x1A, 0xC1, 0xF3] {
            let bytes = Frame::simple(id).encode().unwrap();
            assert_eq!(bytes[5], id, "n ^ 1 ^ 1 == n");
        }
    }

    #[test]
    fn round_trips_with_magic_bytes_in_payload() {
        // The case a scan-for-magic decoder gets wrong: a payload that is
        // nothing but head and tail bytes.
        let f = Frame::new(0x85, vec![0x55, 0x55, 0xAA, 0xAA, 0x55, 0xAA]);
        let bytes = f.encode().unwrap();
        let (back, used) = Frame::decode(&bytes).unwrap();
        assert_eq!(back, f);
        assert_eq!(used, bytes.len());
    }

    #[test]
    fn rejects_oversized_payload() {
        let f = Frame::new(0x85, vec![0u8; 256]);
        assert_eq!(f.encode(), Err(ProtoError::PayloadTooLong(256)));
        // 255 is the boundary and must still work.
        assert!(Frame::new(0x85, vec![0u8; 255]).encode().is_ok());
    }

    #[test]
    fn detects_corrupt_checksum() {
        let mut bytes = Frame::new(0x1A, [0x01]).encode().unwrap();
        bytes[5] ^= 0xFF;
        assert!(matches!(
            Frame::decode(&bytes),
            Err(ProtoError::BadChecksum { .. })
        ));
    }

    #[test]
    fn truncated_frame_reports_how_much_is_missing() {
        let bytes = Frame::new(0x1A, [0x01]).encode().unwrap();
        assert_eq!(
            Frame::decode(&bytes[..5]),
            Err(ProtoError::Truncated { need: 8, have: 5 })
        );
    }

    #[test]
    fn firmware_frames_have_twelve_bytes_of_overhead() {
        assert_eq!(FrameShape::of(0x91), FrameShape::Firmware);
        assert_eq!(FrameShape::of(0x9E), FrameShape::Firmware);
        // 0x9F is not in the set, and neither is 0x93.
        assert_eq!(FrameShape::of(0x9F), FrameShape::Standard);
        assert_eq!(FrameShape::of(0x93), FrameShape::Standard);
    }
}
