//! The NIIMBOT label printer wire protocol.
//!
//! Bytes in, bytes out. No I/O, no async, no GUI types, and one dependency, so
//! the fuzz target builds in a second and the protocol tests need neither a
//! Bluetooth stack nor a font. Everything that touches a device lives in
//! `relabel-core`; everything that draws lives in `relabel-render`.
//!
//! # Where the facts came from
//!
//! Protocol behaviour is reverse-engineered from open-source reimplementations
//! and from published packet captures - interoperability information, not
//! copyrightable expression. The model registry in [`models`] is a vendored
//! snapshot of NIIMBOT's own device database. Nothing in this crate is
//! transliterated from a GPL implementation or from the vendor's SDK.
//!
//! Individual facts carry a citation in a comment where a maintainer would
//! otherwise have to take them on faith, and the places where sources genuinely
//! disagree say so rather than picking a side silently.

pub mod accumulator;
pub mod command;
pub mod error;
pub mod frame;
pub mod image;
pub mod models;
pub mod parse;
pub mod payload;
pub mod task;

pub use accumulator::{Accumulator, DesyncReason, Event};
pub use command::{Expect, RequestId, ResponseId};
pub use error::{PrinterErrorCode, ProtoError};
pub use frame::{Frame, MAX_FRAME, MAX_PAYLOAD};
pub use image::{BitPlane, CountsMode, EncodedPage, HeadSpec, encode_page};
pub use models::{BlindZone, LabelType, ModelSpec, PrintDirection, RfidKind, Vendor};
pub use task::{PrintFlow, copies_per_job, select_flow};
