//! Transports, session and the print-job engine.
//!
//! This is the layer that owns time and I/O. `relabel-proto` below it is pure
//! and synchronous. The GUI above it uses [`link`], whose API names no
//! `btleplug` or `tokio` type and which runs on a thread of its own.
//! [`transport::ble`] still hands `btleplug` types to the command line, which
//! runs its own runtime and needs the lower-level calls.

pub mod error;
pub mod job;
pub mod link;
pub mod printer;
pub mod session;
pub mod table;
pub mod transport;

pub use error::{JobError, SessionError, TransportError};
pub use job::{
    PrintOptions, Progress, Reached, RunEnd, RunNews, RunReport, Timeouts, print, print_labels,
};
pub use printer::{HandshakeStep, Identity, handshake, handshake_with};
pub use session::{PrinterEvent, Session, TraceEntry};
pub use table::{CsvError, CsvTable, read_csv};
pub use transport::{Transport, TransportKind};
