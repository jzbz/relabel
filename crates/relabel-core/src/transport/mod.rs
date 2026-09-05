//! The byte pipe, and nothing else.
//!
//! Deliberately dumb. Reassembly, request/response correlation, pacing and the
//! one-outstanding-request rule all live in [`crate::session`], once, so a new
//! transport is a few dozen lines rather than a reimplementation of the
//! protocol's timing rules.

use crate::error::TransportError;
use async_trait::async_trait;
use tokio::sync::mpsc;

pub mod ble;
pub mod mock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Ble,
    Serial,
    Mock,
}

/// A duplex byte pipe to a printer.
#[async_trait]
pub trait Transport: Send {
    /// Write bytes. May contain one or several *complete* frames.
    ///
    /// Never a partial frame: bundling whole frames into one write is documented
    /// as safe and is worth roughly four times the throughput on BLE, but
    /// splitting a single frame across two writes is neither documented nor
    /// tested anywhere.
    async fn write(&mut self, buf: &[u8]) -> Result<(), TransportError>;

    /// Take the inbound byte stream. Never frame-aligned.
    ///
    /// Callable once; subsequent calls return `None`. The session takes it at
    /// construction and owns it for the life of the connection.
    fn take_reader(&mut self) -> Option<mpsc::UnboundedReceiver<Vec<u8>>>;

    /// Largest single write this link accepts.
    ///
    /// BLE: the negotiated MTU less three bytes of ATT header. Serial:
    /// effectively unbounded.
    fn max_write(&self) -> usize;

    /// How long to wait after each write before the next.
    ///
    /// A transport property rather than a model one. Unpaced BLE writes drop
    /// rows on some host and adapter combinations, and every BLE implementation
    /// in the wild paces every model identically; meanwhile at least one printer
    /// over USB serial is reported to break if *any* artificial delay is added,
    /// which is precisely why this is not a constant.
    fn default_pace(&self) -> std::time::Duration;

    fn kind(&self) -> TransportKind;

    async fn close(&mut self) -> Result<(), TransportError>;
}
