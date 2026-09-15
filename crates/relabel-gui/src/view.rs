//! Where the canvas is seen from: the zoom the operator chose, the pan, and the
//! rules that keep every element reachable whichever of them is in force.
//!
//! # The rule
//!
//! Nothing on the label may become unreachable because of where the view is. Three
//! things make that true, and removing any one breaks it:
//!
//! - In `canvas.slint`, an element wholly outside the view keeps a node the size of
//!   a point just inside the view, so it stays in the accessibility tree and the Tab
//!   order.
//! - Here, the pan always leaves some of the canvas in view, because the canvas is
//!   the list node every element's node sits under and a hidden node takes its
//!   subtree with it.
//! - Also here, whenever anything but a held pointer selects an element or moves the
//!   selected one, the view pans the least distance that shows it, or, where a press
//!   left the pan past the scene along an axis with no room, returns to the one pan
//!   that axis allows.
//!
//! Never while a pointer holds an element: the drag's grab offset was measured
//! against where the canvas was at the press.
//!
//! # Units
//!
//! Whole device pixels, with the canvas origin at 0,0; an element hanging off the
//! canvas has negative coordinates. The zoom is `Scale::up`'s whole pixels per dot.
//! This slice never reduces, because the canvas's frames, focus points and pointer
//! arithmetic are all multiples of one whole-pixel dot.

use crate::canvas::{self, Canvas, Cause, ViewTell};
use crate::doc::Doc;
use crate::preview::NoDots;
use crate::rows::ViewHost;
use relabel_render::document::ElementId;
use relabel_render::preview::{Extent, Scale};
use std::cell::RefCell;
use std::rc::Rc;

/// The magnifications Zoom in and Zoom out walk. No step more than doubles the one
/// below, so neither key throws the operator's place away. Sixteen is the top
/// because every dragged dot repaints the whole plane: 6.4 ms a repaint at 16x on a
/// 40 mm label and 22 ms on a 100 mm one, against 27 ms at 26x (measured, release,
/// fit prototype). Sixteen device pixels is still eight logical pixels a dot on a
/// 2x display.
pub const STEPS: [u32; 8] = [1, 2, 3, 4, 6, 8, 12, 16];

/// Logical pixels of wheel travel per zoom step: one notch, which the winit backend
/// reports as a sixty-pixel line (i-slint-backend-winit event_loop.rs:401-421).
pub const NOTCH: f32 = 60.0;

/// Device pixels of canvas every pan keeps in view on each axis. Two rather than
/// one because `is_visible` counts touching edges (item_tree.rs:397-408) and a
/// point reached through two chains of float additions can land a hair outside.
pub const KEEP: i64 = 2;

/// What the operator chose. Fit is kept as a choice rather than as the number it
/// produced, so a window resized or moved to a display at another scale is fitted
/// again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Fit,
    Fixed(u32),
}

/// What `wire` starts at. The application opens fitted. A shell test opens at
/// `At(DEFAULT_ZOOM)`, so its distances do not depend on its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    Fit,
    At(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pan {
    pub x: i64,
    pub y: i64,
}

/// A rectangle in device pixels, canvas origin at 0,0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PxRect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
}

/// What a zoom keeps in place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// Keys and buttons: the selected element's centre while any of it is in view,
    /// else the view's centre; then the selection is revealed.
    Selection,
    /// Ctrl+wheel: device pixels from the view's top-left.
    Point(f64, f64),
    /// Fit: the canvas origin at the view's top-left; then the selection is revealed.
    Origin,
}

/// Why a view command changed nothing, as the status line says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    AtTop(u32),
    AtOne,
    Held,
    StillLarger,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::AtTop(z) => write!(f, "{} is as close as this label is drawn", Scale::up(*z)),
            Refusal::AtOne => f.write_str(
                "1x is as far out as the canvas draws: every dot keeps a pixel of its own",
            ),
            Refusal::Held => f.write_str("The zoom waits until the element is let go"),
            Refusal::StillLarger => f.write_str(
                "at 1x this label is still larger than the view: scroll, or Tab to an element \
                 to bring it into view",
            ),
        }
    }
}

/// The canvas's size in dots as `Scale` and `Extent` take it. The renderer's plane
/// is a `u16` on each side, so a side outside that range is never drawn at any
/// zoom; a negative side reads as none and a longer one saturates, which keeps the
/// arithmetic defined without inventing a label of some other size.
pub fn dims(dots: (i32, i32)) -> (u16, u16) {
    let side = |d: i32| u16::try_from(d.max(0)).unwrap_or(u16::MAX);
    (side(dots.0), side(dots.1))
}

/// The largest step this label has an `Extent` at, or 1. Past it the pane would
/// draw frames over nothing (canvas.rs repaint's `TooLarge` arm).
pub fn top(dots: (i32, i32)) -> u32 {
    let (w, h) = dims(dots);
    STEPS
        .iter()
        .rev()
        .copied()
        .find(|&z| Extent::new(w, h, Scale::up(z)).is_some())
        .unwrap_or(1)
}

/// Fit for a view `view` whole device pixels across and down:
/// `Scale::for_pane(..).pixels_per_dot()` where that magnifies, 1 where it would
/// reduce, and never above `top`. `for_pane` already steps down to a scale with an
/// `Extent`, and the division is relabel-render's, so a fit cannot round differently
/// from the pane (relabel-render preview.rs:136-160). A view with no size yet
/// answers 1.
pub fn fit(dots: (i32, i32), view: (i32, i32)) -> u32 {
    if view.0 <= 0 || view.1 <= 0 {
        return 1;
    }
    let (w, h) = dims(dots);
    let s = Scale::for_pane(w, h, view.0 as u32, view.1 as u32);
    if s.dots_per_pixel() > 1 {
        1
    } else {
        s.pixels_per_dot().min(top(dots))
    }
}

/// The next step in `dir` from `zoom`, which Fit may have left between two steps,
/// or `None` at 1 or at `top`.
pub fn step(zoom: u32, dir: Step, dots: (i32, i32)) -> Option<u32> {
    match dir {
        Step::In => STEPS
            .iter()
            .copied()
            .find(|&z| z > zoom)
            .filter(|&z| z <= top(dots)),
        Step::Out => STEPS.iter().rev().copied().find(|&z| z < zoom),
    }
}

/// The readout: `Fit, 2x` while Fit is the choice, else `3x`, in `Scale`'s spelling
/// (relabel-render preview.rs:203-212).
pub fn says(choice: Zoom, zoom: u32) -> String {
    match choice {
        Zoom::Fit => format!("Fit, {}", Scale::up(zoom)),
        Zoom::Fixed(_) => Scale::up(zoom).to_string(),
    }
}

/// A frame in dots, as `doc::Shown` carries it, at `zoom`.
pub fn frame_px(x: i32, y: i32, w: i32, h: i32, zoom: u32) -> PxRect {
    let z = i64::from(zoom);
    PxRect {
        x: i64::from(x) * z,
        y: i64::from(y) * z,
        w: i64::from(w) * z,
        h: i64::from(h) * z,
    }
}

/// What a pan may show at `zoom`: the canvas, every element frame, and, while an
/// unlocked element is selected, that frame's far edges carried out by `reach` device
/// pixels, so its grips can be panned into view. The near edges are not carried out.
///
/// `reach` is the canvas's `grip-reach-px`: the grips' far edge past the frame in whole
/// device pixels, plus one for the half pixel the dot grid's anchor can add. Without it
/// the pan stopped at the canvas's edge, and the east and corner grips of an element on
/// the label's end lay outside the view's clip, where they could be neither seen nor
/// pressed (measured under Xvfb at 4 pixels to the dot). The scene follows the
/// selection rather than widening for every element, so a Fit with nothing on the far
/// edges selected gains no scroll bar.
pub fn scene(doc: &Doc, zoom: u32, reach: i64) -> PxRect {
    let (cw, ch) = doc.canvas_dots();
    let z = i64::from(zoom);
    let (mut x0, mut y0) = (0, 0);
    let (mut x1, mut y1) = (i64::from(cw) * z, i64::from(ch) * z);
    for s in doc.shown() {
        let f = frame_px(s.x, s.y, s.w, s.h, zoom);
        x0 = x0.min(f.x);
        y0 = y0.min(f.y);
        x1 = x1.max(f.x + f.w);
        y1 = y1.max(f.y + f.h);
        if Some(s.id) == doc.selected() && !s.locked {
            x1 = x1.max(f.x + f.w + reach);
            y1 = y1.max(f.y + f.h + reach);
        }
    }
    PxRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

/// The least pan along one axis that shows `lo..lo + len` in a view `view` long.
///
/// A span longer than the view fills it by the least movement, so a view already
/// inside the span does not move. Showing the span's start instead threw the view
/// 788 px on a Tab between two full-width elements, and snapped it to the left end
/// on every nudge (measured by both judges).
pub fn reveal_axis(pan: i64, lo: i64, len: i64, view: i64) -> i64 {
    if len > view {
        pan.clamp(lo, lo + len - view)
    } else if lo < pan {
        lo
    } else if lo + len > pan + view {
        lo + len - view
    } else {
        pan
    }
}

/// Both axes of [`reveal_axis`].
pub fn reveal(pan: Pan, frame: PxRect, view: (i64, i64)) -> Pan {
    Pan {
        x: reveal_axis(pan.x, frame.x, frame.w, view.0),
        y: reveal_axis(pan.y, frame.y, frame.h, view.1),
    }
}

/// The pans a view command may set on one axis: the scene, widened so that 0 is
/// always one of them. So Fit's origin survives an element hanging off the canvas's
/// start, and a canvas smaller than its view is never panned.
pub fn range_axis(scene_lo: i64, scene_len: i64, view: i64) -> (i64, i64) {
    let lo = scene_lo.min(0);
    let hi = (scene_lo + scene_len - view).max(0);
    (lo, hi)
}

/// One axis kept to at least `KEEP` pixels of canvas in view. Wins over the scene.
pub fn keep_axis(pan: i64, canvas_len: i64, view: i64) -> i64 {
    pan.clamp(KEEP - view, (canvas_len - KEEP).max(KEEP - view))
}

/// Both halves, scene then keep, for a pan a view command sets.
pub fn clamp(pan: Pan, scene: PxRect, canvas: (i64, i64), view: (i64, i64)) -> Pan {
    let axis = |p: i64, lo: i64, len: i64, canvas_len: i64, v: i64| {
        let (a, b) = range_axis(lo, len, v);
        keep_axis(p.clamp(a, b), canvas_len, v)
    };
    Pan {
        x: axis(pan.x, scene.x, scene.w, canvas.0, view.0),
        y: axis(pan.y, scene.y, scene.h, canvas.1, view.1),
    }
}

/// [`clamp`] for a pan moving `from` one place toward `want`: along an axis with room
/// ([`room_axis`]) the scene half never takes it further from the scene than `from`
/// already is, and along an axis without it is [`clamp`].
///
/// A pan can lie past the scene without any view command having set it there, because
/// a press never moves the view and the scene follows the selection: pressing another
/// element after the pan reached past the canvas for the selected one's grips leaves
/// the pan past the canvas. [`clamp`] from there pulled a wheel pan toward the label's
/// end back by up to the reach, the opposite way to its travel, and pulled a selection
/// already in view back too, though a selection moves the view the least distance that
/// shows it.
///
/// Along an axis with no room the scene allows one pan and no scroll bar is drawn, so
/// a pan kept past the scene there could be returned by nothing but a zoom or the other
/// axis's bar, and a plain wheel, which turns across there, left the top of the canvas
/// hidden. There the pan is clamped outright, and the next wheel, trackpad pan or
/// selection returns the view to that one pan.
pub fn clamp_from(
    from: Pan,
    want: Pan,
    scene: PxRect,
    canvas: (i64, i64),
    view: (i64, i64),
) -> Pan {
    let axis = |f: i64, p: i64, lo: i64, len: i64, canvas_len: i64, v: i64| {
        let (a, b) = range_axis(lo, len, v);
        let (a, b) = if room_axis(lo, len, canvas_len, v) {
            (a.min(f), b.max(f))
        } else {
            (a, b)
        };
        keep_axis(p.clamp(a, b), canvas_len, v)
    };
    Pan {
        x: axis(from.x, want.x, scene.x, scene.w, canvas.0, view.0),
        y: axis(from.y, want.y, scene.y, scene.h, canvas.1, view.1),
    }
}

/// The keep half alone, for what the view does after an ordinary outcome or a
/// resize: it never moves the view unless the canvas would otherwise leave it.
pub fn keep(pan: Pan, canvas: (i64, i64), view: (i64, i64)) -> Pan {
    Pan {
        x: keep_axis(pan.x, canvas.0, view.0),
        y: keep_axis(pan.y, canvas.1, view.1),
    }
}

/// Whether a view command can set more than one pan along one axis: the pans
/// [`clamp`] allows run from `range_axis`'s low end kept to its high end kept, and
/// `keep_axis` never reverses their order. For a view and a canvas each longer than
/// `KEEP` pixels this is exactly when that axis's scroll bar is drawn (zoom.slint's
/// `needed`). A fact of the scene, the canvas and the view, never of the pan: a view
/// at the end of its room still has room.
pub fn room_axis(scene_lo: i64, scene_len: i64, canvas_len: i64, view: i64) -> bool {
    let (a, b) = range_axis(scene_lo, scene_len, view);
    keep_axis(a, canvas_len, view) != keep_axis(b, canvas_len, view)
}

/// Scroll travel in device pixels as a pan spends it. Travel with nothing across and
/// something down, over a view with no room down, pans across instead - down to the
/// right, as Shift sends it - because a label is drawn far wider than it is tall and at
/// most zooms has no room down, where a plain wheel otherwise panned nothing. Anything
/// else is spent as it came: travel with any part across is never turned, travel across
/// is never turned down, and nothing is carried from one axis to the other at an edge.
///
/// Keyed on the travel, not the device: the scroll event holds two deltas and the
/// modifiers and nothing else (i-slint-common builtin_structs.rs:65-75), `TouchArea`
/// drops the phase (i-slint-core items/input_items.rs:193-197), and a wheel's lines and
/// a trackpad's pixels arrive as the same logical pixels (i-slint-backend-winit
/// event_loop.rs:401-421). So a trackpad swipe with nothing across is turned too. Either
/// sign of zero counts as nothing across: the scroll handler's own negation
/// (shell.slint's `-e.delta-x`) makes the +0.0 of a wheel's absent axis a -0.0.
///
/// Where one event carries both axes (Wayland, macOS), a swipe that alternates frames
/// with nothing across and frames with a little across moves unevenly, since only the
/// first are turned: once the part across reaches a pixel a frame, one leaning left
/// steps right and back in turn.
pub fn wheel_travel(dx: f32, dy: f32, room_down: bool) -> Travel {
    if dx == 0.0 && dy != 0.0 && !room_down {
        Travel::Turned(dy)
    } else {
        Travel::AsItCame(dx, dy)
    }
}

/// Scroll travel in device pixels as [`wheel_travel`] routes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Travel {
    /// Across and down, spent as they came.
    AsItCame(f32, f32),
    /// Travel down, spent across.
    Turned(f32),
}

/// The pan that keeps the scene point under `at` there when the zoom goes from
/// `old` to `new`.
pub fn zoom_about(pan: Pan, old: u32, new: u32, at: (f64, f64)) -> Pan {
    let axis =
        |p: i64, a: f64| ((p as f64 + a) / f64::from(old) * f64::from(new) - a).round() as i64;
    Pan {
        x: axis(pan.x, at.0),
        y: axis(pan.y, at.1),
    }
}

/// Whether any device pixel of `frame` is in view.
pub fn shows_any(pan: Pan, frame: PxRect, view: (i64, i64)) -> bool {
    frame.x < pan.x + view.0
        && frame.x + frame.w > pan.x
        && frame.y < pan.y + view.1
        && frame.y + frame.h > pan.y
}

/// Where a key or button zoom is anchored, in view pixels.
pub fn key_anchor(pan: Pan, frame: Option<PxRect>, view: (i64, i64)) -> (f64, f64) {
    match frame.filter(|f| shows_any(pan, *f, view)) {
        Some(f) => (
            (f.x - pan.x) as f64 + f.w as f64 / 2.0,
            (f.y - pan.y) as f64 + f.h as f64 / 2.0,
        ),
        None => (view.0 as f64 / 2.0, view.1 as f64 / 2.0),
    }
}

/// Pan travel smaller than a device pixel, kept per axis until it adds up to one.
///
/// A trackpad reports fractions of a pixel an event, unrounded (i-slint-backend-winit
/// event_loop.rs:404-407). Rounded one event at a time, forty events of 0.4 px panned
/// nothing and forty of 0.5625 px panned forty, where sixteen and twenty-two and a half
/// were covered.
#[derive(Debug, Default)]
pub struct Drift {
    x: f64,
    y: f64,
    /// Travel down turned across, kept apart from `x`. Kept together, a swipe down and
    /// a little to the left sent an axis an event (winit x11/event_processor.rs:1148-1170,
    /// windows/event_loop.rs:1727-1765) reversed the one kept value every event, so each
    /// part wiped the other's fraction and forty pairs of 0.4 px down and 0.1 px left
    /// panned nothing, where 12 px net were covered.
    turned: f64,
    /// The pan the kept travel was left at. A view anywhere else was moved by another
    /// command since, and a fraction of a pixel of the old place is no part of the new.
    at: Option<Pan>,
}

impl Drift {
    /// Whole device pixels in `dx` and `dy` plus what was kept, for a view now at
    /// `from`. A change of direction starts that axis again, as [`Wheel`] does.
    pub fn pixels(&mut self, from: Pan, dx: f32, dy: f32) -> (i64, i64) {
        self.start_at(from);
        (spend(&mut self.x, dx), spend(&mut self.y, dy))
    }

    /// [`Drift::pixels`] for travel as [`wheel_travel`] routes it, travel turned across
    /// spending a kept fraction of its own.
    pub fn routed(&mut self, from: Pan, travel: Travel) -> (i64, i64) {
        match travel {
            Travel::AsItCame(dx, dy) => self.pixels(from, dx, dy),
            Travel::Turned(down) => {
                self.start_at(from);
                (spend(&mut self.turned, down), 0)
            }
        }
    }

    fn start_at(&mut self, from: Pan) {
        if self.at != Some(from) {
            self.x = 0.0;
            self.y = 0.0;
            self.turned = 0.0;
        }
    }

    /// Where the view came to rest once the pixels were spent.
    pub fn rest_at(&mut self, pan: Pan) {
        self.at = Some(pan);
    }
}

/// One axis of [`Drift::pixels`].
fn spend(kept: &mut f64, delta: f32) -> i64 {
    if delta == 0.0 {
        return 0;
    }
    if !delta.is_finite() {
        *kept = 0.0;
        // `as` saturates, and reads a NaN as 0.
        return delta as i64;
    }
    let delta = f64::from(delta);
    if *kept != 0.0 && (*kept > 0.0) != (delta > 0.0) {
        *kept = 0.0;
    }
    *kept += delta;
    let whole = kept.trunc();
    *kept -= whole;
    whole as i64
}

/// Wheel travel not yet spent on a step.
#[derive(Debug, Default)]
pub struct Wheel {
    travel: f32,
}

impl Wheel {
    /// Whole steps in `delta` logical pixels plus what was kept. A change of
    /// direction starts again, so a trackpad that reverses does not zoom the wrong
    /// way on travel left over from before.
    pub fn notches(&mut self, delta: f32) -> i32 {
        if delta == 0.0 || !delta.is_finite() {
            return 0;
        }
        if self.travel != 0.0 && (self.travel > 0.0) != (delta > 0.0) {
            self.travel = 0.0;
        }
        self.travel += delta;
        let whole = (self.travel / NOTCH).trunc();
        self.travel -= whole * NOTCH;
        whole as i32
    }
}

/// What the view keeps between callbacks. Shared by the view's callbacks and the
/// hook the canvas calls; never borrowed while the canvas calls that hook from
/// inside a view callback, because no view callback refreshes the canvas.
struct Seen {
    choice: Zoom,
    pan: Pan,
    /// The selection and its frame in dots as of the last outcome, so an edit
    /// that moved nothing does not snap the view back to it.
    last: Option<(ElementId, [i32; 4])>,
    wheel: Wheel,
    drift: Drift,
    /// A resize arrived while a pointer held an element: refit and keep at release.
    owed: bool,
}

/// A device-pixel count as the window's `int` properties hold it.
fn narrow(px: i64) -> i32 {
    px.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn view_of<H: ViewHost>(host: &H) -> (i64, i64) {
    (
        i64::from(host.get_view_px_w().max(0)),
        i64::from(host.get_view_px_h().max(0)),
    )
}

/// How far past the selected frame's far edges a pan reaches for its grips, in device
/// pixels, read when needed like the view's size.
fn reach_of<H: ViewHost>(host: &H) -> i64 {
    i64::from(host.get_grip_reach_px().max(0))
}

fn canvas_px(doc: &Doc, zoom: u32) -> (i64, i64) {
    let (w, h) = doc.canvas_dots();
    let z = i64::from(zoom);
    (i64::from(w) * z, i64::from(h) * z)
}

fn selected_dots(doc: &Doc) -> Option<(ElementId, [i32; 4])> {
    let s = doc.shown_one(doc.selected()?)?;
    Some((s.id, [s.x, s.y, s.w, s.h]))
}

fn selected_px(doc: &Doc, zoom: u32) -> Option<PxRect> {
    selected_dots(doc).map(|(_, [x, y, w, h])| frame_px(x, y, w, h, zoom))
}

fn push_pan<H: ViewHost>(host: &H, pan: Pan) {
    host.set_pan_x(narrow(pan.x));
    host.set_pan_y(narrow(pan.y));
}

/// The scroll bars' extent: exactly the pans `range_axis` allows, plus one view, so the
/// two ends of a track stand for the two ends of that range and a bar is drawn whenever
/// the range is more than one pan.
///
/// The scene widened only to 0 left out every pan between its end less a view and 0
/// when an element hung off the label's start: a press on the thumb with no travel then
/// panned 69 px, and no drag of the thumb could reach the label's origin. The extent
/// depends on the view, so it is pushed again whenever the view's size settles.
fn push_scene<H: ViewHost>(host: &H, doc: &Doc, zoom: u32) {
    let s = scene(doc, zoom, reach_of(host));
    let view = view_of(host);
    let (x0, x1) = range_axis(s.x, s.w, view.0);
    let (y0, y1) = range_axis(s.y, s.h, view.1);
    host.set_scene_x(narrow(x0));
    host.set_scene_y(narrow(y0));
    host.set_scene_w(narrow(x1 - x0 + view.0));
    host.set_scene_h(narrow(y1 - y0 + view.1));
}

fn push_says<H: ViewHost>(host: &H, choice: Zoom, zoom: u32) {
    host.set_zoom_says(says(choice, zoom).as_str().into());
}

fn refuse<H: ViewHost>(host: &H, why: Refusal) {
    host.set_status(why.to_string().as_str().into());
}

fn at_limit(dir: Step, dots: (i32, i32)) -> Refusal {
    match dir {
        Step::In => Refusal::AtTop(top(dots)),
        Step::Out => Refusal::AtOne,
    }
}

/// Change the zoom and place the view by `anchor`, then bring the selection into
/// view for every anchor but the pointer's, then clamp. Nothing here refreshes the
/// canvas, so the hook is not called while `Seen` is borrowed.
fn zoom_to<H: ViewHost>(
    host: &H,
    c: &mut Canvas<H>,
    s: &mut Seen,
    want: u32,
    anchor: Anchor,
) -> Result<(), NoDots> {
    let old = c.zoom();
    let view = view_of(host);
    let at = match anchor {
        Anchor::Selection => Some(key_anchor(s.pan, selected_px(c.doc(), old), view)),
        Anchor::Point(x, y) => Some((x, y)),
        Anchor::Origin => None,
    };
    c.set_zoom(host, want)?;
    let new = c.zoom();
    let doc = c.doc();
    let mut pan = at.map_or(Pan::default(), |a| zoom_about(s.pan, old, new, a));
    // After the anchor, so a selection that still fits is moved by the least that
    // shows it and a long one the anchor already fills is not moved at all. Not for
    // the wheel: a wheel operator aims with the pointer.
    if !matches!(anchor, Anchor::Point(..))
        && let Some(frame) = selected_px(doc, new)
    {
        pan = reveal(pan, frame, view);
    }
    s.pan = clamp(
        pan,
        scene(doc, new, reach_of(host)),
        canvas_px(doc, new),
        view,
    );
    push_pan(host, s.pan);
    push_scene(host, doc, new);
    Ok(())
}

/// A pan a view command asked for, clamped to the scene and then kept.
fn pan_to<H: ViewHost>(host: &H, c: &Canvas<H>, s: &mut Seen, want: Pan) {
    let (doc, zoom, view) = (c.doc(), c.zoom(), view_of(host));
    let pan = clamp(
        want,
        scene(doc, zoom, reach_of(host)),
        canvas_px(doc, zoom),
        view,
    );
    if pan != s.pan {
        s.pan = pan;
        push_pan(host, pan);
    }
}

/// What a change of the view's size costs: a refit while Fit is the choice and it
/// answers another zoom, otherwise only the keep clamp. No note either way, so the
/// orientation note `run_app` writes is not overwritten by the window settling.
fn settle<H: ViewHost>(host: &H, c: &mut Canvas<H>, s: &mut Seen) {
    let view = view_of(host);
    if s.choice == Zoom::Fit {
        let z = fit(c.doc().canvas_dots(), (narrow(view.0), narrow(view.1)));
        if z != c.zoom() && zoom_to(host, c, s, z, Anchor::Origin).is_ok() {
            push_says(host, s.choice, c.zoom());
            return;
        }
    }
    let pan = keep(s.pan, canvas_px(c.doc(), c.zoom()), view);
    if pan != s.pan {
        s.pan = pan;
        push_pan(host, pan);
    }
    push_scene(host, c.doc(), c.zoom());
}

/// Wire the view to a canvas already wired, and the pane if there is one. Called
/// last, after `canvas::wire` and `ins::wire`.
///
/// Every closure upgrades a weak handle or returns, as `canvas::wire`'s do, and every
/// view command does nothing but say why while a pointer holds an element: the
/// drag's grab offset is in dots of the zoom, and against the canvas's place, at the
/// press.
pub fn wire<H: ViewHost>(host: &H, canvas: &Rc<RefCell<Canvas<H>>>, opening: Opening) {
    let seen = Rc::new(RefCell::new(Seen {
        choice: Zoom::Fixed(canvas.borrow().zoom()),
        pan: Pan::default(),
        last: selected_dots(canvas.borrow().doc()),
        wheel: Wheel::default(),
        drift: Drift::default(),
        owed: false,
    }));

    // Called at the end of every `Canvas::refresh`, after the pane push. Rust sees
    // every way a selection is made or the selected element moves - Tab, an
    // assistive technology, a nudge, a typed placement, undo - and Slint's focus
    // sees only some of them, so the reveal lives here.
    {
        let seen = seen.clone();
        let tell: ViewTell<H> = Rc::new(move |host: &H, doc: &Doc, zoom: u32, cause: Cause| {
            let mut s = seen.borrow_mut();
            let now = selected_dots(doc);
            let moved = now != s.last;
            s.last = now;
            // Unconditionally: the scroll bars have no other source.
            push_scene(host, doc, zoom);
            // A held pointer's outcomes never move the view. A resize owed to the
            // release is paid by the end-move handler below, once this refresh has
            // returned, because a refit changes the zoom and that is not this hook's
            // to do from inside the canvas's own refresh.
            if host.get_pointer_down() {
                return;
            }
            let view = view_of(host);
            // From the pan, so an element already in view leaves it where it is even
            // when the pan lies past a scene the selection has just shrunk.
            let pan = match now {
                Some((_, [x, y, w, h])) if cause == Cause::Selection || moved => clamp_from(
                    s.pan,
                    reveal(s.pan, frame_px(x, y, w, h, zoom), view),
                    scene(doc, zoom, reach_of(host)),
                    canvas_px(doc, zoom),
                    view,
                ),
                // Only the keep clamp after an ordinary outcome: the scene clamp here
                // would move the view when a release shrank the scene.
                _ => keep(s.pan, canvas_px(doc, zoom), view),
            };
            if pan != s.pan {
                s.pan = pan;
                push_pan(host, pan);
            }
        });
        canvas.borrow_mut().tell_view(tell);
    }

    // Replaces `canvas::wire`'s handler, which Slint does not chain, with one that
    // runs the same end of the gesture and then pays a resize owed by the press.
    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_end_move(move |committed| {
            let Some(host) = weak.upgrade() else { return };
            canvas::end_move(&host, &canvas, committed);
            let owed = std::mem::take(&mut seen.borrow_mut().owed);
            if owed && !host.get_pointer_down() {
                let mut c = canvas.borrow_mut();
                let mut s = seen.borrow_mut();
                settle(&host, &mut c, &mut s);
            }
        });
    }

    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_zoom_step(move |by| {
            let Some(host) = weak.upgrade() else { return };
            if host.get_pointer_down() {
                refuse(&host, Refusal::Held);
                return;
            }
            let dir = match by.signum() {
                1 => Step::In,
                -1 => Step::Out,
                _ => return,
            };
            let mut c = canvas.borrow_mut();
            let mut s = seen.borrow_mut();
            let dots = c.doc().canvas_dots();
            let Some(z) = step(c.zoom(), dir, dots) else {
                refuse(&host, at_limit(dir, dots));
                return;
            };
            if zoom_to(&host, &mut c, &mut s, z, Anchor::Selection).is_ok() {
                s.choice = Zoom::Fixed(c.zoom());
                push_says(&host, s.choice, c.zoom());
            }
        });
    }
    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_zoom_one(move || {
            let Some(host) = weak.upgrade() else { return };
            if host.get_pointer_down() {
                refuse(&host, Refusal::Held);
                return;
            }
            let mut c = canvas.borrow_mut();
            let mut s = seen.borrow_mut();
            if zoom_to(&host, &mut c, &mut s, 1, Anchor::Selection).is_ok() {
                s.choice = Zoom::Fixed(c.zoom());
                push_says(&host, s.choice, c.zoom());
            }
        });
    }
    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_zoom_fit(move || {
            let Some(host) = weak.upgrade() else { return };
            if host.get_pointer_down() {
                refuse(&host, Refusal::Held);
                return;
            }
            let mut c = canvas.borrow_mut();
            let mut s = seen.borrow_mut();
            let view = view_of(&host);
            let z = fit(c.doc().canvas_dots(), (narrow(view.0), narrow(view.1)));
            if zoom_to(&host, &mut c, &mut s, z, Anchor::Origin).is_ok() {
                s.choice = Zoom::Fit;
                push_says(&host, s.choice, c.zoom());
                // No reach: this sentence is about the label, not its grips.
                let whole = scene(c.doc(), c.zoom(), 0);
                if c.zoom() == 1 && (whole.w > view.0 || whole.h > view.1) {
                    refuse(&host, Refusal::StillLarger);
                }
            }
        });
    }
    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_zoom_wheel(move |travel, at_x, at_y| {
            let Some(host) = weak.upgrade() else { return };
            if host.get_pointer_down() {
                refuse(&host, Refusal::Held);
                return;
            }
            let mut c = canvas.borrow_mut();
            let mut s = seen.borrow_mut();
            let notches = s.wheel.notches(travel);
            let dir = match notches.signum() {
                1 => Step::In,
                -1 => Step::Out,
                _ => return,
            };
            let dots = c.doc().canvas_dots();
            let mut changed = false;
            for _ in 0..notches.unsigned_abs() {
                let Some(z) = step(c.zoom(), dir, dots) else {
                    if !changed {
                        refuse(&host, at_limit(dir, dots));
                    }
                    break;
                };
                let about = Anchor::Point(f64::from(at_x), f64::from(at_y));
                if zoom_to(&host, &mut c, &mut s, z, about).is_err() {
                    break;
                }
                changed = true;
            }
            if changed {
                s.choice = Zoom::Fixed(c.zoom());
                push_says(&host, s.choice, c.zoom());
            }
        });
    }
    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_pan_by(move |dx, dy| {
            let Some(host) = weak.upgrade() else { return };
            if host.get_pointer_down() {
                return;
            }
            let c = canvas.borrow();
            let mut s = seen.borrow_mut();
            let (doc, zoom) = (c.doc(), c.zoom());
            // The scene `push_scene` drew the bars from, so room down is the bar.
            let whole = scene(doc, zoom, reach_of(&host));
            let room_down = room_axis(whole.y, whole.h, canvas_px(doc, zoom).1, view_of(&host).1);
            let from = s.pan;
            let (px, py) = s.drift.routed(from, wheel_travel(dx, dy, room_down));
            let want = Pan {
                x: from.x.saturating_add(px),
                y: from.y.saturating_add(py),
            };
            // Relative travel never moves the view against itself, so it is clamped
            // from where it started rather than by `pan_to`.
            let pan = clamp_from(from, want, whole, canvas_px(doc, zoom), view_of(&host));
            if pan != s.pan {
                s.pan = pan;
                push_pan(&host, pan);
            }
            let now = s.pan;
            s.drift.rest_at(now);
        });
    }
    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_pan_to(move |x, y| {
            let Some(host) = weak.upgrade() else { return };
            if host.get_pointer_down() {
                return;
            }
            let c = canvas.borrow();
            let mut s = seen.borrow_mut();
            let want = Pan {
                x: i64::from(x),
                y: i64::from(y),
            };
            pan_to(&host, &c, &mut s, want);
        });
    }
    {
        let (weak, canvas, seen) = (host.as_weak(), canvas.clone(), seen.clone());
        host.on_view_resized(move || {
            let Some(host) = weak.upgrade() else { return };
            if host.get_pointer_down() {
                seen.borrow_mut().owed = true;
                return;
            }
            let mut c = canvas.borrow_mut();
            let mut s = seen.borrow_mut();
            settle(&host, &mut c, &mut s);
        });
    }

    let mut c = canvas.borrow_mut();
    let mut s = seen.borrow_mut();
    match opening {
        Opening::Fit => {
            s.choice = Zoom::Fit;
            let view = view_of(host);
            let z = fit(c.doc().canvas_dots(), (narrow(view.0), narrow(view.1)));
            let _ = zoom_to(host, &mut c, &mut s, z, Anchor::Origin);
        }
        Opening::At(z) => {
            let _ = zoom_to(host, &mut c, &mut s, z, Anchor::Origin);
            s.choice = Zoom::Fixed(c.zoom());
        }
    }
    push_pan(host, s.pan);
    push_scene(host, c.doc(), c.zoom());
    push_says(host, s.choice, c.zoom());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Cmd;
    use relabel_render::document::{Common, Element, ElementOrRaw, Label, RectDots, RectEl};
    use relabel_render::target::Dpmm;

    const REFERENCE: (i32, i32) = (320, 96);
    const B4_PRO: (i32, i32) = (1229, 4134);
    const ET10: (i32, i32) = (1600, 1598);

    fn rect_label(dots: (i32, i32), frames: &[(i32, i32, i32, i32)]) -> Doc {
        let mut label = Label::new(dots.0, dots.1, Dpmm::DPI_203);
        label.elements = frames
            .iter()
            .enumerate()
            .map(|(i, &(feed, head, feed_len, head_len))| {
                let common = Common::new(
                    i as u32 + 1,
                    RectDots {
                        feed,
                        head,
                        feed_len,
                        head_len,
                    },
                );
                ElementOrRaw::Known(Element::Rect(RectEl {
                    common,
                    stroke_dots: 0,
                    radius_dots: 0,
                }))
            })
            .collect();
        Doc::open(label, relabel_proto::PrintDirection::Left).expect("a label")
    }

    #[test]
    fn fit_is_the_largest_whole_magnification_at_which_both_axes_fit() {
        assert_eq!(fit(REFERENCE, (709, 525)), 2);
        assert_eq!(fit(REFERENCE, (887, 657)), 2);
        assert_eq!(fit(REFERENCE, (1064, 788)), 3);
        assert_eq!(fit(REFERENCE, (1419, 1051)), 4);
        assert_eq!(fit((96, 96), (709, 525)), 5);
    }

    #[test]
    fn fit_is_scale_for_panes_own_answer_wherever_that_answer_magnifies() {
        for dots in [REFERENCE, (96, 96), (800, 96), (40, 300), B4_PRO, ET10] {
            let (w, h) = dims(dots);
            for vw in (0..4000).step_by(97) {
                for vh in (0..4000).step_by(89) {
                    let s = Scale::for_pane(w, h, vw, vh);
                    if s.dots_per_pixel() == 1 && s.pixels_per_dot() <= top(dots) {
                        assert_eq!(
                            fit(dots, (vw as i32, vh as i32)),
                            if vw == 0 || vh == 0 {
                                1
                            } else {
                                s.pixels_per_dot()
                            },
                            "{dots:?} in {vw}x{vh}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn fit_never_goes_below_one_pixel_to_the_dot() {
        assert_eq!(fit((800, 96), (709, 525)), 1);
        assert_eq!(fit(B4_PRO, (709, 525)), 1);
    }

    #[test]
    fn fit_is_never_more_than_the_top_step() {
        assert_eq!(fit((20, 20), (1000, 1000)), 16);
    }

    #[test]
    fn a_view_with_no_size_yet_fits_at_one_to_one() {
        assert_eq!(fit(REFERENCE, (0, 0)), 1);
        assert_eq!(fit(REFERENCE, (-5, -5)), 1);
    }

    #[test]
    fn the_top_step_is_the_largest_a_label_has_dots_at() {
        assert_eq!(top(REFERENCE), 16);
        assert_eq!(top(B4_PRO), 2);
        assert_eq!(top(ET10), 2);
    }

    #[test]
    fn zooming_in_from_between_two_steps_goes_to_the_next_step_up_and_out_to_the_next_step_down() {
        assert_eq!(step(5, Step::In, REFERENCE), Some(6));
        assert_eq!(step(5, Step::Out, REFERENCE), Some(4));
    }

    #[test]
    fn zooming_stops_at_one_to_one_and_at_the_top_step() {
        assert_eq!(step(1, Step::Out, REFERENCE), None);
        assert_eq!(step(16, Step::In, REFERENCE), None);
        assert_eq!(step(2, Step::In, B4_PRO), None);
    }

    #[test]
    fn the_readout_says_fit_and_the_scale_it_chose_or_only_the_scale() {
        assert_eq!(says(Zoom::Fit, 2), "Fit, 2x");
        assert_eq!(says(Zoom::Fixed(3), 3), "3x");
    }

    #[test]
    fn a_frame_already_in_view_leaves_the_pan_where_it_is() {
        let frame = PxRect {
            x: 480,
            y: 32,
            w: 160,
            h: 96,
        };
        assert_eq!(
            reveal(Pan { x: 411, y: 0 }, frame, (709, 525)),
            Pan { x: 411, y: 0 }
        );
    }

    #[test]
    fn a_frame_past_an_edge_is_brought_in_by_the_least_distance() {
        assert_eq!(reveal_axis(0, 960, 160, 709), 411);
        assert_eq!(reveal_axis(411, 480, 160, 709), 411);
        assert_eq!(reveal_axis(411, 32, 160, 709), 32);
    }

    #[test]
    fn a_frame_longer_than_the_view_moves_nothing_when_the_view_is_already_inside_it() {
        assert_eq!(reveal_axis(852, 0, 2432, 709), 852);
        assert_eq!(reveal_axis(-40, 0, 2432, 709), 0);
        assert_eq!(reveal_axis(2000, 0, 2432, 709), 1723);
    }

    #[test]
    fn a_frame_longer_than_the_view_and_wholly_out_of_it_is_shown_from_its_nearer_end() {
        assert_eq!(reveal_axis(0, 1000, 2000, 500), 1000);
        assert_eq!(reveal_axis(5000, 1000, 2000, 500), 2500);
    }

    #[test]
    fn the_pan_may_always_be_zero_even_with_an_element_hanging_off_the_start() {
        assert_eq!(range_axis(-156, 1436, 709), (-156, 571));
        let scene = PxRect {
            x: -156,
            y: 0,
            w: 1436,
            h: 384,
        };
        assert_eq!(
            clamp(Pan::default(), scene, (1280, 384), (709, 525)),
            Pan::default()
        );
    }

    #[test]
    fn a_pan_stops_at_the_scene_so_the_part_of_an_element_off_the_canvas_can_be_scrolled_to() {
        let scene = PxRect {
            x: -156,
            y: 0,
            w: 1436,
            h: 384,
        };
        assert_eq!(
            clamp(Pan { x: -500, y: 0 }, scene, (1280, 384), (709, 525)).x,
            -156
        );
    }

    #[test]
    fn a_pan_already_past_the_scene_is_clamped_only_toward_it() {
        let scene = PxRect {
            x: 0,
            y: 0,
            w: 1280,
            h: 384,
        };
        let (canvas, view) = ((1280, 384), (709, 525));
        let at = |x| Pan { x, y: 0 };
        assert_eq!(
            clamp(at(586), scene, canvas, view),
            at(571),
            "clamp pulls it back"
        );
        assert_eq!(
            clamp_from(at(586), at(596), scene, canvas, view),
            at(586),
            "further past"
        );
        assert_eq!(
            clamp_from(at(586), at(586), scene, canvas, view),
            at(586),
            "staying"
        );
        assert_eq!(
            clamp_from(at(586), at(576), scene, canvas, view),
            at(576),
            "back toward it"
        );
        assert_eq!(
            clamp_from(at(586), at(-50), scene, canvas, view),
            at(0),
            "back past its start"
        );
        assert_eq!(
            clamp_from(at(300), at(900), scene, canvas, view),
            clamp(at(900), scene, canvas, view),
            "from inside the scene it is clamp"
        );
        assert_eq!(
            clamp_from(at(2000), at(2000), scene, canvas, view),
            at(1278),
            "the keep clamp still wins"
        );
        let down = |y| Pan { x: 0, y };
        assert!(!room_axis(scene.y, scene.h, canvas.1, view.1));
        assert_eq!(
            clamp_from(down(10), down(10), scene, canvas, view),
            down(0),
            "no room down: the one pan"
        );
        assert_eq!(
            clamp_from(down(10), down(20), scene, canvas, view),
            clamp(down(20), scene, canvas, view),
            "no room down: clamp"
        );
    }

    #[test]
    fn no_pan_takes_the_whole_canvas_out_of_the_view() {
        let canvas = (800, 480);
        let view = (517, 517);
        let off_start = PxRect {
            x: -784,
            y: 0,
            w: 1584,
            h: 480,
        };
        assert_eq!(
            clamp(Pan { x: -5000, y: 0 }, off_start, canvas, view).x,
            -515
        );
        let off_end = PxRect {
            x: 0,
            y: 0,
            w: 1584,
            h: 480,
        };
        assert_eq!(
            clamp(Pan { x: 1_000_000, y: 0 }, off_end, canvas, view).x,
            798
        );
        // The keep half alone, as an ordinary outcome applies it.
        assert_eq!(
            keep(Pan { x: -5000, y: 5000 }, canvas, view),
            Pan { x: -515, y: 478 }
        );
    }

    #[test]
    fn a_label_smaller_than_its_view_is_not_panned() {
        let scene = PxRect {
            x: 0,
            y: 0,
            w: 640,
            h: 192,
        };
        for p in [-300, -1, 1, 300] {
            assert_eq!(
                clamp(Pan { x: p, y: p }, scene, (640, 192), (709, 525)),
                Pan::default()
            );
        }
    }

    #[test]
    fn a_zoom_about_a_point_keeps_the_dot_under_that_point() {
        let pan = zoom_about(Pan { x: 603, y: 0 }, 4, 8, (437.0, 0.0));
        assert_eq!(pan.x, 1643);
        assert_eq!((603 + 437) / 4, (pan.x + 437) / 8, "the dot under 437");
        let view = (709.0, 525.0);
        for old in STEPS {
            for new in STEPS {
                for at in [
                    (0.0, 0.0),
                    (view.0, 0.0),
                    (0.0, view.1),
                    (view.0, view.1),
                    (view.0 / 2.0, view.1 / 2.0),
                ] {
                    let before = Pan { x: 137, y: 61 };
                    let after = zoom_about(before, old, new, at);
                    for (p0, p1, a) in [(before.x, after.x, at.0), (before.y, after.y, at.1)] {
                        let was = (p0 as f64 + a) / f64::from(old);
                        let is = (p1 as f64 + a) / f64::from(new);
                        assert!(
                            (was - is).abs() <= 0.5 / f64::from(new) + 1e-9,
                            "{old}x to {new}x about {a}: the point under it moved from dot {was} \
                             to dot {is}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_keyboard_zooms_about_the_selected_element_while_any_of_it_is_in_view_and_about_the_views_centre_otherwise()
     {
        let view = (709, 525);
        let partly = PxRect {
            x: 600,
            y: 32,
            w: 160,
            h: 96,
        };
        assert_eq!(
            key_anchor(Pan::default(), Some(partly), view),
            (680.0, 80.0)
        );
        let gone = PxRect {
            x: 960,
            y: 32,
            w: 160,
            h: 96,
        };
        assert_eq!(key_anchor(Pan::default(), Some(gone), view), (354.5, 262.5));
        assert_eq!(key_anchor(Pan::default(), None, view), (354.5, 262.5));
    }

    #[test]
    fn wheel_travel_makes_one_zoom_step_per_notch_however_it_arrives() {
        let mut w = Wheel::default();
        assert_eq!(w.notches(60.0), 1);
        assert_eq!(w.travel, 0.0);
        let mut w = Wheel::default();
        let n: i32 = (0..12).map(|_| w.notches(5.0)).sum();
        assert_eq!((n, w.travel), (1, 0.0));
        let mut w = Wheel::default();
        assert_eq!(w.notches(-60.0), -1);
        let mut w = Wheel::default();
        assert_eq!((w.notches(90.0), w.travel), (1, 30.0));
    }

    /// Forty events at a time, as a slow trackpad sends them, with the pan moved by
    /// what each returns.
    fn pan_by_events(d: &mut Drift, dx: f32, events: u32) -> i64 {
        let mut pan = Pan::default();
        for _ in 0..events {
            let (x, _) = d.pixels(pan, dx, 0.0);
            pan.x += x;
            d.rest_at(pan);
        }
        pan.x
    }

    #[test]
    fn trackpad_travel_of_under_a_pixel_an_event_pans_by_the_pixels_it_covered() {
        assert_eq!(pan_by_events(&mut Drift::default(), 0.4, 40), 16);
        assert_eq!(pan_by_events(&mut Drift::default(), 0.5625, 40), 22);
        assert_eq!(pan_by_events(&mut Drift::default(), -0.5625, 40), -22);
        assert_eq!(pan_by_events(&mut Drift::default(), 60.0, 3), 180);
    }

    #[test]
    fn kept_pan_travel_starts_again_when_the_direction_reverses_or_the_view_was_moved_elsewhere() {
        let mut d = Drift::default();
        let origin = Pan::default();
        assert_eq!(d.pixels(origin, 0.75, 0.75), (0, 0));
        d.rest_at(origin);
        assert_eq!(
            d.pixels(origin, -0.5, 0.5),
            (0, 1),
            "across reversed and starts again; down goes on"
        );
        d.rest_at(Pan { x: 0, y: 1 });
        assert_eq!(
            d.pixels(Pan { x: 40, y: 1 }, 0.75, 0.75),
            (0, 0),
            "the view was moved by something else"
        );
    }

    #[test]
    fn travel_turned_across_keeps_a_fraction_of_its_own_beside_the_travel_across() {
        let mut d = Drift::default();
        let mut pan = Pan::default();
        for _ in 0..40 {
            for travel in [Travel::Turned(0.4), Travel::AsItCame(-0.1, 0.0)] {
                let (x, y) = d.routed(pan, travel);
                assert_eq!(y, 0);
                pan.x += x;
                d.rest_at(pan);
            }
        }
        assert_eq!(
            pan.x, 12,
            "16 turned right and 4 left, neither wiping the other"
        );

        let mut d = Drift::default();
        let origin = Pan::default();
        assert_eq!(d.routed(origin, Travel::Turned(0.75)), (0, 0));
        d.rest_at(origin);
        assert_eq!(
            d.routed(origin, Travel::Turned(-0.5)),
            (0, 0),
            "turned travel that reverses starts again"
        );
        d.rest_at(origin);
        assert_eq!(d.routed(origin, Travel::Turned(0.75)), (0, 0));
        d.rest_at(origin);
        assert_eq!(
            d.routed(Pan { x: 40, y: 0 }, Travel::Turned(0.75)),
            (0, 0),
            "the view was moved by something else"
        );
        d.rest_at(Pan { x: 40, y: 0 });
        assert_eq!(d.routed(Pan { x: 40, y: 0 }, Travel::Turned(0.5)), (1, 0));
    }

    #[test]
    fn pan_travel_that_is_not_a_number_pans_nothing_and_an_infinite_one_saturates() {
        let mut d = Drift::default();
        let origin = Pan::default();
        assert_eq!(d.pixels(origin, f32::NAN, 0.0), (0, 0));
        d.rest_at(origin);
        assert_eq!(
            d.pixels(origin, f32::INFINITY, f32::NEG_INFINITY),
            (i64::MAX, i64::MIN)
        );
        d.rest_at(origin);
        assert_eq!(d.pixels(origin, 0.5, 0.0), (0, 0), "and keeps no NaN");
        d.rest_at(origin);
        assert_eq!(d.pixels(origin, 0.5, 0.0), (1, 0));
    }

    #[test]
    fn a_change_of_wheel_direction_starts_the_count_again() {
        let mut w = Wheel::default();
        assert_eq!(w.notches(50.0), 0);
        assert_eq!((w.notches(-20.0), w.travel), (0, -20.0));
    }

    #[test]
    fn the_scene_is_the_canvas_and_every_frame_at_the_zoom() {
        let doc = rect_label(REFERENCE, &[(8, 8, 40, 24)]);
        assert_eq!(
            scene(&doc, 4, 15),
            PxRect {
                x: 0,
                y: 0,
                w: 1280,
                h: 384
            }
        );
        let doc = rect_label(REFERENCE, &[(-30, 8, 40, 24), (300, 80, 40, 24)]);
        assert_eq!(
            scene(&doc, 2, 15),
            PxRect {
                x: -60,
                y: 0,
                w: 340 * 2 + 60,
                h: 208
            }
        );
    }

    #[test]
    fn the_scene_reaches_past_the_selected_elements_far_edges_by_the_grips() {
        let px = |x, y, w, h| PxRect { x, y, w, h };
        let mut doc = rect_label(REFERENCE, &[(0, 0, 320, 96)]);
        assert_eq!(scene(&doc, 4, 15), px(0, 0, 1280, 384), "nothing selected");
        doc.apply(Cmd::Select(Some(1)));
        assert_eq!(scene(&doc, 4, 15), px(0, 0, 1295, 399));
        assert_eq!(scene(&doc, 4, 0), px(0, 0, 1280, 384));
        assert_eq!(range_axis(0, 1295, 709), (0, 586));
        assert!(room_axis(0, 399, 384, 390));
        assert!(!room_axis(0, 384, 384, 390));

        let mut label = Label::new(320, 96, Dpmm::DPI_203);
        let mut common = Common::new(
            1,
            RectDots {
                feed: 0,
                head: 0,
                feed_len: 320,
                head_len: 96,
            },
        );
        common.locked = true;
        label.elements = vec![ElementOrRaw::Known(Element::Rect(RectEl {
            common,
            stroke_dots: 0,
            radius_dots: 0,
        }))];
        let mut locked = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a label");
        locked.apply(Cmd::Select(Some(1)));
        assert_eq!(
            scene(&locked, 4, 15),
            px(0, 0, 1280, 384),
            "a locked element has no grips"
        );

        let mut inside = rect_label(REFERENCE, &[(8, 8, 40, 24)]);
        inside.apply(Cmd::Select(Some(1)));
        assert_eq!(
            scene(&inside, 4, 15),
            px(0, 0, 1280, 384),
            "grips that stay on the canvas"
        );
    }

    #[test]
    fn room_on_an_axis_is_more_than_one_pan_a_view_command_can_set() {
        assert!(!room_axis(0, 384, 384, 525), "the reference label at 4x");
        assert!(room_axis(0, 576, 576, 525), "at 6x");
        assert!(
            room_axis(-240, 624, 384, 525),
            "an element hanging 60 dots off the top at 4x"
        );
        assert!(!room_axis(0, 525, 525, 525));
        assert!(room_axis(0, 526, 526, 525));

        // Against the pans `clamp` allows.
        for lo in -6..=0 {
            for len in 1..=12 {
                for canvas_len in 1..=12 {
                    for view in -2..=12 {
                        let scene = PxRect {
                            x: 0,
                            y: lo,
                            w: 1,
                            h: len,
                        };
                        let at = |y| clamp(Pan { x: 0, y }, scene, (1, canvas_len), (1, view)).y;
                        assert_eq!(
                            room_axis(lo, len, canvas_len, view),
                            at(i64::MIN) != at(i64::MAX),
                            "lo {lo}, len {len}, canvas {canvas_len}, view {view}"
                        );
                    }
                }
            }
        }

        // Against the drawn bar: `push_scene` sets the extent to the range plus the
        // view, and zoom.slint's `needed` draws the bar when the extent is longer than
        // the view and the view is longer than 0.
        for canvas in 3..=12 {
            for view in 3..=16 {
                for lo in -6..=0 {
                    for end in canvas..=canvas + 6 {
                        let len = end - lo;
                        let (a, b) = range_axis(lo, len, view);
                        let extent = b - a + view;
                        assert_eq!(
                            room_axis(lo, len, canvas, view),
                            extent > view && view > 0,
                            "lo {lo}, len {len}, canvas {canvas}, view {view}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn scroll_travel_with_nothing_across_goes_across_only_where_there_is_no_room_down() {
        use Travel::{AsItCame, Turned};
        assert_eq!(
            wheel_travel(0.0, 60.0, false),
            Turned(60.0),
            "wheel down pans right"
        );
        assert_eq!(wheel_travel(0.0, -60.0, false), Turned(-60.0));
        assert_eq!(
            wheel_travel(-0.0, 0.4, false),
            Turned(0.4),
            "negative zero is nothing across"
        );
        assert_eq!(
            wheel_travel(0.0, 60.0, true),
            AsItCame(0.0, 60.0),
            "room down, even at its end"
        );
        assert_eq!(
            wheel_travel(30.0, 60.0, false),
            AsItCame(30.0, 60.0),
            "any travel across is spent as it came"
        );
        assert_eq!(wheel_travel(-5.0, 20.0, false), AsItCame(-5.0, 20.0));
        assert_eq!(
            wheel_travel(60.0, 0.0, false),
            AsItCame(60.0, 0.0),
            "travel across is never turned down"
        );
        assert_eq!(wheel_travel(60.0, 0.0, true), AsItCame(60.0, 0.0));
        assert_eq!(wheel_travel(0.0, 0.0, false), AsItCame(0.0, 0.0));
        let nan = wheel_travel(0.0, f32::NAN, false);
        assert!(matches!(nan, Turned(d) if d.is_nan()), "{nan:?}");
    }

    #[test]
    fn the_reference_label_has_no_room_down_up_to_four_and_has_some_from_six_in_the_scale_one_view()
    {
        let room = |doc: &Doc, z: u32| {
            let s = scene(doc, z, 15);
            room_axis(s.y, s.h, canvas_px(doc, z).1, 525)
        };
        let doc = rect_label(REFERENCE, &[(8, 8, 40, 24)]);
        for z in [2, 3, 4] {
            assert!(!room(&doc, z), "no room down at {z}x");
        }
        for z in [6, 8] {
            assert!(room(&doc, z), "room down at {z}x");
        }
        let hanging = rect_label(REFERENCE, &[(8, -60, 40, 64)]);
        assert!(room(&hanging, 4), "an element off the top gives room down");
    }

    #[test]
    fn each_refusal_says_what_it_refused() {
        assert_eq!(
            Refusal::AtTop(16).to_string(),
            "16x is as close as this label is drawn"
        );
        assert_eq!(
            Refusal::AtOne.to_string(),
            "1x is as far out as the canvas draws: every dot keeps a pixel of its own"
        );
        assert_eq!(
            Refusal::Held.to_string(),
            "The zoom waits until the element is let go"
        );
        assert_eq!(
            Refusal::StillLarger.to_string(),
            "at 1x this label is still larger than the view: scroll, or Tab to an element to \
             bring it into view"
        );
    }
}
