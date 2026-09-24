//! The printer link the editor talks to.
//!
//! One actor owns everything to do with the radio: finding printers, opening
//! one, the handshake, noticing the link drop, and closing it. The editor sends
//! it [`Order`]s through a [`Link`] and hears back through [`Events`]. Nothing
//! in this module's public API names a tokio or btleplug type, so the GUI can
//! use it without depending on either.
//!
//! # Threads
//!
//! [`Link::start`] runs the actor on its own thread, `relabel-link`, inside a
//! multi-thread tokio runtime built there and dropped there. The multi-thread
//! flavour is deliberate:
//!
//! - It is the only flavour BLE has actually run on here: the command line
//!   uses `#[tokio::main]`, which is multi-thread.
//! - The session loop, the notification drain and bluez-async's D-Bus task
//!   are all spawned tasks that must keep running while the actor awaits
//!   something else. A current-thread runtime suspends spawned tasks whenever
//!   `block_on` is not running (tokio-1.53.1 `runtime/runtime.rs:283-313`),
//!   so one refactor could freeze them without any test noticing.
//!
//! `enable_all` turns on the timer everywhere and, on Linux, the IO driver
//! too: tokio's `net` feature arrives there through dbus-tokio, and bluez-async
//! needs it.
//!
//! Every btleplug object - the manager and adapter, a peripheral, the adapter
//! event stream, the notification stream, the session - is created, used and
//! dropped inside the actor or a task it spawns, so all of them live and die on
//! that runtime.
//! That matters because bluez-async's message stream calls `tokio::spawn` in
//! its `Drop` (bluez-async-0.8.2 `messagestream.rs:36-41`). The adapter is made
//! on the first [`Order::Find`], not at start, so starting a link opens no
//! system bus.
//!
//! A panic in the actor unwinds out of the runtime's `block_on`, dropping the
//! session it held unclosed. [`Link::start`] catches it there and closes the
//! last session opened before the thread ends, so the printer's link does not
//! stay up with nothing left to close it.
//!
//! [`Link::send`] is `UnboundedSender::send`, which is synchronous and needs no
//! runtime. [`Events::next`] is `UnboundedReceiver::recv`, and tokio's sync
//! primitives are runtime agnostic (tokio-1.53.1 `sync/mod.rs:438-442`), so the
//! GUI can await it on its own executor without ever entering a tokio context.
//!
//! # Cancelling
//!
//! A request dropped halfway through a live session is dangerous: its reply
//! can arrive late and be taken for the answer to the next request. So the
//! actor only abandons a handshake when it is also finished with the session -
//! on any order that arrives mid-handshake (the editor sends only
//! [`Order::StopWaiting`] then), at shutdown, or when the platform says the
//! device is gone - and it closes that session straight away before carrying
//! out the order.

use crate::error::TransportError;
use crate::job::Timeouts;
use crate::printer::{HandshakeStep, Identity, handshake_with};
use crate::session::{Closer, Session};
use crate::transport::ble::{self, BleTransport};
use async_trait::async_trait;
use btleplug::api::{Central as _, Peripheral as _};
use btleplug::platform::{Adapter, PeripheralId};
use futures_util::future::BoxFuture;
use relabel_proto::{ModelSpec, PrintDirection};
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

pub mod mock;

/// How long a connect may take, from the order to a session, before relabel
/// gives up on it. BlueZ's own limits are 30 s for the D-Bus call and then 5 s
/// for service discovery (bluez-async-0.8.2 `lib.rs:57, 61`), and the connect
/// retry sets no overall deadline, so without this a printer that is off could
/// hold the editor for well over half a minute.
pub const CONNECT_WITHIN: Duration = Duration::from_secs(15);
/// How long to spend telling the platform to give up a connect.
pub const ABANDON_WITHIN: Duration = Duration::from_secs(2);
/// How long to spend closing a session: unsubscribe, disconnect.
pub const CLOSE_WITHIN: Duration = Duration::from_secs(2);
/// How long to spend stopping a scan.
pub const STOP_FINDING_WITHIN: Duration = Duration::from_secs(1);

/// Names a device the last [`Order::Find`] reported. Only meaningful to the
/// opener that issued it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceKey(pub u32);

/// A printer a scan found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub key: DeviceKey,
    pub name: String,
    /// Whether the scan heard it just now, as far as the platform says. Used
    /// for ordering and wording only, never to decide whether to connect.
    pub heard: bool,
}

/// What the editor asks of the link. Each is sent with a ticket, and every
/// answer carries the ticket of the order it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    /// Scan for this many seconds and report the printers found.
    Find { secs: u64 },
    /// Connect to a printer the last scan found, and identify it.
    Open { key: DeviceKey },
    /// Stop a scan or a connect in progress, or close a link silently. Has no
    /// answer.
    StopWaiting,
    /// Close the link. Answered with [`Heard::Closed`] whether or not a link
    /// was up.
    Close,
    /// Stop whatever is in progress, close any link, and end the actor.
    Shutdown,
}

/// What the link tells the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    Found {
        ticket: u64,
        seen: Vec<Seen>,
    },
    /// The handshake is about to send this request.
    Step {
        ticket: u64,
        step: HandshakeStep,
    },
    Ready {
        ticket: u64,
        facts: Facts,
    },
    Failed {
        ticket: u64,
        failure: Failure,
    },
    /// The platform says the link went. Carries the ticket of the open.
    Dropped {
        ticket: u64,
    },
    Closed {
        ticket: u64,
    },
}

impl Heard {
    pub fn ticket(&self) -> u64 {
        match self {
            Heard::Found { ticket, .. }
            | Heard::Step { ticket, .. }
            | Heard::Ready { ticket, .. }
            | Heard::Failed { ticket, .. }
            | Heard::Dropped { ticket }
            | Heard::Closed { ticket } => *ticket,
        }
    }
}

/// What the editor needs to know about a connected printer, taken from its
/// handshake.
#[derive(Debug, Clone)]
pub struct Facts {
    /// The name it advertises, which carries its serial.
    pub name: String,
    pub model: Option<&'static ModelSpec>,
    pub model_id: Option<u16>,
    pub candidates: Vec<&'static ModelSpec>,
    pub protocol: u8,
    pub protocol_assumed: bool,
    pub firmware: Option<String>,
    pub battery_percent: Option<u8>,
    /// Zero, with `dpi` zero and `direction` Top, when there is no model.
    pub head: u16,
    pub dpi: u16,
    pub direction: PrintDirection,
    /// The largest single write the link accepts.
    pub max_write: usize,
    /// Whether relabel can draw for this printer at all.
    pub printable: Result<(), NotPrintable>,
}

/// Why relabel cannot print on a printer it connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotPrintable {
    /// No model, or a model whose head width the registry does not give.
    UnknownModel { model_id: Option<u16> },
    /// The id names several models whose heads differ, so drawing for the
    /// first would be a guess.
    CandidatesDisagree { names: Vec<&'static str> },
}

impl Facts {
    pub fn of(id: &Identity, name: &str, max_write: usize) -> Facts {
        let head_of = |m: &ModelSpec| (m.printhead_px, m.dpi, m.direction);
        let (head, dpi, direction) = id.model.map(head_of).unwrap_or((0, 0, PrintDirection::Top));
        let printable = match id.model {
            Some(m) if m.printhead_px != 0 => {
                if id.candidates.iter().all(|c| head_of(c) == head_of(m)) {
                    Ok(())
                } else {
                    Err(NotPrintable::CandidatesDisagree {
                        names: id.candidates.iter().map(|c| c.name).collect(),
                    })
                }
            }
            _ => Err(NotPrintable::UnknownModel {
                model_id: id.model_id,
            }),
        };
        Facts {
            name: name.to_owned(),
            model: id.model,
            model_id: id.model_id,
            candidates: id.candidates.clone(),
            protocol: id.protocol,
            protocol_assumed: id.protocol_assumed,
            firmware: id.software.clone(),
            battery_percent: id.battery_percent,
            head,
            dpi,
            direction,
            max_write,
            printable,
        }
    }
}

/// Models are compared by name: the registry is a `const`, so two references
/// to one entry need not share an address.
impl PartialEq for Facts {
    fn eq(&self, other: &Facts) -> bool {
        let names = |ms: &[&'static ModelSpec]| ms.iter().map(|m| m.name).collect::<Vec<_>>();
        self.name == other.name
            && self.model.map(|m| m.name) == other.model.map(|m| m.name)
            && self.model_id == other.model_id
            && names(&self.candidates) == names(&other.candidates)
            && self.protocol == other.protocol
            && self.protocol_assumed == other.protocol_assumed
            && self.firmware == other.firmware
            && self.battery_percent == other.battery_percent
            && self.head == other.head
            && self.dpi == other.dpi
            && self.direction == other.direction
            && self.max_write == other.max_write
            && self.printable == other.printable
    }
}

impl Eq for Facts {}

/// Why a find or an open did not get anywhere. Each carries what the editor
/// needs to say it in plain words; none carries a protocol detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The platform's Bluetooth service could not be reached, with its words.
    NoBluetooth(String),
    NoAdapter,
    BluetoothOff,
    NotAllowed,
    /// The scan itself failed, with the platform's words.
    ScanFailed(String),
    /// The device is no longer in the platform's list.
    NotFound {
        name: String,
    },
    /// The connect did not finish within [`CONNECT_WITHIN`].
    TimedOut {
        name: String,
        secs: u64,
    },
    ConnectFailed {
        name: String,
        detail: String,
    },
    /// Connected, but it has no channel relabel knows how to use.
    NoChannel {
        name: String,
    },
    /// The link died during the handshake, at this step.
    Handshake {
        name: String,
        step: HandshakeStep,
    },
    /// The platform reported the device gone during the handshake.
    DroppedWhileConnecting {
        name: String,
    },
}

/// Finds and opens printers. [`BleOpener`] is the real one; [`mock`] has one
/// that exists only in memory.
#[async_trait]
pub trait Opener: Send {
    /// Scan for `secs` seconds. The actor lists heard devices first.
    async fn find(&mut self, secs: u64) -> Result<Vec<Seen>, Failure>;
    /// Stop a scan that `find` started and the actor abandoned.
    async fn stop_finding(&mut self);
    /// Connect to a device the last `find` reported and start a session on it.
    async fn open(&mut self, key: DeviceKey) -> Result<Opened, Failure>;
    /// Give up a connect the actor abandoned part way.
    async fn abandon(&mut self, key: DeviceKey);
}

/// A connected printer, before the handshake.
pub struct Opened {
    pub session: Session,
    pub name: String,
    /// Resolves when the platform says the device has disconnected.
    pub gone: BoxFuture<'static, ()>,
}

/// The actor has ended, so the order was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the printer link has stopped")
    }
}

impl std::error::Error for Gone {}

/// How [`Link::shutdown`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finished {
    /// The link thread has ended. It says nothing about how: a thread that
    /// panicked earlier has ended too.
    Clean,
    /// The thread was still running when the wait ran out.
    TimedOut,
}

/// The editor's end of the link, for sending orders.
pub struct Link {
    orders: mpsc::UnboundedSender<(u64, Order)>,
    /// Signalled when the link thread has finished. `None` for a link made
    /// by [`parts`], whose actor the caller runs.
    done: Option<std::sync::mpsc::Receiver<()>>,
}

/// The editor's end of the link, for hearing back.
pub struct Events {
    heard: mpsc::UnboundedReceiver<Heard>,
}

/// A link whose actor the caller runs, for tests: spawn the future on a
/// runtime, paused or not. The actor ends on [`Order::Shutdown`] or when the
/// [`Link`] is dropped.
pub fn parts(opener: Box<dyn Opener>) -> (Link, Events, impl Future<Output = ()> + Send) {
    let (link, events, actor, _) = assemble(opener);
    (link, events, actor)
}

/// [`parts`], with the slot that holds a closer for the last session opened.
fn assemble(
    opener: Box<dyn Opener>,
) -> (
    Link,
    Events,
    impl Future<Output = ()> + Send,
    Arc<StdMutex<Option<Closer>>>,
) {
    let (orders_tx, orders_rx) = mpsc::unbounded_channel();
    let (heard_tx, heard_rx) = mpsc::unbounded_channel();
    let last_opened = Arc::default();
    let actor = Actor {
        opener,
        orders: orders_rx,
        heard: heard_tx,
        names: HashMap::new(),
        link: None,
        last_opened: Arc::clone(&last_opened),
    };
    (
        Link {
            orders: orders_tx,
            done: None,
        },
        Events { heard: heard_rx },
        actor.run(),
        last_opened,
    )
}

impl Link {
    /// Start the actor on its own thread, with its own runtime. See the module
    /// note. Fails when the thread or the runtime cannot be built.
    pub fn start(opener: Box<dyn Opener>) -> std::io::Result<(Link, Events)> {
        let (mut link, events, actor, last_opened) = assemble(opener);
        let (built_tx, built_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("relabel-link".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .thread_name("relabel-io")
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = built_tx.send(Err(e));
                        return;
                    }
                };
                let _ = built_tx.send(Ok(()));
                // A panic in the actor unwinds out of `block_on` and drops the
                // session it held without closing it, which leaves a Bluetooth
                // link up (see `Session`). The runtime survives the unwind, so
                // close the last session opened on it; one the actor already
                // closed answers at once. The panic has been reported by the
                // panic hook, and the editor hears the actor end.
                let ran = std::panic::catch_unwind(AssertUnwindSafe(|| rt.block_on(actor)));
                if ran.is_err() {
                    let last = match last_opened.lock() {
                        Ok(mut slot) => slot.take(),
                        Err(poisoned) => poisoned.into_inner().take(),
                    };
                    if let Some(closer) = last {
                        rt.block_on(async {
                            let _ = timeout(CLOSE_WITHIN, closer.close()).await;
                        });
                    }
                }
                // The actor has closed everything it opened; this only bounds
                // how long a stuck platform task can hold the thread.
                rt.shutdown_timeout(Duration::from_secs(1));
                let _ = done_tx.send(());
            })?;
        match built_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::other(
                    "the printer link thread ended before its runtime was built",
                ));
            }
        }
        link.done = Some(done_rx);
        Ok((link, events))
    }

    /// Hand the actor an order. Never blocks, and needs no runtime.
    pub fn send(&self, ticket: u64, order: Order) -> Result<(), Gone> {
        self.orders.send((ticket, order)).map_err(|_| Gone)
    }

    /// Stop the actor and wait up to `within` for its thread to finish.
    ///
    /// The actor stops a scan (up to 1 s), abandons a connect (2 s) and closes
    /// a session (2 s) before it returns, so five seconds covers every case
    /// that is not stuck. Blocks the calling thread, so it must not be called
    /// from inside a runtime. For a link made by [`parts`] it only sends the
    /// order.
    pub fn shutdown(self, within: Duration) -> Finished {
        let Link { orders, done } = self;
        let _ = orders.send((0, Order::Shutdown));
        drop(orders);
        let Some(done) = done else {
            return Finished::Clean;
        };
        // std's JoinHandle cannot be joined with a timeout, hence the channel.
        match done.recv_timeout(within) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Finished::Clean,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Finished::TimedOut,
        }
    }
}

impl Events {
    /// The next thing the link says, or `None` once the actor has ended.
    ///
    /// Safe to await on any executor: see the module note.
    pub async fn next(&mut self) -> Option<Heard> {
        self.heard.recv().await
    }

    /// Whatever has already arrived, without waiting. For tests.
    pub fn try_next(&mut self) -> Option<Heard> {
        self.heard.try_recv().ok()
    }

    /// Block the calling thread for up to `within` for the next thing the link
    /// says, looking every 2 ms. `None` on timeout or once the actor has ended.
    /// For tests that run the threaded link from a thread with no runtime.
    pub fn wait(&mut self, within: Duration) -> Option<Heard> {
        let deadline = std::time::Instant::now() + within;
        loop {
            match self.heard.try_recv() {
                Ok(h) => return Some(h),
                Err(mpsc::error::TryRecvError::Disconnected) => return None,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// A printer that is connected and identified.
struct Live {
    session: Session,
    gone: BoxFuture<'static, ()>,
    /// The ticket of the open, for [`Heard::Dropped`].
    ticket: u64,
}

struct Actor {
    opener: Box<dyn Opener>,
    orders: mpsc::UnboundedReceiver<(u64, Order)>,
    heard: mpsc::UnboundedSender<Heard>,
    /// Names from the last scan, for failures the opener cannot name.
    names: HashMap<DeviceKey, String>,
    link: Option<Live>,
    /// A closer for the last session opened, for [`Link::start`] to close it
    /// with if the actor panics. Overwritten by every open and never cleared:
    /// a closer for a session already closed does nothing.
    last_opened: Arc<StdMutex<Option<Closer>>>,
}

/// What woke an idle actor.
enum Wake {
    Order((u64, Order)),
    Gone,
}

/// How a handshake ended.
enum Shake {
    Done(Result<Identity, crate::SessionError>),
    Gone,
    Interrupted((u64, Order)),
}

/// What a device is called when nothing says. Starts a sentence.
const UNNAMED: &str = "The printer";

/// Every sender dropped means nobody can order anything again.
fn or_shutdown(order: Option<(u64, Order)>) -> (u64, Order) {
    order.unwrap_or((0, Order::Shutdown))
}

impl Actor {
    async fn run(mut self) {
        // An order that interrupted a find or an open, to carry out next.
        let mut next: Option<(u64, Order)> = None;
        loop {
            let (ticket, order) = match next.take() {
                Some(order) => order,
                None => match self.wait().await {
                    Wake::Order(order) => order,
                    Wake::Gone => {
                        self.dropped().await;
                        continue;
                    }
                },
            };
            match order {
                Order::Find { secs } => {
                    self.close_link().await;
                    next = self.find(ticket, secs).await;
                }
                Order::Open { key } => {
                    self.close_link().await;
                    next = self.open(ticket, key).await;
                }
                // Anything in progress was already stopped by this order
                // arriving; a link that is up goes quietly, because the editor
                // never thought it connected.
                Order::StopWaiting => self.close_link().await,
                Order::Close => {
                    self.close_link().await;
                    self.say(Heard::Closed { ticket });
                }
                Order::Shutdown => {
                    self.close_link().await;
                    return;
                }
            }
        }
    }

    /// Wait for an order, and while a link is up, for it to go.
    async fn wait(&mut self) -> Wake {
        let order = match &mut self.link {
            Some(live) => tokio::select! {
                // Orders first: a Close that arrives with the drop still
                // closes, and says Closed rather than Dropped.
                biased;
                order = self.orders.recv() => order,
                () = &mut live.gone => return Wake::Gone,
            },
            None => self.orders.recv().await,
        };
        Wake::Order(or_shutdown(order))
    }

    async fn find(&mut self, ticket: u64, secs: u64) -> Option<(u64, Order)> {
        let found = tokio::select! {
            biased;
            order = self.orders.recv() => {
                let _ = timeout(STOP_FINDING_WITHIN, self.opener.stop_finding()).await;
                return Some(or_shutdown(order));
            }
            found = self.opener.find(secs) => found,
        };
        match found {
            Ok(mut seen) => {
                // Stable, so the opener's own order holds within each group.
                seen.sort_by_key(|s| !s.heard);
                self.names = seen.iter().map(|s| (s.key, s.name.clone())).collect();
                self.say(Heard::Found { ticket, seen });
            }
            Err(failure) => self.say(Heard::Failed { ticket, failure }),
        }
        None
    }

    async fn open(&mut self, ticket: u64, key: DeviceKey) -> Option<(u64, Order)> {
        let opened = tokio::select! {
            biased;
            order = self.orders.recv() => {
                let _ = timeout(ABANDON_WITHIN, self.opener.abandon(key)).await;
                return Some(or_shutdown(order));
            }
            opened = timeout(CONNECT_WITHIN, self.opener.open(key)) => opened,
        };
        let Opened {
            session,
            name,
            mut gone,
        } = match opened {
            Ok(Ok(opened)) => {
                if let Ok(mut slot) = self.last_opened.lock() {
                    *slot = Some(opened.session.closer());
                }
                opened
            }
            Ok(Err(failure)) => {
                self.say(Heard::Failed { ticket, failure });
                return None;
            }
            Err(_elapsed) => {
                let _ = timeout(ABANDON_WITHIN, self.opener.abandon(key)).await;
                let name = self.names.get(&key).cloned();
                self.say(Heard::Failed {
                    ticket,
                    failure: Failure::TimedOut {
                        name: name.unwrap_or_else(|| UNNAMED.into()),
                        secs: CONNECT_WITHIN.as_secs(),
                    },
                });
                return None;
            }
        };

        let mut last = None;
        let shake = {
            let heard = &self.heard;
            let t = Timeouts::default();
            let handshake = handshake_with(&session, &t, |step| {
                last = Some(step);
                let _ = heard.send(Heard::Step { ticket, step });
            });
            tokio::select! {
                biased;
                order = self.orders.recv() => Shake::Interrupted(or_shutdown(order)),
                () = &mut gone => Shake::Gone,
                identity = handshake => Shake::Done(identity),
            }
        };
        match shake {
            Shake::Done(Ok(identity)) => {
                let facts = Facts::of(&identity, &name, session.max_write());
                self.link = Some(Live {
                    session,
                    gone,
                    ticket,
                });
                self.say(Heard::Ready { ticket, facts });
                None
            }
            Shake::Done(Err(_)) => {
                drop(gone);
                close(session).await;
                let step = last.unwrap_or(HandshakeStep::Connect);
                self.say(Heard::Failed {
                    ticket,
                    failure: Failure::Handshake { name, step },
                });
                None
            }
            Shake::Gone => {
                close(session).await;
                self.say(Heard::Failed {
                    ticket,
                    failure: Failure::DroppedWhileConnecting { name },
                });
                None
            }
            Shake::Interrupted(order) => {
                drop(gone);
                close(session).await;
                Some(order)
            }
        }
    }

    /// The platform said the link went.
    async fn dropped(&mut self) {
        if let Some(Live {
            session, ticket, ..
        }) = self.link.take()
        {
            close(session).await;
            self.say(Heard::Dropped { ticket });
        }
    }

    /// Close the link if one is up. Its `gone` is dropped first, so no
    /// Dropped can follow.
    async fn close_link(&mut self) {
        if let Some(Live { session, gone, .. }) = self.link.take() {
            drop(gone);
            close(session).await;
        }
    }

    fn say(&self, heard: Heard) {
        // Nobody listening is not the actor's problem: it still closes what it
        // opens when told to stop.
        let _ = self.heard.send(heard);
    }
}

async fn close(session: Session) {
    let _ = timeout(CLOSE_WITHIN, session.close()).await;
}

/// The real opener: Bluetooth Low Energy through btleplug.
///
/// Makes the adapter on the first find, not before, so a link that is started
/// and never asked to find anything opens no system bus.
#[derive(Default)]
pub struct BleOpener {
    adapter: Option<Adapter>,
    /// What the last find reported, by key. Keys are never reused, so a key
    /// from an older find is simply not found.
    table: HashMap<DeviceKey, (PeripheralId, String)>,
    next_key: u32,
}

impl BleOpener {
    pub fn new() -> BleOpener {
        BleOpener::default()
    }

    async fn adapter(&mut self) -> Result<Adapter, Failure> {
        if let Some(a) = &self.adapter {
            return Ok(a.clone());
        }
        // Making the adapter connects to the system bus with a blocking call
        // inside an async fn (bluez-async-0.8.2 `lib.rs:285-286`, reached from
        // btleplug-0.13.0 `bluez/manager.rs:13-16`). Run on the actor's own
        // thread, that call would keep the actor from seeing Stop waiting or
        // Shutdown until the bus answered, so it runs on a worker, and the
        // actor only awaits its handle. A handle dropped by an interrupted find
        // leaves the task to finish on the runtime and drop what it made.
        let a = match tokio::spawn(ble::adapter()).await {
            Ok(made) => made.map_err(scan_failure)?,
            Err(e) => return Err(Failure::NoBluetooth(e.to_string())),
        };
        self.adapter = Some(a.clone());
        Ok(a)
    }
}

/// A Bluetooth failure while looking for printers.
fn scan_failure(e: TransportError) -> Failure {
    match e {
        TransportError::NoBluetooth(detail) => Failure::NoBluetooth(detail),
        TransportError::NoAdapter => Failure::NoAdapter,
        TransportError::BluetoothOff => Failure::BluetoothOff,
        TransportError::NotAllowed => Failure::NotAllowed,
        other => Failure::ScanFailed(detail(other)),
    }
}

/// The platform's own words, without the "could not connect" the command line
/// puts in front of them.
fn detail(e: TransportError) -> String {
    match e {
        TransportError::Connect(msg) => msg,
        other => other.to_string(),
    }
}

#[async_trait]
impl Opener for BleOpener {
    async fn find(&mut self, secs: u64) -> Result<Vec<Seen>, Failure> {
        let adapter = self.adapter().await?;
        ble::powered(&adapter).await.map_err(scan_failure)?;
        let mut found: Vec<ble::Discovered> = ble::scan(&adapter, Duration::from_secs(secs))
            .await
            .map_err(scan_failure)?
            .into_iter()
            .filter(|d| d.likely_printer)
            .collect();
        found.sort_by(|a, b| {
            b.heard
                .cmp(&a.heard)
                .then(b.rssi.cmp(&a.rssi))
                .then(a.name.cmp(&b.name))
        });
        self.table.clear();
        let mut seen = Vec::with_capacity(found.len());
        for d in found {
            let key = DeviceKey(self.next_key);
            self.next_key = self.next_key.wrapping_add(1);
            // A likely printer always has a name: that is how it was judged.
            let name = d.name.unwrap_or_default();
            self.table.insert(key, (d.id, name.clone()));
            seen.push(Seen {
                key,
                name,
                heard: d.heard,
            });
        }
        Ok(seen)
    }

    async fn stop_finding(&mut self) {
        if let Some(a) = &self.adapter {
            let _ = a.stop_scan().await;
        }
    }

    async fn open(&mut self, key: DeviceKey) -> Result<Opened, Failure> {
        let (Some(adapter), Some((id, name))) =
            (self.adapter.clone(), self.table.get(&key).cloned())
        else {
            return Err(Failure::NotFound {
                name: UNNAMED.into(),
            });
        };
        let failed = |e: TransportError| Failure::ConnectFailed {
            name: name.clone(),
            detail: detail(e),
        };
        // Before the connect, so a drop during it is not missed.
        let gone = ble::watch_disconnect(&adapter, id.clone())
            .await
            .map_err(failed)?;
        let peripheral = ble::find(&adapter, &id).await.map_err(|e| match e {
            TransportError::NotConnected => Failure::NotFound { name: name.clone() },
            other => failed(other),
        })?;
        let transport = match BleTransport::connect(peripheral.clone()).await {
            Ok(t) => t,
            Err(e) => {
                // The connect may have got as far as a link before failing on
                // services or the channel. Do not leave that link up.
                let _ = timeout(ABANDON_WITHIN, peripheral.disconnect()).await;
                return Err(match e {
                    TransportError::NoChannel => Failure::NoChannel { name: name.clone() },
                    other => failed(other),
                });
            }
        };
        Ok(Opened {
            session: Session::start(Box::new(transport)),
            name,
            gone,
        })
    }

    async fn abandon(&mut self, key: DeviceKey) {
        let (Some(adapter), Some((id, _))) = (&self.adapter, self.table.get(&key)) else {
            return;
        };
        if let Ok(p) = adapter.peripheral(id).await {
            let _ = p.disconnect().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_proto::models;

    fn identity(model_id: Option<u16>) -> Identity {
        let candidates: Vec<_> = model_id
            .map(models::by_wire_id)
            .into_iter()
            .flatten()
            .collect();
        Identity {
            model_id,
            model: candidates.first().copied(),
            candidates,
            ..Identity::default()
        }
    }

    #[test]
    fn facts_refuse_what_cannot_print() {
        let unknown = Facts::of(&identity(Some(9999)), "X", 244);
        assert_eq!(
            unknown.printable,
            Err(NotPrintable::UnknownModel {
                model_id: Some(9999)
            })
        );
        assert_eq!(
            (unknown.head, unknown.dpi, unknown.direction),
            (0, 0, PrintDirection::Top)
        );

        let silent = Facts::of(&identity(None), "X", 244);
        assert_eq!(
            silent.printable,
            Err(NotPrintable::UnknownModel { model_id: None })
        );

        let ambiguous = Facts::of(&identity(Some(2305)), "X", 244);
        match &ambiguous.printable {
            Err(NotPrintable::CandidatesDisagree { names }) => {
                assert!(names.contains(&"D110"), "{names:?}");
                assert!(names.contains(&"Hi-D110"), "{names:?}");
            }
            other => panic!("{other:?}"),
        }

        let d110_m = Facts::of(&identity(Some(2320)), "D110_M-GB28010920", 244);
        assert_eq!(d110_m.printable, Ok(()));
        assert_eq!(
            (d110_m.head, d110_m.dpi, d110_m.direction),
            (96, 203, PrintDirection::Left)
        );
        assert_eq!(d110_m.model.map(|m| m.name), Some("D110_M"));
    }
}
