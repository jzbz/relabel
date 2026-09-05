//! Typed builders for the commands relabel actually sends.
//!
//! Every function here returns a [`Frame`]; nothing writes to a wire. The
//! payload *widths* are the interesting part: several commands have four or five
//! documented forms and picking the wrong one is the difference between a page
//! and a stalled printer, so each form is a separate constructor with a name
//! that says which print flow wants it.

use crate::command::RequestId;
use crate::error::ProtoError;
use crate::frame::Frame;
use crate::models::LabelType;

/// Serialise a frame the way it goes on the wire.
///
/// Identical to [`Frame::encode`] except for `Connect`, which is preceded by a
/// bare `0x03` outside the head and outside the checksum. Keeping that rule in
/// exactly one place is deliberate - see
/// [`RequestId::has_connect_prefix`] for why it must never be mirrored on the
/// receive side.
pub fn encode_on_wire(frame: &Frame) -> Result<Vec<u8>, ProtoError> {
    let body = frame.encode()?;
    if RequestId(frame.id).has_connect_prefix() {
        let mut out = Vec::with_capacity(body.len() + 1);
        out.push(0x03);
        out.extend_from_slice(&body);
        Ok(out)
    } else {
        Ok(body)
    }
}

/// What the printer will be asked to lay down on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageColor {
    SingleColor = 0,
    DoubleColor = 1,
    SingleColorAlt = 2,
    Multiorder = 3,
}

/// Which heartbeat variant to ask for.
///
/// The request byte selects the reply shape. Advanced2 is the richest and is
/// what protocol version 3 and above answer; below that, Advanced1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HeartbeatKind {
    Advanced1 = 1,
    Basic = 2,
    Unknown = 3,
    Advanced2 = 4,
}

impl HeartbeatKind {
    /// The variant appropriate to a negotiated protocol version.
    pub fn for_protocol(version: u8) -> HeartbeatKind {
        if version >= 3 {
            HeartbeatKind::Advanced2
        } else {
            HeartbeatKind::Advanced1
        }
    }
}

/// `PrinterInfo` sub-types. The response id is `0x40 + the sub-type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InfoField {
    Density = 1,
    Speed = 2,
    LabelType = 3,
    PrintheadWidth = 4,
    PrintingAccuracy = 5,
    Language = 6,
    AutoShutdownTime = 7,
    ModelId = 8,
    SoftwareVersion = 9,
    BatteryChargeLevel = 10,
    SerialNumber = 11,
    HardwareVersion = 12,
    BluetoothAddress = 13,
    PrintMode = 14,
    Area = 15,
}

/// The order the connect handshake reads identity in.
///
/// Model id first because everything downstream - printhead width, density
/// range, print direction, which print flow to use - depends on it. Each read is
/// individually allowed to fail: several models answer some of these with
/// `NotSupported` and the session must degrade rather than abort.
pub const IDENTITY_READS: &[InfoField] = &[
    InfoField::ModelId,
    InfoField::SerialNumber,
    InfoField::HardwareVersion,
    InfoField::BatteryChargeLevel,
    InfoField::AutoShutdownTime,
    InfoField::LabelType,
    InfoField::SoftwareVersion,
    InfoField::Density,
];

pub fn connect() -> Frame {
    Frame::simple(RequestId::CONNECT.0)
}

pub fn printer_info(field: InfoField) -> Frame {
    Frame::new(RequestId::PRINTER_INFO.0, [field as u8])
}

pub fn printer_status_data() -> Frame {
    Frame::simple(RequestId::PRINTER_STATUS_DATA.0)
}

pub fn heartbeat(kind: HeartbeatKind) -> Frame {
    Frame::new(RequestId::HEARTBEAT.0, [kind as u8])
}

pub fn set_density(level: u8) -> Frame {
    Frame::new(RequestId::SET_DENSITY.0, [level])
}

pub fn set_label_type(kind: LabelType) -> Frame {
    Frame::new(RequestId::SET_LABEL_TYPE.0, [kind.code()])
}

pub fn set_auto_shutdown(level: u8) -> Frame {
    Frame::new(RequestId::SET_AUTO_SHUTDOWN.0, [level])
}

pub fn rfid_info() -> Frame {
    Frame::simple(RequestId::RFID_INFO.0)
}

pub fn rfid_info2() -> Frame {
    Frame::simple(RequestId::RFID_INFO2.0)
}

pub fn get_paper_info() -> Frame {
    Frame::simple(RequestId::GET_PAPER_INFO.0)
}

pub fn calibrate() -> Frame {
    Frame::simple(RequestId::POSITIONING_CALIBRATION.0)
}

pub fn print_test_page() -> Frame {
    Frame::simple(RequestId::PRINT_TEST_PAGE.0)
}

pub fn printer_reset() -> Frame {
    Frame::simple(RequestId::PRINTER_RESET.0)
}

pub fn print_clear() -> Frame {
    Frame::simple(RequestId::PRINT_CLEAR.0)
}

pub fn page_start() -> Frame {
    Frame::simple(RequestId::PAGE_START.0)
}

pub fn page_end() -> Frame {
    Frame::simple(RequestId::PAGE_END.0)
}

pub fn print_end() -> Frame {
    Frame::simple(RequestId::PRINT_END.0)
}

pub fn print_status() -> Frame {
    Frame::simple(RequestId::PRINT_STATUS.0)
}

pub fn cancel_print() -> Frame {
    Frame::simple(RequestId::CANCEL_PRINT.0)
}

pub fn pause(on: bool) -> Frame {
    Frame::new(RequestId::PAUSE.0, [u8::from(on)])
}

/// `PrinterCheckLine`, the flow-control checkpoint the B21 flows send every 200
/// rows. The reply is the printer's own count of rows received.
pub fn printer_check_line(rows: u16) -> Frame {
    Frame::new(RequestId::PRINTER_CHECK_LINE.0, rows.to_be_bytes())
}

pub fn print_quantity(n: u16) -> Frame {
    Frame::new(RequestId::PRINT_QUANTITY.0, n.to_be_bytes())
}

/// `PrintStart` has four documented forms and the flows disagree about which to
/// use. Picking wrong breaks printing outright, so each is named for the flow
/// that wants it rather than being chosen by a length argument.
pub mod print_start {
    use super::*;

    /// One byte. The D11, D110 and B21 v1 flows.
    pub fn simple() -> Frame {
        Frame::simple(RequestId::PRINT_START.0)
    }

    /// Two bytes: total pages. The H1S flow.
    pub fn with_pages(pages: u16) -> Frame {
        Frame::new(RequestId::PRINT_START.0, pages.to_be_bytes())
    }

    /// Seven bytes: pages, four reserved zeros, and the page colour. The B1
    /// flow, which is also the safest fallback for an unknown model below
    /// protocol version 5.
    pub fn b1(pages: u16, color: PageColor) -> Frame {
        let p = pages.to_be_bytes();
        Frame::new(
            RequestId::PRINT_START.0,
            [p[0], p[1], 0, 0, 0, 0, color as u8],
        )
    }

    /// Nine bytes: the B1 form plus speed and a flag byte. The V4 flow.
    pub fn v4(pages: u16, color: PageColor, speed: u8, flag: u8) -> Frame {
        let p = pages.to_be_bytes();
        Frame::new(
            RequestId::PRINT_START.0,
            [p[0], p[1], 0, 0, 0, 0, color as u8, speed, flag],
        )
    }
}

/// `SetPageSize` has six documented forms.
///
/// **Rows come first, then columns**, both big-endian - rows along the feed
/// axis, columns across the printhead. Getting the order backwards produces a
/// page rotated 90° with no error from the printer.
pub mod set_page_size {
    use super::*;

    /// Two bytes: rows only. The D11 v1 flow.
    pub fn rows_only(rows: u16) -> Frame {
        Frame::new(RequestId::SET_PAGE_SIZE.0, rows.to_be_bytes())
    }

    /// Four bytes: rows and columns. The D110 and B21 flows.
    pub fn rows_cols(rows: u16, cols: u16) -> Frame {
        let mut d = Vec::with_capacity(4);
        d.extend_from_slice(&rows.to_be_bytes());
        d.extend_from_slice(&cols.to_be_bytes());
        Frame::new(RequestId::SET_PAGE_SIZE.0, d)
    }

    /// Six bytes: rows, columns and the copy count. The B1 flow.
    pub fn rows_cols_copies(rows: u16, cols: u16, copies: u16) -> Frame {
        let mut d = Vec::with_capacity(6);
        d.extend_from_slice(&rows.to_be_bytes());
        d.extend_from_slice(&cols.to_be_bytes());
        d.extend_from_slice(&copies.to_be_bytes());
        Frame::new(RequestId::SET_PAGE_SIZE.0, d)
    }

    /// Nine bytes: adds a cut height and cut type. The H1S flow.
    pub fn with_cut(rows: u16, cols: u16, copies: u16, cut_height: u16, cut_type: u8) -> Frame {
        let mut d = Vec::with_capacity(9);
        d.extend_from_slice(&rows.to_be_bytes());
        d.extend_from_slice(&cols.to_be_bytes());
        d.extend_from_slice(&copies.to_be_bytes());
        d.extend_from_slice(&cut_height.to_be_bytes());
        d.push(cut_type);
        Frame::new(RequestId::SET_PAGE_SIZE.0, d)
    }

    /// Thirteen bytes. The V4 flow.
    pub fn v4(rows: u16, cols: u16, copies: u16, send_all: bool, part_height: u16) -> Frame {
        let mut d = Vec::with_capacity(13);
        d.extend_from_slice(&rows.to_be_bytes());
        d.extend_from_slice(&cols.to_be_bytes());
        d.extend_from_slice(&copies.to_be_bytes());
        d.extend_from_slice(&[0, 0, 0, 0]);
        d.push(u8::from(send_all));
        d.extend_from_slice(&part_height.to_be_bytes());
        Frame::new(RequestId::SET_PAGE_SIZE.0, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_carries_the_prefix_and_nothing_else_does() {
        assert_eq!(
            encode_on_wire(&connect()).unwrap(),
            vec![0x03, 0x55, 0x55, 0xc1, 0x01, 0x01, 0xc1, 0xaa, 0xaa]
        );
        assert_eq!(
            encode_on_wire(&page_start()).unwrap(),
            vec![0x55, 0x55, 0x03, 0x01, 0x01, 0x03, 0xaa, 0xaa]
        );
    }

    #[test]
    fn rfid_request_matches_the_published_example() {
        assert_eq!(
            rfid_info().encode().unwrap(),
            vec![0x55, 0x55, 0x1a, 0x01, 0x01, 0x1a, 0xaa, 0xaa]
        );
    }

    #[test]
    fn advanced2_heartbeat_matches_the_published_example() {
        assert_eq!(
            heartbeat(HeartbeatKind::Advanced2).encode().unwrap(),
            vec![0x55, 0x55, 0xdc, 0x01, 0x04, 0xd9, 0xaa, 0xaa]
        );
    }

    #[test]
    fn heartbeat_variant_tracks_the_protocol_version() {
        assert_eq!(HeartbeatKind::for_protocol(0), HeartbeatKind::Advanced1);
        assert_eq!(HeartbeatKind::for_protocol(2), HeartbeatKind::Advanced1);
        assert_eq!(HeartbeatKind::for_protocol(3), HeartbeatKind::Advanced2);
        assert_eq!(HeartbeatKind::for_protocol(5), HeartbeatKind::Advanced2);
    }

    #[test]
    fn page_size_forms_have_the_documented_widths() {
        assert_eq!(set_page_size::rows_only(200).data.len(), 2);
        assert_eq!(set_page_size::rows_cols(200, 96).data.len(), 4);
        assert_eq!(set_page_size::rows_cols_copies(200, 96, 1).data.len(), 6);
        assert_eq!(set_page_size::with_cut(200, 96, 1, 0, 0).data.len(), 9);
        assert_eq!(set_page_size::v4(200, 96, 1, true, 0).data.len(), 13);
    }

    #[test]
    fn page_size_puts_rows_before_columns() {
        // 200 rows along the feed, 96 dots across a D11 head.
        let f = set_page_size::rows_cols(200, 96);
        assert_eq!(f.data, vec![0x00, 0xC8, 0x00, 0x60]);
    }

    #[test]
    fn print_start_forms_have_the_documented_widths() {
        assert_eq!(print_start::simple().data.len(), 1);
        assert_eq!(print_start::with_pages(3).data.len(), 2);
        assert_eq!(print_start::b1(3, PageColor::SingleColor).data.len(), 7);
        assert_eq!(
            print_start::v4(3, PageColor::SingleColor, 0, 0).data.len(),
            9
        );
    }

    #[test]
    fn identity_reads_start_with_the_model_id() {
        assert_eq!(IDENTITY_READS[0], InfoField::ModelId);
        assert_eq!(printer_info(InfoField::ModelId).data, vec![8]);
    }
}
