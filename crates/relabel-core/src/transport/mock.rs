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

use crate::error::TransportError;
use crate::transport::{Transport, TransportKind};
use async_trait::async_trait;
use relabel_proto::{Accumulator, Event, Frame};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Decides what a request gets back. Return an empty vec for silence.
pub type Responder = Box<dyn FnMut(&Frame) -> Vec<Frame> + Send>;

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
        MockTransport::new(Box::new(|req: &Frame| {
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
        }))
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
