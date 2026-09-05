//! Session behaviour, exercised against the in-memory printer.
//!
//! Everything here runs on tokio's paused clock, so the timeout tests take no
//! wall-clock time and the pacing tests are deterministic.

use relabel_core::session::PrinterEvent;
use relabel_core::transport::mock::{Fragmentation, MockTransport};
use relabel_core::{Session, SessionError};
use relabel_proto::{Frame, PrinterErrorCode, payload};
use std::time::Duration;

const T: Duration = Duration::from_millis(1000);

#[tokio::test(start_paused = true)]
async fn request_gets_its_documented_reply() {
    let (t, handle) = MockTransport::always_ok();
    let s = Session::start(Box::new(t));
    let reply = s.request(payload::connect(), T).await.unwrap();
    assert_eq!(reply.id, 0xC2, "Connect answers on 0xC2");
    assert_eq!(handle.received_ids(), vec![0xC1]);
}

#[tokio::test(start_paused = true)]
async fn connect_goes_out_with_its_prefix_byte() {
    let (t, _handle) = MockTransport::always_ok();
    let s = Session::start(Box::new(t));
    s.request(payload::connect(), T).await.unwrap();
    let trace = s.trace().await;
    let out = trace.iter().find(|e| e.outbound).unwrap();
    assert_eq!(
        out.bytes,
        vec![0x03, 0x55, 0x55, 0xc1, 0x01, 0x01, 0xc1, 0xaa, 0xaa]
    );
}

/// The case a real BLE link produces constantly and every naive client gets
/// wrong: one frame split across several reads.
#[tokio::test(start_paused = true)]
async fn a_reply_split_across_reads_still_arrives() {
    let (t, _h) = MockTransport::always_ok();
    let s = Session::start(Box::new(t.with_fragmentation(Fragmentation::Chunks(3))));
    let reply = s.request(payload::print_status(), T).await.unwrap();
    assert_eq!(reply.id, 0xB3);
}

/// And the other direction: several whole frames in one read.
#[tokio::test(start_paused = true)]
async fn coalesced_replies_are_all_seen() {
    let (t, _h) = MockTransport::new(Box::new(|req: &Frame| {
        if req.id == 0xA3 {
            // An unsolicited page counter riding along with the real reply.
            vec![
                Frame::new(0xE0, 7u16.to_be_bytes()),
                Frame::new(0xB3, [0x00, 0x07, 100, 100]),
            ]
        } else {
            vec![]
        }
    }));
    let s = Session::start(Box::new(t.with_fragmentation(Fragmentation::Coalesced)));
    let mut events = s.events();
    let reply = s.request(payload::print_status(), T).await.unwrap();
    assert_eq!(reply.id, 0xB3);
    assert_eq!(events.recv().await.unwrap(), PrinterEvent::PageIndex(7));
}

#[tokio::test(start_paused = true)]
async fn a_fault_report_ends_the_wait_instead_of_timing_out() {
    let (t, _h) = MockTransport::new(Box::new(|_req: &Frame| vec![Frame::new(0xDB, [0x01])]));
    let s = Session::start(Box::new(t));
    let started = tokio::time::Instant::now();
    let err = s.request(payload::page_start(), T).await.unwrap_err();
    assert_eq!(err, SessionError::Printer(PrinterErrorCode::CoverOpen));
    assert!(
        started.elapsed() < T,
        "should short-circuit, not run the timeout out"
    );
}

#[tokio::test(start_paused = true)]
async fn not_supported_is_distinguishable_from_a_real_failure() {
    let (t, _h) = MockTransport::new(Box::new(|_req: &Frame| vec![Frame::new(0x00, [0x01])]));
    let s = Session::start(Box::new(t));
    let err = s.request(payload::rfid_info2(), T).await.unwrap_err();
    assert!(err.is_unsupported(), "got {err:?}");
}

/// `0xE0` and `0xD3` arrive on the printer's own schedule. Matching one to a
/// pending request would desynchronise the whole conversation.
#[tokio::test(start_paused = true)]
async fn unsolicited_packets_never_satisfy_a_pending_request() {
    let (t, _h) = MockTransport::new(Box::new(|req: &Frame| {
        if req.id == 0x03 {
            // The page counter first, then the real answer.
            vec![
                Frame::new(0xE0, 3u16.to_be_bytes()),
                Frame::new(0x04, [0x01]),
            ]
        } else {
            vec![]
        }
    }));
    let s = Session::start(Box::new(t));
    let mut events = s.events();
    let reply = s.request(payload::page_start(), T).await.unwrap();
    assert_eq!(reply.id, 0x04, "the reply, not the page counter");
    assert_eq!(events.recv().await.unwrap(), PrinterEvent::PageIndex(3));
}

#[tokio::test(start_paused = true)]
async fn a_silent_printer_times_out() {
    let (t, _h) = MockTransport::new(Box::new(|_req: &Frame| vec![]));
    let s = Session::start(Box::new(t));
    let err = s.request(payload::page_start(), T).await.unwrap_err();
    assert!(
        matches!(err, SessionError::Timeout { ms: 1000, .. }),
        "{err:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn waiting_on_a_one_way_command_is_refused_without_sending_it() {
    let (t, handle) = MockTransport::always_ok();
    let s = Session::start(Box::new(t));
    let err = s.request(Frame::new(0x85, [0u8; 8]), T).await.unwrap_err();
    assert_eq!(err, SessionError::OneWayCommand(0x85));
    assert!(
        handle.received().is_empty(),
        "a refused request must have no side effect"
    );
}

#[tokio::test(start_paused = true)]
async fn bulk_sends_one_frame_per_write_by_default() {
    let (t, handle) = MockTransport::always_ok();
    let s = Session::start(Box::new(t));
    let rows: Vec<Frame> = (0..5u16)
        .map(|i| Frame::new(0x84, [i.to_be_bytes()[0], i.to_be_bytes()[1], 1]))
        .collect();
    s.send_bulk(&rows).await.unwrap();
    assert_eq!(
        handle.write_count(),
        5,
        "bundling is off until it is tested"
    );
    assert_eq!(handle.received().len(), 5);
}

#[tokio::test(start_paused = true)]
async fn bundling_packs_whole_frames_and_never_splits_one() {
    let (t, handle) = MockTransport::always_ok();
    let mut s = Session::start(Box::new(t));
    s.set_bundling(true);
    let rows: Vec<Frame> = (0..20u16)
        .map(|i| Frame::new(0x84, [i.to_be_bytes()[0], i.to_be_bytes()[1], 1]))
        .collect();
    s.send_bulk(&rows).await.unwrap();
    assert!(handle.write_count() < 20, "bundling should coalesce");
    // The mock decodes with the real accumulator, so every frame arriving whole
    // is proof none was cut in half.
    assert_eq!(handle.received().len(), 20);
}

#[tokio::test(start_paused = true)]
async fn a_frame_too_large_for_the_link_is_refused_not_split() {
    let (t, handle) = MockTransport::always_ok();
    let s = Session::start(Box::new(t.with_max_write(32)));
    let err = s
        .send_oneway(Frame::new(0x85, vec![0u8; 200]))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            SessionError::Transport(relabel_core::TransportError::FrameTooLarge { .. })
        ),
        "{err:?}"
    );
    assert!(handle.received().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_trace_records_both_directions_in_order() {
    let (t, _h) = MockTransport::always_ok();
    let s = Session::start(Box::new(t));
    s.request(payload::connect(), T).await.unwrap();
    let trace = s.trace().await;
    assert_eq!(trace.len(), 2);
    assert!(trace[0].outbound);
    assert_eq!(trace[0].name, "Connect");
    assert!(!trace[1].outbound);
    assert_eq!(trace[1].id, 0xC2);
}

#[tokio::test(start_paused = true)]
async fn a_dropped_link_ends_a_pending_request() {
    let (t, handle) = MockTransport::new(Box::new(|_req: &Frame| vec![]));
    let s = Session::start(Box::new(t));
    handle.disconnect();
    let err = s.request(payload::page_start(), T).await.unwrap_err();
    assert!(
        matches!(
            err,
            SessionError::Transport(relabel_core::TransportError::Disconnected)
        ),
        "{err:?}"
    );
}
