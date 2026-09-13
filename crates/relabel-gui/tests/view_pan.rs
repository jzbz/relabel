//! Panning by pointer: the wheel, the trackpad and the two scroll bars, which are
//! the only pointer routes to pan - no drag on the canvas ever pans.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::ElementHandle;
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins::{self, Field};
use relabel_gui::view::{self, Opening};
use relabel_render::document::{ElementId, Rot};
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::cell::RefCell;
use std::rc::Rc;
use support::{down, frame, frame_of, move_to, press, rect, reference_label, type_text, up, wheel};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

fn probe(label: relabel_render::Label, scale: f32, opening: Opening) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    view::wire(&ui, &canvas, opening);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

fn by_id(ui: &InsProbe, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, id)
        .next()
        .unwrap_or_else(|| panic!("no element {id}"))
}

/// A point in the view, given in device pixels from its top-left.
fn in_view(ui: &InsProbe, x: f32, y: f32) -> LogicalPosition {
    let port = by_id(ui, "Designer::port").absolute_position();
    let s = ui.window().scale_factor();
    LogicalPosition::new(port.x + x / s, port.y + y / s)
}

/// The scroll bar thumbs that are drawn; an undrawn one is not in the tree.
fn thumbs(ui: &InsProbe) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(ui, "Scroller::thumb").collect()
}

fn across_thumb(ui: &InsProbe) -> ElementHandle {
    thumbs(ui)
        .into_iter()
        .find(|t| t.size().width > t.size().height)
        .expect("the horizontal thumb is drawn")
}

fn staged(canvas: &Wired) -> Option<(ElementId, Field, String)> {
    canvas
        .borrow()
        .doc()
        .staged()
        .map(|(id, f, t)| (id, f, t.to_string()))
}

#[test]
fn a_wheel_over_an_element_pans_the_view_and_moves_nothing() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(8));
    let before = canvas.borrow().doc().shown();
    // Element 1's centre at 8x: feed 28 and head 20 dots.
    let over = in_view(&ui, 224.0, 160.0);
    wheel(ui.window(), over, 0.0, -60.0, false, false);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 60));
    wheel(ui.window(), over, -60.0, 0.0, false, false);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (60, 60));
    assert_eq!(canvas.borrow().doc().shown(), before, "nothing moved");
    assert_eq!(canvas.borrow().doc().selected(), None, "nothing selected");
}

#[test]
fn shift_and_a_vertical_wheel_pans_across_and_a_horizontal_wheel_pans_across_on_its_own() {
    let (ui, _canvas) = probe(reference_label(), 1.25, Opening::At(DEFAULT_ZOOM));
    let over = in_view(&ui, 20.0, 20.0);
    wheel(ui.window(), over, 0.0, -60.0, true, false);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (75, 0));
    wheel(ui.window(), over, -60.0, 0.0, false, false);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (150, 0));
}

#[test]
fn control_and_the_wheel_zooms_about_the_pointer() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let (ax, ay) = (112.0, 80.0);
    let (px, py) = (ui.get_pan_x() as f32, ui.get_pan_y() as f32);
    wheel(ui.window(), in_view(&ui, ax, ay), 0.0, 60.0, false, true);
    assert_eq!(canvas.borrow().zoom(), 6);
    let (qx, qy) = (ui.get_pan_x() as f32, ui.get_pan_y() as f32);
    for (axis, before, after) in [
        ("across", (px + ax) / 4.0, (qx + ax) / 6.0),
        ("down", (py + ay) / 4.0, (qy + ay) / 6.0),
    ] {
        assert!(
            (before - after).abs() <= 0.5 / 6.0 + 1e-4,
            "the dot under the pointer {axis} was {before} and is {after}"
        );
    }
}

#[test]
fn small_trackpad_deltas_add_up_to_one_zoom_step() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let over = in_view(&ui, 300.0, 200.0);
    for i in 1..=11 {
        wheel(ui.window(), over, 0.0, 5.0, false, true);
        assert_eq!(canvas.borrow().zoom(), 4, "after {} logical pixels", i * 5);
    }
    wheel(ui.window(), over, 0.0, 5.0, false, true);
    assert_eq!(canvas.borrow().zoom(), 6, "sixty pixels is one notch");
    wheel(ui.window(), over, 0.0, -5.0, false, true);
    assert_eq!(
        canvas.borrow().zoom(),
        6,
        "a reversal starts the count again"
    );
}

#[test]
fn the_scroll_bar_thumb_pans_the_view_and_never_selects_an_element() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    assert_eq!(ui.get_pan_x(), 0);
    let thumb = across_thumb(&ui);
    let (at, size) = (thumb.absolute_position(), thumb.size());
    let from = LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    let track = by_id(&ui, "Designer::wheel").size().width;
    let travel = (ui.get_scene_w() - ui.get_view_px_w()) as f32;
    let w = ui.window();
    down(w, from);
    move_to(w, LogicalPosition::new(from.x + 100.0, from.y));
    up(w, LogicalPosition::new(from.x + 100.0, from.y));
    let expected = (100.0 / (track - size.width) * travel).round() as i32;
    assert!(
        (ui.get_pan_x() - expected).abs() <= 1,
        "the thumb dragged 100 px along a {track} px track panned {} where {expected} was due",
        ui.get_pan_x()
    );
    assert_eq!(canvas.borrow().doc().selected(), None);
    assert!(!ui.get_pointer_down());
}

#[test]
fn a_press_on_the_scroll_bar_track_centres_the_thumb_there() {
    let (ui, _canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let area = by_id(&ui, "Designer::wheel");
    let (origin, track) = (area.absolute_position(), area.size().width);
    let at = LogicalPosition::new(origin.x + 400.0, origin.y + area.size().height + 6.0);
    let w = ui.window();
    down(w, at);
    up(w, at);
    let thumb = across_thumb(&ui);
    let centre = thumb.absolute_position().x + thumb.size().width / 2.0 - origin.x;
    assert!(
        (centre - 400.0).abs() <= 1.0,
        "the thumb's middle is at {centre} after a press at 400 on a {track} px track"
    );
    let travel = (ui.get_scene_w() - ui.get_view_px_w()) as f32;
    let expected =
        ((400.0 - thumb.size().width / 2.0) / (track - thumb.size().width) * travel).round() as i32;
    assert!((ui.get_pan_x() - expected).abs() <= 1, "{}", ui.get_pan_x());
}

#[test]
fn a_pan_past_either_end_stops_at_the_scene() {
    let mut label = reference_label();
    label.elements = vec![
        rect(1, frame(-30, 8, 40, 24), Rot::D0, false),
        rect(2, frame(300, 8, 40, 24), Rot::D0, false),
    ];
    let (ui, _canvas) = probe(label, 1.0, Opening::At(DEFAULT_ZOOM));
    ui.invoke_pan_by(-1.0e6, 0.0);
    assert_eq!(
        ui.get_pan_x(),
        -120,
        "the part of element 1 off the label's start"
    );
    ui.invoke_pan_by(1.0e6, 0.0);
    assert_eq!(
        ui.get_pan_x(),
        340 * 4 - ui.get_view_px_w(),
        "the far end of element 2, off the label's end"
    );
}

#[test]
fn panning_is_not_an_undo_step_and_does_not_commit_a_typed_field() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    press(w, Key::F6, false, false);
    press(w, Key::Tab, false, false);
    type_text(w, "50");
    wheel(w, in_view(&ui, 300.0, 200.0), -100.0, 0.0, false, false);
    assert_eq!(ui.get_pan_x(), 100);
    assert_eq!(staged(&canvas), Some((1, Field::Feed, "50".to_string())));
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8);
    assert!(!canvas.borrow().doc().dirty(), "no undo step to take back");
}

/// A trackpad scrolled slowly reports fractions of a pixel an event: winit's
/// `PixelDelta` is converted to logical pixels unrounded (i-slint-backend-winit
/// event_loop.rs:404-407). Forty events of 0.4 px is sixteen pixels of travel.
#[test]
fn slow_trackpad_travel_pans_the_view_by_the_distance_it_covered() {
    let (ui, _canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let over = in_view(&ui, 300.0, 200.0);
    for _ in 0..40 {
        wheel(ui.window(), over, -0.4, 0.0, false, false);
    }
    assert!(
        (ui.get_pan_x() - 16).abs() <= 1,
        "forty trackpad events of 0.4 px panned {} device px where 16 were covered",
        ui.get_pan_x()
    );
}

#[test]
fn trackpad_travel_just_over_half_a_pixel_an_event_is_not_doubled() {
    let (ui, _canvas) = probe(reference_label(), 1.25, Opening::At(DEFAULT_ZOOM));
    let over = in_view(&ui, 300.0, 200.0);
    // 0.45 logical px is 0.5625 device px at 1.25x: forty of them is 22.5 px.
    for _ in 0..40 {
        wheel(ui.window(), over, -0.45, 0.0, false, false);
    }
    assert!(
        (ui.get_pan_x() - 22).abs() <= 1,
        "forty trackpad events of 0.5625 device px panned {} where 22.5 were covered",
        ui.get_pan_x()
    );
}

/// Press and release on the middle of the thumb, with no travel, and return the pan
/// on that axis before and after.
fn tap_thumb(ui: &InsProbe, across: bool) -> (i32, i32) {
    let pan = |ui: &InsProbe| {
        if across {
            ui.get_pan_x()
        } else {
            ui.get_pan_y()
        }
    };
    let before = pan(ui);
    let thumb = thumbs(ui)
        .into_iter()
        .find(|t| (t.size().width > t.size().height) == across)
        .expect("the thumb is drawn");
    let (at, size) = (thumb.absolute_position(), thumb.size());
    let on = LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    down(ui.window(), on);
    up(ui.window(), on);
    (before, pan(ui))
}

/// An element hanging off the label's start, at a zoom where the label itself fits
/// the view on that axis. `range_axis` lets the pan run from the hanging part to 0,
/// so the bar's track has to stand for exactly that range.
#[test]
fn a_press_on_the_scroll_bar_thumb_that_does_not_move_leaves_the_view_where_it_is() {
    let mut failures = Vec::new();

    // Across, at 2x - the zoom the stock label opens at - with element 1 hanging 36
    // of its 40 dots off the start.
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(-36, 8, 40, 24), Rot::D0, false);
    let (ui, _canvas) = probe(label, 1.0, Opening::At(2));
    assert!(
        ui.get_scene_w() > ui.get_view_px_w(),
        "the bar across is drawn"
    );
    let (before, after) = tap_thumb(&ui, true);
    if before != after {
        failures.push(format!("across at 2x: pan {before} became {after}"));
    }

    // Down, at 4x, with element 1 hanging 60 of its 64 dots off the top.
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, -60, 40, 64), Rot::D0, false);
    let (ui, _canvas) = probe(label, 1.0, Opening::At(DEFAULT_ZOOM));
    assert!(
        ui.get_scene_h() > ui.get_view_px_h(),
        "the bar down is drawn"
    );
    let (before, after) = tap_thumb(&ui, false);
    if before != after {
        failures.push(format!("down at 4x: pan {before} became {after}"));
    }

    assert!(
        failures.is_empty(),
        "a press on the thumb with no travel moved the view: {failures:?}"
    );
}
