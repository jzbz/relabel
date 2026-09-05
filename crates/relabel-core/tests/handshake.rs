//! Identification against printers that answer awkwardly.
//!
//! Every case here is a documented real-world behaviour, not an invented edge.

use relabel_core::job::Timeouts;
use relabel_core::transport::mock::MockTransport;
use relabel_core::{Session, handshake};
use relabel_proto::{Expect, Frame, RequestId};

fn reply_for(req: &Frame, model_id: u16, short_status: bool, heartbeat_ok: bool) -> Vec<Frame> {
    match req.id {
        // "Connected, level 2 or better."
        0xC1 => vec![Frame::new(0xC2, [0x03])],
        0xA5 if short_status => vec![Frame::new(0xB5, [0x00, 0x01])],
        0xA5 => {
            let mut d = vec![0u8; 11];
            d.push(3);
            d.push(2); // 3.02 -> level 5
            vec![Frame::new(0xB5, d)]
        }
        0xDC if !heartbeat_ok => vec![Frame::new(0x00, [0x01])],
        0x40 => {
            let sub = req.data.first().copied().unwrap_or(0);
            let payload: Vec<u8> = match sub {
                8 => model_id.to_be_bytes().to_vec(),
                11 => b"D110ABCD".to_vec(),
                9 => vec![2, 16],
                _ => vec![1],
            };
            vec![Frame::new(0x40u8.wrapping_add(sub), payload)]
        }
        _ => match RequestId(req.id).expect() {
            Expect::Nothing => vec![],
            Expect::One(r) => vec![Frame::new(r.0, [0x01])],
            Expect::Any(set) => vec![Frame::new(set[0].0, [0x01])],
            Expect::InfoSubtype => vec![],
        },
    }
}

/// The D110 answers the status query with a payload too short to carry a
/// version, and answers the newer heartbeat with "not supported".
///
/// The level therefore stays at the connect baseline - and that baseline must be
/// **2**, because 2 selects the two-byte `PrintStart` and 3 would select the
/// one-byte one. Getting this wrong is silent: the printer acknowledges either.
#[tokio::test(start_paused = true)]
async fn a_d110_that_cannot_resolve_its_level_stays_at_the_connect_baseline() {
    let (t, _h) = MockTransport::new(Box::new(|req: &Frame| reply_for(req, 2304, true, false)));
    let s = Session::start(Box::new(t));
    let id = handshake(&s, &Timeouts::default()).await.unwrap();

    assert_eq!(id.model_id, Some(2304));
    assert_eq!(id.name(), "D110");
    assert_eq!(id.protocol, 2, "the baseline is 2, not 3");
    assert!(id.protocol_assumed, "and it is flagged as an assumption");
    assert_eq!(id.flow(), relabel_proto::PrintFlow::High);
    assert_eq!(id.printhead_px(), 96);
    // Both awkward answers are reported rather than silently swallowed.
    assert!(id.missing.contains(&"protocol level"));
    assert!(id.missing.contains(&"heartbeat"));
    assert_eq!(id.serial.as_deref(), Some("D110ABCD"));
}

/// A printer that answers the status query properly gets an exact level, and it
/// is no longer flagged as assumed.
#[tokio::test(start_paused = true)]
async fn a_full_status_reply_resolves_the_level_exactly() {
    let (t, _h) = MockTransport::new(Box::new(|req: &Frame| reply_for(req, 4096, false, true)));
    let s = Session::start(Box::new(t));
    let id = handshake(&s, &Timeouts::default()).await.unwrap();
    assert_eq!(id.name(), "B1");
    assert_eq!(id.protocol, 5);
    assert!(!id.protocol_assumed);
    assert_eq!(id.flow(), relabel_proto::PrintFlow::V4);
}

/// A printer that never answers Connect at all must still be usable: several
/// working implementations never send the command.
#[tokio::test(start_paused = true)]
async fn a_silent_connect_falls_back_without_failing() {
    let (t, _h) = MockTransport::new(Box::new(|req: &Frame| {
        if req.id == 0xC1 {
            return vec![];
        }
        reply_for(req, 512, true, true)
    }));
    let s = Session::start(Box::new(t));
    let id = handshake(&s, &Timeouts::default()).await.unwrap();
    assert_eq!(id.protocol, 0);
    assert!(id.protocol_assumed);
    assert!(id.missing.contains(&"connect"));
    assert_eq!(id.name(), "D11", "identity still read fine");
}

/// An id that names two different printers offers both rather than guessing:
/// they differ in print head width.
#[tokio::test(start_paused = true)]
async fn an_ambiguous_wire_id_reports_every_candidate() {
    let (t, _h) = MockTransport::new(Box::new(|req: &Frame| reply_for(req, 2305, true, true)));
    let s = Session::start(Box::new(t));
    let id = handshake(&s, &Timeouts::default()).await.unwrap();
    let names: Vec<&str> = id.candidates.iter().map(|m| m.name).collect();
    assert!(names.contains(&"D110"), "{names:?}");
    assert!(names.contains(&"Hi-D110"), "{names:?}");
    assert!(id.candidates.len() >= 2);
}
