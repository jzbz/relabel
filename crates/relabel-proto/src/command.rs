//! Opcodes, and the rule for matching a reply to a request.
//!
//! # Why two types and not one
//!
//! Several response ids reuse values that are also request opcodes - `0x0D`,
//! `0x0E`, `0x0F`, `0x11`, `0x12`, `0x17` and `0x19` are all echoed back
//! unchanged, and `0xD3` is both a command the client sends and a packet the
//! printer sends unprompted. A single shared enum would make
//! `id == RequestId::PageStart` true for an inbound byte that means something
//! else entirely. [`RequestId`] and [`ResponseId`] are therefore separate
//! types that never compare equal.
//!
//! # Why newtypes and not enums with a variant each
//!
//! An unknown opcode is a normal event, not a parse failure: the catalogue spans
//! firmware generations we cannot enumerate, and a printer answering something
//! undocumented should reach the protocol log rather than be rejected. A
//! newtype over `u8` with associated constants gives the type separation the
//! protocol needs while letting an unrecognised byte through with its value
//! intact.

use crate::frame::is_unparseable;

/// A command byte the client sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u8);

/// A command byte the printer sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResponseId(pub u8);

/// What reply, if any, a request should be waited on for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Exactly one response id.
    One(ResponseId),
    /// Any one of several - the printer picks, and all of them mean "answered".
    Any(&'static [ResponseId]),
    /// `PrinterInfo`: the response id is `0x40 + the sub-type byte requested`.
    InfoSubtype,
    /// Nothing comes back. Bitmap rows and firmware chunks are one-way, and so
    /// are the two deliberate sacrificial sends in the V4 print flow.
    Nothing,
}

macro_rules! request_consts {
    ($($name:ident = $val:expr, $label:literal;)*) => {
        impl RequestId {
            $(pub const $name: RequestId = RequestId($val);)*

            /// A short human name for the protocol log.
            pub fn name(self) -> &'static str {
                match self.0 {
                    $($val => $label,)*
                    _ => "Unknown",
                }
            }
        }
    };
}

request_consts! {
    PRINT_START = 0x01, "PrintStart";
    PAGE_START = 0x03, "PageStart";
    PRINTER_LOG = 0x05, "PrinterLog";
    PRINTER_CONFIG2 = 0x07, "PrinterConfig2";
    GET_KEY_FUNCTION = 0x09, "GetKeyFunction";
    ANTI_FAKE = 0x0B, "AntiFake";
    GET_PRINT_QUALITY = 0x0D, "GetPrintQuality";
    TUBE_SETTINGS = 0x0E, "TubeSettings";
    TUBE_TYPE_AND_WIDTH = 0x0F, "TubeTypeAndWidth";
    SET_OUT_NET = 0x10, "SetOutNet";
    SET_LOCAL_TEMPLATE = 0x11, "SetLocalTemplate";
    GET_TIME_FORMAT = 0x12, "GetTimeFormat";
    SET_PAGE_SIZE = 0x13, "SetPageSize";
    PAGE_HEIGHT = 0x14, "PageHeight";
    PRINT_QUANTITY = 0x15, "PrintQuantity";
    TEMPLATE_HISTORY = 0x17, "TemplateHistory";
    PRINTER_USAGE = 0x19, "PrinterUsage";
    RFID_INFO = 0x1A, "RfidInfo";
    RFID_INFO2 = 0x1C, "RfidInfo2";
    REVERSE_FEED = 0x1E, "ReverseFeed";
    MARGIN_TOP_HEIGHT = 0x1F, "MarginTopHeight";
    PRINT_CLEAR = 0x20, "PrintClear";
    SET_DENSITY = 0x21, "SetDensity";
    SET_PRINT_SPEED = 0x22, "SetPrintSpeed";
    SET_LABEL_TYPE = 0x23, "SetLabelType";
    SET_LANGUAGE_TYPE = 0x26, "SetLanguageType";
    SET_AUTO_SHUTDOWN = 0x27, "SetAutoShutdown";
    PRINTER_RESET = 0x28, "PrinterReset";
    SET_PRINT_MODE = 0x2C, "SetPrintMode";
    SET_LABEL_MATERIAL = 0x2D, "SetLabelMaterial";
    SET_VOLUME_LEVEL = 0x2E, "SetVolumeLevel";
    SET_ANTI_SETTER = 0x2F, "SetAntiSetter";
    PRINTER_INFO = 0x40, "PrinterInfo";
    PRINTING_HISTORY = 0x52, "PrintingHistory";
    RFID_SUCCESS_TIMES = 0x54, "RfidSuccessTimes";
    SOUND_SETTINGS = 0x58, "SoundSettings";
    GET_PAPER_INFO = 0x59, "GetPaperInfo";
    PRINT_TEST_PAGE = 0x5A, "PrintTestPage";
    HALF_CUT = 0x5C, "HalfCut";
    GET_VOLUME_OR_WRITE_RFID = 0x70, "GetVolume/WriteRfid";
    BITMAP_ROW_INDEXED = 0x83, "BitmapRowIndexed";
    EMPTY_ROW = 0x84, "EmptyRow";
    BITMAP_ROW = 0x85, "BitmapRow";
    PRINTER_CHECK_LINE = 0x86, "PrinterCheckLine";
    COMPRESS_COMMAND = 0x87, "CompressCommand";
    GET_COMPRESS = 0x89, "GetCompress";
    BITMAP_ROW_DOUBLE_COLOR = 0x8A, "BitmapRowDoubleColor";
    POSITIONING_CALIBRATION = 0x8E, "PositioningCalibration";
    FIRMWARE_CRC = 0x91, "FirmwareCrc";
    FIRMWARE_CHUNK = 0x92, "FirmwareChunk";
    FIRMWARE_START = 0x9B, "FirmwareStart";
    FIRMWARE_END = 0x9C, "FirmwareEnd";
    SET_WIFI_CONFIG = 0xA1, "SetWifiConfig";
    GET_WIFI_CONFIG = 0xA2, "GetWifiConfig";
    PRINT_STATUS = 0xA3, "PrintStatus";
    PRINTER_STATUS_DATA = 0xA5, "PrinterStatusData";
    PAUSE = 0xA6, "Pause";
    COMPRESS_IMAGE = 0xA7, "CompressImage";
    PRINTER_CONFIG = 0xAF, "PrinterConfig";
    CONNECT = 0xC1, "Connect";
    PRINTER_FREE = 0xC3, "PrinterFree";
    CANCEL_PRINT = 0xDA, "CancelPrint";
    HEARTBEAT = 0xDC, "Heartbeat";
    PAGE_END = 0xE3, "PageEnd";
    PRINT_END = 0xF3, "PrintEnd";
    START_FIRMWARE_UPGRADE = 0xF5, "StartFirmwareUpgrade";
}

macro_rules! response_consts {
    ($($name:ident = $val:expr, $label:literal;)*) => {
        impl ResponseId {
            $(pub const $name: ResponseId = ResponseId($val);)*

            pub fn name(self) -> &'static str {
                match self.0 {
                    $($val => $label,)*
                    // PrinterInfo answers on 0x41..=0x4F, one id per sub-type.
                    0x41..=0x4F => "PrinterInfo",
                    _ => "Unknown",
                }
            }
        }
    };
}

response_consts! {
    NOT_SUPPORTED = 0x00, "NotSupported";
    PRINT_START = 0x02, "PrintStart";
    PAGE_START = 0x04, "PageStart";
    SET_PAGE_SIZE = 0x14, "SetPageSize";
    PRINT_QUANTITY = 0x16, "PrintQuantity";
    RFID_INFO = 0x1B, "RfidInfo";
    RFID_INFO2 = 0x1D, "RfidInfo2";
    PRINT_CLEAR = 0x30, "PrintClear";
    SET_DENSITY = 0x31, "SetDensity";
    SET_PRINT_SPEED = 0x32, "SetPrintSpeed";
    SET_LABEL_TYPE = 0x33, "SetLabelType";
    SET_AUTO_SHUTDOWN = 0x37, "SetAutoShutdown";
    PRINTER_RESET = 0x38, "PrinterReset";
    GET_PAPER_INFO = 0x69, "GetPaperInfo";
    PRINT_TEST_PAGE = 0x6A, "PrintTestPage";
    GET_COMPRESS = 0x88, "GetCompress";
    POSITIONING_CALIBRATION = 0x8F, "PositioningCalibration";
    PRINT_STATUS = 0xB3, "PrintStatus";
    PRINTER_STATUS_DATA = 0xB5, "PrinterStatusData";
    PAUSE = 0xB6, "Pause";
    PRINTER_CONFIG = 0xBF, "PrinterConfig";
    CONNECT = 0xC2, "Connect";
    PRINTER_FREE = 0xC4, "PrinterFree";
    RESET_TIMEOUT = 0xC6, "ResetTimeout";
    CANCEL_PRINT = 0xD0, "CancelPrint";
    PRINTER_CHECK_LINE = 0xD3, "PrinterCheckLine";
    HEARTBEAT_ADVANCED2 = 0xD9, "HeartbeatAdvanced2";
    PRINT_ERROR = 0xDB, "PrintError";
    HEARTBEAT_ADVANCED1 = 0xDD, "HeartbeatAdvanced1";
    HEARTBEAT_BASIC = 0xDE, "HeartbeatBasic";
    HEARTBEAT_UNKNOWN = 0xDF, "HeartbeatUnknown";
    PRINTER_PAGE_INDEX = 0xE0, "PrinterPageIndex";
    PAGE_END = 0xE4, "PageEnd";
    PRINT_END = 0xF4, "PrintEnd";
    START_FIRMWARE_UPGRADE = 0xF6, "StartFirmwareUpgrade";
}

/// Replies that can arrive in place of *any* expected response.
///
/// The session layer short-circuits a pending request when one of these lands,
/// rather than waiting out the timeout. `0xDB` carries a
/// [`PrinterErrorCode`](crate::PrinterErrorCode); `0x00` means the printer does
/// not implement the command at all.
pub const UNIVERSAL_ERRORS: &[ResponseId] = &[ResponseId::PRINT_ERROR, ResponseId::NOT_SUPPORTED];

/// Packets the printer sends unprompted, at any time.
///
/// These must never be mistaken for a reply. The session routes anything in this
/// set to its event channel regardless of what request is outstanding.
///
/// `0xD3` is the printer's own count of rows it has received - relabel logs it,
/// and it is the raw material for a genuine dropped-row check later. `0xE0` is
/// the page counter that the D11 flow waits on instead of polling. `0xC6` is
/// undocumented beyond its name; no implementation acts on it.
pub const UNSOLICITED: &[ResponseId] = &[
    ResponseId::PRINTER_CHECK_LINE,
    ResponseId::PRINTER_PAGE_INDEX,
    ResponseId::RESET_TIMEOUT,
];

const HEARTBEAT_REPLIES: &[ResponseId] = &[
    ResponseId::HEARTBEAT_ADVANCED2,
    ResponseId::HEARTBEAT_ADVANCED1,
    ResponseId::HEARTBEAT_BASIC,
    ResponseId::HEARTBEAT_UNKNOWN,
];

// niimbluelib accepts 0x1E or 0x1F here and the community table lists only
// 0x1F. Accepting both costs nothing and neither command is on a print path.
const REVERSE_FEED_REPLIES: &[ResponseId] = &[ResponseId(0x1E), ResponseId(0x1F)];

// Same disagreement, same resolution: one source maps each of 0x11 and 0x12 to
// itself, another maps 0x11 to either and 0x12 to 0x11.
const TEMPLATE_TIME_REPLIES: &[ResponseId] = &[ResponseId(0x11), ResponseId(0x12)];

const TEMPLATE_HISTORY_REPLIES: &[ResponseId] = &[ResponseId(0x17), ResponseId(0x63)];
const PRINTING_HISTORY_REPLIES: &[ResponseId] = &[ResponseId(0x62), ResponseId(0x63)];

impl RequestId {
    /// What to wait for after sending this command.
    ///
    /// Three families cover most of the set - `+0x01`, `+0x10`, and echo - but
    /// the exceptions are not decoration: getting `0xDC` or `0x86` wrong means
    /// waiting out a full timeout on every heartbeat and every flow-control
    /// checkpoint.
    pub fn expect(self) -> Expect {
        match self.0 {
            // One-way. Bitmap rows are never answered, and neither are firmware
            // chunks. The compression opcodes are here because relabel never
            // sends them and would not be able to parse a reply if it did.
            0x83 | 0x84 | 0x85 | 0x8A | 0x91 | 0x92 | 0x9B | 0x9C => Expect::Nothing,
            id if is_unparseable(id) => Expect::Nothing,

            0x40 => Expect::InfoSubtype,
            0xDC => Expect::Any(HEARTBEAT_REPLIES),
            0xDA => Expect::One(ResponseId::CANCEL_PRINT),
            0x86 => Expect::One(ResponseId::PRINTER_CHECK_LINE),
            0x89 => Expect::One(ResponseId::GET_COMPRESS),
            0x1E => Expect::Any(REVERSE_FEED_REPLIES),
            0x11 | 0x12 => Expect::Any(TEMPLATE_TIME_REPLIES),
            0x17 => Expect::Any(TEMPLATE_HISTORY_REPLIES),
            0x52 => Expect::Any(PRINTING_HISTORY_REPLIES),

            // Echoed back with the same id.
            0x0D | 0x0E | 0x0F | 0x10 | 0x14 | 0x19 | 0x1F => Expect::One(ResponseId(self.0)),

            // +0x01.
            0x01 | 0x03 | 0x05 | 0x07 | 0x09 | 0x0B | 0x13 | 0x15 | 0x1A | 0x1C | 0x70 | 0x8E
            | 0xC1 | 0xC3 | 0xE3 | 0xF3 | 0xF5 => Expect::One(ResponseId(self.0 + 0x01)),

            // +0x10.
            0x20..=0x2F | 0x54 | 0x58 | 0x59 | 0x5A | 0x5C | 0xA1..=0xA6 | 0xAF => {
                Expect::One(ResponseId(self.0 + 0x10))
            }

            // Unknown opcode. Waiting on a reply that never comes is worse than
            // not waiting, so treat it as one-way and let the caller decide.
            _ => Expect::Nothing,
        }
    }

    /// Whether this command carries the `0x03` prefix byte on the wire.
    ///
    /// Only `Connect` does. The byte sits outside the frame head, is not covered
    /// by the checksum, and does not appear on the reply.
    ///
    /// It is a write-side convention, not a framing rule: four independent
    /// implementations omit it entirely and print successfully, and four others
    /// send it. relabel sends it because it costs nothing and matches the
    /// most-exercised BLE client - but **the receive path must never
    /// special-case a leading `0x03`**, because `PageStart` *is* command
    /// `0x03` and a decoder that skips it will mis-parse every page.
    pub fn has_connect_prefix(self) -> bool {
        self == RequestId::CONNECT
    }
}

impl ResponseId {
    /// Whether this can arrive in place of any expected reply.
    pub fn is_universal_error(self) -> bool {
        UNIVERSAL_ERRORS.contains(&self)
    }

    /// Whether this arrives unprompted and must not be matched to a request.
    pub fn is_unsolicited(self) -> bool {
        UNSOLICITED.contains(&self)
    }

    /// Whether this satisfies a wait for `expect`.
    pub fn satisfies(self, expect: Expect, info_subtype: Option<u8>) -> bool {
        match expect {
            Expect::One(want) => self == want,
            Expect::Any(set) => set.contains(&self),
            Expect::InfoSubtype => match info_subtype {
                Some(sub) => self.0 == 0x40u8.wrapping_add(sub),
                // Without knowing which sub-type was asked for, accept the whole
                // documented range rather than nothing.
                None => (0x41..=0x4F).contains(&self.0),
            },
            Expect::Nothing => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_one_family() {
        for (req, resp) in [
            (0x01u8, 0x02u8),
            (0x03, 0x04),
            (0x1A, 0x1B),
            (0xC1, 0xC2),
            (0xE3, 0xE4),
            (0xF3, 0xF4),
        ] {
            assert_eq!(
                RequestId(req).expect(),
                Expect::One(ResponseId(resp)),
                "{req:#04x}"
            );
        }
    }

    #[test]
    fn plus_sixteen_family() {
        for (req, resp) in [
            (0x20u8, 0x30u8),
            (0x21, 0x31),
            (0x23, 0x33),
            (0xA3, 0xB3),
            (0xA5, 0xB5),
            (0xAF, 0xBF),
        ] {
            assert_eq!(
                RequestId(req).expect(),
                Expect::One(ResponseId(resp)),
                "{req:#04x}"
            );
        }
    }

    #[test]
    fn exceptions_are_not_arithmetic() {
        assert_eq!(
            RequestId::CANCEL_PRINT.expect(),
            Expect::One(ResponseId(0xD0))
        );
        assert_eq!(
            RequestId::PRINTER_CHECK_LINE.expect(),
            Expect::One(ResponseId(0xD3))
        );
        assert_eq!(
            RequestId::GET_COMPRESS.expect(),
            Expect::One(ResponseId(0x88))
        );
        assert!(matches!(RequestId::HEARTBEAT.expect(), Expect::Any(_)));
    }

    #[test]
    fn bitmap_rows_are_one_way() {
        for id in [0x83u8, 0x84, 0x85, 0x8A] {
            assert_eq!(RequestId(id).expect(), Expect::Nothing, "{id:#04x}");
        }
    }

    #[test]
    fn printer_info_response_id_tracks_the_subtype() {
        let e = RequestId::PRINTER_INFO.expect();
        assert_eq!(e, Expect::InfoSubtype);
        // Sub-type 8 is the model id, and it answers on 0x48.
        assert!(ResponseId(0x48).satisfies(e, Some(8)));
        assert!(!ResponseId(0x48).satisfies(e, Some(9)));
        assert!(ResponseId(0x49).satisfies(e, Some(9)));
    }

    #[test]
    fn request_and_response_ids_do_not_conflate() {
        // 0x03 is PageStart outbound. Inbound it is nothing of the sort, and
        // the two types make that a compile-time distinction rather than a
        // runtime hazard.
        assert_eq!(RequestId::PAGE_START.0, 0x03);
        assert_eq!(ResponseId(0x03).name(), "Unknown");
    }

    #[test]
    fn only_connect_carries_the_prefix() {
        assert!(RequestId::CONNECT.has_connect_prefix());
        assert!(!RequestId::PAGE_START.has_connect_prefix());
        assert!(!RequestId::PRINT_START.has_connect_prefix());
    }

    #[test]
    fn universal_and_unsolicited_sets_are_disjoint() {
        for u in UNSOLICITED {
            assert!(!u.is_universal_error(), "{u:?}");
        }
    }
}
