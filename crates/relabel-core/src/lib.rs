//! Transports, session and the print-job engine.
//!
//! This is the layer that owns time and I/O. `relabel-proto` below it is pure
//! and synchronous; the GUI above it never sees a `btleplug` type, only the
//! types declared here.

pub mod error;
pub mod job;
pub mod session;
pub mod transport;

pub use error::{JobError, SessionError, TransportError};
pub use job::{PrintOptions, Progress, Timeouts, print};
pub use session::{PrinterEvent, Session, TraceEntry};
pub use transport::{Transport, TransportKind};
