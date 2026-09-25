//! A printer that exists only in memory.
//!
//! This is how the session and the job engine get tested: no Bluetooth stack, no
//! permissions, no hardware, and no flakiness. It decodes what the session
//! writes with the same accumulator the real path uses, hands each frame to a
//! responder, and feeds the replies back as if they had arrived over a wire.
//!
//! It can also be told to fragment its replies, which is the only way to test
//! the reassembly rules that matter most: a real BLE link delivers several
//! frames in one notification and splits single frames across two.
//!
//! [`FakePrinter`] plays a whole D110_M, jobs included, as the hardware was
//! recorded doing it. Like everything here it is public rather than
//! `cfg(test)`, so the link's and the editor's tests can print over it.

use crate::error::TransportError;
use crate::transport::{Transport, TransportKind};
use async_trait::async_trait;
use relabel_proto::{Accumulator, Event, Frame};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Decides what a request gets back. Return an empty vec for silence.
pub type Responder = Box<dyn FnMut(&Frame) -> Vec<Frame> + Send>;

/// A printer that identifies itself the way the handshake reads one.
///
/// `model_id` is what it reports for its model (info 8). `status` is the pair
/// that ends its status reply, `(3, 1)` meaning level 4; `None` is the two-byte
/// reply a D110 gives, too short to carry a level. `heartbeat_ok: false`
/// answers the heartbeat with "not supported", as the D110 does.
///
/// Beyond that it gives fixed answers: Connect says "level 2 or better", the
/// serial is `GB28010920`, the firmware 4.17, the hardware 4.01, the battery
/// three bars of four, and every other request gets the generic success
/// [`MockTransport::always_ok`] gives. `identifying(2320, Some((3, 1)), true)`
/// is the author's D110_M as its handshake reads it.
pub fn identifying(model_id: u16, status: Option<(u8, u8)>, heartbeat_ok: bool) -> Responder {
    Box::new(move |req: &Frame| match req.id {
        // "Connected, level 2 or better."
        0xC1 => vec![Frame::new(0xC2, [0x03])],
        0xA5 => {
            let d = match status {
                Some((hi, lo)) => {
                    let mut d = vec![0u8; 11];
                    d.extend_from_slice(&[hi, lo]);
                    d
                }
                None => vec![0x00, 0x01],
            };
            vec![Frame::new(0xB5, d)]
        }
        0xDC if !heartbeat_ok => vec![Frame::new(0x00, [0x01])],
        0x40 => {
            let sub = req.data.first().copied().unwrap_or(0);
            let payload: Vec<u8> = match sub {
                8 => model_id.to_be_bytes().to_vec(),
                11 => b"GB28010920".to_vec(),
                9 => vec![4, 17],
                12 => vec![4, 1],
                10 => vec![3],
                _ => vec![1],
            };
            vec![Frame::new(0x40u8.wrapping_add(sub), payload)]
        }
        _ => generic_success(req),
    })
}

/// A printer that answers nothing at all: every request runs its timeout out.
pub fn silent() -> Responder {
    Box::new(|_req: &Frame| vec![])
}

/// The documented success reply to any request, or nothing to a one-way one.
fn generic_success(req: &Frame) -> Vec<Frame> {
    match relabel_proto::RequestId(req.id).expect() {
        relabel_proto::Expect::Nothing => vec![],
        relabel_proto::Expect::One(r) => vec![Frame::new(r.0, [0x01])],
        relabel_proto::Expect::Any(set) => {
            vec![Frame::new(set[0].0, [0x01])]
        }
        // The reply id is 0x40 plus the requested sub-type.
        relabel_proto::Expect::InfoSubtype => {
            let sub = req.data.first().copied().unwrap_or(0);
            vec![Frame::new(0x40u8.wrapping_add(sub), [0x01])]
        }
    }
}

/// How the mock chops its replies up on the way back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fragmentation {
    /// One write per reply frame.
    #[default]
    Whole,
    /// All replies to one request coalesced into a single write, which is what a
    /// real link does when frames arrive back to back.
    Coalesced,
    /// Split every reply into chunks of this many bytes, so a frame spans
    /// several reads.
    Chunks(usize),
}

#[derive(Default)]
struct State {
    received: Vec<Frame>,
    closed: bool,
    writes: usize,
    /// Every write from now on waits forever.
    hung: bool,
}

/// Assertions and unsolicited injection, usable while the session runs.
#[derive(Clone)]
pub struct MockHandle {
    state: Arc<Mutex<State>>,
    to_session: mpsc::UnboundedSender<Vec<u8>>,
}

impl MockHandle {
    /// Every frame the session has written so far, in order.
    pub fn received(&self) -> Vec<Frame> {
        self.state.lock().unwrap().received.clone()
    }

    /// Command bytes the session has written, for terse assertions.
    pub fn received_ids(&self) -> Vec<u8> {
        self.received().iter().map(|f| f.id).collect()
    }

    /// How many separate transport writes happened, which is what actually
    /// costs time on BLE.
    pub fn write_count(&self) -> usize {
        self.state.lock().unwrap().writes
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    /// Push a frame at the session that nothing asked for.
    pub fn inject(&self, frame: &Frame) {
        if let Ok(bytes) = frame.encode() {
            let _ = self.to_session.send(bytes);
        }
    }

    /// Drop the link mid-conversation.
    pub fn disconnect(&self) {
        self.state.lock().unwrap().closed = true;
    }

    /// Make every later write wait forever, as a write to a device that went
    /// away without the platform noticing might. The session's task then
    /// waits with it, so a close queued behind that write never runs either.
    pub fn hang_writes(&self) {
        self.state.lock().unwrap().hung = true;
    }
}

pub struct MockTransport {
    to_session: mpsc::UnboundedSender<Vec<u8>>,
    reader: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    acc: Accumulator,
    responder: Responder,
    state: Arc<Mutex<State>>,
    fragmentation: Fragmentation,
    max_write: usize,
    pace: Duration,
}

impl MockTransport {
    pub fn new(responder: Responder) -> (MockTransport, MockHandle) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(State::default()));
        let handle = MockHandle {
            state: Arc::clone(&state),
            to_session: tx.clone(),
        };
        let t = MockTransport {
            to_session: tx,
            reader: Some(rx),
            acc: Accumulator::new(),
            responder,
            state,
            fragmentation: Fragmentation::Whole,
            // A typical negotiated BLE MTU less the three-byte ATT header.
            max_write: 244,
            // Tests run in simulated time; a real pace only slows them down.
            pace: Duration::ZERO,
        };
        (t, handle)
    }

    /// A printer that acknowledges everything with the documented success reply.
    ///
    /// Good enough to drive the whole job engine, and the baseline that more
    /// interesting fakes are built by wrapping.
    pub fn always_ok() -> (MockTransport, MockHandle) {
        MockTransport::new(Box::new(generic_success))
    }

    pub fn with_fragmentation(mut self, f: Fragmentation) -> Self {
        self.fragmentation = f;
        self
    }

    pub fn with_max_write(mut self, n: usize) -> Self {
        self.max_write = n;
        self
    }

    fn emit(&self, replies: &[Frame]) {
        let mut bytes = Vec::new();
        for f in replies {
            match f.encode() {
                Ok(b) => bytes.extend_from_slice(&b),
                Err(_) => continue,
            }
        }
        if bytes.is_empty() {
            return;
        }
        match self.fragmentation {
            Fragmentation::Coalesced => {
                let _ = self.to_session.send(bytes);
            }
            Fragmentation::Whole => {
                for f in replies {
                    if let Ok(b) = f.encode() {
                        let _ = self.to_session.send(b);
                    }
                }
            }
            Fragmentation::Chunks(n) => {
                for chunk in bytes.chunks(n.max(1)) {
                    let _ = self.to_session.send(chunk.to_vec());
                }
            }
        }
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<(), TransportError> {
        let hung = self.state.lock().unwrap().hung;
        if hung {
            std::future::pending::<()>().await;
        }
        if self.state.lock().unwrap().closed {
            return Err(TransportError::Disconnected);
        }
        if buf.len() > self.max_write {
            return Err(TransportError::FrameTooLarge {
                frame: buf.len(),
                max: self.max_write,
            });
        }
        self.state.lock().unwrap().writes += 1;
        self.acc.push(buf);
        let mut requests = Vec::new();
        while let Some(ev) = self.acc.pop() {
            if let Event::Frame(f) = ev {
                self.state.lock().unwrap().received.push(f.clone());
                requests.push(f);
            }
        }
        for req in requests {
            let replies = (self.responder)(&req);
            self.emit(&replies);
        }
        Ok(())
    }

    fn take_reader(&mut self) -> Option<mpsc::UnboundedReceiver<Vec<u8>>> {
        self.reader.take()
    }

    fn max_write(&self) -> usize {
        self.max_write
    }

    fn default_pace(&self) -> Duration {
        self.pace
    }

    fn kind(&self) -> TransportKind {
        TransportKind::Mock
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.state.lock().unwrap().closed = true;
        Ok(())
    }
}

/// A D110_M that prints: it counts pages per job, takes a moment to feed each
/// label out, and refuses the next job's first commands for a moment after
/// each `PrintEnd`, as PROTOCOL-NOTES 3a and 3b record the hardware doing.
///
/// Anything it does not model is answered exactly as
/// `identifying(2320, Some((3, 1)), true)` answers it, so a handshake reads it
/// as the author's printer. What it models:
///
/// - `SetLabelType`, `SetDensity` and `PrintStart` are answered with a bare
///   `0x00` (refused) within `refusal_window` of the last `PrintEnd`, and
///   accepted otherwise.
/// - An accepted `PrintStart` opens a job: the page counter goes back to 0 and
///   the copies to 1. `SetPageSize` carries the copies in its fifth and sixth
///   bytes.
/// - From `PageEnd`, one label comes out every `per_label`, its print
///   percentage rising first and then its feed percentage, until the counter
///   reaches the copies.
/// - `PrintStatus` is the D110_M's eight bytes: page, print %, feed %, two
///   bytes it always sends, a zero, and whether a job is open.
/// - `PrintEnd` closes the job. Labels the counter reached count as out; a
///   label it was part way through counts as cut.
///
/// Time is tokio's, so on a paused clock a label takes no real time at all.
#[derive(Debug, Clone)]
pub struct FakePrinter {
    per_label: Duration,
    refusal_window: Duration,
    refusing_forever: bool,
    fault_in_wait: Option<(u16, u8)>,
    fault_on_setting: Option<u8>,
    heartbeat: Heartbeat,
    stuck: bool,
    caps_one_page: bool,
    silent_after_handshake: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heartbeat {
    Answers,
    NotSupported,
    Fault(u8),
}

/// What a [`FakePrinter`] has put out, readable while it runs.
#[derive(Debug, Clone, Default)]
pub struct FakeCounts {
    counts: Arc<Mutex<Counts>>,
}

#[derive(Debug, Default)]
struct Counts {
    jobs_started: u16,
    labels_out: u16,
    labels_cut: u16,
}

impl FakeCounts {
    /// Jobs whose `PrintStart` was accepted.
    pub fn jobs_started(&self) -> u16 {
        self.counts.lock().unwrap().jobs_started
    }

    /// Labels that came out whole, counted as each job ends.
    pub fn labels_out(&self) -> u16 {
        self.counts.lock().unwrap().labels_out
    }

    /// Labels a `PrintEnd` arrived in the middle of.
    pub fn labels_cut(&self) -> u16 {
        self.counts.lock().unwrap().labels_cut
    }
}

impl FakePrinter {
    /// The author's D110_M: three seconds a label, refusing for 150 ms after
    /// each `PrintEnd`.
    pub fn d110_m() -> FakePrinter {
        FakePrinter {
            per_label: Duration::from_secs(3),
            refusal_window: Duration::from_millis(150),
            refusing_forever: false,
            fault_in_wait: None,
            fault_on_setting: None,
            heartbeat: Heartbeat::Answers,
            stuck: false,
            caps_one_page: false,
            silent_after_handshake: false,
        }
    }

    /// How long each label takes to come out, from `PageEnd`.
    pub fn per_label(mut self, d: Duration) -> FakePrinter {
        self.per_label = d;
        self
    }

    /// How long after a `PrintEnd` the next job's commands are refused.
    pub fn refusal_window(mut self, d: Duration) -> FakePrinter {
        self.refusal_window = d;
        self
    }

    /// Refuse `SetLabelType`, `SetDensity` and `PrintStart` every time.
    pub fn refusing_forever(mut self) -> FakePrinter {
        self.refusing_forever = true;
        self
    }

    /// Answer the first status poll after job `job`'s `PageEnd` with the
    /// fault `code`, jobs counted from 1.
    pub fn fault_in_wait(mut self, job: u16, code: u8) -> FakePrinter {
        self.fault_in_wait = Some((job, code));
        self
    }

    /// Answer `SetLabelType` and `SetDensity` with the fault `code`.
    pub fn fault_on_setting(mut self, code: u8) -> FakePrinter {
        self.fault_on_setting = Some(code);
        self
    }

    /// Answer the heartbeat with "not supported", as a D110 does.
    pub fn heartbeat_not_supported(mut self) -> FakePrinter {
        self.heartbeat = Heartbeat::NotSupported;
        self
    }

    /// Answer the heartbeat with the fault `code`.
    pub fn heartbeat_fault(mut self, code: u8) -> FakePrinter {
        self.heartbeat = Heartbeat::Fault(code);
        self
    }

    /// Take jobs and never print: the counter and both percentages stay at 0.
    pub fn stuck(mut self) -> FakePrinter {
        self.stuck = true;
        self
    }

    /// Print one label per job whatever the copies say, and leave the counter
    /// at 1.
    pub fn caps_one_page(mut self) -> FakePrinter {
        self.caps_one_page = true;
        self
    }

    /// Answer nothing at all once a heartbeat has been answered. After a
    /// handshake, which ends on one, that is a printer that has gone quiet.
    pub fn silent_after_handshake(mut self) -> FakePrinter {
        self.silent_after_handshake = true;
        self
    }

    pub fn responder(self) -> Responder {
        self.build().0
    }

    /// The responder, and a handle on what it has put out.
    pub fn build(self) -> (Responder, FakeCounts) {
        let counts = FakeCounts::default();
        let tally = Arc::clone(&counts.counts);
        let cfg = self;
        let mut identity = identifying(2320, Some((3, 1)), true);
        let mut job = FakeJob::default();
        let responder: Responder = Box::new(move |req: &Frame| {
            let now = tokio::time::Instant::now();
            if job.silent {
                return vec![];
            }
            let refusing = cfg.refusing_forever
                || job
                    .last_print_end
                    .is_some_and(|at| now.saturating_duration_since(at) < cfg.refusal_window);
            if matches!(req.id, 0x23 | 0x21)
                && let Some(code) = cfg.fault_on_setting
            {
                return vec![Frame::new(0xDB, [code])];
            }
            match req.id {
                0x23 | 0x21 | 0x01 if refusing => answer(req, 0x00),
                0x01 => {
                    job.open = true;
                    job.number += 1;
                    job.copies = 1;
                    job.page_end_at = None;
                    job.faulted = false;
                    tally.lock().unwrap().jobs_started += 1;
                    answer(req, 0x01)
                }
                0x13 => {
                    if req.data.len() >= 6 {
                        job.copies = u16::from_be_bytes([req.data[4], req.data[5]]).max(1);
                    }
                    answer(req, 0x01)
                }
                0xE3 => {
                    job.page_end_at = Some(now);
                    answer(req, 0x01)
                }
                0xA3 => {
                    if let Some((number, code)) = cfg.fault_in_wait
                        && job.number == number
                        && job.page_end_at.is_some()
                        && !job.faulted
                    {
                        job.faulted = true;
                        return vec![Frame::new(0xDB, [code])];
                    }
                    let (page, print, feed) = job.progress(&cfg, now);
                    let [hi, lo] = page.to_be_bytes();
                    vec![Frame::new(
                        0xB3,
                        [hi, lo, print, feed, 0x23, 0x26, 0x00, u8::from(job.open)],
                    )]
                }
                0xF3 => {
                    let (page, print, _) = job.progress(&cfg, now);
                    let target = job.target(&cfg);
                    {
                        let mut t = tally.lock().unwrap();
                        t.labels_out += page;
                        if job.page_end_at.is_some() && page < target && print > 0 {
                            t.labels_cut += 1;
                        }
                    }
                    job.open = false;
                    job.page_end_at = None;
                    job.last_print_end = Some(now);
                    answer(req, 0x01)
                }
                0xDC => {
                    if cfg.silent_after_handshake {
                        job.silent = true;
                    }
                    match cfg.heartbeat {
                        Heartbeat::Answers => identity(req),
                        Heartbeat::NotSupported => vec![Frame::new(0x00, [0x01])],
                        Heartbeat::Fault(code) => vec![Frame::new(0xDB, [code])],
                    }
                }
                _ => identity(req),
            }
        });
        (responder, counts)
    }
}

/// A [`FakePrinter`]'s job, as it stands.
#[derive(Debug, Default)]
struct FakeJob {
    open: bool,
    /// Accepted `PrintStart`s so far, so the current job's number from 1.
    number: u16,
    copies: u16,
    page_end_at: Option<tokio::time::Instant>,
    last_print_end: Option<tokio::time::Instant>,
    faulted: bool,
    silent: bool,
}

impl FakeJob {
    /// How many labels this job can put out.
    fn target(&self, cfg: &FakePrinter) -> u16 {
        if cfg.caps_one_page {
            1
        } else {
            self.copies.max(1)
        }
    }

    /// The counter, the print percentage and the feed percentage now.
    fn progress(&self, cfg: &FakePrinter, now: tokio::time::Instant) -> (u16, u8, u8) {
        let Some(at) = self.page_end_at else {
            return (0, 0, 0);
        };
        if cfg.stuck {
            return (0, 0, 0);
        }
        let per = cfg.per_label.as_millis().max(1);
        let elapsed = now.saturating_duration_since(at).as_millis();
        let target = self.target(cfg);
        let done = u16::try_from(elapsed / per).unwrap_or(u16::MAX);
        if done >= target {
            return (target, 100, 100);
        }
        // Across one label: printing over its first half, feeding over its
        // second.
        let part = u8::try_from((elapsed % per) * 200 / per).unwrap_or(200);
        (done, part.min(100), part.saturating_sub(100).min(100))
    }
}

/// The reply a step that is accepted or refused gets, carrying `byte`.
fn answer(req: &Frame, byte: u8) -> Vec<Frame> {
    generic_success(req)
        .into_iter()
        .map(|f| Frame::new(f.id, [byte]))
        .collect()
}

/// A D110_M taking three seconds a label.
pub fn d110_m_printer() -> Responder {
    FakePrinter::d110_m().responder()
}

/// A D110_M taking a tenth of a second a label, for tests on real time.
pub fn d110_m_quick() -> Responder {
    FakePrinter::d110_m()
        .per_label(Duration::from_millis(100))
        .responder()
}

/// A D110_M that answers its handshake and then nothing.
pub fn d110_m_silent_after_handshake() -> Responder {
    FakePrinter::d110_m().silent_after_handshake().responder()
}

/// A D110_M that runs out of labels while printing the second job.
pub fn d110_m_out_of_labels_on_second() -> Responder {
    FakePrinter::d110_m().fault_in_wait(2, 0x02).responder()
}

/// A D110_M that refuses every job.
pub fn d110_m_refusing_forever() -> Responder {
    FakePrinter::d110_m().refusing_forever().responder()
}

/// A D110_M that takes every job and never prints.
pub fn d110_m_stuck() -> Responder {
    FakePrinter::d110_m().stuck().responder()
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_proto::payload::{self, HeartbeatKind, IDENTITY_READS, PageColor};

    fn ask(r: &mut Responder, f: Frame) -> Vec<Frame> {
        r(&f)
    }

    #[tokio::test(start_paused = true)]
    async fn the_fake_counts_per_job_and_answers_like_the_d110_m() {
        let (mut fake, counts) = FakePrinter::d110_m().build();
        let mut real = identifying(2320, Some((3, 1)), true);

        // Every request a handshake sends, answered as `identifying` answers.
        let mut handshake = vec![payload::connect(), payload::printer_status_data()];
        handshake.extend(IDENTITY_READS.iter().map(|f| payload::printer_info(*f)));
        handshake.push(payload::heartbeat(HeartbeatKind::Advanced2));
        for req in handshake {
            assert_eq!(
                ask(&mut fake, req.clone()),
                ask(&mut real, req.clone()),
                "{req:?}"
            );
        }

        let status = |r: &mut Responder| {
            let reply = ask(r, payload::print_status());
            assert_eq!(reply.len(), 1);
            assert_eq!(reply[0].id, 0xB3);
            assert_eq!(reply[0].data.len(), 8, "the D110_M's eight bytes");
            reply[0].data.clone()
        };
        let job = |r: &mut Responder| {
            assert_eq!(
                ask(r, payload::print_start::v4(1, PageColor::SingleColor, 0, 0))[0].data,
                [0x01]
            );
            ask(r, payload::set_page_size::v4(40, 96, 1, true, 0));
            ask(r, payload::page_end());
        };

        job(&mut fake);
        assert_eq!(status(&mut fake)[..2], [0, 0]);
        assert_eq!(status(&mut fake)[7], 1, "a job is open");
        tokio::time::advance(Duration::from_secs(3)).await;
        assert_eq!(status(&mut fake)[..4], [0, 1, 100, 100]);
        ask(&mut fake, payload::print_end());
        assert_eq!(status(&mut fake)[7], 0, "and closed");

        // The next job's settings are refused for a moment, then taken.
        tokio::time::advance(Duration::from_millis(100)).await;
        let lt = payload::set_label_type(relabel_proto::LabelType::Gap);
        assert_eq!(ask(&mut fake, lt.clone())[0].data, [0x00]);
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(ask(&mut fake, lt)[0].data, [0x01]);

        job(&mut fake);
        assert_eq!(status(&mut fake)[..2], [0, 0], "the counter is per job");
        tokio::time::advance(Duration::from_millis(1500)).await;
        let half = status(&mut fake);
        assert_eq!(half[..4], [0, 0, 100, 0], "printed, not yet fed");
        tokio::time::advance(Duration::from_millis(1500)).await;
        assert_eq!(status(&mut fake)[..2], [0, 1]);
        ask(&mut fake, payload::print_end());

        assert_eq!(counts.jobs_started(), 2);
        assert_eq!(counts.labels_out(), 2);
        assert_eq!(counts.labels_cut(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_print_end_mid_label_cuts_it() {
        let (mut fake, counts) = FakePrinter::d110_m().build();
        ask(
            &mut fake,
            payload::print_start::v4(3, PageColor::SingleColor, 0, 0),
        );
        ask(&mut fake, payload::set_page_size::v4(40, 96, 3, true, 0));
        ask(&mut fake, payload::page_end());
        tokio::time::advance(Duration::from_millis(4000)).await;
        ask(&mut fake, payload::print_end());
        assert_eq!((counts.labels_out(), counts.labels_cut()), (1, 1));
    }

    #[tokio::test]
    async fn a_hung_write_never_returns() {
        let (mut t, h) = MockTransport::always_ok();
        h.hang_writes();
        let bytes = payload::connect().encode().unwrap();
        let write = tokio::time::timeout(Duration::from_millis(50), t.write(&bytes)).await;
        assert!(write.is_err(), "still waiting");
        assert!(h.received().is_empty());
    }
}
