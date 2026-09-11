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

pub mod artwork;
pub mod barcode;
pub mod calibration;
pub mod canvas;
pub mod code2d;
pub mod coverage;
pub mod document;
pub mod error;
pub mod greymap;
pub mod halftone;
pub mod rasterise;
pub mod srgb;
mod svg;
pub mod target;
pub mod text;

pub use barcode::{BarcodeError, Symbology};
pub use canvas::Canvas;
pub use document::{Element, ElementOrRaw, Ink, Label, LoadError};
pub use error::RenderError;
pub use greymap::GreyMap;
pub use halftone::{BinariseOpts, Halftone, Zone, binarise};
pub use rasterise::{RenderReport, local_size, rasterise, rasterise_plane, zone_table};
pub use target::{Dpmm, Target};
