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

/// The interval a placement may land in on one axis: far enough on that one dot
/// of the element is still on the canvas.
///
/// Split out of [`keep_reachable`] rather than restated, because the inspector
/// announces this interval to a reader and the editor enforces it - and a field
/// that announced a bound the editor did not keep would be worse than a field
/// with no bound at all. Both branches are here, including the one a frame with
/// no extent along this axis takes: it has no dots to keep on the canvas, so it
/// is held inside the canvas itself.
pub fn reach(len: i32, canvas: i32) -> (i32, i32) {
    let r = if len <= 0 {
        (0, canvas.max(0))
    } else {
        (1 - len, (canvas - 1).max(0))
    };
    debug_assert!(r.0 <= r.1, "an axis with no room to land in: {r:?}");
    r
}

/// One axis of [`keep_reachable`], clamped to exactly the interval [`reach`]
/// announces.
fn keep_axis(pos: i32, len: i32, canvas: i32) -> i32 {
    let (lo, hi) = reach(len, canvas);
    pos.clamp(lo, hi)
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

/// The most dots of pointer padding a frame is offered on any side.
///
/// The canvas pads a frame smaller than its minimum grab by at most half of that
/// grab, five logical pixels, which at one pixel to the dot is five dots for every
/// unit of display scale: thirty-two covers every scale up to 6.4. A gap this wide
/// never limits anything, so an element across the label costs nothing.
pub const CLEAR_MAX: i32 = 32;

/// Whole dots of canvas beside each frame - left, right, up, down - that its pointer
/// padding may cover without reaching the frame of any element listed before it.
///
/// `frames` are canvas `(x, y, w, h)` in z-order, and the canvas hit-tests rows in
/// reverse, so a later element is asked about a press before every earlier one. Its
/// exact frame should win, being drawn over theirs; its padding should not, or a press
/// on an earlier element's own drawn frame selects a thin neighbour drawn after it. A
/// one-dot rule one dot below a box took presses on the box's last row at 1x and 2x,
/// where ten logical pixels of padding are several dots. Padding over a *later* frame
/// costs nothing, because that frame is asked first.
///
/// The frame grown by these amounts meets no earlier frame that has area:
/// - one overlapping the frame on both axes stops the padding on each side it runs
///   past;
/// - one beside the frame, overlapping it on the other axis, limits that side to the
///   gap between them;
/// - one off a corner limits whichever of its two sides has the wider gap, which gives
///   up the least.
pub fn clearances(frames: &[(i32, i32, i32, i32)]) -> Vec<[i32; 4]> {
    frames
        .iter()
        .enumerate()
        .map(|(i, &(x, y, w, h))| {
            let mut clear = [CLEAR_MAX; 4];
            let mut limit = |side: usize, gap: i64| {
                clear[side] = gap.min(i64::from(clear[side])) as i32;
            };
            // A frame with no area draws nothing a press could be meant for.
            for &(gx, gy, gw, gh) in frames[..i].iter().filter(|g| g.2 > 0 && g.3 > 0) {
                match (span(x, w, gx, gw), span(y, h, gy, gh)) {
                    (
                        Span::Over {
                            before: left,
                            after: right,
                        },
                        Span::Over {
                            before: up,
                            after: down,
                        },
                    ) => {
                        for (side, past) in [left, right, up, down].into_iter().enumerate() {
                            if past {
                                limit(side, 0);
                            }
                        }
                    }
                    (Span::Over { .. }, Span::Clear { after, gap }) => {
                        limit(2 + usize::from(after), gap)
                    }
                    (Span::Clear { after, gap }, Span::Over { .. }) => {
                        limit(usize::from(after), gap)
                    }
                    (
                        Span::Clear {
                            after: right,
                            gap: across,
                        },
                        Span::Clear {
                            after: down,
                            gap: along,
                        },
                    ) => {
                        if across >= along {
                            limit(usize::from(right), across);
                        } else {
                            limit(2 + usize::from(down), along);
                        }
                    }
                }
            }
            clear
        })
        .collect()
}

/// Where another span lies against a frame's span on one axis.
enum Span {
    /// Overlapping it, and whether it runs past its start and past its end.
    Over { before: bool, after: bool },
    /// Clear of it by `gap` dots, before its start or after its end.
    Clear { after: bool, gap: i64 },
}

/// The span `other..other + other_len` against `at..at + len`, both half-open. A
/// frame with no extent on this axis is overlapped only by a span that strictly
/// straddles it, which is what a press on its padding could land on.
fn span(at: i32, len: i32, other: i32, other_len: i32) -> Span {
    let (a0, a1) = (i64::from(at), i64::from(at) + i64::from(len));
    let (b0, b1) = (i64::from(other), i64::from(other) + i64::from(other_len));
    if b0 < a1 && b1 > a0 {
        Span::Over {
            before: b0 < a0,
            after: b1 > a1,
        }
    } else if b1 <= a0 {
        Span::Clear {
            after: false,
            gap: a0 - b1,
        }
    } else {
        Span::Clear {
            after: true,
            gap: b0 - a1,
        }
    }
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

    /// Every placement inside the announced interval is kept exactly, and every
    /// one outside it lands on the interval's nearer edge - so a field that
    /// announces `reach` is announcing what the editor will do.
    #[test]
    fn the_interval_a_placement_may_land_in_is_the_one_the_clamp_enforces() {
        let label = Label::new(96, 320, Dpmm::DPI_203);
        let b = Bounds::of(&label);
        for drawn in [
            frame(0, 0, 24, 40),
            frame(0, 0, 400, 1),
            frame(0, 0, 1, 900),
        ] {
            let feed = reach(drawn.feed_len, b.canvas.feed_len);
            let head = reach(drawn.head_len, b.canvas.head_len);
            for f in feed.0 - 3..=feed.1 + 3 {
                for h in [head.0 - 1, head.0, head.1, head.1 + 1] {
                    assert_eq!(
                        keep_reachable(&b, drawn, f, h),
                        (f.clamp(feed.0, feed.1), h.clamp(head.0, head.1)),
                        "{drawn:?} at feed {f}, head {h}"
                    );
                }
            }
        }
        assert_eq!(reach(24, 96), (-23, 95));
    }

    const WIDE: [i32; 4] = [CLEAR_MAX; 4];

    #[test]
    fn an_element_drawn_first_may_pad_its_frame_on_every_side() {
        assert_eq!(clearances(&[(8, 8, 40, 24)]), vec![WIDE]);
    }

    /// The case that was measured: an underline one dot below the box it underlines.
    #[test]
    fn a_rule_drawn_one_dot_below_an_earlier_box_may_pad_one_dot_towards_it() {
        let c = clearances(&[(8, 8, 40, 24), (8, 33, 40, 1)]);
        assert_eq!(c[0], WIDE);
        assert_eq!(c[1], [CLEAR_MAX, CLEAR_MAX, 1, CLEAR_MAX]);
    }

    #[test]
    fn only_an_element_drawn_before_limits_a_frames_padding() {
        let c = clearances(&[(8, 33, 40, 1), (8, 8, 40, 24)]);
        assert_eq!(c[0], WIDE, "the later box is asked about a press first");
        assert_eq!(c[1], [CLEAR_MAX, CLEAR_MAX, CLEAR_MAX, 1]);
    }

    #[test]
    fn an_earlier_frame_under_a_later_one_stops_its_padding_on_each_side_it_runs_past() {
        let c = clearances(&[(0, 0, 20, 20), (10, 10, 20, 20)]);
        assert_eq!(c[1], [0, CLEAR_MAX, 0, CLEAR_MAX]);
        let c = clearances(&[(0, 0, 40, 40), (10, 10, 4, 1)]);
        assert_eq!(c[1], [0, 0, 0, 0], "a rule inside a box drawn before it");
    }

    #[test]
    fn an_earlier_frame_off_a_corner_limits_the_side_with_the_wider_gap() {
        let c = clearances(&[(0, 0, 7, 9), (10, 10, 4, 4)]);
        assert_eq!(c[1], [3, CLEAR_MAX, CLEAR_MAX, CLEAR_MAX]);
    }

    #[test]
    fn a_frame_with_no_area_or_a_gap_wider_than_any_padding_limits_nothing() {
        let c = clearances(&[(0, 0, 0, 40), (0, 0, 40, 0), (10, 10, 4, 4)]);
        assert_eq!(c[2], WIDE);
        let c = clearances(&[(-100, 10, 20, 4), (10, 10, 4, 4)]);
        assert_eq!(c[1], WIDE);
    }

    /// Every frame grown by its clearances, against every earlier frame with area,
    /// dot by dot: no dot of the growth lies on one. Frames are drawn from a small
    /// square so that they touch, overlap and sit off each other's corners often.
    #[test]
    fn a_frame_padded_by_its_clearances_never_covers_an_earlier_frame() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: i32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as i32
        };
        for _ in 0..400 {
            let frames: Vec<(i32, i32, i32, i32)> = (0..6)
                .map(|_| (next(40) - 4, next(40) - 4, next(12), next(12)))
                .collect();
            for (i, (&(x, y, w, h), c)) in frames.iter().zip(clearances(&frames)).enumerate() {
                let on = |px: i32, py: i32, (fx, fy, fw, fh): (i32, i32, i32, i32)| {
                    px >= fx && px < fx + fw && py >= fy && py < fy + fh
                };
                for px in x - c[0]..x + w + c[1] {
                    for py in y - c[2]..y + h + c[3] {
                        if on(px, py, (x, y, w, h)) {
                            continue;
                        }
                        for &g in &frames[..i] {
                            assert!(
                                !on(px, py, g),
                                "frame {i} of {frames:?} padded by {c:?} covers dot {px},{py} \
                                 of earlier frame {g:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The two branches differ, which is why the interval is computed once: a
    /// frame with no extent along an axis restated as `1 - len ..= canvas - 1`
    /// would be announced as 1 to 319 and then held in 0 to 320.
    #[test]
    fn a_frame_with_no_extent_announces_the_interval_it_is_actually_held_in() {
        let label = Label::new(96, 320, Dpmm::DPI_203);
        let b = Bounds::of(&label);
        assert_eq!(reach(0, 320), (0, 320));
        assert_ne!(reach(0, 320), (1, 319));
        assert_eq!(
            keep_reachable(&b, frame(0, 0, 0, 0), -4, 9000),
            (reach(0, 96).0, reach(0, 320).1)
        );
    }
}
