//! Whole print jobs against a fake printer that counts pages.
//!
//! These are the tests that would have caught the D101 mis-mapping: they assert
//! on the exact command sequence and payload widths that go out, per flow.

use relabel_core::job::{PrintOptions, Timeouts, print};
use relabel_core::session::Session;
use relabel_core::transport::mock::{MockHandle, MockTransport};
use relabel_core::{JobError, SessionError};
use relabel_proto::{
    BitPlane, EncodedPage, HeadSpec, PrintDirection, PrintFlow, PrinterErrorCode, encode_page,
};
use std::sync::{Arc, Mutex};

/// A printer that acknowledges everything and advances its page counter as a
/// real one would.
///
/// The copy count reaches it two different ways depending on the flow - inside
/// `SetPageSize` for the B1-style flows, or as its own `PrintQuantity` command
/// for the D11-style ones - and one upload of N copies advances the counter N
/// times. A fake that ignored that would make a passing test out of a job the
/// hardware would stall on.
fn counting_printer() -> (MockTransport, MockHandle) {
    let pages = Arc::new(Mutex::new(0u16));
    let copies = Arc::new(Mutex::new(1u16));
    MockTransport::new(Box::new(move |req: &relabel_proto::Frame| {
        use relabel_proto::{Expect, Frame, RequestId};
        match req.id {
            // The six-byte SetPageSize carries copies in its last field.
            0x13 if req.data.len() >= 6 => {
                *copies.lock().unwrap() = u16::from_be_bytes([req.data[4], req.data[5]]).max(1);
            }
            0x15 if req.data.len() >= 2 => {
                *copies.lock().unwrap() = u16::from_be_bytes([req.data[0], req.data[1]]).max(1);
            }
            // PageEnd marks the upload done; the printer then works through
            // however many copies it was told to make.
            0xE3 => {
                let n = *copies.lock().unwrap();
                *pages.lock().unwrap() += n;
            }
            _ => {}
        }
        match RequestId(req.id).expect() {
            Expect::Nothing => vec![],
            Expect::InfoSubtype => {
                let sub = req.data.first().copied().unwrap_or(0);
                vec![Frame::new(0x40u8.wrapping_add(sub), [0x01])]
            }
            Expect::Any(set) => vec![Frame::new(set[0].0, [0x01])],
            Expect::One(r) => {
                if r.0 == 0xB3 {
                    // page u16, print %, feed %
                    let p = *pages.lock().unwrap();
                    let b = p.to_be_bytes();
                    vec![Frame::new(0xB3, [b[0], b[1], 100, 100])]
                } else {
                    vec![Frame::new(r.0, [0x01])]
                }
            }
        }
    }))
}

/// A printer that answers the job-level commands with a bare `0x00` the first
/// `n` times it sees each, and behaves like a real one after that.
///
/// This is a D110_M being asked to start a second label while the first is
/// still feeding out: it refuses the settings and `PrintStart`, and answers
/// normally a few hundred milliseconds later.
fn printer_refusing_first(n: usize) -> (MockTransport, MockHandle) {
    let pages = Arc::new(Mutex::new(0u16));
    let copies = Arc::new(Mutex::new(1u16));
    let seen: Arc<Mutex<std::collections::HashMap<u8, usize>>> =
        Arc::new(Mutex::new(Default::default()));
    MockTransport::new(Box::new(move |req: &relabel_proto::Frame| {
        use relabel_proto::{Expect, Frame, RequestId};
        let refusing = matches!(req.id, 0x01 | 0x21 | 0x23) && {
            let mut s = seen.lock().unwrap();
            let c = s.entry(req.id).or_insert(0);
            *c += 1;
            *c <= n
        };
        match req.id {
            0x13 if req.data.len() >= 6 => {
                *copies.lock().unwrap() = u16::from_be_bytes([req.data[4], req.data[5]]).max(1);
            }
            0x15 if req.data.len() >= 2 => {
                *copies.lock().unwrap() = u16::from_be_bytes([req.data[0], req.data[1]]).max(1);
            }
            0xE3 => {
                let n = *copies.lock().unwrap();
                *pages.lock().unwrap() += n;
            }
            _ => {}
        }
        let answer = if refusing { 0x00 } else { 0x01 };
        match RequestId(req.id).expect() {
            Expect::Nothing => vec![],
            Expect::InfoSubtype => {
                let sub = req.data.first().copied().unwrap_or(0);
                vec![Frame::new(0x40u8.wrapping_add(sub), [0x01])]
            }
            Expect::Any(set) => vec![Frame::new(set[0].0, [answer])],
            Expect::One(r) => {
                if r.0 == 0xB3 {
                    let p = *pages.lock().unwrap();
                    let b = p.to_be_bytes();
                    vec![Frame::new(0xB3, [b[0], b[1], 100, 100])]
                } else {
                    vec![Frame::new(r.0, [answer])]
                }
            }
        }
    }))
}

fn one_page(width: u16, height: u16, head: u16) -> Vec<EncodedPage> {
    let mut p = BitPlane::new(width, height);
    p.set(0, 0, true);
    vec![encode_page(&p, &HeadSpec::new(head, PrintDirection::Top)).unwrap()]
}

/// The D11 family. One-byte PrintStart, four-byte SetPageSize, PrintClear
/// before each page, copies in PrintQuantity.
#[tokio::test(start_paused = true)]
async fn base_flow_sends_the_d11_sequence() {
    let (t, handle) = counting_printer();
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    print(
        &s,
        PrintFlow::Base,
        &pages,
        &PrintOptions::default(),
        |_| {},
    )
    .await
    .unwrap();

    let got = handle.received();
    let ids: Vec<u8> = got.iter().map(|f| f.id).collect();
    // Settings, start, then the page, then the wait, then the end.
    assert_eq!(&ids[..6], &[0x21, 0x23, 0x01, 0x20, 0x03, 0x13]);
    assert!(ids.contains(&0x15), "PrintQuantity carries the copy count");
    assert_eq!(*ids.last().unwrap(), 0xF3, "PrintEnd is last");

    let start = got.iter().find(|f| f.id == 0x01).unwrap();
    assert_eq!(start.data.len(), 1, "one-byte PrintStart");
    let size = got.iter().find(|f| f.id == 0x13).unwrap();
    assert_eq!(size.data.len(), 4, "four-byte SetPageSize");
    assert_eq!(&size.data[..2], &40u16.to_be_bytes(), "rows first");
    assert_eq!(&size.data[2..], &96u16.to_be_bytes(), "then columns");
}

/// The B1 flow at protocol 3 and above. Seven-byte PrintStart, six-byte
/// SetPageSize, no PrintClear, copies inside the geometry.
#[tokio::test(start_paused = true)]
async fn v4_flow_sends_the_b1_sequence() {
    let (t, handle) = counting_printer();
    let s = Session::start(Box::new(t));
    let pages = one_page(384, 40, 384);
    let opts = PrintOptions {
        copies: 3,
        ..PrintOptions::default()
    };
    print(&s, PrintFlow::V4, &pages, &opts, |_| {})
        .await
        .unwrap();

    let got = handle.received();
    let ids: Vec<u8> = got.iter().map(|f| f.id).collect();
    assert!(!ids.contains(&0x20), "V4 sends no PrintClear");
    assert!(
        !ids.contains(&0x15),
        "copies ride in SetPageSize, not PrintQuantity"
    );

    let start = got.iter().find(|f| f.id == 0x01).unwrap();
    assert_eq!(start.data.len(), 7);
    assert_eq!(&start.data[..2], &3u16.to_be_bytes(), "total pages");
    let size = got.iter().find(|f| f.id == 0x13).unwrap();
    assert_eq!(size.data.len(), 6);
    assert_eq!(&size.data[4..], &3u16.to_be_bytes(), "copies");
}

/// The correction that came out of vendor-source analysis, end to end: no
/// protocol level puts a D101 on the seven-byte start.
#[tokio::test(start_paused = true)]
async fn d101_never_sends_a_seven_byte_print_start() {
    for protocol in 0..=5u8 {
        let (t, handle) = counting_printer();
        let s = Session::start(Box::new(t));
        let flow = relabel_proto::select_flow(Some(2560), protocol);
        let pages = one_page(192, 40, 192);
        print(&s, flow, &pages, &PrintOptions::default(), |_| {})
            .await
            .unwrap();
        let start = handle
            .received()
            .into_iter()
            .find(|f| f.id == 0x01)
            .unwrap();
        assert!(
            start.data.len() <= 2,
            "protocol {protocol} produced a {}-byte PrintStart",
            start.data.len()
        );
    }
}

/// Skipping this leaves the label parked under the print head.
#[tokio::test(start_paused = true)]
async fn print_end_runs_even_when_the_job_fails() {
    let (t, handle) = MockTransport::new(Box::new(|req: &relabel_proto::Frame| {
        use relabel_proto::{Expect, Frame, RequestId};
        // Refuse PageStart outright.
        if req.id == 0x03 {
            return vec![Frame::new(0x04, [0x00])];
        }
        match RequestId(req.id).expect() {
            Expect::Nothing => vec![],
            Expect::One(r) => vec![Frame::new(r.0, [0x01])],
            Expect::Any(set) => vec![Frame::new(set[0].0, [0x01])],
            Expect::InfoSubtype => vec![],
        }
    }));
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    let err = print(
        &s,
        PrintFlow::Base,
        &pages,
        &PrintOptions::default(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(err, JobError::Refused { step: "PageStart" });
    assert!(
        handle.received_ids().contains(&0xF3),
        "PrintEnd must still go out on the error path"
    );
}

#[tokio::test(start_paused = true)]
async fn a_printer_fault_aborts_the_job() {
    let (t, handle) = MockTransport::new(Box::new(|req: &relabel_proto::Frame| {
        use relabel_proto::{Expect, Frame, RequestId};
        if req.id == 0x13 {
            // Cover open, in place of the geometry acknowledgement.
            return vec![Frame::new(0xDB, [0x01])];
        }
        match RequestId(req.id).expect() {
            Expect::Nothing => vec![],
            Expect::One(r) => vec![Frame::new(r.0, [0x01])],
            Expect::Any(set) => vec![Frame::new(set[0].0, [0x01])],
            Expect::InfoSubtype => vec![],
        }
    }));
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    let err = print(
        &s,
        PrintFlow::Base,
        &pages,
        &PrintOptions::default(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        JobError::Session(SessionError::Printer(PrinterErrorCode::CoverOpen))
    );
    assert!(handle.received_ids().contains(&0xF3));
}

#[tokio::test(start_paused = true)]
async fn progress_reaches_the_total_and_stops() {
    let (t, _h) = counting_printer();
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let opts = PrintOptions {
        copies: 1,
        ..PrintOptions::default()
    };
    print(&s, PrintFlow::Base, &pages, &opts, move |p| {
        sink.lock().unwrap().push(p);
    })
    .await
    .unwrap();
    let seen = seen.lock().unwrap();
    let last = seen.last().expect("at least one progress report");
    assert_eq!(last.page, 1);
    assert_eq!(last.total, 1);
}

/// A printer that acknowledges everything but never advances its counter is the
/// documented failure mode of a stalled job, and it must end rather than hang.
#[tokio::test(start_paused = true)]
async fn a_counter_that_never_advances_hits_the_job_deadline() {
    let (t, handle) = MockTransport::new(Box::new(|req: &relabel_proto::Frame| {
        use relabel_proto::{Expect, Frame, RequestId};
        match RequestId(req.id).expect() {
            Expect::Nothing => vec![],
            Expect::One(r) if r.0 == 0xB3 => {
                vec![Frame::new(0xB3, [0x00, 0x00, 0, 0])]
            }
            Expect::One(r) => vec![Frame::new(r.0, [0x01])],
            Expect::Any(set) => vec![Frame::new(set[0].0, [0x01])],
            Expect::InfoSubtype => vec![],
        }
    }));
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    let err = print(
        &s,
        PrintFlow::Base,
        &pages,
        &PrintOptions::default(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(err, JobError::DeadlineExceeded { .. }), "{err:?}");
    assert!(handle.received_ids().contains(&0xF3));
}

#[tokio::test(start_paused = true)]
async fn rows_are_streamed_before_the_page_ends() {
    let (t, handle) = counting_printer();
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 20, 96);
    print(
        &s,
        PrintFlow::Base,
        &pages,
        &PrintOptions::default(),
        |_| {},
    )
    .await
    .unwrap();
    let ids = handle.received_ids();
    let first_row = ids.iter().position(|i| *i == 0x85 || *i == 0x84).unwrap();
    let page_end = ids.iter().position(|i| *i == 0xE3).unwrap();
    let geometry = ids.iter().position(|i| *i == 0x13).unwrap();
    assert!(geometry < first_row, "geometry before rows");
    assert!(first_row < page_end, "rows before PageEnd");
}

/// The D110 acknowledges a multi-copy request, prints once, and then refuses
/// everything. Asking it for three copies in one job produces one label and a
/// timeout, so the copies have to become three complete jobs.
#[tokio::test(start_paused = true)]
async fn a_printer_that_caps_at_one_page_gets_one_job_per_copy() {
    // Mimics the documented behaviour: the counter never passes 1 within a job,
    // and it resets when a new job starts.
    let printed = Arc::new(Mutex::new(0u16));
    let in_job = Arc::new(Mutex::new(0u16));
    let tally = Arc::clone(&printed);
    let (t, handle) = MockTransport::new(Box::new(move |req: &relabel_proto::Frame| {
        use relabel_proto::{Expect, Frame, RequestId};
        match req.id {
            0x01 => *in_job.lock().unwrap() = 0,
            0xE3 => {
                let mut n = in_job.lock().unwrap();
                if *n == 0 {
                    *n = 1;
                    *tally.lock().unwrap() += 1;
                }
            }
            _ => {}
        }
        match RequestId(req.id).expect() {
            Expect::Nothing => vec![],
            Expect::One(r) if r.0 == 0xB3 => {
                let p = *in_job.lock().unwrap();
                let b = p.to_be_bytes();
                vec![Frame::new(0xB3, [b[0], b[1], 100, 100])]
            }
            Expect::One(r) => vec![Frame::new(r.0, [0x01])],
            Expect::Any(set) => vec![Frame::new(set[0].0, [0x01])],
            Expect::InfoSubtype => vec![],
        }
    }));
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    let opts = PrintOptions {
        copies: 3,
        ..PrintOptions::default()
    }
    .for_model(Some(2304));
    assert_eq!(
        opts.copies_per_job,
        Some(1),
        "the D110 is a known-capped model"
    );

    print(&s, PrintFlow::Base, &pages, &opts, |_| {})
        .await
        .unwrap();

    assert_eq!(*printed.lock().unwrap(), 3, "three labels came out");
    let starts = handle.received_ids().iter().filter(|i| **i == 0x01).count();
    let ends = handle.received_ids().iter().filter(|i| **i == 0xF3).count();
    assert_eq!(starts, 3, "three complete jobs");
    assert_eq!(ends, 3, "each one ended properly");
}

/// A model with no known cap must not be split into separate jobs.
#[tokio::test(start_paused = true)]
async fn an_uncapped_printer_still_gets_one_job() {
    let (t, handle) = counting_printer();
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    let opts = PrintOptions {
        copies: 4,
        ..PrintOptions::default()
    }
    .for_model(Some(512)); // D11
    assert_eq!(opts.copies_per_job, None);
    print(&s, PrintFlow::Base, &pages, &opts, |_| {})
        .await
        .unwrap();
    let starts = handle.received_ids().iter().filter(|i| **i == 0x01).count();
    assert_eq!(starts, 1, "one job carrying four copies");
}

/// A D110_M reports protocol level 4, which is the widest flow: nine-byte
/// PrintStart, thirteen-byte SetPageSize, no PageStart, label type set before
/// density, and two deliberately discarded packets.
#[tokio::test(start_paused = true)]
async fn v5_flow_matches_what_a_d110_m_wants() {
    let (t, handle) = counting_printer();
    let s = Session::start(Box::new(t));
    let flow = relabel_proto::select_flow(Some(2320), 4);
    assert_eq!(flow, PrintFlow::V5);
    let pages = one_page(96, 40, 96);
    print(&s, flow, &pages, &PrintOptions::default(), |_| {})
        .await
        .unwrap();

    let got = handle.received();
    let ids: Vec<u8> = got.iter().map(|f| f.id).collect();

    assert!(!ids.contains(&0x03), "no PageStart on this flow");
    assert!(!ids.contains(&0x20), "and no PrintClear");

    // Label type before density.
    let lt = ids.iter().position(|i| *i == 0x23).unwrap();
    let de = ids.iter().position(|i| *i == 0x21).unwrap();
    assert!(lt < de, "label type is set first");

    let start = got.iter().find(|f| f.id == 0x01).unwrap();
    assert_eq!(start.data.len(), 9);
    let size = got.iter().find(|f| f.id == 0x13).unwrap();
    assert_eq!(size.data.len(), 13);

    // A discarded status request right after PrintStart, and a discarded
    // heartbeat after PrintEnd.
    let ps = ids.iter().position(|i| *i == 0x01).unwrap();
    let first_status = ids.iter().position(|i| *i == 0xA3).unwrap();
    assert!(first_status > ps, "the sacrificial send follows PrintStart");
    assert_eq!(
        *ids.last().unwrap(),
        0xDC,
        "and a heartbeat trails PrintEnd"
    );
}

/// A refused step is sent again. On a D110_M the second label of a run is
/// refused for a moment while the first is still feeding out, and a job that
/// took the byte at face value printed one label of two and then waited out
/// its deadline.
#[tokio::test(start_paused = true)]
async fn a_refused_step_is_sent_again() {
    let (t, handle) = printer_refusing_first(2);
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    print(&s, PrintFlow::V5, &pages, &PrintOptions::default(), |_| {})
        .await
        .unwrap();
    let ids = handle.received_ids();
    let count = |id: u8| ids.iter().filter(|i| **i == id).count();
    assert_eq!(
        count(0x01),
        3,
        "PrintStart: two refusals, then the one that took"
    );
    assert_eq!(count(0x23), 3, "and the settings the same way");
    assert_eq!(count(0x21), 3);
    assert_eq!(
        &ids[ids.len() - 2..],
        &[0xF3, 0xDC],
        "PrintEnd, then the discarded heartbeat this flow ends on"
    );
}

/// A printer that means its refusal stops the job, naming the step. The old
/// behaviour was to stream a page into a printer with no job open and then
/// wait out 25 seconds for a page counter that could never move.
#[tokio::test(start_paused = true)]
async fn a_printer_that_keeps_refusing_stops_the_job() {
    let (t, handle) = printer_refusing_first(usize::MAX);
    let s = Session::start(Box::new(t));
    let pages = one_page(96, 40, 96);
    let opts = PrintOptions {
        timeouts: Timeouts {
            refusals: 4,
            ..Timeouts::default()
        },
        ..PrintOptions::default()
    };
    let e = print(&s, PrintFlow::V5, &pages, &opts, |_| {})
        .await
        .unwrap_err();
    assert!(matches!(e, JobError::Refused { .. }), "{e:?}");
    let ids = handle.received_ids();
    assert_eq!(
        ids.iter().filter(|i| **i == 0x23).count(),
        4,
        "tried the agreed number of times"
    );
    assert!(
        !ids.iter().any(|i| matches!(i, 0x84 | 0x85)),
        "no page data reaches a printer that refused the job"
    );
    assert_eq!(
        &ids[ids.len() - 2..],
        &[0xF3, 0xDC],
        "PrintEnd, then the discarded heartbeat this flow ends on"
    );
}
