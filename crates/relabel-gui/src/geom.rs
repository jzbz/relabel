//! Dot arithmetic for the canvas.
//!
//! No toolkit type appears in any signature here, which is what lets every
//! rule below be reached by an ordinary unit test rather than through a
//! simulated gesture. The canvas in `.slint` multiplies dots by a zoom and
//! nothing else.

use relabel_render::document::{Element, ElementId, RectDots, Rot};
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

/// The longest a frame is made by a drag or a typed length, in dots.
///
/// No canvas is longer on either axis: `Doc::open` refuses one that does not fit a
/// `u16`. The file's own lengths are signed 32-bit, so a longer one loaded from a
/// file is kept, and only an edit is held to this.
pub const MAX_LEN: i32 = u16::MAX as i32;

/// The interval a frame's length along one axis may be set to, given its origin
/// on that axis: at least one dot, and far enough past a negative origin that one
/// dot of it is still on the canvas.
///
/// The length's half of [`reach`]: that one keeps a dot on the canvas by moving the
/// origin, and a resize never moves the origin, so it keeps the dot by the length.
/// Announced by the length fields and enforced by the typed length and the grips
/// alike. Past an origin more than [`MAX_LEN`] dots off the canvas the interval is
/// the one length that reaches back onto it.
pub fn len_reach(origin: i32) -> (i32, i32) {
    let lo = 1i32.saturating_sub(origin).max(1);
    (lo, lo.max(MAX_LEN))
}

/// Which edge of the drawn frame a pointer grip drags.
///
/// Only the far edges: the origin is the placement the operator set, and a grip
/// that moved it would be a move and a resize in one gesture, two undoable things
/// the status line could only say as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    /// The right-hand edge: the canvas width.
    East,
    /// The lower edge: the canvas height.
    South,
    /// Their corner: both.
    SouthEast,
}

impl Grip {
    /// The integer the canvas sends back, for `Field::token`'s reason.
    pub fn token(self) -> i32 {
        self as i32
    }

    pub fn from_token(t: i32) -> Option<Grip> {
        match t {
            0 => Some(Grip::East),
            1 => Some(Grip::South),
            2 => Some(Grip::SouthEast),
            _ => None,
        }
    }

    fn across(self) -> bool {
        matches!(self, Grip::East | Grip::SouthEast)
    }

    fn down(self) -> bool {
        matches!(self, Grip::South | Grip::SouthEast)
    }
}

/// A frame resized by a grip dragged to canvas dots `right` and `bottom`, the
/// exclusive far edges, and what the operator is told when a bound held it.
///
/// Worked on the drawn canvas box, which is what the pointer is over, and written
/// back as stored lengths through [`Target::extent`] and the quarter-turn swap
/// [`as_drawn`] makes, so a Top printer and a turned element resize the side under
/// the pointer. The origin never moves, and the axis a grip does not name keeps its
/// length. Each named length is held to [`len_reach`]; past the opposite edge it
/// stops at one dot rather than turning the frame inside out, which would draw
/// nothing and report nothing.
///
/// `who` names the element in the note, which is said once, at the release.
pub fn resized(
    t: &Target,
    from: RectDots,
    rot: Rot,
    grip: Grip,
    right: i32,
    bottom: i32,
    who: &str,
) -> (RectDots, Option<String>) {
    let (x0, y0, w0, h0) = on_canvas(t, from, rot);
    let quarter = matches!(rot.quantised(), Rot::D90 | Rot::D270);
    // Which stored length a canvas axis is, for the note.
    let stored = |w: i32, h: i32| {
        let (dfl, dhl) = t.extent(w, h);
        if quarter { (dhl, dfl) } else { (dfl, dhl) }
    };
    let name = |horizontal: bool| {
        let (feed, _) = if horizontal {
            stored(1, 0)
        } else {
            stored(0, 1)
        };
        if feed == 1 { "feed" } else { "head" }
    };
    let mut notes = Vec::new();
    let mut one = |named: bool, start: i32, far: i32, now: i32, horizontal: bool| {
        if !named {
            return now;
        }
        let asked = i64::from(far) - i64::from(start);
        let (lo, hi) = len_reach(start);
        let got = asked.clamp(i64::from(lo), i64::from(hi)) as i32;
        let axis = name(horizontal);
        if asked < i64::from(lo) {
            notes.push(if lo == 1 {
                format!("{who} stops at a {axis} length of 1 dot")
            } else {
                format!(
                    "{who} stops at a {axis} length of {lo} dots so that a dot of it stays on \
                     the canvas"
                )
            });
        } else if asked > i64::from(hi) {
            notes.push(format!(
                "{who} stops at a {axis} length of {hi} dots, the longest a canvas is"
            ));
        }
        got
    };
    let w = one(grip.across(), x0, right, w0, true);
    let h = one(grip.down(), y0, bottom, h0, false);
    let (feed_len, head_len) = stored(w, h);
    let frame = RectDots {
        feed_len,
        head_len,
        ..from
    };
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    (frame, note)
}

/// The most dots of room a grip is looked for beyond an edge.
///
/// The canvas offers a grip only where its room is at least twenty logical pixels,
/// which is twenty dots at one pixel to the dot and scale 1: sixty-four covers the
/// scales and zooms at which a grip is offered at all, and bounds the search.
pub const GRIP_ROOM_MAX: i32 = 64;

/// What the canvas is pushed for the selected element's grips: its drawn canvas
/// box, whether it is locked, and the room beyond its east edge, south edge and
/// south-east corner, in that order, as [`grip_room`] measures it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grips {
    pub id: ElementId,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub room: [i32; 3],
}

/// Where an element puts ink, as placement and the grips keep clear of it: its drawn
/// canvas box `(x, y, w, h)`, the canvas box its ink is counted in, and, for a
/// rectangle whose outline leaves paper inside it, the outline's width in dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Solid {
    pub frame: (i32, i32, i32, i32),
    pub ink: (i32, i32, i32, i32),
    pub hollow: Option<i32>,
}

/// An element's [`Solid`] on this target.
///
/// Hollow exactly for a rectangle whose stroke is at least one dot and less than half
/// of each side. `rasterise` draws an outline as four straight bands of the stroke, or
/// of the shorter side where that is less, along the frame's edges, and draws no
/// radius, so the paper it leaves is the frame inset by the stroke on every side. The
/// inset is the same on every side, so a quarter turn or a Top printer, which swap the
/// sides, leave it the same. A stroke of 0 is a filled rectangle, and a stroke of half
/// a side or more meets itself and inks the whole frame. Text, symbols and artwork are
/// ink across their frame.
///
/// A line's ink box is its frame deepened to its thickness. `rasterise` draws a line as
/// one band from the frame's origin along the longer side, its thickness deep whatever
/// the frame's depth, so a thick line on a shallow frame inks past the frame, and a
/// line on a frame with no depth still inks. The frame is counted as well, so a grip
/// never lies on a line's frame and a line thinner than its frame is counted deeper
/// than it inks.
///
/// `the_paper_inside_a_hollow_rectangle_is_the_paper_the_renderer_leaves` and
/// `a_lines_ink_is_counted_wherever_the_renderer_draws_it` hold this to the dots
/// `rasterise` inks, so a renderer that draws a radius or a line another way fails
/// there first.
pub fn solid(t: &Target, e: &Element) -> Solid {
    let c = e.common();
    let frame = on_canvas(t, c.frame, c.rot);
    let (ink, hollow) = match e {
        Element::Rect(r) => {
            let s = i64::from(r.stroke_dots);
            let (w, h) = (i64::from(frame.2), i64::from(frame.3));
            let hollow = (s >= 1 && 2 * s < w && 2 * s < h).then_some(i32::from(r.stroke_dots));
            (frame, hollow)
        }
        Element::Line(l) => {
            let d = as_drawn(c.frame, c.rot);
            let depth = i32::from(l.thickness_dots).max(1);
            let band = if d.feed_len >= d.head_len {
                RectDots {
                    head_len: d.head_len.max(depth),
                    ..d
                }
            } else {
                RectDots {
                    feed_len: d.feed_len.max(depth),
                    ..d
                }
            };
            (on_canvas(t, band, Rot::D0), None)
        }
        _ => (frame, None),
    };
    Solid { frame, ink, hollow }
}

/// Whether the half-open canvas box `[x0, x1) x [y0, y1)`, given as
/// `[x0, x1, y0, y1]`, holds a dot of `s`'s ink as [`solid`] counts it.
///
/// True when the box meets the ink box and, for a hollow rectangle, does not lie wholly
/// on the paper inside its stroke. For a rectangle that is the same as holding an
/// inked dot: the paper is inset at least one dot on every side, so a box that meets
/// the frame and leaves the paper crosses the stroke. An ink box with no area and an
/// empty box hold no ink.
pub fn covers(s: &Solid, b: [i64; 4]) -> bool {
    covers_in(s, b, 1)
}

/// [`covers`] for a box given in `1 / unit` dots, so a grip's room can be centred on
/// the middle of an edge an odd number of dots long.
fn covers_in(s: &Solid, b: [i64; 4], unit: i64) -> bool {
    let (x, y, w, h) = s.ink;
    let (x, y) = (i64::from(x) * unit, i64::from(y) * unit);
    let (w, h) = (i64::from(w) * unit, i64::from(h) * unit);
    let [x0, x1, y0, y1] = b;
    if w <= 0 || h <= 0 || x0 >= x1 || y0 >= y1 {
        return false;
    }
    let meets = x0 < x + w && x < x1 && y0 < y + h && y < y1;
    let on_paper = s.hollow.is_some_and(|stroke| {
        let st = i64::from(stroke) * unit;
        x0 >= x + st && x1 <= x + w - st && y0 >= y + st && y1 <= y + h - st
    });
    meets && !on_paper
}

/// Whole dots of room beyond element `i`'s east edge, south edge and south-east
/// corner, which a grip may sit in without covering another element's ink.
///
/// Every other element counts, whatever its z-order, because the grips are hit-tested
/// before every row. Each answer is the largest `r` up to [`GRIP_ROOM_MAX`] for which
/// no other element's ink ([`covers`]) is in a square `r` dots on a side, written in
/// half dots so the middle of an edge an odd number of dots long is whole:
/// - east: `[2(x+w), 2(x+w+r)) x [2y+h-r, 2y+h+r)`, beyond the east edge's middle;
/// - south: `[2x+w-r, 2x+w+r) x [2(y+h), 2(y+h+r))`, beyond the south edge's middle;
/// - south-east: `[2(x+w), 2(x+w+r)) x [2(y+h), 2(y+h+r))`, beyond the corner.
///
/// The squares are centred where the canvas centres the grips. A grip spans six to
/// fourteen pixels beyond its edge and four either side of the middle, so a room of
/// twenty pixels or more leaves six clear of it on every side, more than the five the
/// widest grab padding reaches: a grip covers no element's ink and no padding. Two
/// exceptions follow from what counts as ink. An element whose frame has no area holds
/// none, unless it is a line with length, whose thickness inks even a frame with no
/// depth; so a grip can cover the padding of an element with no area or a line with no
/// length. And a rectangle with a stroke is ink only under its stroke, so a
/// grip may lie on the paper inside it, over its frame, where a press on the grip
/// resizes the selected element instead of moving the rectangle.
///
/// A search rather than a formula, because it is at most sixty-four widenings of three
/// squares against the label's elements, once per outcome, and the statement above is
/// then the code.
pub fn grip_room(solids: &[Solid], i: usize) -> [i32; 3] {
    let Some(s) = solids.get(i) else {
        return [0; 3];
    };
    let (x, y, w, h) = s.frame;
    let (x, y, w, h) = (i64::from(x), i64::from(y), i64::from(w), i64::from(h));
    let square = |side: usize, r: i64| match side {
        0 => [2 * (x + w), 2 * (x + w + r), 2 * y + h - r, 2 * y + h + r],
        1 => [2 * x + w - r, 2 * x + w + r, 2 * (y + h), 2 * (y + h + r)],
        _ => [2 * (x + w), 2 * (x + w + r), 2 * (y + h), 2 * (y + h + r)],
    };
    let blocked = |b: [i64; 4]| {
        solids
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && covers_in(other, b, 2))
    };
    [0, 1, 2].map(|side| {
        (1..=i64::from(GRIP_ROOM_MAX))
            .take_while(|&r| !blocked(square(side, r)))
            .last()
            .unwrap_or(0) as i32
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_proto::PrintDirection;
    use relabel_render::document::{Common, ElementOrRaw, Label, LineEl, RectEl};
    use relabel_render::rasterise::{local_size, rasterise};

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

    #[test]
    fn a_grip_token_round_trips_and_nothing_else_is_a_grip() {
        for g in [Grip::East, Grip::South, Grip::SouthEast] {
            assert_eq!(Grip::from_token(g.token()), Some(g));
        }
        assert_eq!(
            [Grip::East, Grip::South, Grip::SouthEast].map(Grip::token),
            [0, 1, 2]
        );
        assert_eq!(Grip::from_token(3), None);
        assert_eq!(Grip::from_token(-1), None);
    }

    #[test]
    fn a_resize_grows_the_drawn_box_under_every_turn_on_both_printers() {
        for t in [
            Target::for_label(96, 320, 203, PrintDirection::Left).expect("target"),
            Target::for_label(384, 240, 203, PrintDirection::Top).expect("target"),
        ] {
            for rot in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
                let f = frame(8, 8, 40, 24);
                let (x, y, w, h) = on_canvas(&t, f, rot);
                let grown = |grip, right, bottom| {
                    let (r, note) = resized(&t, f, rot, grip, right, bottom, "it");
                    assert_eq!(note, None, "{rot:?} {grip:?}");
                    assert_eq!((r.feed, r.head), (8, 8), "the origin never moves");
                    on_canvas(&t, r, rot)
                };
                let ctx = format!("{:?} {rot:?}", t.place(1, 0));
                assert_eq!(grown(Grip::East, x + w + 10, 0), (x, y, w + 10, h), "{ctx}");
                assert_eq!(grown(Grip::South, 0, y + h + 5), (x, y, w, h + 5), "{ctx}");
                assert_eq!(
                    grown(Grip::SouthEast, x + w + 10, y + h + 5),
                    (x, y, w + 10, h + 5),
                    "{ctx}"
                );
            }
        }
    }

    #[test]
    fn a_resize_stops_at_one_dot_and_keeps_a_dot_on_the_canvas() {
        let t = Target::for_label(96, 320, 203, PrintDirection::Left).expect("target");
        let (r, note) = resized(&t, frame(8, 8, 40, 24), Rot::D0, Grip::East, -100, 0, "it");
        assert_eq!(r, frame(8, 8, 1, 24));
        assert_eq!(note.as_deref(), Some("it stops at a feed length of 1 dot"));
        let (r, note) = resized(&t, frame(-30, 8, 40, 24), Rot::D0, Grip::East, -25, 0, "it");
        assert_eq!(r, frame(-30, 8, 31, 24));
        let note = note.expect("the clamp is said");
        assert!(note.contains("stays on the canvas"), "{note}");
        let (r, _) = resized(&t, frame(8, 8, 40, 24), Rot::D0, Grip::South, 0, 0, "it");
        assert_eq!(r.head_len, 1);
        let (r, note) = resized(
            &t,
            frame(8, 8, 40, 24),
            Rot::D90,
            Grip::East,
            90_000,
            0,
            "it",
        );
        assert_eq!(
            r.head_len, MAX_LEN,
            "a quarter turn draws the head length across"
        );
        let note = note.expect("the clamp is said");
        assert!(note.contains("head length of 65535"), "{note}");
    }

    /// Ink across the whole frame.
    fn plain(x: i32, y: i32, w: i32, h: i32) -> Solid {
        Solid {
            frame: (x, y, w, h),
            ink: (x, y, w, h),
            hollow: None,
        }
    }

    /// A rectangle stroked `stroke` dots.
    fn stroked(x: i32, y: i32, w: i32, h: i32, stroke: i32) -> Solid {
        Solid {
            hollow: Some(stroke),
            ..plain(x, y, w, h)
        }
    }

    fn line_el(f: RectDots, rot: Rot, thickness_dots: u16) -> Element {
        let mut common = Common::new(1, f);
        common.rot = rot;
        Element::Line(LineEl {
            common,
            thickness_dots,
        })
    }

    fn rect_el(f: RectDots, rot: Rot, stroke_dots: u16, radius_dots: u16) -> Element {
        let mut common = Common::new(1, f);
        common.rot = rot;
        Element::Rect(RectEl {
            common,
            stroke_dots,
            radius_dots,
        })
    }

    #[test]
    fn grip_room_is_the_clear_square_beyond_each_edge() {
        let one = plain(8, 8, 40, 24);
        assert_eq!(grip_room(&[one, plain(52, 8, 40, 24)], 0), [4, 64, 64]);
        // Centred on the east edge's middle, so a frame below and beyond that edge
        // limits the corner most.
        assert_eq!(grip_room(&[one, plain(60, 40, 40, 24)], 0), [40, 64, 12]);
        assert_eq!(grip_room(&[one, plain(50, 8, 0, 24)], 0), [64, 64, 64]);
        assert_eq!(grip_room(&[one], 0), [64, 64, 64]);
        // An element drawn before counts as much as one drawn after.
        assert_eq!(grip_room(&[plain(52, 8, 40, 24), one], 1), [4, 64, 64]);
        // Frames beyond both ends of a long edge: strips the length of the edge gave
        // 8, 8 and 8.
        assert_eq!(
            grip_room(
                &[
                    plain(8, 8, 96, 24),
                    plain(0, 40, 4, 4),
                    plain(108, 40, 4, 4)
                ],
                0
            ),
            [40, 64, 8]
        );
    }

    #[test]
    fn a_hollow_rectangle_is_ink_only_under_its_stroke_and_every_other_element_is_ink_across_its_frame()
     {
        let f = r#""frame": {"feed": 8, "head": 4, "feed_len": 96, "head_len": 24}"#;
        let json = format!(
            r#"{{"format": "relabel-label", "version": 1,
                "media": {{"feed_tenths_mm": 400, "head_tenths_mm": 120}},
                "canvas": {{"feed": 320, "head": 96, "dpmm": {{"num": 8, "den": 1}}}},
                "elements": [
                  {{"type": "text", "id": 1, {f}, "text": "{{sku}}", "size": 20.0}},
                  {{"type": "barcode", "id": 2, {f}, "symbology": "code128", "data": "1", "module_dots": 2, "bar_dots": 0}},
                  {{"type": "qr", "id": 3, {f}, "data": "x", "module_dots": 2}},
                  {{"type": "data_matrix", "id": 4, {f}, "data": "x", "module_dots": 2}},
                  {{"type": "image", "id": 5, {f}}},
                  {{"type": "rect", "id": 6, {f}, "stroke_dots": 2}},
                  {{"type": "line", "id": 7, {f}}},
                  {{"type": "rect", "id": 8, {f}, "stroke_dots": 0}}
                ]}}"#
        );
        let label = Label::from_json(&json).expect("a label of every kind");
        let t = Target::for_label(96, 320, 203, PrintDirection::Left).expect("target");
        assert_eq!(label.drawable().count(), 8);
        for e in label.drawable() {
            let c = e.common();
            let s = solid(&t, e);
            assert_eq!(s.frame, on_canvas(&t, c.frame, c.rot), "element {}", c.id);
            assert_eq!(
                s.ink, s.frame,
                "element {}: a one-dot line on a 24-dot frame",
                c.id
            );
            assert_eq!(s.hollow, (c.id == 6).then_some(2), "element {}", c.id);
        }

        let whole = frame(0, 0, 320, 96);
        assert_eq!(
            solid(&t, &rect_el(whole, Rot::D0, 2, 0)),
            stroked(0, 0, 320, 96, 2)
        );
        assert_eq!(
            solid(&t, &rect_el(whole, Rot::D0, 48, 0)).hollow,
            None,
            "a stroke that meets itself"
        );
        assert_eq!(
            solid(&t, &rect_el(whole, Rot::D0, 2, 6)).hollow,
            Some(2),
            "the renderer draws no radius"
        );

        let top = Target::for_label(384, 240, 203, PrintDirection::Top).expect("target");
        let turned = solid(&top, &rect_el(frame(8, 8, 40, 24), Rot::D90, 3, 0));
        assert_eq!(turned.frame, on_canvas(&top, frame(8, 8, 40, 24), Rot::D90));
        assert_eq!(turned.hollow, Some(3));

        let border = stroked(0, 0, 320, 96, 2);
        assert!(!covers(&border, [8, 56, 8, 40]), "on the paper inside");
        assert!(!covers(&border, [2, 318, 2, 94]), "all the paper inside");
        assert!(covers(&border, [0, 56, 8, 40]), "across the stroke");
        assert!(covers(&border, [2, 319, 2, 94]), "one dot onto the stroke");
        assert!(!covers(&border, [320, 330, 0, 96]), "beyond the frame");
        assert!(!covers(&border, [8, 8, 8, 40]), "an empty box");
        assert!(covers(&plain(0, 0, 320, 96), [8, 56, 8, 40]));
        assert!(!covers(&plain(8, 8, 0, 24), [0, 320, 0, 96]), "no area");
    }

    /// The renderer's own dots, one by one: `GreyMap` is 0 inked and 255 paper, and the
    /// hard paint has no partial coverage.
    #[test]
    fn the_paper_inside_a_hollow_rectangle_is_the_paper_the_renderer_leaves() {
        for (dots, t) in [
            (
                (320, 96),
                Target::for_label(96, 320, 203, PrintDirection::Left).expect("target"),
            ),
            (
                (240, 384),
                Target::for_label(384, 240, 203, PrintDirection::Top).expect("target"),
            ),
        ] {
            for rot in [Rot::D0, Rot::D90] {
                for stroke in [1u16, 2, 5, 30] {
                    let ctx = format!("{:?} {rot:?} stroke {stroke}", t.place(1, 0));
                    let e = rect_el(frame(10, 12, 60, 40), rot, stroke, 0);
                    let mut label = Label::new(dots.0, dots.1, Dpmm::DPI_203);
                    label.elements = vec![ElementOrRaw::Known(e.clone())];
                    let (g, _) = rasterise(&label, &t).expect("drawn");
                    let s = solid(&t, &e);
                    assert_eq!(
                        s.hollow,
                        (stroke < 30).then_some(i32::from(stroke)),
                        "{ctx}"
                    );
                    for y in 0..g.h {
                        for x in 0..g.w {
                            let ink = g.at(x, y);
                            assert!(ink == 0 || ink == 255, "{ctx}: dot {x},{y} is {ink}");
                            let (x0, y0) = (i64::from(x), i64::from(y));
                            assert_eq!(
                                covers(&s, [x0, x0 + 1, y0, y0 + 1]),
                                ink == 0,
                                "{ctx}: dot {x},{y}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The renderer's own dots, one by one, for lines on frames shallower than, as deep
    /// as, and deeper than their thickness, and on frames with no depth or no area.
    #[test]
    fn a_lines_ink_is_counted_wherever_the_renderer_draws_it() {
        for (dots, t) in [
            (
                (320, 96),
                Target::for_label(96, 320, 203, PrintDirection::Left).expect("target"),
            ),
            (
                (240, 384),
                Target::for_label(384, 240, 203, PrintDirection::Top).expect("target"),
            ),
        ] {
            for rot in [Rot::D0, Rot::D90] {
                for f in [
                    frame(10, 12, 60, 2),
                    frame(10, 12, 60, 40),
                    frame(10, 12, 60, 0),
                    frame(10, 12, 0, 40),
                    frame(10, 12, 0, 0),
                ] {
                    for thickness in [0u16, 1, 5, 40] {
                        let ctx =
                            format!("{:?} {rot:?} {f:?} thickness {thickness}", t.place(1, 0));
                        let e = line_el(f, rot, thickness);
                        let mut label = Label::new(dots.0, dots.1, Dpmm::DPI_203);
                        label.elements = vec![ElementOrRaw::Known(e.clone())];
                        let (g, _) = rasterise(&label, &t).expect("drawn");
                        let s = solid(&t, &e);
                        assert_eq!(s.frame, on_canvas(&t, f, rot), "{ctx}");
                        let depth = f.feed_len.min(f.head_len);
                        let exact = i32::from(thickness).max(1) >= depth;
                        for y in 0..g.h {
                            for x in 0..g.w {
                                let ink = g.at(x, y) == 0;
                                let (x0, y0) = (i64::from(x), i64::from(y));
                                let counted = covers(&s, [x0, x0 + 1, y0, y0 + 1]);
                                assert!(counted || !ink, "{ctx}: dot {x},{y} is inked");
                                assert!(!exact || counted == ink, "{ctx}: dot {x},{y}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// A line forty dots thick on a one-dot frame beside the selected element. Its
    /// frame alone gave the east grip thirty-eight dots of room, on the line's ink.
    #[test]
    fn grip_room_keeps_clear_of_a_lines_ink_where_it_runs_past_the_lines_frame() {
        let t = Target::for_label(96, 320, 203, PrintDirection::Left).expect("target");
        let line = solid(&t, &line_el(frame(50, 0, 100, 1), Rot::D0, 40));
        assert_eq!(line.frame, (50, 0, 100, 1));
        assert_eq!(line.ink, (50, 0, 100, 40));
        assert_eq!(grip_room(&[plain(8, 8, 40, 24), line], 0), [2, 44, 2]);
        let flat = solid(&t, &line_el(frame(0, 16, 320, 0), Rot::D0, 1));
        assert!(
            covers(&flat, [8, 48, 8, 32]),
            "a frame with no depth still inks"
        );
        let upright = solid(&t, &line_el(frame(0, 16, 0, 60), Rot::D0, 3));
        assert_eq!(upright.ink, (0, 16, 3, 60));
        let nothing = solid(&t, &line_el(frame(5, 5, 0, 0), Rot::D0, 3));
        assert!(
            !covers(&nothing, [0, 320, 0, 96]),
            "a frame with no length inks nothing"
        );
    }

    #[test]
    fn grip_room_keeps_clear_of_a_rectangles_stroke_and_not_of_the_paper_inside_it() {
        let mut s = [
            stroked(0, 0, 320, 96, 2),
            plain(8, 8, 40, 24),
            plain(120, 8, 40, 24),
            plain(240, 40, 40, 24),
        ];
        assert_eq!(grip_room(&s, 1), [36, 52, 62]);
        assert_eq!(grip_room(&s, 2), [36, 62, 62]);
        assert_eq!(grip_room(&s, 3), [38, 30, 30]);
        assert_eq!(grip_room(&s, 0), [64, 64, 64]);
        s[0].hollow = None;
        assert_eq!(grip_room(&s, 1), [0, 0, 0], "a filled rectangle under it");
    }

    #[test]
    fn a_length_may_be_set_from_one_dot_or_from_the_dot_that_keeps_it_on_the_canvas() {
        assert_eq!(len_reach(8), (1, 65535));
        assert_eq!(len_reach(0), (1, 65535));
        assert_eq!(len_reach(-30), (31, 65535));
        assert_eq!(len_reach(-70000), (70001, 70001));
        assert_eq!(len_reach(i32::MIN), (i32::MAX, i32::MAX));
    }
}
