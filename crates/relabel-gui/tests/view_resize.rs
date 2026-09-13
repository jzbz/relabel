//! Fit as a mode: the label is fitted again when the view's size in device pixels
//! changes, and a change while a pointer holds an element waits for the release.
//!
//! The shell hears of a new size through a `changed` handler, which runs at the next
//! event-loop turn rather than inside the dispatch that caused it, so each test here
//! drains with mock time before it asserts (i-slint-backend-testing
//! testing_backend.rs:45-56 runs the change handlers).

#[path = "support/mod.rs"]
mod support;

use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins;
use relabel_gui::view::{self, Opening};
use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use support::{down, press, reference_label, rows_of, up};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

fn probe(scale: f32, opening: Opening) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(reference_label(), relabel_proto::PrintDirection::Left)
        .expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    view::wire(&ui, &canvas, opening);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    drain();
    (ui, canvas)
}

fn drain() {
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
}

fn rescale(ui: &InsProbe, scale: f32) {
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    drain();
}

#[test]
fn changing_the_display_scale_fits_the_label_again_while_fit_is_chosen() {
    let (ui, canvas) = probe(1.0, Opening::Fit);
    assert_eq!(canvas.borrow().zoom(), 2);
    rescale(&ui, 1.5);
    assert_eq!(canvas.borrow().zoom(), 3);
    assert_eq!(ui.get_zoom_says(), "Fit, 3x");
    assert!(ui.get_plane_px_w() <= ui.get_view_px_w());
    rescale(&ui, 1.0);
    assert_eq!(canvas.borrow().zoom(), 2);
}

#[test]
fn changing_the_display_scale_leaves_a_zoom_the_operator_chose() {
    let (ui, canvas) = probe(1.0, Opening::Fit);
    press(ui.window(), "=", false, true);
    assert_eq!(canvas.borrow().zoom(), 3);
    rescale(&ui, 1.5);
    assert_eq!(canvas.borrow().zoom(), 3);
    rescale(&ui, 1.0);
    assert_eq!(canvas.borrow().zoom(), 3);
    assert_eq!(ui.get_zoom_says(), "3x");
}

#[test]
fn a_scale_change_during_a_drag_is_paid_at_the_release() {
    let (ui, canvas) = probe(1.0, Opening::Fit);
    assert_eq!(canvas.borrow().zoom(), 2);
    let row = rows_of(&ui)[0].clone();
    let at = slint::LogicalPosition::new(
        row.absolute_position().x + row.size().width / 2.0,
        row.absolute_position().y + row.size().height / 2.0,
    );
    down(ui.window(), at);
    assert!(ui.get_pointer_down());
    rescale(&ui, 1.5);
    assert_eq!(canvas.borrow().zoom(), 2, "not while the element is held");
    up(ui.window(), at);
    assert!(!ui.get_pointer_down());
    assert_eq!(canvas.borrow().zoom(), 3, "at the release");
    assert_eq!(ui.get_zoom_says(), "Fit, 3x");
}
