//! Typed parsers for what the printer sends back.
//!
//! Every function returns a `Result` and every read goes through [`Reader`],
//! which bounds-checks. Nothing here indexes a slice directly, because every
//! byte it sees came off a wire from a device we do not control - the reference
//! implementations variously throw on trailing bytes, `struct.unpack` past the
//! end, or read a length prefix and trust it.
//!
//! Be strict about what is required and lenient about what follows it: several
//! models append bytes this table does not know, and a trailing surprise is not
//! a reason to fail a read that already succeeded.

use crate::error::{PrinterErrorCode, ProtoError};

/// A bounds-checked cursor over a payload.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8], what: &'static str) -> Reader<'a> {
        Reader { buf, pos: 0, what }
    }

    fn need(&self, n: usize) -> Result<(), ProtoError> {
        if self.pos + n > self.buf.len() {
            Err(ProtoError::ShortPayload {
                what: self.what,
                need: self.pos + n,
                have: self.buf.len(),
            })
        } else {
            Ok(())
        }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn u8(&mut self) -> Result<u8, ProtoError> {
        self.need(1)?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    /// Big-endian, like every multi-byte field in the protocol except the
    /// bitmap black-pixel total.
    pub fn u16(&mut self) -> Result<u16, ProtoError> {
        self.need(2)?;
        let v = u16::from_be_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        self.need(n)?;
        let v = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(v)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), ProtoError> {
        self.need(n)?;
        self.pos += n;
        Ok(())
    }

    /// A `u8` length followed by that many bytes, as UTF-8 with replacement.
    pub fn len_prefixed_string(&mut self) -> Result<String, ProtoError> {
        let n = self.u8()? as usize;
        let raw = self.take(n)?;
        Ok(String::from_utf8_lossy(raw).into_owned())
    }
}

/// What `Connect` came back with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectResult {
    Disconnect,
    /// Protocol version 0.
    Connected,
    /// Protocol version 1.
    ConnectedNew,
    /// Version 3 or above; the exact number comes from `PrinterStatusData`.
    ConnectedV3,
    FirmwareErrors,
    Unknown(u8),
}

impl ConnectResult {
    pub fn parse(data: &[u8]) -> Result<ConnectResult, ProtoError> {
        let mut r = Reader::new(data, "ConnectResult");
        Ok(match r.u8()? {
            0 => ConnectResult::Disconnect,
            1 => ConnectResult::Connected,
            2 => ConnectResult::ConnectedNew,
            3 => ConnectResult::ConnectedV3,
            0x5A => ConnectResult::FirmwareErrors,
            other => ConnectResult::Unknown(other),
        })
    }

    /// The protocol version this result implies on its own.
    ///
    /// `ConnectedV3` returns `None`: it means "3 or more" and the precise number
    /// has to come from `PrinterStatusData`.
    pub fn protocol_version(self) -> Option<u8> {
        match self {
            ConnectResult::Connected => Some(0),
            ConnectResult::ConnectedNew => Some(1),
            _ => None,
        }
    }
}

/// `PrinterStatusData` (`0xB5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusData {
    pub single_color_cache: u16,
    pub single_packet_size: u16,
    pub max_send_size: u16,
    pub packet_interval: u8,
    pub supports_color: bool,
    /// 3, 4 or 5. `None` when the payload was too short to say - the D110
    /// answers this command with two bytes.
    pub protocol_version: Option<u8>,
}

impl StatusData {
    pub fn parse(data: &[u8]) -> Result<StatusData, ProtoError> {
        // Short replies are normal, not an error: several models answer with a
        // truncated form and the session must carry on with what it has.
        if data.len() < 13 {
            return Ok(StatusData::default());
        }
        let mut r = Reader::new(data, "PrinterStatusData");
        let single_color_cache = r.u16()?;
        let single_packet_size = r.u16()?;
        let max_send_size = r.u16()?;
        let packet_interval = r.u8()?;
        r.skip(3)?; // one undocumented byte at offset 7, then 8 and 9
        let supports_color = r.u8()? != 0;
        let hi = r.u8()?;
        let lo = r.u8()?;
        // The version arrives as two decimal-ish bytes: 2.04 and above is v3,
        // 3.00 is v4, 3.02 and above is v5.
        let n = u16::from(hi) * 100 + u16::from(lo);
        let protocol_version = match n {
            204..300 => Some(3),
            300..302 => Some(4),
            302.. => Some(5),
            _ => None,
        };
        Ok(StatusData {
            single_color_cache,
            single_packet_size,
            max_send_size,
            packet_interval,
            supports_color,
            protocol_version,
        })
    }
}

/// `PrintStatus` (`0xB3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintStatus {
    /// Pages the printer says it has completed.
    pub page: u16,
    /// The printer's own figure, 0-100.
    pub print_progress: u8,
    /// The printer's own figure. Not reliably 0-100 across models, which is why
    /// completion is judged on `page` and never on this.
    pub feed_progress: u8,
    /// Non-zero means the job failed.
    pub error: Option<PrinterErrorCode>,
    /// Two undocumented bytes, surfaced raw.
    ///
    /// `data[4] == 0x02` correlates with a job that never started, which would
    /// make a far better diagnostic than a timeout if anyone works out what the
    /// byte means. Kept so the protocol log can show it.
    pub undocumented: (u8, u8),
}

impl PrintStatus {
    pub fn parse(data: &[u8]) -> Result<PrintStatus, ProtoError> {
        let mut r = Reader::new(data, "PrintStatus");
        let page = r.u16()?;
        let print_progress = r.u8()?;
        let feed_progress = r.u8()?;
        let undocumented = if r.remaining() >= 2 {
            (r.u8()?, r.u8()?)
        } else {
            (0, 0)
        };
        // Only the ten-byte form carries an error code.
        let error = if data.len() >= 10 {
            match data[6] {
                0 => None,
                code => Some(PrinterErrorCode::from_byte(code)),
            }
        } else {
            None
        };
        Ok(PrintStatus {
            page,
            print_progress,
            feed_progress,
            error,
            undocumented,
        })
    }
}

/// A label or ribbon tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RfidTag {
    pub uuid: [u8; 8],
    pub barcode: String,
    pub serial: String,
    pub total: u16,
    pub used: u16,
    /// Same id space as [`LabelType`](crate::models::LabelType).
    pub consumable_type: u8,
    pub capacity: Option<u16>,
}

/// What a tag read produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRead {
    Tag(Box<RfidTag>),
    /// The printer answered and said there is no tag.
    NoTag,
}

impl RfidTag {
    /// Parse an `RfidInfo` reply.
    ///
    /// **The no-tag sentinel is a payload length of one**, not a zero first
    /// byte. Both Python clients test `data[0] == 0`, which is wrong twice over:
    /// the first byte is a real UUID byte that can legitimately be zero, and if
    /// the sentinel byte is ever non-zero the test misses and the parser then
    /// reads past a one-byte buffer.
    ///
    /// A third outcome exists and is not represented here: on many models the
    /// printer simply never answers. That is a timeout for the session layer to
    /// report as "no tag information", and it is not an error.
    pub fn parse(data: &[u8]) -> Result<TagRead, ProtoError> {
        if data.len() <= 1 {
            return Ok(TagRead::NoTag);
        }
        let mut r = Reader::new(data, "RfidInfo");
        let mut uuid = [0u8; 8];
        uuid.copy_from_slice(r.take(8)?);
        let barcode = r.len_prefixed_string()?;
        let serial = r.len_prefixed_string()?;
        // Unsigned. niimbluelib reads these with a signed getInt16, which turns
        // a roll of more than 32767 into a negative count.
        let total = r.u16()?;
        let used = r.u16()?;
        let consumable_type = r.u8()?;
        // Present on some models only. Trailing bytes beyond it are ignored
        // rather than rejected.
        let capacity = if r.remaining() >= 2 {
            Some(r.u16()?)
        } else {
            None
        };
        Ok(TagRead::Tag(Box::new(RfidTag {
            uuid,
            barcode,
            serial,
            total,
            used,
            consumable_type,
            capacity,
        })))
    }

    pub fn uuid_hex(&self) -> String {
        self.uuid.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn remaining(&self) -> u16 {
        self.total.saturating_sub(self.used)
    }
}

/// Model ids whose heartbeat reports the lid flag inverted.
///
/// Includes the whole D11 family (512, 2304, 2560). The flag is display-only in
/// relabel regardless - see [`Heartbeat`] - so getting this wrong misleads a
/// user rather than blocking a print.
pub const INVERTED_LID_MODELS: &[u16] = &[
    272, 273, 274, 512, 513, 514, 1792, 2304, 2560, 3584, 3840, 4352, 5120,
];

/// A heartbeat reply.
///
/// **Every field here is display-only and none of them gates a print.** The
/// units and polarities are genuinely disputed between sources: one documents
/// the charge byte as a 0-4 enum while its own worked example carries `0x64`,
/// and the same diagram annotates paper RFID as "0 = ok" and ribbon RFID as
/// "1 = ok". Blocking a job on a flag nobody can agree the sense of would turn
/// a cosmetic uncertainty into an unprintable printer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Heartbeat {
    pub charge: Option<u8>,
    /// Units unstated anywhere. An example value of 77 is implausible as an idle
    /// print-head temperature in Celsius.
    pub temperature: Option<u8>,
    pub lid_closed: Option<bool>,
    pub paper_inserted: Option<bool>,
    pub paper_rfid_ok: Option<bool>,
    pub ribbon_rfid_ok: Option<bool>,
    pub ribbon_inserted: Option<bool>,
}

impl Heartbeat {
    /// Parse the rich `0xD9` form.
    pub fn parse_advanced2(data: &[u8], model_id: Option<u16>) -> Result<Heartbeat, ProtoError> {
        if data.len() < 9 {
            return Ok(Heartbeat::default());
        }
        let mut r = Reader::new(data, "HeartbeatAdvanced2");
        r.skip(2)?;
        let charge = r.u8()?;
        let temperature = r.u8()?;
        let lid = r.u8()?;
        let paper = r.u8()?;
        let paper_rfid = r.u8()?;
        let ribbon_rfid = r.u8()?;
        let ribbon = r.u8()?;
        Ok(Heartbeat {
            charge: Some(charge),
            temperature: Some(temperature),
            lid_closed: Some(lid_closed(lid, model_id)),
            // Zero means inserted, which reads backwards but is what the
            // captures show.
            paper_inserted: Some(paper == 0),
            paper_rfid_ok: Some(paper_rfid != 0),
            ribbon_rfid_ok: Some(ribbon_rfid != 0),
            ribbon_inserted: Some(ribbon != 0),
        })
    }

    /// Parse the older `0xDD` form, whose field offsets depend on the payload
    /// length.
    ///
    /// One source selects by length as done here; another says the vendor SDK
    /// selects by model id, with figures that differ from these by exactly the
    /// seven-byte frame overhead. That is suggestive but unconfirmed, and since
    /// every field is display-only the cost of choosing wrong is low.
    pub fn parse_advanced1(data: &[u8], model_id: Option<u16>) -> Result<Heartbeat, ProtoError> {
        let mut hb = Heartbeat::default();
        let mut r = Reader::new(data, "HeartbeatAdvanced1");
        match data.len() {
            10 => {
                r.skip(8)?;
                hb.lid_closed = Some(lid_closed(r.u8()?, model_id));
                hb.charge = Some(r.u8()?);
            }
            13 | 19 => {
                r.skip(if data.len() == 13 { 9 } else { 15 })?;
                hb.lid_closed = Some(lid_closed(r.u8()?, model_id));
                hb.charge = Some(r.u8()?);
                hb.paper_inserted = Some(r.u8()? == 0);
                hb.paper_rfid_ok = Some(r.u8()? != 0);
            }
            20 => {
                r.skip(18)?;
                hb.paper_inserted = Some(r.u8()? == 0);
                hb.paper_rfid_ok = Some(r.u8()? != 0);
            }
            _ => {}
        }
        Ok(hb)
    }
}

fn lid_closed(raw: u8, model_id: Option<u16>) -> bool {
    let inverted = model_id.is_some_and(|id| INVERTED_LID_MODELS.contains(&id));
    // Zero normally means closed.
    if inverted { raw != 0 } else { raw == 0 }
}

/// The model id from `PrinterInfo` sub-type 8 (`0x48`).
///
/// **Branch on payload length, not on firmware version.** A two-byte payload is
/// a big-endian `u16`; a one-byte payload is that byte in the high position.
/// Decoded unsigned - niimbluelib reads the two-byte form as a signed `i16`,
/// which is harmless for every documented id but has no reason to be inherited.
pub fn model_id(data: &[u8]) -> Result<u16, ProtoError> {
    match data.len() {
        0 => Err(ProtoError::ShortPayload {
            what: "model id",
            need: 1,
            have: 0,
        }),
        1 => Ok(u16::from(data[0]) << 8),
        _ => Ok(u16::from_be_bytes([data[0], data[1]])),
    }
}

/// The serial number from `PrinterInfo` sub-type 11 (`0x4B`).
///
/// ASCII when the payload is eight bytes or more; otherwise the uppercase hex of
/// the first four bytes. Under four bytes means the printer does not have one.
pub fn serial_number(data: &[u8]) -> Option<String> {
    if data.len() >= 8 {
        Some(String::from_utf8_lossy(data).trim_end_matches('\0').into())
    } else if data.len() >= 4 {
        Some(data[..4].iter().map(|b| format!("{b:02X}")).collect())
    } else {
        None
    }
}

/// The battery level from `PrinterInfo` sub-type 10 (`0x4A`), as a percentage.
///
/// The wire value is a 0-4 enum, so the percentage is a label for a coarse bar
/// and should be presented as approximate.
pub fn battery_percent(data: &[u8]) -> Option<u8> {
    let level = *data.first()?;
    Some(match level {
        0..=4 => level * 25,
        // Some firmware appears to send a real percentage here. Passing an
        // out-of-enum value through unchanged is better than clamping it to 100.
        other => other.min(100),
    })
}

/// A two-byte version field, as `major.minor`.
pub fn version(data: &[u8]) -> Option<String> {
    match data.len() {
        0 => None,
        1 => Some(format!("{}", data[0])),
        _ => Some(format!("{}.{:02}", data[0], data[1])),
    }
}

/// The page counter the printer sends unprompted (`0xE0`).
pub fn page_index(data: &[u8]) -> Result<u16, ProtoError> {
    Reader::new(data, "PrinterPageIndex").u16()
}

/// A single-byte accepted/refused reply.
///
/// `PageStart`, `PageEnd`, `PrintEnd`, `PrintClear` and calibration all answer
/// this way: `0x01` accepted or finished, `0x00` refused or still printing.
pub fn accepted(data: &[u8]) -> bool {
    data.first().is_some_and(|b| *b == 0x01)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tag_is_a_length_of_one_not_a_zero_byte() {
        assert_eq!(RfidTag::parse(&[0x00]).unwrap(), TagRead::NoTag);
        // A real tag whose UUID begins with 0x00 must still parse. This is the
        // case the `data[0] == 0` test in both Python clients gets wrong.
        let mut payload = vec![0x00, 1, 2, 3, 4, 5, 6, 7];
        payload.push(3);
        payload.extend_from_slice(b"ABC");
        payload.push(2);
        payload.extend_from_slice(b"SN");
        payload.extend_from_slice(&100u16.to_be_bytes());
        payload.extend_from_slice(&7u16.to_be_bytes());
        payload.push(1);
        match RfidTag::parse(&payload).unwrap() {
            TagRead::Tag(t) => {
                assert_eq!(t.barcode, "ABC");
                assert_eq!(t.serial, "SN");
                assert_eq!(t.total, 100);
                assert_eq!(t.used, 7);
                assert_eq!(t.remaining(), 93);
                assert_eq!(t.consumable_type, 1);
                assert_eq!(t.capacity, None);
                assert_eq!(t.uuid_hex(), "0001020304050607");
            }
            other => panic!("expected a tag, got {other:?}"),
        }
    }

    #[test]
    fn rfid_counters_are_unsigned() {
        let mut payload = vec![0u8; 8];
        payload.push(0);
        payload.push(0);
        payload.extend_from_slice(&40000u16.to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes());
        payload.push(1);
        match RfidTag::parse(&payload).unwrap() {
            TagRead::Tag(t) => assert_eq!(t.total, 40000),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_truncated_tag_is_an_error_not_a_panic() {
        // A length prefix that overruns the buffer.
        let payload = vec![0u8, 1, 2, 3, 4, 5, 6, 7, 200];
        assert!(matches!(
            RfidTag::parse(&payload),
            Err(ProtoError::ShortPayload { .. })
        ));
    }

    #[test]
    fn model_id_branches_on_length() {
        assert_eq!(model_id(&[0x02, 0x00]).unwrap(), 512, "D11");
        assert_eq!(model_id(&[0x09, 0x00]).unwrap(), 2304, "D110");
        assert_eq!(model_id(&[0x02]).unwrap(), 512, "one byte sits high");
        assert!(model_id(&[]).is_err());
    }

    #[test]
    fn serial_number_switches_on_length() {
        assert_eq!(serial_number(b"ABCD1234").as_deref(), Some("ABCD1234"));
        assert_eq!(
            serial_number(&[0xDE, 0xAD, 0xBE, 0xEF]).as_deref(),
            Some("DEADBEEF")
        );
        assert_eq!(serial_number(&[0x01, 0x02]), None);
    }

    #[test]
    fn protocol_version_thresholds() {
        let mk = |hi: u8, lo: u8| {
            let mut d = vec![0u8; 11];
            d.push(hi);
            d.push(lo);
            StatusData::parse(&d).unwrap().protocol_version
        };
        assert_eq!(mk(2, 4), Some(3));
        assert_eq!(mk(2, 99), Some(3));
        assert_eq!(mk(3, 0), Some(4));
        assert_eq!(mk(3, 1), Some(4));
        assert_eq!(mk(3, 2), Some(5));
        assert_eq!(mk(2, 3), None, "below 204 is not a known version");
    }

    #[test]
    fn short_status_data_is_not_an_error() {
        // The D110 answers this with two bytes.
        let s = StatusData::parse(&[0x00, 0x01]).unwrap();
        assert_eq!(s.protocol_version, None);
    }

    #[test]
    fn print_status_finds_the_error_only_in_the_ten_byte_form() {
        let short = [0x00, 0x01, 50, 0];
        assert_eq!(PrintStatus::parse(&short).unwrap().error, None);
        let mut long = vec![0x00, 0x02, 100, 100, 0x02, 0x00, 0x01, 0, 0, 0];
        let s = PrintStatus::parse(&long).unwrap();
        assert_eq!(s.page, 2);
        assert_eq!(s.error, Some(PrinterErrorCode::CoverOpen));
        assert_eq!(s.undocumented, (0x02, 0x00));
        long[6] = 0;
        assert_eq!(PrintStatus::parse(&long).unwrap().error, None);
    }

    #[test]
    fn lid_polarity_is_inverted_on_the_d11_family() {
        // D11 is 512 and is in the inverted set.
        let d11 = Heartbeat::parse_advanced2(&[0, 0, 80, 30, 1, 0, 1, 1, 0], Some(512)).unwrap();
        assert_eq!(d11.lid_closed, Some(true));
        // A B1 is not.
        let b1 = Heartbeat::parse_advanced2(&[0, 0, 80, 30, 1, 0, 1, 1, 0], Some(4096)).unwrap();
        assert_eq!(b1.lid_closed, Some(false));
    }

    #[test]
    fn heartbeat_tolerates_a_short_payload() {
        assert_eq!(
            Heartbeat::parse_advanced2(&[0, 0], None).unwrap(),
            Heartbeat::default()
        );
        assert_eq!(
            Heartbeat::parse_advanced1(&[0; 7], None).unwrap(),
            Heartbeat::default()
        );
    }

    #[test]
    fn connect_result_maps_to_a_version() {
        assert_eq!(
            ConnectResult::parse(&[1]).unwrap().protocol_version(),
            Some(0)
        );
        assert_eq!(
            ConnectResult::parse(&[2]).unwrap().protocol_version(),
            Some(1)
        );
        assert_eq!(
            ConnectResult::parse(&[3]).unwrap(),
            ConnectResult::ConnectedV3
        );
        assert_eq!(ConnectResult::parse(&[3]).unwrap().protocol_version(), None);
    }

    #[test]
    fn battery_is_a_coarse_enum() {
        assert_eq!(battery_percent(&[0]), Some(0));
        assert_eq!(battery_percent(&[4]), Some(100));
        assert_eq!(battery_percent(&[]), None);
    }
}
