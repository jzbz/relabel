//! What can go wrong between here and the print head.

use relabel_proto::{PrinterErrorCode, ProtoError, ResponseId};

/// A transport-level failure: the byte pipe, not the protocol.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("not connected")]
    NotConnected,
    #[error("the link dropped")]
    Disconnected,
    #[error("write failed: {0}")]
    Write(String),
    #[error("could not connect: {0}")]
    Connect(String),
    /// A frame is larger than the transport can carry in one write.
    ///
    /// relabel never splits a frame across writes - that direction is
    /// undocumented and untested on every model - so this is a refusal to print
    /// rather than something to work around.
    #[error("a {frame} byte frame does not fit this link's {max} byte write limit")]
    FrameTooLarge { frame: usize, max: usize },
    #[error("no usable read/write characteristic on this device")]
    NoChannel,

    // The four below were `Connect` strings until the GUI needed to tell them
    // apart. Each keeps the words the command line printed for it, prefix
    // included, so relabel-cli's output does not change.
    /// The platform's Bluetooth service could not be reached at all.
    #[error(
        "could not connect: could not reach the Bluetooth stack: {0}\n\
         On Linux this usually means bluetoothd is not running, or this \
         process cannot reach the D-Bus system bus."
    )]
    NoBluetooth(String),
    #[error(
        "could not connect: no Bluetooth adapter found. On Linux, check that bluetoothd is running"
    )]
    NoAdapter,
    /// The platform refused this program Bluetooth: macOS's privacy setting,
    /// or Windows's. BlueZ never reports this.
    #[error("could not connect: this program is not allowed to use Bluetooth")]
    NotAllowed,
    /// The adapter is there and switched off. On BlueZ this is also what an
    /// adapter whose state cannot be read at all reports (see
    /// [`crate::transport::ble::powered`]).
    #[error("could not connect: Bluetooth is switched off")]
    BluetoothOff,
}

/// A protocol-level failure while talking to a printer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Proto(#[from] ProtoError),

    /// The printer reported a fault instead of answering.
    ///
    /// Arrives as `0xDB` and can land in place of *any* expected reply, so it
    /// short-circuits the wait rather than running out the timeout.
    #[error("printer error: {0}")]
    Printer(PrinterErrorCode),

    /// The printer does not implement the command (`0x00`).
    ///
    /// Not always a failure: an optional read - the ribbon tag, paper info,
    /// half-cut - answering this way just means the feature is absent.
    #[error("the printer does not support {0:?}")]
    NotSupported(ResponseId),

    #[error("timed out after {ms} ms waiting for {what}")]
    Timeout { what: &'static str, ms: u64 },

    /// `request` was called on a command the printer never answers.
    ///
    /// A caller bug rather than a device problem: bitmap rows and firmware
    /// chunks are one-way, and waiting on them would burn a full timeout every
    /// row. The frame is not sent, because a side effect plus an error is worse
    /// than an error alone.
    #[error("command {0:#04x} is one-way and has no reply to wait for")]
    OneWayCommand(u8),

    /// The session task is gone, which means the connection was closed.
    #[error("the session has ended")]
    Ended,
}

impl SessionError {
    /// Whether this is the printer politely declining an optional feature.
    pub fn is_unsupported(&self) -> bool {
        matches!(self, SessionError::NotSupported(_))
    }
}

/// A print job failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error(transparent)]
    Session(#[from] SessionError),

    /// The printer refused a step by answering `0x00` where `0x01` was needed.
    #[error("the printer refused {step}")]
    Refused { step: &'static str },

    /// The page counter stopped advancing before the job finished.
    #[error("printing stalled at page {reached} of {expected}")]
    Stalled { reached: u16, expected: u16 },

    #[error("the job exceeded its {ms} ms deadline")]
    DeadlineExceeded { ms: u64 },

    #[error("cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What relabel-cli printed for these before they had their own variants,
    /// when they were `Connect` strings.
    #[test]
    fn the_cli_keeps_its_words() {
        assert_eq!(
            TransportError::NoAdapter.to_string(),
            "could not connect: no Bluetooth adapter found. On Linux, check that bluetoothd is running"
        );
        assert_eq!(
            TransportError::NoAdapter.to_string(),
            TransportError::Connect(
                "no Bluetooth adapter found. On Linux, check that bluetoothd is running".into()
            )
            .to_string()
        );
        let e = TransportError::NoBluetooth("x".into()).to_string();
        assert!(
            e.starts_with("could not connect: could not reach the Bluetooth stack: x\n"),
            "{e}"
        );
        assert!(e.contains("bluetoothd"), "{e}");
        assert!(e.ends_with("cannot reach the D-Bus system bus."), "{e}");
    }
}
