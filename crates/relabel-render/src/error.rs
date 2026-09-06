//! What can go wrong turning a label into dots.

use relabel_proto::ProtoError;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderError {
    /// A target with no area, which nothing downstream can use.
    #[error("a target must have a non-zero size, got {w}x{h}")]
    EmptyTarget { w: u16, h: u16 },

    /// The canvas is not exactly as wide as the print head.
    ///
    /// For a model that prints along its short axis the encoder rotates the
    /// plane before sending it, so a canvas narrower than the head does not
    /// come out inset - it comes out *shifted*, because the rotation puts the
    /// padding on one side. Equality is the only safe rule, and it is checked
    /// here rather than left to fail confusingly at encode time.
    #[error("canvas is {got} dots on the head axis but the print head is {want}")]
    HeadAxisMismatch { got: u16, want: u16 },

    /// A geometry value that cannot be placed on a dot grid.
    ///
    /// Snapping happens in this crate, deliberately and visibly, rather than
    /// being left to the rasteriser: `tiny_skia::Rect::round` floors, so a bar
    /// edge sweeping 10.0 to 10.875 would silently start at dot 10 either way,
    /// and a caller passing a fraction has almost certainly made an arithmetic
    /// error worth reporting.
    #[error("{what} must be a whole number of dots, got {value}")]
    NotOnGrid { what: &'static str, value: f32 },

    /// Rasterisation produced a buffer the plane cannot hold.
    #[error(transparent)]
    Proto(#[from] ProtoError),

    /// The element type is designed but not yet built.
    #[error("{0} is not implemented yet")]
    Unimplemented(&'static str),
}
