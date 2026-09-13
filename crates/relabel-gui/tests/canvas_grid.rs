//! Where the dot grid lands, and what a fractional display scale does to it.
//!
//! This is the file that answers the question the whole canvas is arranged
//! around. A dot is a whole number of *device* pixels, never of logical ones,
//! so at the 1.25x and 1.5x that Windows and GNOME default to the numbers below
//! are fractions in logical units and exact integers in device units. The
//! measurements here are the ones a screenshot on a scaled display would have
//! been needed for.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::ElementHandle;
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::preview::{NoDots, Pane};
use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;
use support::{canvas_of, reference_label, rows_of};

include!(concat!(env!("OUT_DIR"), "/canvas-probe.rs"));

relabel_gui::impl_canvas_host!(CanvasProbe, ElRow);

/// The probe puts the canvas at 13.5 logical pixels, which is 16.875 device
/// pixels at 1.25x: a position no dot grid can start at.
const PLACED_AT: f32 = 13.5;

fn probe(scale: f32) -> (CanvasProbe, Rc<RefCell<Canvas<CanvasProbe>>>) {
    support::ensure_platform();
    let ui = CanvasProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(reference_label(), relabel_proto::PrintDirection::Left).expect("a label");
    let canvas = wire(&ui, doc);
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

/// Device pixels, from the logical position the toolkit reports.
fn device(logical: f32, scale: f32) -> f32 {
    logical * scale
}

fn is_whole(px: f32) -> bool {
    (px - px.round()).abs() < 1.0e-3
}

/// The pane is placed off the device grid and the grid anchor inside it puts the
/// dots back on. Both halves are asserted, because a test that only checked the
/// anchor would pass just as well with no anchor and a pane that happened to
/// land on a whole pixel.
#[test]
fn the_dot_grid_lands_on_whole_device_pixels_at_a_fractional_display_scale() {
    for scale in [1.25, 1.5] {
        let (ui, _canvas) = probe(scale);
        let pane = ElementHandle::find_by_element_id(&ui, "CanvasProbe::canvas")
            .next()
            .expect("the canvas instance");
        assert!(
            !is_whole(device(pane.absolute_position().x, scale)),
            "at {scale}x the pane itself is at {} device pixels, which was supposed to be a \
             fraction - the rest of this test proves nothing if it is not",
            device(PLACED_AT, scale)
        );
        let grid = canvas_of(&ui).absolute_position();
        for (axis, logical) in [("x", grid.x), ("y", grid.y)] {
            let px = device(logical, scale);
            assert!(
                is_whole(px),
                "at {scale}x the dot grid's {axis} is {px} device pixels"
            );
        }
    }
}

/// Every element frame is an exact number of dots from that origin, so no
/// element edge can fall between two device pixels however far down the layout
/// the canvas sits.
#[test]
fn every_element_frame_sits_on_the_dot_grid_at_a_fractional_scale() {
    let scale = 1.25;
    let (ui, canvas) = probe(scale);
    let origin = canvas_of(&ui).absolute_position();
    let shown = canvas.borrow().doc().shown();
    for (row, s) in rows_of(&ui).iter().zip(&shown) {
        let at = row.absolute_position();
        let size = row.size();
        assert_eq!(
            (
                device(at.x - origin.x, scale).round() as i32,
                device(at.y - origin.y, scale).round() as i32
            ),
            (s.x * DEFAULT_ZOOM as i32, s.y * DEFAULT_ZOOM as i32),
            "element {} is not on the grid",
            s.id
        );
        assert!(
            is_whole(device(at.x, scale)) && is_whole(device(at.y, scale)),
            "element {} is at a fractional device position",
            s.id
        );
        assert_eq!(
            (
                device(size.width, scale).round() as i32,
                device(size.height, scale).round() as i32
            ),
            (s.w * DEFAULT_ZOOM as i32, s.h * DEFAULT_ZOOM as i32),
            "element {} is not a whole number of dots across",
            s.id
        );
    }
}

/// The pane is the label, at the zoom, in device pixels - and the image in it is
/// exactly the extent the renderer reported, so the blit is one pixel to one
/// pixel and no filter runs.
#[test]
fn the_pane_is_the_label_at_the_zoom_in_device_pixels() {
    let scale = 1.5;
    let (ui, canvas) = probe(scale);
    let (dots_x, dots_y) = canvas.borrow().doc().canvas_dots();
    assert_eq!(
        (dots_x, dots_y),
        (320, 96),
        "the reference label is landscape"
    );
    assert_eq!(
        (ui.get_plane_px_w(), ui.get_plane_px_h()),
        (dots_x * DEFAULT_ZOOM as i32, dots_y * DEFAULT_ZOOM as i32)
    );
    let size = canvas_of(&ui).size();
    assert_eq!(
        (
            device(size.width, scale).round() as i32,
            device(size.height, scale).round() as i32
        ),
        (ui.get_plane_px_w(), ui.get_plane_px_h())
    );
}

/// The scale factor can change under a running window - dragging it to another
/// monitor - and nothing about the document is measured in pixels, so the dots
/// keep their size on paper and the grid stays on the device grid.
#[test]
fn the_grid_survives_the_display_scale_changing_under_it() {
    let (ui, canvas) = probe(1.0);
    let before = canvas.borrow().doc().shown();
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
    assert_eq!(canvas.borrow().doc().shown(), before, "nothing moved");
    let origin = canvas_of(&ui).absolute_position();
    assert!(is_whole(device(origin.x, 1.25)) && is_whole(device(origin.y, 1.25)));
    let row = rows_of(&ui)[0].size();
    assert_eq!(
        device(row.width, 1.25).round() as i32,
        40 * DEFAULT_ZOOM as i32
    );
}

/// Equal stamps mean equal pixels, so a repaint that would draw what is already
/// on screen is skipped before the buffer is even allocated - which is the only
/// saving available, because Slint compares image buffers by data pointer and a
/// fresh one always looks new.
#[test]
fn the_dots_are_not_expanded_again_when_nothing_moved() {
    let doc = Doc::open(reference_label(), relabel_proto::PrintDirection::Left).expect("a label");
    let mut pane = Pane::default();
    assert!(pane.draw(&doc, DEFAULT_ZOOM).expect("drawable").is_some());
    assert!(
        pane.draw(&doc, DEFAULT_ZOOM).expect("drawable").is_none(),
        "the same document twice is one expansion and one upload"
    );
    pane.invalidate();
    assert!(
        pane.draw(&doc, DEFAULT_ZOOM).expect("drawable").is_some(),
        "a zoom change has different pixels for the same dots"
    );
}

/// A zoom the label has no plane at is refused whole. Stored anyway, the frames
/// would be drawn at that zoom over a plane drawn at another, or over none.
#[test]
fn a_zoom_the_label_has_no_dots_at_is_refused_and_changes_nothing() {
    let (ui, canvas) = probe(1.0);
    assert_eq!(
        canvas.borrow_mut().set_zoom(&ui, 32),
        Err(NoDots::TooLarge),
        "320 by 96 dots at 32x is past the preview's byte cap"
    );
    assert_eq!(canvas.borrow().zoom(), DEFAULT_ZOOM);
    assert_eq!(ui.get_zoom(), DEFAULT_ZOOM as i32);
    assert_eq!(ui.get_plane_px_w(), 1280);
}

/// The zoom `dot` is multiplied by and the zoom the plane was expanded at are one
/// number, clamped once by `Scale::up`, and a real change of zoom repaints without
/// anyone forgetting the stamp first.
#[test]
fn the_zoom_pushed_to_the_window_is_the_one_the_dots_were_drawn_at() {
    let (ui, canvas) = probe(1.0);
    canvas
        .borrow_mut()
        .set_zoom(&ui, 0)
        .expect("zero reads as one");
    assert_eq!(
        (canvas.borrow().zoom(), ui.get_zoom(), ui.get_plane_px_w()),
        (1, 1, 320)
    );
    let one = ui.get_plane();
    assert_eq!((one.size().width, one.size().height), (320, 96));
    canvas
        .borrow_mut()
        .set_zoom(&ui, 2)
        .expect("drawable at 2x");
    assert_eq!(
        (canvas.borrow().zoom(), ui.get_zoom(), ui.get_plane_px_w()),
        (2, 2, 640)
    );
    let two = ui.get_plane();
    assert_eq!((two.size().width, two.size().height), (640, 192));
    assert!(two != one, "a new image for the new zoom");
}
