//! Dot arithmetic for the canvas.
//!
//! No toolkit type appears in any signature here, which is what lets every
//! rule below be reached by an ordinary unit test rather than through a
//! simulated gesture. The canvas in `.slint` multiplies dots by a zoom and
//! nothing else.

use relabel_render::document::{RectDots, Rot};
use relabel_render::media::Bounds;
use relabel_render::target::{Dpmm, Target};

/// The frame as it is drawn: a quarter turn swaps the element's sides.
///
/// The origin stays where it was, which is the rasteriser's own rule - only
/// the extent turns, because a rotated rectangle is still a rectangle. A
/// canvas that drew the unrotated frame would put a 90-degree barcode's box
/// across its bars.
///
/// `relabel-render` has this same function, private, in `rasterise.rs`; it is
/// repeated here rather than copied blind, and
/// `the_drawn_frame_agrees_with_local_size_on_every_quarter_turn` pins the two
/// together through the public `local_size`, so a change there fails here.
pub fn as_drawn(f: RectDots, rot: Rot) -> RectDots {
    match rot.quantised() {
        Rot::D90 | Rot::D270 => RectDots {
            feed_len: f.head_len,
            head_len: f.feed_len,
            ..f
        },
        _ => f,
    }
}

/// The drawn frame in canvas coordinates: `(x, y, w, h)` in dots, the picture
/// as the operator sees it.
///
/// The axis swap is [`Target::place`]'s, the same call the rasteriser blits
/// through, so the frame and the dots under it cannot disagree. Which axis is
/// horizontal is the printer's business and not the canvas's: every model in
/// the D11/D110/D101 family reads landscape, where a *feed* coordinate is the
/// horizontal one.
pub fn on_canvas(t: &Target, f: RectDots, rot: Rot) -> (i32, i32, i32, i32) {
    let d = as_drawn(f, rot);
    let (x, y) = t.place(d.feed, d.head);
    let (w, h) = t.extent(d.feed_len, d.head_len);
    (x, y, w, h)
}

/// Canvas dots back to the document's own axes, as `(feed, head)`.
///
/// [`Target::place`] is its own inverse - it either swaps or it does not - and
/// `the_canvas_axes_round_trip_for_both_print_directions` is what keeps that
/// from being a coincidence someone relies on.
pub fn to_document(t: &Target, x: i32, y: i32) -> (i32, i32) {
    t.place(x, y)
}

/// One millimetre, in dots, at this label's own pitch.
///
/// The coarse nudge, because a millimetre is the unit the label itself is
/// quoted in and the unit the unprintable inset is quoted in: eight dots at
/// 203 dpi and twelve at 300, rather than a constant eight that would mean
/// two thirds of a millimetre on half the catalogue.
pub fn millimetre(dpmm: Dpmm) -> i32 {
    dpmm.dots_from_tenths_mm(10).max(1)
}

/// Where a placement may actually land: far enough onto the canvas that one
/// dot of the element is still on it.
///
/// Not a clamp to [`Bounds::usable`], which is where an element *should* go.
/// The document's rectangle is signed on purpose and hanging off the edge is
/// something preflight reports rather than something the editor forbids - so
/// the canvas tints the frame and lets it happen. What it will not allow is a
/// placement with nothing left to grab, because an element dragged entirely
/// off the canvas cannot be dragged back and that is data loss by gesture.
///
/// Takes the *drawn* frame, so a rotated element is kept reachable by the side
/// that is actually on screen.
pub fn keep_reachable(bounds: &Bounds, drawn: RectDots, feed: i32, head: i32) -> (i32, i32) {
    (
        keep_axis(feed, drawn.feed_len, bounds.canvas.feed_len),
        keep_axis(head, drawn.head_len, bounds.canvas.head_len),
    )
}

/// One axis of [`keep_reachable`]. A frame with no extent along this axis has
/// no dots to keep on the canvas, so it is held inside the canvas itself.
fn keep_axis(pos: i32, len: i32, canvas: i32) -> i32 {
    if len <= 0 {
        return pos.clamp(0, canvas.max(0));
    }
    pos.clamp(1 - len, (canvas - 1).max(0))
}

/// Whether a drawn frame lies wholly inside the canvas.
///
/// [`Bounds::keepout`](relabel_render::media::Bounds::keepout) answers only for
/// a frame already known to be on the canvas, and reads an off-canvas one as
/// the tail, so this is asked first - the order `rasterise` uses, where off the
/// canvas is a different sentence from on paper the label does not keep.
pub fn inside_canvas(canvas: &RectDots, f: &RectDots) -> bool {
    f.feed >= canvas.feed
        && f.head >= canvas.head
        && f.feed.saturating_add(f.feed_len) <= canvas.feed.saturating_add(canvas.feed_len)
        && f.head.saturating_add(f.head_len) <= canvas.head.saturating_add(canvas.head_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_proto::PrintDirection;
    use relabel_render::document::{Common, Label};
    use relabel_render::rasterise::local_size;

    fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
        RectDots {
            feed,
            head,
            feed_len,
            head_len,
        }
    }

    #[test]
    fn a_quarter_turn_swaps_the_frames_sides_and_leaves_its_origin() {
        let f = frame(10, 20, 30, 40);
        assert_eq!(as_drawn(f, Rot::D0), f);
        assert_eq!(as_drawn(f, Rot::D180), f);
        assert_eq!(as_drawn(f, Rot::D90), frame(10, 20, 40, 30));
        assert_eq!(as_drawn(f, Rot::D270), frame(10, 20, 40, 30));
    }

    /// A free angle is drawn at the nearest quarter turn, so the canvas must
    /// draw the box the rasteriser will use rather than the one the file asks
    /// for.
    #[test]
    fn an_angle_this_build_cannot_draw_is_boxed_at_the_nearest_quarter_turn() {
        let f = frame(0, 0, 30, 40);
        assert_eq!(as_drawn(f, Rot::Other(80)), frame(0, 0, 40, 30));
        assert_eq!(as_drawn(f, Rot::Other(10)), f);
    }

    /// `local_size` is the public half of the rasteriser's own private swap.
    /// If that swap ever changes, this fails here rather than silently drawing
    /// every rotated element's box across its content.
    #[test]
    fn the_drawn_frame_agrees_with_local_size_on_every_quarter_turn() {
        for direction in [PrintDirection::Left, PrintDirection::Top] {
            let t = Target::for_label(96, 320, 203, direction).expect("a real target");
            for rot in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
                let c = {
                    let mut c = Common::new(1, frame(4, 8, 30, 40));
                    c.rot = rot;
                    c
                };
                let (_, _, w, h) = on_canvas(&t, c.frame, rot);
                let upright = local_size(&c, &t).expect("the frame has area");
                let quarter = matches!(rot.quantised(), Rot::D90 | Rot::D270);
                let seen = if quarter { (h, w) } else { (w, h) };
                assert_eq!(
                    (seen.0 as u32, seen.1 as u32),
                    upright,
                    "{direction:?} {rot:?}"
                );
            }
        }
    }

    #[test]
    fn the_canvas_axes_round_trip_for_both_print_directions() {
        for direction in [PrintDirection::Left, PrintDirection::Top] {
            let t = Target::for_label(96, 320, 203, direction).expect("a real target");
            let (x, y) = t.place(7, 11);
            assert_eq!(to_document(&t, x, y), (7, 11), "{direction:?}");
        }
    }

    /// The whole reason the row struct carries canvas coordinates rather than
    /// the document's: on the hardware this project was written for, a feed
    /// coordinate is the horizontal one, and getting it the other way round
    /// draws a picture that looks perfectly reasonable.
    #[test]
    fn a_landscape_printer_puts_the_feed_axis_across_the_canvas() {
        let left = Target::for_label(96, 320, 203, PrintDirection::Left).expect("target");
        let top = Target::for_label(96, 320, 203, PrintDirection::Top).expect("target");
        assert_eq!(
            on_canvas(&left, frame(40, 8, 100, 20), Rot::D0),
            (40, 8, 100, 20)
        );
        assert_eq!(
            on_canvas(&top, frame(40, 8, 100, 20), Rot::D0),
            (8, 40, 20, 100)
        );
    }

    #[test]
    fn a_coarse_nudge_is_one_millimetre_of_dots_at_either_resolution() {
        assert_eq!(millimetre(Dpmm::DPI_203), 8);
        assert_eq!(millimetre(Dpmm::DPI_300), 12);
    }

    #[test]
    fn a_placement_may_hang_off_the_canvas_but_keeps_one_dot_on_it() {
        let label = Label::new(96, 320, Dpmm::DPI_203);
        let b = Bounds::of(&label);
        let drawn = frame(0, 0, 24, 40);
        assert_eq!(
            keep_reachable(&b, drawn, -5, -5),
            (-5, -5),
            "overhang is allowed"
        );
        assert_eq!(keep_reachable(&b, drawn, -9000, -9000), (-23, -39));
        assert_eq!(keep_reachable(&b, drawn, 9000, 9000), (95, 319));
    }

    #[test]
    fn a_frame_with_no_extent_is_held_inside_the_canvas() {
        let label = Label::new(96, 320, Dpmm::DPI_203);
        let b = Bounds::of(&label);
        assert_eq!(keep_reachable(&b, frame(0, 0, 0, 0), -4, 9000), (0, 320));
    }
}
