//! Bluetooth Low Energy, via btleplug.
//!
//! Everything here that looks like paranoia is a documented failure mode.
//!
//! - **Scanning is deliberately unfiltered.** `ScanFilter` matches on advertised
//!   service UUIDs, and these printers do not advertise theirs. On Linux it is
//!   worse than useless: BlueZ merges discovery filters across every D-Bus
//!   client on the system, so a narrow filter here can affect an unrelated
//!   program. Scan wide, filter on the name afterwards.
//! - **The notification stream is opened before subscribing.** Notifications
//!   delivered before the stream exists are dropped outright, and on two of the
//!   three backends the channel is a bounded broadcast whose lag errors are
//!   swallowed silently. Opening first and draining into an unbounded channel
//!   immediately is what stops rows going missing.
//! - **`mtu()` can panic.** The BlueZ backend unwraps an `Option` that is empty
//!   when BlueZ does not publish the characteristic MTU, which is the case on
//!   older versions. It is called inside `catch_unwind` for that reason and no
//!   other.
//! - **The characteristic pair is not guessed.** The commonly seen pair is tried
//!   first, then a capability scan for anything that can notify and write on a
//!   non-standard service, because at least one model is reported to use
//!   different UUIDs.

use crate::error::TransportError;
use crate::transport::{Transport, TransportKind};
use async_trait::async_trait;
use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::{Uuid, uuid};

/// The service and characteristic seen on most of these printers.
///
/// Not NIIMBOT-specific - it is a stock BLE-to-serial bridge that appears on
/// unrelated hardware too - and not universal, which is why
/// [`find_by_capability`] exists.
pub const KNOWN_SERVICE: Uuid = uuid!("e7810a71-73ae-499d-8c15-faa9aef0c3f2");
pub const KNOWN_CHARACTERISTIC: Uuid = uuid!("bef8d6c9-9c21-4c9e-b632-bd58c1009f9f");

/// The minimum ATT MTU. Every printer's row packet is larger than this, so a
/// link that never negotiates upward cannot print - which is reported rather
/// than worked around, because splitting a frame is undocumented.
const MIN_MTU: u16 = 23;

/// Three bytes of ATT header come off the top of every write.
const ATT_OVERHEAD: usize = 3;

/// A printer seen while scanning.
#[derive(Debug, Clone)]
pub struct Discovered {
    /// Stable across reconnects, and the right thing to persist.
    ///
    /// The Bluetooth address is not: on macOS it comes back all zeroes.
    pub id: PeripheralId,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    /// Whether the name looks like one of these printers. Advisory only - the
    /// device list offers a "show everything" escape, because the naming is not
    /// consistent enough to hide devices on.
    pub likely_printer: bool,
}

/// Name prefixes these printers have been seen advertising.
///
/// A hint for sorting the list, never a gate. Two different models share a
/// prefix, some carry a serial suffix, and the list is certainly incomplete.
const NAME_HINTS: &[&str] = &[
    "D11", "D110", "D101", "D11S", "B1", "B18", "B21", "B203", "B3S", "B31", "B32", "K3", "P1",
    "H1", "Z401", "M2", "M3", "T8", "NIIMBOT", "JC", "Fust", "Betty",
];

fn looks_like_printer(name: Option<&str>) -> bool {
    let Some(n) = name else { return false };
    let upper = n.to_ascii_uppercase();
    NAME_HINTS
        .iter()
        .any(|h| upper.starts_with(&h.to_ascii_uppercase()))
}

/// Get the first Bluetooth adapter the platform offers.
pub async fn adapter() -> Result<Adapter, TransportError> {
    let manager = Manager::new().await.map_err(|e| {
        // The overwhelmingly common cause on Linux is that there is no session
        // bus to talk to, which the underlying message does say but not in a
        // way that suggests what to do about it.
        TransportError::Connect(format!(
            "could not reach the Bluetooth stack: {e}\n\
             On Linux this usually means bluetoothd is not running, or this \
             process cannot reach the D-Bus system bus."
        ))
    })?;
    let adapters = manager
        .adapters()
        .await
        .map_err(|e| TransportError::Connect(format!("listing adapters: {e}")))?;
    adapters.into_iter().next().ok_or_else(|| {
        TransportError::Connect(
            "no Bluetooth adapter found. On Linux, check that bluetoothd is running".into(),
        )
    })
}

/// Scan for `duration` and return everything seen.
pub async fn scan(
    adapter: &Adapter,
    duration: Duration,
) -> Result<Vec<Discovered>, TransportError> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| TransportError::Connect(format!("starting scan: {e}")))?;
    tokio::time::sleep(duration).await;
    let found = adapter
        .peripherals()
        .await
        .map_err(|e| TransportError::Connect(format!("listing peripherals: {e}")))?;
    let _ = adapter.stop_scan().await;

    let mut out = Vec::new();
    for p in found {
        // A peripheral whose properties cannot be read is not worth reporting,
        // but it is also not an error worth failing the whole scan over.
        let Ok(Some(props)) = p.properties().await else {
            continue;
        };
        let name = props.local_name.or(props.advertisement_name);
        out.push(Discovered {
            id: p.id(),
            likely_printer: looks_like_printer(name.as_deref()),
            name,
            rssi: props.rssi,
        });
    }
    // Likely printers first, then by signal strength.
    out.sort_by(|a, b| {
        b.likely_printer
            .cmp(&a.likely_printer)
            .then(b.rssi.cmp(&a.rssi))
    });
    Ok(out)
}

/// Find a peripheral by the id a previous scan reported.
pub async fn find(adapter: &Adapter, id: &PeripheralId) -> Result<Peripheral, TransportError> {
    let found = adapter
        .peripherals()
        .await
        .map_err(|e| TransportError::Connect(format!("listing peripherals: {e}")))?;
    found
        .into_iter()
        .find(|p| p.id() == *id)
        .ok_or(TransportError::NotConnected)
}

pub struct BleTransport {
    peripheral: Peripheral,
    characteristic: Characteristic,
    write_type: WriteType,
    reader: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    max_write: usize,
    pace: Duration,
}

impl BleTransport {
    /// Connect, discover the channel, and start draining notifications.
    pub async fn connect(peripheral: Peripheral) -> Result<BleTransport, TransportError> {
        connect_with_retry(&peripheral).await?;
        peripheral
            .discover_services()
            .await
            .map_err(|e| TransportError::Connect(format!("discovering services: {e}")))?;

        let characteristic = find_known(&peripheral)
            .or_else(|| find_by_capability(&peripheral))
            .ok_or(TransportError::NoChannel)?;

        // Write-without-response wherever the characteristic allows it. On macOS
        // in particular, using write-with-response on a characteristic that only
        // advertises write-without-response is reported to wreck timing and
        // produce blank labels with no error anywhere - so this is discovered,
        // never assumed.
        let write_type = if characteristic
            .properties
            .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
        {
            WriteType::WithoutResponse
        } else {
            WriteType::WithResponse
        };

        // Before subscribe(). See the module note.
        let mut notifications = peripheral
            .notifications()
            .await
            .map_err(|e| TransportError::Connect(format!("opening notifications: {e}")))?;
        peripheral
            .subscribe(&characteristic)
            .await
            .map_err(|e| TransportError::Connect(format!("subscribing: {e}")))?;

        let (tx, rx) = mpsc::unbounded_channel();
        let want = characteristic.uuid;
        tokio::spawn(async move {
            while let Some(n) = notifications.next().await {
                if n.uuid != want {
                    continue;
                }
                if tx.send(n.value).is_err() {
                    // The session is gone; nothing left to deliver to.
                    break;
                }
            }
        });

        let mtu = negotiated_mtu(&peripheral);
        Ok(BleTransport {
            peripheral,
            characteristic,
            write_type,
            reader: Some(rx),
            max_write: usize::from(mtu).saturating_sub(ATT_OVERHEAD),
            // Unpaced writes drop rows on some host and adapter combinations,
            // and every BLE client in the wild paces every model identically.
            // Ten milliseconds is that convention, not a measurement.
            pace: Duration::from_millis(10),
        })
    }

    /// What the link negotiated, for the device pane to show.
    pub fn mtu(&self) -> usize {
        self.max_write + ATT_OVERHEAD
    }

    pub fn set_pace(&mut self, pace: Duration) {
        self.pace = pace;
    }

    pub fn id(&self) -> PeripheralId {
        self.peripheral.id()
    }
}

/// Connect, tolerating an attempt that is already under way.
///
/// BlueZ answers `In Progress` when something else - often this same program a
/// moment earlier, or a previous run whose link has not finished tearing down -
/// already has a connect in flight for this device. `is_connected` reports
/// false throughout that window, so checking it first does not help: the honest
/// fix is to wait for the attempt in flight to settle and look again.
///
/// Three tries at 400 ms covers the case comfortably. Anything longer is a real
/// failure and should be reported as one rather than hidden behind a spinner.
async fn connect_with_retry(peripheral: &Peripheral) -> Result<(), TransportError> {
    const TRIES: usize = 3;
    const PAUSE: Duration = Duration::from_millis(400);

    let mut last = String::new();
    for attempt in 0..TRIES {
        if peripheral.is_connected().await.unwrap_or(false) {
            return Ok(());
        }
        match peripheral.connect().await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let msg = e.to_string();
                let busy = msg.contains("In Progress") || msg.contains("in progress");
                if !busy || attempt + 1 == TRIES {
                    return Err(TransportError::Connect(msg));
                }
                last = msg;
                tokio::time::sleep(PAUSE).await;
            }
        }
    }
    Err(TransportError::Connect(format!(
        "still busy after {TRIES} attempts: {last}"
    )))
}

/// Read the MTU without letting a backend panic take the process down.
///
/// The BlueZ backend unwraps an `Option` that is `None` when BlueZ does not
/// publish the characteristic MTU property. `catch_unwind` is not a general
/// habit in this codebase; it is here because this one call is known to panic on
/// a supported platform and the fallback is well defined.
fn negotiated_mtu(peripheral: &Peripheral) -> u16 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| peripheral.mtu()))
        .unwrap_or(MIN_MTU)
        .max(MIN_MTU)
}

fn find_known(p: &Peripheral) -> Option<Characteristic> {
    p.characteristics()
        .into_iter()
        .find(|c| c.uuid == KNOWN_CHARACTERISTIC && c.service_uuid == KNOWN_SERVICE)
}

/// Anything on a vendor service that can both notify and be written to.
///
/// Standard SIG services are skipped: their UUIDs share the Bluetooth base and
/// none of them is a printer's data channel.
fn find_by_capability(p: &Peripheral) -> Option<Characteristic> {
    p.characteristics()
        .into_iter()
        .filter(|c| !is_sig_uuid(c.service_uuid))
        .find(|c| {
            c.properties.contains(CharPropFlags::NOTIFY)
                && c.properties
                    .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
        })
}

/// Whether a UUID is one of the Bluetooth SIG's own, i.e. `0000xxxx-0000-1000-8000-00805f9b34fb`.
fn is_sig_uuid(u: Uuid) -> bool {
    // The base is 0000xxxx-0000-1000-8000-00805F9B34FB: the top 32 bits are the
    // short id, the low 96 are fixed. Mask the id off and compare the rest.
    const SIG_SUFFIX: u128 = 0x0000_1000_8000_0080_5f9b_34fb;
    const MASK: u128 = (1u128 << 96) - 1;
    (u.as_u128() & MASK) == SIG_SUFFIX
}

#[async_trait]
impl Transport for BleTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<(), TransportError> {
        if buf.len() > self.max_write {
            return Err(TransportError::FrameTooLarge {
                frame: buf.len(),
                max: self.max_write,
            });
        }
        self.peripheral
            .write(&self.characteristic, buf, self.write_type)
            .await
            .map_err(|e| TransportError::Write(e.to_string()))
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
        TransportKind::Ble
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        let _ = self.peripheral.unsubscribe(&self.characteristic).await;
        self.peripheral
            .disconnect()
            .await
            .map_err(|e| TransportError::Write(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sig_uuids_are_recognised() {
        // Battery Service, 0x180F.
        assert!(is_sig_uuid(uuid!("0000180f-0000-1000-8000-00805f9b34fb")));
        // The printers' own service is not a SIG one.
        assert!(!is_sig_uuid(KNOWN_SERVICE));
    }

    #[test]
    fn name_hints_are_advisory_and_case_insensitive() {
        assert!(looks_like_printer(Some("D110-A1B2")));
        assert!(looks_like_printer(Some("b21_1234")));
        assert!(!looks_like_printer(Some("Someone's Headphones")));
        assert!(!looks_like_printer(None));
    }

    #[test]
    fn the_minimum_mtu_cannot_carry_a_row_packet() {
        // Not a behaviour test so much as a recorded fact: a link stuck at the
        // default MTU cannot print anything, which is why it is surfaced.
        let usable = usize::from(MIN_MTU) - ATT_OVERHEAD;
        let smallest_row = 6 + 12 + relabel_proto::frame::STANDARD_OVERHEAD; // 96-dot head
        assert!(usable < smallest_row);
    }
}
