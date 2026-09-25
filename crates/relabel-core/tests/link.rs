//! The printer link's actor, against printers that exist only in memory.
//!
//! All but the last three tests run the actor from `link::parts` on tokio's
//! paused clock, so the 15-second connect deadline and the handshake's timeouts
//! take no wall-clock time and every timing assertion is exact. The last three
//! run the real threaded link, on real time, the way the editor does.

use async_trait::async_trait;
use relabel_core::job::{PrintOptions, Reached, RunEnd, RunNews, RunReport, print_labels};
use relabel_core::link::mock::{self, Behaviour, D110_M_NAME, MockDevice, MockLink, MockOpener};
use relabel_core::link::{
    self, CLOSE_WITHIN, DeviceKey, Events, Failure, Finished, Heard, Link, LinkAfter, NotPrintable,
    Opened, Opener, Order, PrintRefusal, Seen,
};
use relabel_core::transport::mock::{
    MockHandle, MockTransport, Responder, d110_m_quick, d110_m_silent_after_handshake, identifying,
    silent,
};
use relabel_core::{HandshakeStep, Identity, JobError, Session, SessionError, TransportError};
use relabel_proto::payload::{HeartbeatKind, InfoField};
use relabel_proto::{
    BitPlane, EncodedPage, Frame, HeadSpec, LabelType, PrintDirection, encode_page, models,
};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;

const SCAN: u64 = 4;

fn start(opener: MockOpener) -> (Link, Events, JoinHandle<()>) {
    let (link, events, actor) = link::parts(Box::new(opener));
    (link, events, tokio::spawn(actor))
}

/// The next thing heard, or a panic naming what was being waited for.
async fn next(events: &mut Events, what: &str) -> Heard {
    match tokio::time::timeout(Duration::from_secs(60), events.next()).await {
        Ok(Some(h)) => h,
        Ok(None) => panic!("the actor ended while waiting for {what}"),
        Err(_) => panic!("nothing heard while waiting for {what}"),
    }
}

/// Whether anything at all is heard within `within`.
async fn quiet_for(events: &mut Events, within: Duration) -> Option<Heard> {
    tokio::time::timeout(within, events.next())
        .await
        .ok()
        .flatten()
}

/// Scan with ticket 1 and return what was seen.
async fn find(link: &Link, events: &mut Events) -> Vec<Seen> {
    link.send(1, Order::Find { secs: SCAN }).unwrap();
    match next(events, "Found").await {
        Heard::Found { ticket: 1, seen } => seen,
        other => panic!("{other:?}"),
    }
}

/// Everything heard up to and including the first Ready or Failed.
async fn until_settled(events: &mut Events) -> Vec<Heard> {
    let mut heard = Vec::new();
    loop {
        let h = next(events, "Ready or Failed").await;
        let done = matches!(h, Heard::Ready { .. } | Heard::Failed { .. });
        heard.push(h);
        if done {
            return heard;
        }
    }
}

/// Scan, open the first printer found with ticket 1, and wait for Ready.
async fn connected(devices: Vec<MockDevice>) -> (Link, Events, JoinHandle<()>, MockLink) {
    let (opener, mock) = MockOpener::new(devices);
    let (link, mut events, actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let heard = until_settled(&mut events).await;
    assert!(
        matches!(heard.last(), Some(Heard::Ready { ticket: 1, .. })),
        "{heard:?}"
    );
    (link, events, actor, mock)
}

fn ambiguous() -> Responder {
    identifying(2305, None, true)
}

fn b21() -> Responder {
    identifying(768, None, true)
}

#[tokio::test(start_paused = true)]
async fn one_heard_printer_is_found_and_identified() {
    let (opener, _mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let (link, mut events, _actor) = start(opener);
    let started = Instant::now();
    let seen = find(&link, &mut events).await;
    assert_eq!(started.elapsed(), Duration::from_secs(SCAN));
    assert_eq!(
        seen,
        vec![Seen {
            key: DeviceKey(0),
            name: D110_M_NAME.into(),
            heard: true
        }]
    );

    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let heard = until_settled(&mut events).await;
    let steps: Vec<HandshakeStep> = heard
        .iter()
        .filter_map(|h| match h {
            Heard::Step { ticket: 1, step } => Some(*step),
            _ => None,
        })
        .collect();
    assert_eq!(steps.len(), 11, "{steps:?}");
    assert_eq!(steps[0], HandshakeStep::Connect);
    assert_eq!(steps[1], HandshakeStep::Status);
    assert_eq!(steps[10], HandshakeStep::Heartbeat);
    assert_eq!(heard.len(), 12, "only steps come before Ready: {heard:?}");

    let Some(Heard::Ready { ticket: 1, facts }) = heard.last() else {
        panic!("{heard:?}");
    };
    assert_eq!(facts.name, D110_M_NAME);
    assert_eq!(facts.model.map(|m| m.name), Some("D110_M"));
    assert_eq!(facts.head, 96);
    assert_eq!(facts.dpi, 203);
    assert_eq!(facts.direction, PrintDirection::Left);
    assert_eq!(facts.max_write, 244);
    assert_eq!(facts.firmware.as_deref(), Some("4.17"));
    assert_eq!(facts.battery_percent, Some(75));
    assert_eq!(facts.printable, Ok(()));
}

/// `mock::facts` stands in for a handshake in the editor's tests, so it has to
/// be what a handshake through the link actually gives.
#[tokio::test(start_paused = true)]
async fn the_mock_facts_are_what_a_handshake_reads() {
    for (device, id) in [
        (MockDevice::d110_m(), 2320),
        (
            MockDevice::new("D110-X", true, Behaviour::Answers(ambiguous)),
            2305,
        ),
    ] {
        let name = device.name.clone();
        let (opener, _mock) = MockOpener::new(vec![device]);
        let (link, mut events, _actor) = start(opener);
        let seen = find(&link, &mut events).await;
        link.send(1, Order::Open { key: seen[0].key }).unwrap();
        let heard = until_settled(&mut events).await;
        let Some(Heard::Ready { facts, .. }) = heard.last() else {
            panic!("{heard:?}");
        };
        assert_eq!(*facts, mock::facts(id, &name), "wire id {id}");
    }
}

#[tokio::test(start_paused = true)]
async fn heard_printers_come_first() {
    let (opener, _mock) = MockOpener::new(vec![
        MockDevice::new("B21-C1", false, Behaviour::Answers(b21)),
        MockDevice::d110_m(),
    ]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    let names: Vec<&str> = seen.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, [D110_M_NAME, "B21-C1"]);
    assert_eq!(
        seen[1].key,
        DeviceKey(0),
        "keys follow the device, not the order"
    );
}

#[tokio::test(start_paused = true)]
async fn an_empty_scan_is_an_empty_list() {
    let (opener, _mock) = MockOpener::new(vec![]);
    let (link, mut events, _actor) = start(opener);
    assert!(find(&link, &mut events).await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_scan_failure_keeps_its_kind() {
    for failure in [
        Failure::BluetoothOff,
        Failure::NotAllowed,
        Failure::NoAdapter,
    ] {
        let (opener, _mock) = MockOpener::failing(failure.clone());
        let (link, mut events, _actor) = start(opener);
        link.send(1, Order::Find { secs: SCAN }).unwrap();
        assert_eq!(
            next(&mut events, "Failed").await,
            Heard::Failed { ticket: 1, failure }
        );
    }
}

#[tokio::test(start_paused = true)]
async fn connect_gives_up_at_fifteen_seconds() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::new(
        "D110_M-OFF",
        false,
        Behaviour::NeverConnects,
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    let asked = Instant::now();
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    assert_eq!(
        next(&mut events, "Failed").await,
        Heard::Failed {
            ticket: 1,
            failure: Failure::TimedOut {
                name: "D110_M-OFF".into(),
                secs: 15
            }
        }
    );
    assert_eq!(asked.elapsed(), Duration::from_secs(15));
    assert_eq!(mock.abandons(), 1);
}

#[tokio::test(start_paused = true)]
async fn stop_waiting_cancels_a_connect_and_abandons_it() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::new(
        "D110_M-OFF",
        true,
        Behaviour::NeverConnects,
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    link.send(2, Order::StopWaiting).unwrap();
    assert_eq!(quiet_for(&mut events, Duration::from_secs(20)).await, None);
    assert_eq!(mock.abandons(), 1);

    link.send(3, Order::Find { secs: SCAN }).unwrap();
    assert!(matches!(
        next(&mut events, "Found").await,
        Heard::Found { ticket: 3, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn stop_waiting_cancels_a_scan() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let (link, mut events, _actor) = start(opener);
    link.send(1, Order::Find { secs: SCAN }).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    link.send(2, Order::StopWaiting).unwrap();
    assert_eq!(quiet_for(&mut events, Duration::from_secs(10)).await, None);
    assert_eq!(mock.stops(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_refused_connect_passes_bluetooths_words() {
    let (opener, _mock) = MockOpener::new(vec![MockDevice::new(
        "D110_M-X",
        true,
        Behaviour::Refuses("le-connection-abort-by-local".into()),
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    assert_eq!(
        next(&mut events, "Failed").await,
        Heard::Failed {
            ticket: 1,
            failure: Failure::ConnectFailed {
                name: "D110_M-X".into(),
                detail: "le-connection-abort-by-local".into()
            }
        }
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_with_no_channel_is_said_to_have_none() {
    let (opener, _mock) = MockOpener::new(vec![MockDevice::new(
        "D110_M-X",
        true,
        Behaviour::NoChannel,
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    assert_eq!(
        next(&mut events, "Failed").await,
        Heard::Failed {
            ticket: 1,
            failure: Failure::NoChannel {
                name: "D110_M-X".into()
            }
        }
    );
}

#[tokio::test(start_paused = true)]
async fn a_link_that_dies_mid_handshake_names_the_step() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::new(
        D110_M_NAME,
        true,
        Behaviour::DropsAfter(3, mock::d110_m),
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let heard = until_settled(&mut events).await;
    let steps: Vec<HandshakeStep> = heard
        .iter()
        .filter_map(|h| match h {
            Heard::Step { step, .. } => Some(*step),
            _ => None,
        })
        .collect();
    assert_eq!(steps.len(), 4, "{steps:?}");
    assert_eq!(
        steps[3],
        HandshakeStep::Info {
            field: InfoField::SerialNumber,
            n: 2
        }
    );
    assert_eq!(
        heard.last(),
        Some(&Heard::Failed {
            ticket: 1,
            failure: Failure::Handshake {
                name: D110_M_NAME.into(),
                step: steps[3]
            }
        })
    );
    assert!(mock.transport().unwrap().is_closed());
}

/// A link that goes after the eight reads, as the heartbeat is sent, is a
/// failed handshake at the heartbeat, not a printer said to be ready.
#[tokio::test(start_paused = true)]
async fn a_link_that_dies_at_the_heartbeat_is_not_ready() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::new(
        D110_M_NAME,
        true,
        Behaviour::DropsAfter(10, mock::d110_m),
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let heard = until_settled(&mut events).await;
    assert_eq!(
        heard.last(),
        Some(&Heard::Failed {
            ticket: 1,
            failure: Failure::Handshake {
                name: D110_M_NAME.into(),
                step: HandshakeStep::Heartbeat
            }
        }),
        "{heard:?}"
    );
    assert!(mock.transport().unwrap().is_closed());
}

#[tokio::test(start_paused = true)]
async fn the_platform_disconnect_is_dropped() {
    let (_link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    mock.drop_link();
    assert_eq!(
        next(&mut events, "Dropped").await,
        Heard::Dropped { ticket: 1 }
    );
    assert!(mock.transport().unwrap().is_closed());
}

#[tokio::test(start_paused = true)]
async fn a_drop_during_the_handshake_is_said_as_one() {
    // A silent printer keeps the handshake going for ten seconds, so the drop
    // lands during it; an answering one would be identified before the test
    // runs again.
    let (opener, mock) = MockOpener::new(vec![MockDevice::new(
        D110_M_NAME,
        true,
        Behaviour::Answers(silent),
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    assert!(matches!(
        next(&mut events, "a step").await,
        Heard::Step { .. }
    ));
    let dropped = Instant::now();
    mock.drop_link();
    let heard = until_settled(&mut events).await;
    assert_eq!(
        heard.last(),
        Some(&Heard::Failed {
            ticket: 1,
            failure: Failure::DroppedWhileConnecting {
                name: D110_M_NAME.into()
            }
        })
    );
    assert_eq!(
        dropped.elapsed(),
        Duration::ZERO,
        "at once, not at a timeout"
    );
    assert!(mock.transport().unwrap().is_closed());
}

#[tokio::test(start_paused = true)]
async fn close_disconnects_without_a_drop() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(1, Order::Close).unwrap();
    assert_eq!(
        next(&mut events, "Closed").await,
        Heard::Closed { ticket: 1 }
    );
    assert!(mock.transport().unwrap().is_closed());
    mock.drop_link();
    assert_eq!(quiet_for(&mut events, Duration::from_secs(5)).await, None);
}

#[tokio::test(start_paused = true)]
async fn close_with_nothing_connected_still_answers() {
    let (opener, _mock) = MockOpener::new(vec![]);
    let (link, mut events, _actor) = start(opener);
    link.send(7, Order::Close).unwrap();
    assert_eq!(
        next(&mut events, "Closed").await,
        Heard::Closed { ticket: 7 }
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_closes_the_link_and_ends_the_actor() {
    let (link, _events, actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(2, Order::Shutdown).unwrap();
    tokio::time::timeout(Duration::from_secs(10), actor)
        .await
        .expect("the actor ends")
        .unwrap();
    assert!(mock.transport().unwrap().is_closed());
}

#[tokio::test(start_paused = true)]
async fn dropping_the_link_closes_it_too() {
    let (link, _events, actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    drop(link);
    tokio::time::timeout(Duration::from_secs(10), actor)
        .await
        .expect("the actor ends")
        .unwrap();
    assert!(mock.transport().unwrap().is_closed());
}

/// Every read of a silent printer runs its timeout out, so it takes the
/// handshake's full ten seconds: Connect, eight reads and a heartbeat at one
/// second each. No status query goes out, because a silent Connect leaves the
/// level at 0.
#[tokio::test(start_paused = true)]
async fn a_silent_printer_connects_but_cannot_print() {
    let (opener, _mock) = MockOpener::new(vec![MockDevice::new(
        "Quiet",
        true,
        Behaviour::Answers(silent),
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    let asked = Instant::now();
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let heard = until_settled(&mut events).await;
    assert_eq!(asked.elapsed(), Duration::from_secs(10));
    let Some(Heard::Ready { facts, .. }) = heard.last() else {
        panic!("{heard:?}");
    };
    assert_eq!(
        facts.printable,
        Err(NotPrintable::UnknownModel { model_id: None })
    );
    assert_eq!(facts.head, 0);
}

#[tokio::test(start_paused = true)]
async fn an_ambiguous_id_is_not_printable() {
    let (opener, _mock) = MockOpener::new(vec![MockDevice::new(
        "D110-X",
        true,
        Behaviour::Answers(ambiguous),
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let heard = until_settled(&mut events).await;
    let Some(Heard::Ready { facts, .. }) = heard.last() else {
        panic!("{heard:?}");
    };
    match &facts.printable {
        Err(NotPrintable::CandidatesDisagree { names }) => {
            assert!(names.contains(&"D110"), "{names:?}");
            assert!(names.contains(&"Hi-D110"), "{names:?}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn an_order_mid_operation_cancels_it_then_runs() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::new(
        "D110_M-OFF",
        true,
        Behaviour::NeverConnects,
    )]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    link.send(2, Order::Find { secs: SCAN }).unwrap();
    assert!(matches!(
        next(&mut events, "Found").await,
        Heard::Found { ticket: 2, .. }
    ));
    assert_eq!(mock.abandons(), 1);
}

/// The editor never thought it was connected, so nothing is said.
#[tokio::test(start_paused = true)]
async fn stop_waiting_after_ready_closes_the_link() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(2, Order::StopWaiting).unwrap();
    assert_eq!(quiet_for(&mut events, Duration::from_secs(5)).await, None);
    assert!(mock.transport().unwrap().is_closed());
}

// Printing. `MockDevice::d110_m` is a `FakePrinter` taking a tenth of a
// second a label.

/// A 40-row label as wide as a head `head` dots wide. Drawn already turned,
/// so its columns are the head's dots.
fn page(head: u16) -> EncodedPage {
    let mut p = BitPlane::new(head, 40);
    p.set(0, 0, true);
    encode_page(&p, &HeadSpec::new(head, PrintDirection::Top)).unwrap()
}

fn count(frames: &[Frame], id: u8) -> usize {
    frames.iter().filter(|f| f.id == id).count()
}

/// Everything heard up to and including the first that `last` picks.
async fn until(events: &mut Events, what: &str, last: impl Fn(&Heard) -> bool) -> Vec<Heard> {
    let mut heard = Vec::new();
    loop {
        let h = next(events, what).await;
        let done = last(&h);
        heard.push(h);
        if done {
            return heard;
        }
    }
}

fn is_news(news: RunNews) -> impl Fn(&Heard) -> bool {
    move |h| matches!(h, Heard::Job { news: n, .. } if *n == news)
}

/// Everything heard up to and including JobEnded, and that JobEnded's report
/// and link.
async fn until_ended(events: &mut Events) -> (Vec<Heard>, RunReport, LinkAfter) {
    let heard = until(events, "JobEnded", |h| matches!(h, Heard::JobEnded { .. })).await;
    let Some(Heard::JobEnded { report, link, .. }) = heard.last().cloned() else {
        unreachable!()
    };
    (heard, report, link)
}

fn done(n: u16) -> RunReport {
    RunReport {
        printed: n,
        of: n,
        end: RunEnd::Done,
    }
}

#[tokio::test(start_paused = true)]
async fn print_runs_one_job_per_label_and_answers_once() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    let (heard, report, after) = until_ended(&mut events).await;
    assert_eq!((report, after), (done(3), LinkAfter::Up));
    for h in &heard {
        assert!(
            matches!(
                h,
                Heard::Job { ticket: 1, .. } | Heard::JobEnded { ticket: 1, .. }
            ),
            "{h:?}"
        );
    }
    assert_eq!(quiet_for(&mut events, Duration::from_secs(30)).await, None);

    let t = mock.transport().unwrap();
    let starts: Vec<_> = t.received().into_iter().filter(|f| f.id == 0x01).collect();
    assert_eq!(starts.len(), 3);
    for start in starts {
        assert_eq!(start.data[..2], [0, 1], "one page a job");
    }

    link.send(1, Order::Close).unwrap();
    assert_eq!(
        next(&mut events, "Closed").await,
        Heard::Closed { ticket: 1 }
    );
    assert!(t.is_closed());
}

/// What the actor sends is `print_labels` and nothing else.
#[tokio::test(start_paused = true)]
async fn print_frames_are_the_loops() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    let t = mock.transport().unwrap();
    let before = t.received().len();
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    let (_, report, _) = until_ended(&mut events).await;
    assert_eq!(report, done(3));

    let candidates: Vec<_> = models::by_wire_id(2320).collect();
    let id = Identity {
        protocol: 4,
        model_id: Some(2320),
        model: candidates.first().copied(),
        candidates,
        ..Identity::default()
    };
    let (fresh, handle) = MockTransport::new(d110_m_quick());
    let s = Session::start(Box::new(fresh));
    let alone = print_labels(
        &s,
        id.flow(),
        &page(96),
        &PrintOptions::for_identity(&id, None),
        3,
        HeartbeatKind::Advanced2,
        &AtomicBool::new(false),
        |_| {},
    )
    .await;
    assert_eq!(alone, done(3));
    assert_eq!(t.received()[before..], handle.received()[..]);
}

#[tokio::test(start_paused = true)]
async fn facts_carry_what_the_job_sends() {
    let (opener, _mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let (link, mut events, _actor) = start(opener);
    let seen = find(&link, &mut events).await;
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let heard = until_settled(&mut events).await;
    let Some(Heard::Ready { facts, .. }) = heard.last() else {
        panic!("{heard:?}");
    };
    assert_eq!(facts.print_density, 3);
    assert_eq!(facts.print_label_type, LabelType::Gap);
    assert_eq!(*facts, mock::facts(2320, D110_M_NAME));
}

#[tokio::test(start_paused = true)]
async fn stop_after_label_is_read_between_labels() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    until(
        &mut events,
        "Sending 1",
        is_news(RunNews::Sending { n: 1, of: 3 }),
    )
    .await;
    link.send(1, Order::StopAfterLabel).unwrap();
    let (_, report, after) = until_ended(&mut events).await;
    assert_eq!(
        (report, after),
        (
            RunReport {
                printed: 1,
                of: 3,
                end: RunEnd::Stopped
            },
            LinkAfter::Up
        )
    );
    let frames = mock.transport().unwrap().received();
    assert_eq!(count(&frames, 0x01), 1);
    assert_eq!(frames.last().map(|f| f.id), Some(0xDC));
}

#[tokio::test(start_paused = true)]
async fn print_then_stop_at_once_prints_nothing() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    let t = mock.transport().unwrap();
    let before = t.received().len();
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    link.send(1, Order::StopAfterLabel).unwrap();
    let (_, report, _) = until_ended(&mut events).await;
    assert_eq!(
        report,
        RunReport {
            printed: 0,
            of: 3,
            end: RunEnd::Stopped
        }
    );
    assert_eq!(t.received_ids()[before..], [0xDC]);
}

#[tokio::test(start_paused = true)]
async fn close_mid_job_finishes_the_label_then_closes() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    until(&mut events, "Printer 1", |h| {
        matches!(
            h,
            Heard::Job {
                news: RunNews::Printer { n: 1, .. },
                ..
            }
        )
    })
    .await;
    link.send(1, Order::Close).unwrap();
    let (_, report, _) = until_ended(&mut events).await;
    assert_eq!(
        report,
        RunReport {
            printed: 1,
            of: 3,
            end: RunEnd::Stopped
        }
    );
    assert_eq!(
        next(&mut events, "Closed").await,
        Heard::Closed { ticket: 1 }
    );
    let t = mock.transport().unwrap();
    assert!(t.is_closed());
    assert_eq!(count(&t.received(), 0xF3), 1);
}

#[tokio::test(start_paused = true)]
async fn find_mid_job_waits_for_the_label() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    until(
        &mut events,
        "Sending 1",
        is_news(RunNews::Sending { n: 1, of: 3 }),
    )
    .await;
    link.send(2, Order::Find { secs: SCAN }).unwrap();
    let (heard, report, _) = until_ended(&mut events).await;
    assert!(
        !heard.iter().any(|h| matches!(h, Heard::Found { .. })),
        "{heard:?}"
    );
    assert_eq!(report.printed, 1);
    assert!(matches!(
        next(&mut events, "Found").await,
        Heard::Found { ticket: 2, .. }
    ));
    let t = mock.transport().unwrap();
    assert!(t.is_closed());
    assert_eq!(count(&t.received(), 0xF3), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_mid_job_finishes_the_label() {
    let (link, mut events, actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    until(
        &mut events,
        "Sending 1",
        is_news(RunNews::Sending { n: 1, of: 3 }),
    )
    .await;
    link.send(2, Order::Shutdown).unwrap();
    tokio::time::timeout(Duration::from_secs(60), actor)
        .await
        .expect("the actor ends")
        .unwrap();
    let t = mock.transport().unwrap();
    assert!(t.is_closed());
    let ids = t.received_ids();
    assert_eq!(count(&t.received(), 0x01), 1, "no second label");
    assert_eq!(
        ids[ids.len() - 2..],
        [0xF3, 0xDC],
        "the label ended properly"
    );
}

#[tokio::test(start_paused = true)]
async fn dropping_every_sender_mid_job_finishes_the_label_and_ends() {
    let (link, mut events, actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    until(
        &mut events,
        "Sending 1",
        is_news(RunNews::Sending { n: 1, of: 3 }),
    )
    .await;
    drop(link);
    tokio::time::timeout(Duration::from_secs(60), actor)
        .await
        .expect("the actor ends, and does not spin")
        .unwrap();
    let t = mock.transport().unwrap();
    assert!(t.is_closed());
    assert_eq!(count(&t.received(), 0x01), 1);
    assert_eq!(count(&t.received(), 0xF3), 1);
}

#[tokio::test(start_paused = true)]
async fn a_drop_mid_job_ends_it_at_once() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    until(&mut events, "Printer 2", |h| {
        matches!(
            h,
            Heard::Job {
                news: RunNews::Printer { n: 2, .. },
                ..
            }
        )
    })
    .await;
    let dropped = Instant::now();
    mock.drop_link();
    let (_, report, after) = until_ended(&mut events).await;
    assert!(dropped.elapsed() <= CLOSE_WITHIN);
    assert_eq!(after, LinkAfter::Gone);
    assert_eq!(report.printed, 1);
    let RunEnd::Failed {
        label,
        reached,
        error,
    } = report.end
    else {
        panic!("{report:?}");
    };
    assert_eq!(label, 2);
    assert!(
        matches!(reached, Reached::Rows | Reached::Waiting),
        "{reached:?}"
    );
    assert_eq!(error, JobError::Session(SessionError::Ended));
    assert_eq!(
        quiet_for(&mut events, Duration::from_secs(10)).await,
        None,
        "no Dropped"
    );

    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 1,
        },
    )
    .unwrap();
    assert_eq!(
        next(&mut events, "PrintRefused").await,
        Heard::PrintRefused {
            ticket: 1,
            why: PrintRefusal::NotConnected
        }
    );
}

/// A write that never returns keeps the job, and the session's close, waiting
/// on it; the platform's word still ends the job within the close's bound.
#[tokio::test(start_paused = true)]
async fn a_hung_write_does_not_hold_the_actor() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 3,
        },
    )
    .unwrap();
    until(
        &mut events,
        "Sending 1",
        is_news(RunNews::Sending { n: 1, of: 3 }),
    )
    .await;
    mock.transport().unwrap().hang_writes();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let dropped = Instant::now();
    mock.drop_link();
    let (_, _, after) = until_ended(&mut events).await;
    assert_eq!(after, LinkAfter::Gone);
    assert!(
        dropped.elapsed() <= CLOSE_WITHIN + Duration::from_secs(1),
        "{:?}",
        dropped.elapsed()
    );
    link.send(1, Order::Close).unwrap();
    assert_eq!(
        next(&mut events, "Closed").await,
        Heard::Closed { ticket: 1 }
    );
}

/// A write that fails with the platform saying nothing: the link is taken to
/// have gone, and is closed.
#[tokio::test(start_paused = true)]
async fn a_failed_write_mid_job_closes_the_link() {
    // Eleven handshake requests, the check, then SetLabelType and SetDensity;
    // the link dies as PrintStart is written.
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::new(
        D110_M_NAME,
        true,
        Behaviour::DropsAfter(14, mock::d110_m),
    )])
    .await;
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 2,
        },
    )
    .unwrap();
    let (_, report, after) = until_ended(&mut events).await;
    assert_eq!(after, LinkAfter::Gone);
    assert_eq!(
        report.end,
        RunEnd::Failed {
            label: 1,
            reached: Reached::Settings,
            error: JobError::Session(SessionError::Transport(TransportError::Disconnected))
        }
    );
    assert!(mock.transport().unwrap().is_closed());
    assert_eq!(
        quiet_for(&mut events, Duration::from_secs(10)).await,
        None,
        "no Dropped"
    );
}

#[tokio::test(start_paused = true)]
async fn print_refusals_send_nothing() {
    let refused = |why| Heard::PrintRefused { ticket: 1, why };
    let print = |labels, head| Order::Print {
        page: page(head),
        labels,
    };

    let (opener, _mock) = MockOpener::new(vec![]);
    let (link, mut events, _actor) = start(opener);
    link.send(1, print(1, 96)).unwrap();
    assert_eq!(
        next(&mut events, "a refusal").await,
        refused(PrintRefusal::NotConnected)
    );

    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    let t = mock.transport().unwrap();
    let before = t.received().len();
    for (order, why) in [
        (print(0, 96), PrintRefusal::BadCount),
        (print(100, 96), PrintRefusal::BadCount),
        (print(1, 104), PrintRefusal::WrongHead),
    ] {
        link.send(1, order).unwrap();
        assert_eq!(next(&mut events, "a refusal").await, refused(why));
    }
    assert_eq!(t.received().len(), before);

    let (link, mut events, _actor, mock) = connected(vec![MockDevice::new(
        "D110-X",
        true,
        Behaviour::Answers(ambiguous),
    )])
    .await;
    let t = mock.transport().unwrap();
    let before = t.received().len();
    link.send(1, print(1, 96)).unwrap();
    assert_eq!(
        next(&mut events, "a refusal").await,
        refused(PrintRefusal::WrongHead)
    );
    assert_eq!(t.received().len(), before);
}

#[tokio::test(start_paused = true)]
async fn a_second_print_mid_job_is_busy() {
    let (link, mut events, _actor, _mock) = connected(vec![MockDevice::d110_m()]).await;
    let print = Order::Print {
        page: page(96),
        labels: 2,
    };
    link.send(1, print.clone()).unwrap();
    link.send(2, print).unwrap();
    let (heard, report, _) = until_ended(&mut events).await;
    let busy: Vec<_> = heard
        .iter()
        .filter(|h| matches!(h, Heard::PrintRefused { .. }))
        .collect();
    assert_eq!(
        busy,
        [&Heard::PrintRefused {
            ticket: 2,
            why: PrintRefusal::Busy
        }]
    );
    assert_eq!(report, done(2));
    assert_eq!(quiet_for(&mut events, Duration::from_secs(30)).await, None);
}

#[tokio::test(start_paused = true)]
async fn a_silent_printer_fails_the_check_and_keeps_the_link() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::new(
        D110_M_NAME,
        true,
        Behaviour::Answers(d110_m_silent_after_handshake),
    )])
    .await;
    let t = mock.transport().unwrap();
    let before = t.received().len();
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 2,
        },
    )
    .unwrap();
    let (_, report, after) = until_ended(&mut events).await;
    assert_eq!(after, LinkAfter::Up);
    assert!(
        matches!(
            report.end,
            RunEnd::Failed {
                label: 1,
                reached: Reached::Check,
                error: JobError::Session(SessionError::Timeout { .. })
            }
        ),
        "{report:?}"
    );
    assert_eq!(t.received_ids()[before..], [0xDC]);
    link.send(1, Order::Close).unwrap();
    assert_eq!(
        next(&mut events, "Closed").await,
        Heard::Closed { ticket: 1 }
    );
}

#[tokio::test(start_paused = true)]
async fn job_news_carries_the_open_ticket_and_idle_stop_is_ignored() {
    let (link, mut events, _actor, mock) = connected(vec![MockDevice::d110_m()]).await;
    link.send(1, Order::StopAfterLabel).unwrap();
    assert_eq!(quiet_for(&mut events, Duration::from_secs(5)).await, None);

    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 2,
        },
    )
    .unwrap();
    let (heard, report, _) = until_ended(&mut events).await;
    assert_eq!(report, done(2), "the idle stop was not kept");
    assert!(heard.iter().all(|h| h.ticket() == 1), "{heard:?}");

    mock.drop_link();
    assert_eq!(
        next(&mut events, "Dropped").await,
        Heard::Dropped { ticket: 1 }
    );
}

/// The editor's arrangement: the actor on its own thread and runtime, orders
/// sent and news read from a thread with no runtime at all.
#[test]
fn the_threaded_link_starts_and_shuts_down() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let (link, mut events) =
        Link::start(Box::new(opener.with_scan(Duration::from_millis(10)))).unwrap();

    link.send(1, Order::Find { secs: SCAN }).unwrap();
    let seen = match events.wait(Duration::from_secs(2)) {
        Some(Heard::Found { ticket: 1, seen }) => seen,
        other => panic!("{other:?}"),
    };
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match events.wait(left) {
            Some(Heard::Ready { ticket: 1, facts }) => {
                assert_eq!(facts.model.map(|m| m.name), Some("D110_M"));
                break;
            }
            Some(Heard::Step { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    assert_eq!(link.shutdown(Duration::from_secs(5)), Finished::Clean);
    assert!(mock.transport().unwrap().is_closed());
    // The actor has gone, so nothing more will ever be heard.
    assert_eq!(events.wait(Duration::from_millis(50)), None);
}

/// Opens one printer over the in-memory transport, and then panics in the
/// actor itself, from the future that watches for the platform's disconnect.
struct PanicsWhileConnected {
    transport: Arc<Mutex<Option<MockHandle>>>,
}

#[async_trait]
impl Opener for PanicsWhileConnected {
    async fn find(&mut self, _secs: u64) -> Result<Vec<Seen>, Failure> {
        Ok(vec![Seen {
            key: DeviceKey(0),
            name: D110_M_NAME.into(),
            heard: true,
        }])
    }

    async fn stop_finding(&mut self) {}

    async fn open(&mut self, _key: DeviceKey) -> Result<Opened, Failure> {
        let (t, h) = MockTransport::new(mock::d110_m());
        *self.transport.lock().unwrap() = Some(h);
        Ok(Opened {
            session: Session::start(Box::new(t)),
            name: D110_M_NAME.into(),
            gone: Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                panic!("a deliberate panic on the link thread");
            }),
        })
    }

    async fn abandon(&mut self, _key: DeviceKey) {}
}

/// A panic on the link thread while a printer is connected still closes that
/// printer's link, rather than leaving it up with nobody to close it.
#[test]
fn a_panic_on_the_link_thread_still_closes_the_link() {
    let transport = Arc::default();
    let opener = PanicsWhileConnected {
        transport: Arc::clone(&transport),
    };
    let (link, mut events) = Link::start(Box::new(opener)).unwrap();

    link.send(1, Order::Find { secs: SCAN }).unwrap();
    let seen = match events.wait(Duration::from_secs(2)) {
        Some(Heard::Found { ticket: 1, seen }) => seen,
        other => panic!("{other:?}"),
    };
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match events.wait(left) {
            Some(Heard::Ready { ticket: 1, .. }) => break,
            Some(Heard::Step { .. }) => {}
            other => panic!("{other:?}"),
        }
    }
    // The actor ends with the panic, so nothing more is heard.
    assert_eq!(events.wait(Duration::from_secs(2)), None);
    assert_eq!(link.shutdown(Duration::from_secs(5)), Finished::Clean);
    let handle = transport.lock().unwrap().clone().unwrap();
    assert!(handle.is_closed());
}

/// The editor's arrangement again, printing: `MockDevice::d110_m` finishes its
/// labels on real time too.
#[test]
fn the_threaded_link_prints() {
    let (opener, mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let (link, mut events) =
        Link::start(Box::new(opener.with_scan(Duration::from_millis(10)))).unwrap();
    link.send(1, Order::Find { secs: SCAN }).unwrap();
    let seen = match events.wait(Duration::from_secs(2)) {
        Some(Heard::Found { ticket: 1, seen }) => seen,
        other => panic!("{other:?}"),
    };
    link.send(1, Order::Open { key: seen[0].key }).unwrap();
    let wait_for = |events: &mut Events, last: &dyn Fn(&Heard) -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match events.wait(left) {
                Some(h) if last(&h) => return h,
                Some(_) => {}
                None => panic!("nothing heard"),
            }
        }
    };
    wait_for(&mut events, &|h| matches!(h, Heard::Ready { .. }));
    link.send(
        1,
        Order::Print {
            page: page(96),
            labels: 2,
        },
    )
    .unwrap();
    let ended = wait_for(&mut events, &|h| matches!(h, Heard::JobEnded { .. }));
    assert_eq!(
        ended,
        Heard::JobEnded {
            ticket: 1,
            report: done(2),
            link: LinkAfter::Up
        }
    );
    assert_eq!(link.shutdown(Duration::from_secs(5)), Finished::Clean);
    let t = mock.transport().unwrap();
    assert!(t.is_closed());
    assert_eq!(count(&t.received(), 0x01), 2);
}
