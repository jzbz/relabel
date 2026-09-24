//! A printer link that exists only in memory.
//!
//! [`MockOpener`] finds and opens printers the way [`super::BleOpener`] does,
//! with no Bluetooth stack: each device is a [`MockTransport`] whose responder
//! plays the printer, and the platform's disconnect is something a test fires
//! by hand. Like [`crate::transport::mock`] it is public rather than
//! `cfg(test)`, so the GUI's tests can run the real actor over it.

use super::{DeviceKey, Facts, Failure, Opened, Opener, Seen};
use crate::printer::Identity;
use crate::session::Session;
use crate::transport::mock::{MockHandle, MockTransport, Responder, identifying};
use async_trait::async_trait;
use relabel_proto::models;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// The name the author's printer advertises.
pub const D110_M_NAME: &str = "D110_M-GB28010920";

/// The author's D110_M as its handshake reads it: wire id 2320, level 4.
pub fn d110_m() -> Responder {
    identifying(2320, Some((3, 1)), true)
}

/// How a device behaves when opened.
#[derive(Clone)]
pub enum Behaviour {
    /// Connects, and answers with a fresh responder from this factory.
    Answers(fn() -> Responder),
    /// The connect never finishes.
    NeverConnects,
    /// The connect fails at once, with these words from the platform.
    Refuses(String),
    /// Connects, but has no channel relabel can use.
    NoChannel,
    /// Connects and answers like `Answers`, but the link dies once this many
    /// requests have been answered: every write after that fails. The
    /// platform does not report the drop.
    DropsAfter(usize, fn() -> Responder),
}

/// One device a [`MockOpener`] finds.
#[derive(Clone)]
pub struct MockDevice {
    pub name: String,
    pub heard: bool,
    pub behaviour: Behaviour,
}

impl MockDevice {
    pub fn new(name: &str, heard: bool, behaviour: Behaviour) -> MockDevice {
        MockDevice {
            name: name.into(),
            heard,
            behaviour,
        }
    }

    /// The author's printer, heard, answering as [`d110_m`] does.
    pub fn d110_m() -> MockDevice {
        MockDevice::new(D110_M_NAME, true, Behaviour::Answers(d110_m))
    }
}

#[derive(Default)]
struct Shared {
    transport: Option<MockHandle>,
    gone: Option<oneshot::Sender<()>>,
    opens: usize,
    abandons: usize,
    stops: usize,
}

/// An [`Opener`] over in-memory devices. Each device's key is its index.
pub struct MockOpener {
    devices: Vec<MockDevice>,
    failing: Option<Failure>,
    scan: Option<Duration>,
    shared: Arc<Mutex<Shared>>,
}

/// The test's view of a [`MockOpener`] after it has been handed to a link.
#[derive(Clone)]
pub struct MockLink {
    shared: Arc<Mutex<Shared>>,
}

impl MockOpener {
    pub fn new(devices: Vec<MockDevice>) -> (MockOpener, MockLink) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let opener = MockOpener {
            devices,
            failing: None,
            scan: None,
            shared: Arc::clone(&shared),
        };
        (opener, MockLink { shared })
    }

    /// An opener whose every find fails at once with `failure`.
    pub fn failing(failure: Failure) -> (MockOpener, MockLink) {
        let (mut opener, link) = MockOpener::new(Vec::new());
        opener.failing = Some(failure);
        (opener, link)
    }

    /// Scan for this long whatever a find asks for. By default a find takes
    /// the seconds it is given, in tokio time.
    pub fn with_scan(mut self, scan: Duration) -> MockOpener {
        self.scan = Some(scan);
        self
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }
}

impl MockLink {
    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    /// The transport of the device opened last.
    pub fn transport(&self) -> Option<MockHandle> {
        self.shared().transport.clone()
    }

    /// The platform reports the device gone, and the link stops carrying
    /// anything.
    pub fn drop_link(&self) {
        let (gone, transport) = {
            let mut s = self.shared();
            (s.gone.take(), s.transport.clone())
        };
        if let Some(gone) = gone {
            let _ = gone.send(());
        }
        if let Some(t) = transport {
            t.disconnect();
        }
    }

    /// How many connects were given up part way.
    pub fn abandons(&self) -> usize {
        self.shared().abandons
    }

    /// How many scans were stopped part way.
    pub fn stops(&self) -> usize {
        self.shared().stops
    }

    /// How many connects were started.
    pub fn opens(&self) -> usize {
        self.shared().opens
    }
}

/// A responder that answers `n` requests and then kills the link it plays.
fn dropping_after(
    n: usize,
    mut inner: Responder,
    link: Arc<Mutex<Option<MockHandle>>>,
) -> Responder {
    let mut answered = 0;
    Box::new(move |req| {
        let replies = inner(req);
        answered += 1;
        if answered >= n
            && let Some(h) = link.lock().unwrap().as_ref()
        {
            h.disconnect();
        }
        replies
    })
}

#[async_trait]
impl Opener for MockOpener {
    async fn find(&mut self, secs: u64) -> Result<Vec<Seen>, Failure> {
        if let Some(failure) = &self.failing {
            return Err(failure.clone());
        }
        tokio::time::sleep(self.scan.unwrap_or(Duration::from_secs(secs))).await;
        Ok(self
            .devices
            .iter()
            .zip(0u32..)
            .map(|(d, i)| Seen {
                key: DeviceKey(i),
                name: d.name.clone(),
                heard: d.heard,
            })
            .collect())
    }

    async fn stop_finding(&mut self) {
        self.shared().stops += 1;
    }

    async fn open(&mut self, key: DeviceKey) -> Result<Opened, Failure> {
        self.shared().opens += 1;
        let Some(device) = usize::try_from(key.0)
            .ok()
            .and_then(|i| self.devices.get(i))
            .cloned()
        else {
            return Err(Failure::NotFound {
                name: "The printer".into(),
            });
        };
        let name = device.name;
        let cell = Arc::new(Mutex::new(None));
        let (responder, drops_at) = match device.behaviour {
            Behaviour::NeverConnects => std::future::pending().await,
            Behaviour::Refuses(detail) => return Err(Failure::ConnectFailed { name, detail }),
            Behaviour::NoChannel => return Err(Failure::NoChannel { name }),
            Behaviour::Answers(f) => (f(), None),
            Behaviour::DropsAfter(n, f) => (dropping_after(n, f(), Arc::clone(&cell)), Some(n)),
        };
        let (transport, handle) = MockTransport::new(responder);
        *cell.lock().unwrap() = Some(handle.clone());
        if drops_at == Some(0) {
            handle.disconnect();
        }

        let (gone_tx, gone_rx) = oneshot::channel();
        {
            let mut s = self.shared();
            s.transport = Some(handle);
            s.gone = Some(gone_tx);
        }
        Ok(Opened {
            session: Session::start(Box::new(transport)),
            name,
            gone: Box::pin(async move {
                // Only a deliberate drop fires; the sender being replaced by a
                // later open is not the platform saying anything.
                if gone_rx.await.is_err() {
                    std::future::pending::<()>().await;
                }
            }),
        })
    }

    async fn abandon(&mut self, _key: DeviceKey) {
        self.shared().abandons += 1;
    }
}

/// The facts a link reports after identifying `identifying(model_id, status,
/// true)` over a link with the mock's 244-byte writes, where `status` is
/// `(3, 1)` for the D110_M's id 2320 and the D110's short reply otherwise.
pub fn facts(model_id: u16, name: &str) -> Facts {
    // Level 4 when the status reply carries one, else the connect baseline,
    // flagged as assumed. relabel-core's tests check this against a real
    // handshake through the link.
    let resolved = model_id == 2320;
    let candidates: Vec<_> = models::by_wire_id(model_id).collect();
    let identity = Identity {
        protocol: if resolved { 4 } else { 2 },
        protocol_assumed: !resolved,
        model_id: Some(model_id),
        model: candidates.first().copied(),
        candidates,
        software: Some("4.17".into()),
        battery_percent: Some(75),
        ..Identity::default()
    };
    Facts::of(&identity, name, 244)
}
