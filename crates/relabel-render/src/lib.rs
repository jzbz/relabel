//! The label document, and the rasteriser that turns it into dots.
//!
//! Takes a label - text, barcodes, 2D codes, images, rules - and produces a
//! 1-bit [`BitPlane`](relabel_proto::BitPlane) at a stated dot pitch. It knows
//! a [`Target`] and nothing else about the destination: no model, no transport,
//! no toolkit. Rotation for a printer whose head lies along the short axis
//! happens at the protocol boundary, not here.
//!
//! Nothing in this crate is async and nothing does I/O beyond reading bytes a
//! caller hands it.

pub mod calibration;
pub mod canvas;
pub mod error;
pub mod greymap;
pub mod target;

pub use canvas::{Canvas, Ink};
pub use error::RenderError;
pub use greymap::GreyMap;
pub use target::{Dpmm, Target};
