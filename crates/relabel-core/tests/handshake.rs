//! Identification against printers that answer awkwardly.
//!
//! Every case here is a documented real-world behaviour, not an invented edge.

use relabel_core::job::Timeouts;
use relabel_core::printer::{HandshakeStep, handshake_with};
use relabel_core::transport::mock::{MockHandle, MockTransport, identifying};
use relabel_core::{Session, handshake};
use relabel_proto::Frame;
use relabel_proto::payload::InfoField;
use std::sync::{Arc, Mutex};

/// The D110 answers the status query with a payload too short to carry a
/// version, and answers the newer heartbeat with "not supported".
///
/// The level therefore stays at the connect baseline - and that baseline must be
/// **2**, because 2 selects the two-byte `PrintStart` and 3 would select the
/// one-byte one. Getting this wrong is silent: the printer acknowledges either.
#[tokio::test(start_paused = true)]
async fn a_d110_that_cannot_resolve_its_level_stays_at_the_connect_baseline() {
    let (t, _h) = MockTransport::new(identifying(2304, None, false));
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
    assert_eq!(id.serial.as_deref(), Some("GB28010920"));
}

/// A printer that answers the status query properly gets an exact level, and it
/// is no longer flagged as assumed.
#[tokio::test(start_paused = true)]
async fn a_full_status_reply_resolves_the_level_exactly() {
    let (t, _h) = MockTransport::new(identifying(4096, Some((3, 2)), true));
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
    let mut printer = identifying(512, None, true);
    let (t, _h) = MockTransport::new(Box::new(move |req: &Frame| {
        if req.id == 0xC1 {
            return vec![];
        }
        printer(req)
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
    let (t, _h) = MockTransport::new(identifying(2305, None, true));
    let s = Session::start(Box::new(t));
    let id = handshake(&s, &Timeouts::default()).await.unwrap();
    let names: Vec<&str> = id.candidates.iter().map(|m| m.name).collect();
    assert!(names.contains(&"D110"), "{names:?}");
    assert!(names.contains(&"Hi-D110"), "{names:?}");
    assert!(id.candidates.len() >= 2);
}

/// The steps are what the GUI shows while a connect waits, so they come in the
/// order the requests go out, each reported before its request is sent.
#[tokio::test(start_paused = true)]
async fn handshake_with_names_every_step_in_order() {
    let (t, _h) = MockTransport::new(identifying(2320, Some((3, 1)), true));
    let s = Session::start(Box::new(t));
    let mut steps = Vec::new();
    let with = handshake_with(&s, &Timeouts::default(), |step| steps.push(step))
        .await
        .unwrap();

    let info = |field, n| HandshakeStep::Info { field, n };
    assert_eq!(
        steps,
        vec![
            HandshakeStep::Connect,
            HandshakeStep::Status,
            info(InfoField::ModelId, 1),
            info(InfoField::SerialNumber, 2),
            info(InfoField::HardwareVersion, 3),
            info(InfoField::BatteryChargeLevel, 4),
            info(InfoField::AutoShutdownTime, 5),
            info(InfoField::LabelType, 6),
            info(InfoField::SoftwareVersion, 7),
            info(InfoField::Density, 8),
            HandshakeStep::Heartbeat,
        ]
    );
    assert_eq!(steps[2].says(), "reading its model id (1 of 8)");

    // handshake() is the same conversation with nobody listening.
    let (t, _h) = MockTransport::new(identifying(2320, Some((3, 1)), true));
    let s = Session::start(Box::new(t));
    let plain = handshake(&s, &Timeouts::default()).await.unwrap();
    assert_eq!(plain.protocol, with.protocol);
    assert_eq!(plain.model_id, with.model_id);
    assert_eq!(plain.software, with.software);
    assert_eq!(plain.battery_percent, with.battery_percent);
}

/// The status query is only sent at level 2 or better, and a step is only
/// reported when its request is sent.
#[tokio::test(start_paused = true)]
async fn a_printer_below_level_two_is_not_asked_its_status() {
    let mut printer = identifying(512, None, true);
    let (t, h) = MockTransport::new(Box::new(move |req: &Frame| {
        if req.id == 0xC1 {
            // "Connected", which is level 0.
            return vec![Frame::new(0xC2, [0x01])];
        }
        printer(req)
    }));
    let s = Session::start(Box::new(t));
    let mut steps = Vec::new();
    let id = handshake_with(&s, &Timeouts::default(), |step| steps.push(step))
        .await
        .unwrap();
    assert_eq!(id.protocol, 0);
    assert!(!steps.contains(&HandshakeStep::Status), "{steps:?}");
    assert_eq!(steps.len(), 10);
    assert!(!h.received_ids().contains(&0xA5));
}

/// The author's printer, as every hardware record has it: wire id 2320, a
/// status reply that resolves level 4, and the V5 flow.
#[tokio::test(start_paused = true)]
async fn a_d110_m_is_level_four_and_v5() {
    let (t, _h) = MockTransport::new(identifying(2320, Some((3, 1)), true));
    let s = Session::start(Box::new(t));
    let id = handshake(&s, &Timeouts::default()).await.unwrap();
    assert_eq!(id.protocol, 4);
    assert!(!id.protocol_assumed);
    assert_eq!(id.name(), "D110_M");
    assert_eq!(id.flow(), relabel_proto::PrintFlow::V5);
    assert_eq!(id.printhead_px(), 96);
    assert_eq!(id.software.as_deref(), Some("4.17"));
    assert_eq!(id.battery_percent, Some(75));
    assert!(id.missing.is_empty(), "{:?}", id.missing);
}

/// A link that goes just before the heartbeat fails the handshake like a link
/// that goes at any other step, rather than being reported as identified with
/// the heartbeat merely missing.
#[tokio::test(start_paused = true)]
async fn a_link_lost_at_the_heartbeat_fails_the_handshake() {
    let mut printer = identifying(2320, Some((3, 1)), true);
    let link: Arc<Mutex<Option<MockHandle>>> = Arc::default();
    let lose = Arc::clone(&link);
    let mut answered = 0;
    let (t, h) = MockTransport::new(Box::new(move |req: &Frame| {
        let replies = printer(req);
        answered += 1;
        // Connect, status and the eight reads: then the link goes.
        if answered == 10
            && let Some(h) = lose.lock().unwrap().as_ref()
        {
            h.disconnect();
        }
        replies
    }));
    *link.lock().unwrap() = Some(h);
    let s = Session::start(Box::new(t));
    let mut steps = Vec::new();
    let res = handshake_with(&s, &Timeouts::default(), |step| steps.push(step)).await;
    assert!(res.is_err(), "{res:?}");
    assert_eq!(steps.last(), Some(&HandshakeStep::Heartbeat));
}

/// A session that has already ended cannot identify anything. Every request
/// on it ends at once, and that is the link gone, not a printer declining
/// each read.
#[tokio::test(start_paused = true)]
async fn a_handshake_on_an_ended_session_fails_at_connect() {
    let (t, _h) = MockTransport::new(identifying(2320, Some((3, 1)), true));
    let s = Session::start(Box::new(t));
    s.close().await;
    let mut steps = Vec::new();
    let res = handshake_with(&s, &Timeouts::default(), |step| steps.push(step)).await;
    assert!(res.is_err(), "{res:?}");
    assert_eq!(steps, vec![HandshakeStep::Connect]);
}
