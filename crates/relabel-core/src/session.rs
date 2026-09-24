//! One conversation with one printer.
//!
//! The transport is owned by a task, not shared. Everything else - reassembly,
//! matching a reply to its request, pacing writes, routing the packets nobody
//! asked for - happens here, once, so a new transport never has to reimplement
//! any of it.
//!
//! # One outstanding request, always
//!
//! The frame carries no sequence number and no transaction id, so a reply is
//! matched to a request by its command byte alone. Two requests in flight at
//! once would be genuinely ambiguous, and several commands share a reply id. A
//! mutex around the request path is therefore not caution, it is the only
//! correct arrangement.
//!
//! One-way traffic - bitmap rows, and the deliberate sacrificial sends some
//! print flows need - bypasses the wait but still goes through the same task, so
//! ordering is preserved.

use crate::error::{SessionError, TransportError};
use crate::transport::Transport;
use relabel_proto::{
    Accumulator, DesyncReason, Event, Expect, Frame, PrinterErrorCode, RequestId, ResponseId,
    command::UNIVERSAL_ERRORS, frame::STANDARD_OVERHEAD, parse,
};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::Instant;

/// Something the printer said that no request was waiting for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrinterEvent {
    /// The printer's own page counter (`0xE0`). The D11-family flows wait on
    /// this rather than polling.
    PageIndex(u16),
    /// The printer's count of rows it has received (`0xD3`).
    RowsReceived(u16),
    /// A fault report that arrived with no request outstanding.
    Error(PrinterErrorCode),
    /// Anything else unsolicited, kept whole for the protocol log.
    Other(Frame),
    /// Bytes had to be dropped to resynchronise. The signature of a transport
    /// dropping writes, or a framing this decoder does not know.
    Desync { bytes: usize, reason: DesyncReason },
    /// The link is gone and the session is over.
    Disconnected,
}

/// One line of the protocol log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub outbound: bool,
    pub at_ms: u64,
    pub id: u8,
    pub name: &'static str,
    pub bytes: Vec<u8>,
}

/// How many trace lines to keep. Bounded so a long job cannot grow it without
/// end; large enough that a whole small print is still in it when the user goes
/// looking for why nothing came out.
const TRACE_CAPACITY: usize = 512;

enum Command {
    Request {
        bytes: Vec<u8>,
        id: u8,
        expect: Expect,
        info_subtype: Option<u8>,
        timeout: Duration,
        reply: oneshot::Sender<Result<Frame, SessionError>>,
    },
    Write {
        writes: Vec<Vec<u8>>,
        first_id: u8,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    Trace {
        reply: oneshot::Sender<Vec<TraceEntry>>,
    },
    Close {
        reply: oneshot::Sender<()>,
    },
}

/// A live connection.
///
/// Dropping a `Session` does **not** close its transport. The task that owns
/// the transport ends when it notices every sender has gone (the `cmd.recv()`
/// branch of `run`), and it drops the transport without closing it: a BLE link
/// then stays connected until the printer or the platform gives up on it.
/// Anything that opens a session and outlives it calls [`Session::close`].
pub struct Session {
    cmd: mpsc::Sender<Command>,
    /// Serialises the request path. See the module note.
    gate: Mutex<()>,
    events: broadcast::Sender<PrinterEvent>,
    pace: Duration,
    max_write: usize,
    /// Whether to pack several whole frames into one transport write.
    ///
    /// Documented as safe and worth roughly four times the throughput on BLE,
    /// but untested on all three host platforms, so it is off until it has been.
    /// A frame is never split, whatever this says.
    bundle: bool,
}

impl Session {
    /// Take ownership of a transport and start the task that drives it.
    pub fn start(mut transport: Box<dyn Transport>) -> Session {
        let reader = transport
            .take_reader()
            .expect("a transport hands over its reader exactly once");
        let pace = transport.default_pace();
        let max_write = transport.max_write();
        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let (ev_tx, _) = broadcast::channel(256);
        tokio::spawn(run(transport, reader, cmd_rx, ev_tx.clone()));
        Session {
            cmd: cmd_tx,
            gate: Mutex::new(()),
            events: ev_tx,
            pace,
            max_write,
            bundle: false,
        }
    }

    /// Subscribe to everything the printer says unprompted.
    pub fn events(&self) -> broadcast::Receiver<PrinterEvent> {
        self.events.subscribe()
    }

    pub fn max_write(&self) -> usize {
        self.max_write
    }

    pub fn pace(&self) -> Duration {
        self.pace
    }

    pub fn set_bundling(&mut self, on: bool) {
        self.bundle = on;
    }

    /// Send a frame and wait for its documented reply.
    ///
    /// Short-circuits on `0xDB` and `0x00`, which can arrive in place of any
    /// expected reply, rather than running the timeout out.
    pub async fn request(&self, frame: Frame, timeout: Duration) -> Result<Frame, SessionError> {
        let req = RequestId(frame.id);
        let expect = req.expect();
        if expect == Expect::Nothing {
            return Err(SessionError::OneWayCommand(frame.id));
        }
        let info_subtype = if matches!(expect, Expect::InfoSubtype) {
            frame.data.first().copied()
        } else {
            None
        };
        let bytes = relabel_proto::payload::encode_on_wire(&frame)?;
        self.check_size(bytes.len())?;

        let _guard = self.gate.lock().await;
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(Command::Request {
                bytes,
                id: frame.id,
                expect,
                info_subtype,
                timeout,
                reply: tx,
            })
            .await
            .map_err(|_| SessionError::Ended)?;
        rx.await.map_err(|_| SessionError::Ended)?
    }

    /// Send a frame and do not wait. Bitmap rows, and the sacrificial sends.
    pub async fn send_oneway(&self, frame: Frame) -> Result<(), SessionError> {
        let bytes = relabel_proto::payload::encode_on_wire(&frame)?;
        self.check_size(bytes.len())?;
        self.write_all(vec![bytes], frame.id).await
    }

    /// Stream many frames in order, pacing between writes.
    ///
    /// This is the image path. Throughput here is dominated by the number of
    /// writes multiplied by the pace, not by the byte count, which is why
    /// bundling exists at all.
    pub async fn send_bulk(&self, frames: &[Frame]) -> Result<(), SessionError> {
        if frames.is_empty() {
            return Ok(());
        }
        let first_id = frames[0].id;
        let mut writes: Vec<Vec<u8>> = Vec::new();
        let mut current: Vec<u8> = Vec::new();
        for f in frames {
            let bytes = f.encode()?;
            self.check_size(bytes.len())?;
            // A frame that will not fit alongside what is already buffered
            // starts a new write. It is never cut in half - that direction is
            // undocumented on every model, so the buffer is flushed instead.
            let would_overflow = current.len() + bytes.len() > self.max_write;
            if !current.is_empty() && (!self.bundle || would_overflow) {
                writes.push(std::mem::take(&mut current));
            }
            current.extend_from_slice(&bytes);
        }
        if !current.is_empty() {
            writes.push(current);
        }
        self.write_all(writes, first_id).await
    }

    async fn write_all(&self, writes: Vec<Vec<u8>>, first_id: u8) -> Result<(), SessionError> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(Command::Write {
                writes,
                first_id,
                reply: tx,
            })
            .await
            .map_err(|_| SessionError::Ended)?;
        rx.await.map_err(|_| SessionError::Ended)?
    }

    /// The last few hundred frames, newest last.
    pub async fn trace(&self) -> Vec<TraceEntry> {
        let (tx, rx) = oneshot::channel();
        if self.cmd.send(Command::Trace { reply: tx }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Close the transport and end the session.
    ///
    /// Queued behind any command already sent, since commands run in order. A
    /// request still waiting for its reply ends with [`SessionError::Ended`],
    /// and so does every call made afterwards; [`Session::trace`] then returns
    /// nothing. Returns at once if the session has already ended, and never
    /// fails: a transport that cannot close cleanly is still finished with.
    pub async fn close(&self) {
        close_over(&self.cmd).await;
    }

    /// Something that can close this session later, from wherever the session
    /// itself has gone. While a closer is held, dropping the session does not
    /// end the task that owns the transport, so the closer can still reach it.
    /// Closing an ended session through it returns at once.
    pub fn closer(&self) -> Closer {
        Closer {
            cmd: self.cmd.clone(),
        }
    }

    fn check_size(&self, n: usize) -> Result<(), SessionError> {
        if n > self.max_write {
            return Err(TransportError::FrameTooLarge {
                frame: n,
                max: self.max_write,
            }
            .into());
        }
        Ok(())
    }
}

/// Closes a session from outside whatever owns it. See [`Session::closer`].
#[derive(Clone)]
pub struct Closer {
    cmd: mpsc::Sender<Command>,
}

impl Closer {
    /// What [`Session::close`] does.
    pub async fn close(&self) {
        close_over(&self.cmd).await;
    }
}

async fn close_over(cmd: &mpsc::Sender<Command>) {
    let (tx, rx) = oneshot::channel();
    if cmd.send(Command::Close { reply: tx }).await.is_err() {
        return;
    }
    let _ = rx.await;
}

struct Pending {
    expect: Expect,
    info_subtype: Option<u8>,
    deadline: Instant,
    timeout: Duration,
    reply: oneshot::Sender<Result<Frame, SessionError>>,
}

async fn run(
    mut transport: Box<dyn Transport>,
    mut reader: mpsc::UnboundedReceiver<Vec<u8>>,
    mut cmd: mpsc::Receiver<Command>,
    events: broadcast::Sender<PrinterEvent>,
) {
    let started = Instant::now();
    let pace = transport.default_pace();
    let mut acc = Accumulator::new();
    let mut pending: Option<Pending> = None;
    let mut trace: VecDeque<TraceEntry> = VecDeque::with_capacity(TRACE_CAPACITY);

    loop {
        // `sleep_until` on a far-future instant when nothing is pending, so the
        // branch is always well formed and simply never fires.
        let deadline = pending
            .as_ref()
            .map(|p| p.deadline)
            .unwrap_or_else(|| started + Duration::from_secs(86_400));

        tokio::select! {
            biased;

            // Inbound bytes first: a reply that has already arrived should be
            // delivered before a timeout that expires in the same tick.
            chunk = reader.recv() => {
                let Some(chunk) = chunk else {
                    let _ = events.send(PrinterEvent::Disconnected);
                    if let Some(p) = pending.take() {
                        let _ = p.reply.send(Err(TransportError::Disconnected.into()));
                    }
                    return;
                };
                acc.push(&chunk);
                while let Some(ev) = acc.pop() {
                    match ev {
                        Event::Frame(f) => {
                            push_trace(&mut trace, false, started, &f);
                            dispatch(f, &mut pending, &events);
                        }
                        Event::SkippedFirmware { .. } => {}
                        Event::Desync { bytes, reason } => {
                            let _ = events.send(PrinterEvent::Desync { bytes, reason });
                        }
                    }
                }
            }

            _ = tokio::time::sleep_until(deadline), if pending.is_some() => {
                if let Some(p) = pending.take() {
                    let _ = p.reply.send(Err(SessionError::Timeout {
                        what: "a printer reply",
                        ms: p.timeout.as_millis() as u64,
                    }));
                }
            }

            next = cmd.recv() => {
                let Some(next) = next else { return };
                match next {
                    Command::Trace { reply } => {
                        let _ = reply.send(trace.iter().cloned().collect());
                    }
                    Command::Close { reply } => {
                        // Refuse new commands first, so nothing is queued
                        // behind a close that may take a while. Whatever is
                        // already queued, and a request still pending, is
                        // dropped on return, which its caller sees as Ended.
                        cmd.close();
                        let _ = transport.close().await;
                        let _ = reply.send(());
                        return;
                    }
                    Command::Write { writes, first_id, reply } => {
                        let mut res = Ok(());
                        for (i, w) in writes.iter().enumerate() {
                            record_out(&mut trace, started, first_id, w);
                            if let Err(e) = transport.write(w).await {
                                res = Err(e.into());
                                break;
                            }
                            if !pace.is_zero() && i + 1 < writes.len() {
                                tokio::time::sleep(pace).await;
                            }
                        }
                        let _ = reply.send(res);
                    }
                    Command::Request { bytes, id, expect, info_subtype, timeout, reply } => {
                        record_out(&mut trace, started, id, &bytes);
                        if let Err(e) = transport.write(&bytes).await {
                            let _ = reply.send(Err(e.into()));
                            continue;
                        }
                        if !pace.is_zero() {
                            tokio::time::sleep(pace).await;
                        }
                        // A reply that beat this assignment is impossible: the
                        // reader branch cannot run until this one yields.
                        pending = Some(Pending {
                            expect,
                            info_subtype,
                            deadline: Instant::now() + timeout,
                            timeout,
                            reply,
                        });
                    }
                }
            }
        }
    }
}

/// Route one inbound frame: to the waiting request, or to the event channel.
fn dispatch(f: Frame, pending: &mut Option<Pending>, events: &broadcast::Sender<PrinterEvent>) {
    let id = ResponseId(f.id);

    // Fault reports and "not supported" can arrive in place of any expected
    // reply, so they end the wait rather than running the timeout out.
    if UNIVERSAL_ERRORS.contains(&id) {
        let err = if id == ResponseId::PRINT_ERROR {
            let code = PrinterErrorCode::from_byte(f.data.first().copied().unwrap_or(0));
            SessionError::Printer(code)
        } else {
            SessionError::NotSupported(id)
        };
        if let Some(p) = pending.take() {
            let _ = p.reply.send(Err(err));
        } else if let SessionError::Printer(code) = err {
            let _ = events.send(PrinterEvent::Error(code));
        }
        return;
    }

    // Packets the printer sends on its own schedule must never be mistaken for a
    // reply, even when one happens to be outstanding.
    if id.is_unsolicited() {
        let ev = match id {
            ResponseId::PRINTER_PAGE_INDEX => parse::page_index(&f.data)
                .map(PrinterEvent::PageIndex)
                .unwrap_or(PrinterEvent::Other(f)),
            ResponseId::PRINTER_CHECK_LINE => parse::page_index(&f.data)
                .map(PrinterEvent::RowsReceived)
                .unwrap_or(PrinterEvent::Other(f)),
            _ => PrinterEvent::Other(f),
        };
        let _ = events.send(ev);
        return;
    }

    let matched = pending
        .as_ref()
        .is_some_and(|p| id.satisfies(p.expect, p.info_subtype));
    if matched {
        if let Some(p) = pending.take() {
            let _ = p.reply.send(Ok(f));
        }
    } else {
        let _ = events.send(PrinterEvent::Other(f));
    }
}

fn push_trace(trace: &mut VecDeque<TraceEntry>, outbound: bool, started: Instant, f: &Frame) {
    let bytes = f.encode().unwrap_or_default();
    let name = if outbound {
        RequestId(f.id).name()
    } else {
        ResponseId(f.id).name()
    };
    add(
        trace,
        TraceEntry {
            outbound,
            at_ms: started.elapsed().as_millis() as u64,
            id: f.id,
            name,
            bytes,
        },
    );
}

fn record_out(trace: &mut VecDeque<TraceEntry>, started: Instant, id: u8, bytes: &[u8]) {
    add(
        trace,
        TraceEntry {
            outbound: true,
            at_ms: started.elapsed().as_millis() as u64,
            id,
            name: RequestId(id).name(),
            bytes: bytes.to_vec(),
        },
    );
}

fn add(trace: &mut VecDeque<TraceEntry>, e: TraceEntry) {
    if trace.len() == TRACE_CAPACITY {
        trace.pop_front();
    }
    trace.push_back(e);
}

/// A frame's own wire length, for sizing decisions.
pub fn wire_len(f: &Frame) -> usize {
    f.data.len() + STANDARD_OVERHEAD
}
