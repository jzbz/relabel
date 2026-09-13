//! Every element in the accessibility tree and the Tab order, wherever the view is.
//!
//! Tab gives focus only to an item `is_visible` passes (i-slint-core
//! window.rs:1325-1328), the accessibility tree hides every node that fails it
//! (i-slint-backend-winit accesskit.rs:576-577), and the testing backend's queries
//! skip the same items (search_api.rs:373-376) - so `rows_of` here is what an
//! assistive technology is offered, and a node missing from it is one no keyboard
//! can reach.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::ElementHandle;
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins;
use relabel_gui::view::{self, Opening, Pan, STEPS};
use relabel_render::document::Rot;
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use support::{
    canvas_of, field_of, frame, frame_of, press, rect, reference_label, rows_of, type_text, wheel,
};

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

fn port_of(ui: &InsProbe) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "Designer::port")
        .next()
        .expect("the view's port")
}

/// A point in the view, given in device pixels from its top-left.
fn in_view(ui: &InsProbe, x: f32, y: f32) -> LogicalPosition {
    let port = port_of(ui).absolute_position();
    let s = ui.window().scale_factor();
    LogicalPosition::new(port.x + x / s, port.y + y / s)
}

fn indices(ui: &InsProbe) -> Vec<usize> {
    rows_of(ui)
        .iter()
        .filter_map(ElementHandle::accessible_item_index)
        .collect()
}

/// Whether the node lies wholly inside the view Rust pans, to within a device pixel.
fn wholly_in_view(ui: &InsProbe, node: &ElementHandle) -> bool {
    let s = ui.window().scale_factor();
    let port = port_of(ui).absolute_position();
    let (at, size) = (node.absolute_position(), node.size());
    let slack = 1.0 / s;
    at.x >= port.x - slack
        && at.y >= port.y - slack
        && at.x + size.width <= port.x + ui.get_view_px_w() as f32 / s + slack
        && at.y + size.height <= port.y + ui.get_view_px_h() as f32 / s + slack
}

fn selected(canvas: &Wired) -> Option<u32> {
    canvas.borrow().doc().selected()
}

/// A wheel that pans as far as it goes: right for a positive `dx`, left otherwise.
fn wheel_far(ui: &InsProbe, dx: f32) {
    wheel(
        ui.window(),
        in_view(ui, 300.0, 200.0),
        -dx,
        0.0,
        false,
        false,
    );
}

/// The pans a view command may set, from the scene the view pushed.
fn pan_limits(ui: &InsProbe) -> ((i32, i32), (i32, i32)) {
    let lo = (ui.get_scene_x(), ui.get_scene_y());
    let hi = (
        (lo.0 + ui.get_scene_w() - ui.get_view_px_w()).max(lo.0),
        (lo.1 + ui.get_scene_h() - ui.get_view_px_h()).max(lo.1),
    );
    (lo, hi)
}

/// A label with two elements hanging almost wholly off it, one off each end: the
/// case where only the keep clamp holds the canvas in view.
fn hanging_label() -> relabel_render::Label {
    let mut label = reference_label();
    label.elements = vec![
        rect(1, frame(-303, 8, 304, 24), Rot::D0, false),
        rect(2, frame(319, 40, 304, 24), Rot::D0, false),
    ];
    label
}

#[test]
fn every_element_is_in_the_accessibility_tree_at_every_zoom_pan_and_scale() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, canvas) = probe(reference_label(), scale, Opening::At(DEFAULT_ZOOM));
        ui.invoke_zoom_one();
        for z in STEPS {
            assert_eq!(canvas.borrow().zoom(), z);
            let (lo, hi) = pan_limits(&ui);
            for (x, y) in [
                (-1_000_000, -1_000_000),
                ((lo.0 + hi.0) / 2, (lo.1 + hi.1) / 2),
                (1_000_000, 1_000_000),
            ] {
                ui.invoke_pan_to(x, y);
                let _list = canvas_of(&ui);
                assert_eq!(
                    indices(&ui),
                    vec![0, 1, 2],
                    "at {scale}x, {z}x, panned to {:?}",
                    (ui.get_pan_x(), ui.get_pan_y())
                );
            }
            ui.invoke_zoom_step(1);
        }
    }
}

#[test]
fn an_element_wholly_outside_the_view_is_a_point_just_inside_the_nearest_edge_with_its_whole_sentence()
 {
    let (ui, _canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let port = port_of(&ui);
    let right = port.absolute_position().x + port.size().width;
    let node = rows_of(&ui)[2].clone();
    assert_eq!((node.size().width, node.size().height), (0.0, 0.0));
    assert!(
        (node.absolute_position().x - (right - 1.0)).abs() < 0.01,
        "element 3's point is at {} with the view's right edge at {right}",
        node.absolute_position().x
    );
    assert!(
        (node.absolute_position().y - (port.absolute_position().y + 160.0)).abs() < 0.01,
        "and at its own head, which is in view"
    );
    assert_eq!(
        node.accessible_value().as_deref(),
        Some("Rectangle \u{2018}box 3\u{2019}, feed 240, head 40, 40 by 24 dots")
    );
}

#[test]
fn a_partly_visible_element_keeps_its_whole_frame_as_its_node() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(-1, 8, 40, 24), Rot::D0, false);
    let (ui, _canvas) = probe(label, 1.0, Opening::At(DEFAULT_ZOOM));
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0));
    let node = rows_of(&ui)[0].clone();
    assert_eq!(node.size().width, 160.0);
    assert!((node.absolute_position().x - (port_of(&ui).absolute_position().x - 4.0)).abs() < 0.01);
}

#[test]
fn tab_and_shift_tab_visit_every_element_and_bring_each_wholly_into_view() {
    for (scale, third) in [(1.0, 411), (1.25, 233), (1.5, 56)] {
        let (ui, canvas) = probe(reference_label(), scale, Opening::At(DEFAULT_ZOOM));
        let w = ui.window();
        let mut pans = Vec::new();
        for id in 1..=3 {
            press(w, Key::Tab, false, false);
            assert_eq!(selected(&canvas), Some(id));
            assert!(wholly_in_view(&ui, &rows_of(&ui)[id as usize - 1]));
            pans.push(ui.get_pan_x());
        }
        assert_eq!(
            pans,
            vec![0, 0, third],
            "at {scale}x in a view {} wide",
            ui.get_view_px_w()
        );
        for id in [2, 1] {
            press(w, Key::Tab, true, false);
            assert_eq!(selected(&canvas), Some(id));
            assert!(wholly_in_view(&ui, &rows_of(&ui)[id as usize - 1]));
        }
        assert_eq!(ui.get_pan_x(), 32, "at {scale}x");
    }
}

#[test]
fn tab_onto_a_long_element_that_already_fills_the_view_does_not_move_it() {
    let mut label = reference_label();
    label.elements = vec![
        rect(1, frame(8, 8, 304, 24), Rot::D0, false),
        rect(2, frame(8, 48, 304, 24), Rot::D0, false),
    ];
    let (ui, canvas) = probe(label, 1.0, Opening::At(8));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    assert_eq!(selected(&canvas), Some(1));
    assert_eq!(
        ui.get_pan_x(),
        64,
        "the least that fills the view with the frame"
    );
    let right_end = 64 + 304 * 8 - ui.get_view_px_w();
    ui.invoke_pan_to(right_end, ui.get_pan_y());
    assert_eq!(ui.get_pan_x(), right_end);
    press(w, Key::Tab, false, false);
    assert_eq!(selected(&canvas), Some(2));
    assert_eq!(
        ui.get_pan_x(),
        right_end,
        "the second fills the view across already"
    );
    let node = rows_of(&ui)[1].clone();
    let s = ui.window().scale_factor();
    let port = port_of(&ui).absolute_position();
    assert!(
        node.absolute_position().y >= port.y
            && node.absolute_position().y + node.size().height
                <= port.y + ui.get_view_px_h() as f32 / s + 0.01,
        "and down it is brought into view"
    );
}

#[test]
fn every_element_is_a_tab_stop_from_any_pan_the_view_allows() {
    for scale in [1.0, 1.25, 1.5] {
        for zoom in [DEFAULT_ZOOM, 12] {
            let (ui, canvas) = probe(reference_label(), scale, Opening::At(zoom));
            let (lo, hi) = pan_limits(&ui);
            let along = |a: i32, b: i32, n: i32| (0..=n).map(move |i| a + (b - a) * i / n);
            for x in along(lo.0, hi.0, 5) {
                for y in along(lo.1, hi.1, 2) {
                    ui.invoke_focus_canvas();
                    ui.invoke_pan_to(x, y);
                    for id in 1..=3 {
                        press(ui.window(), Key::Tab, false, false);
                        assert_eq!(
                            selected(&canvas),
                            Some(id),
                            "at {scale}x, {zoom}x, from pan {x},{y}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn tabbing_onto_the_element_already_selected_still_brings_it_into_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    assert_eq!(selected(&canvas), Some(1));
    wheel_far(&ui, 100_000.0);
    assert_eq!(ui.get_pan_x(), 1280 - ui.get_view_px_w());
    press(w, Key::F6, false, false);
    press(w, Key::F6, false, false);
    press(w, Key::Tab, false, false);
    assert_eq!(selected(&canvas), Some(1));
    assert_eq!(ui.get_pan_x(), 32);
}

#[test]
fn an_assistive_technology_writing_a_placement_brings_the_element_into_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    press(ui.window(), Key::Tab, false, false);
    rows_of(&ui)[0].set_accessible_value("feed 290, head 8");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 290);
    assert_eq!(ui.get_pan_x(), 330 * 4 - ui.get_view_px_w());
    assert!(wholly_in_view(&ui, &rows_of(&ui)[0]));
    // And from a view that shows none of it.
    ui.invoke_pan_to(0, 0);
    assert_eq!(rows_of(&ui)[0].size().width, 0.0, "out of view, a point");
    rows_of(&ui)[0].set_accessible_value("feed 150, head 8");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 150);
    assert!(wholly_in_view(&ui, &rows_of(&ui)[0]));
}

#[test]
fn an_arrow_nudge_past_the_view_edge_brings_the_element_back_into_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    for _ in 0..3 {
        press(w, Key::Tab, false, false);
    }
    // Element 3's right edge on the view's right edge.
    let pan = ui.get_pan_x();
    assert_eq!(pan + ui.get_view_px_w(), 1120);
    press(w, Key::RightArrow, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 3).feed, 241);
    assert_eq!(ui.get_pan_x(), pan + 4);
}

#[test]
fn a_nudge_on_an_element_longer_than_the_view_moves_the_pan_by_exactly_the_nudge_or_not_at_all() {
    let mut label = reference_label();
    label.elements = vec![rect(1, frame(8, 8, 304, 24), Rot::D0, false)];
    let (ui, _canvas) = probe(label, 1.0, Opening::At(8));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    assert_eq!(ui.get_pan_x(), 64);
    press(w, Key::RightArrow, false, false);
    assert_eq!(
        ui.get_pan_x(),
        72,
        "at the frame's start, by exactly the nudge"
    );
    ui.invoke_pan_to(900, 0);
    press(w, Key::RightArrow, false, false);
    assert_eq!(ui.get_pan_x(), 900, "inside the frame, not at all");
    press(w, Key::LeftArrow, false, false);
    assert_eq!(ui.get_pan_x(), 900);
}

#[test]
fn a_placement_typed_into_the_pane_brings_the_element_into_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    press(w, Key::F6, false, false);
    press(w, Key::Tab, false, false);
    type_text(w, "300");
    press(w, Key::Return, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 300);
    assert_eq!(ui.get_pan_x(), 340 * 4 - ui.get_view_px_w());
    assert!(wholly_in_view(&ui, &rows_of(&ui)[0]));
}

#[test]
fn undo_brings_the_element_it_moved_back_into_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    press(w, Key::F6, false, false);
    press(w, Key::Tab, false, false);
    type_text(w, "300");
    press(w, Key::Return, false, false);
    assert!(
        ui.get_pan_x() > 192,
        "the view followed the placement past where element 1 was"
    );
    press(w, "z", false, true);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8);
    assert_eq!(ui.get_pan_x(), 32);
    assert!(wholly_in_view(&ui, &rows_of(&ui)[0]));
}

#[test]
fn typing_a_name_leaves_the_view_where_the_operator_put_it() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    ui.invoke_pan_to(400, 0);
    press(w, Key::F6, false, false);
    type_text(w, "abc");
    press(w, Key::Return, false, false);
    assert_eq!(
        field_of(&ui, "name").accessible_value().as_deref(),
        Some("abc")
    );
    assert!(canvas.borrow().doc().dirty());
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (400, 0));
}

#[test]
fn the_keys_still_work_after_the_element_holding_the_keyboard_is_wheeled_out_of_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    wheel_far(&ui, 100_000.0);
    assert_eq!(
        rows_of(&ui)[0].size().width,
        0.0,
        "element 1 is out of view"
    );
    press(w, Key::RightArrow, false, false);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        9,
        "the arrow nudged"
    );
    assert_eq!(ui.get_pan_x(), 36, "and brought it back into view");
    wheel_far(&ui, 100_000.0);
    press(w, "z", false, true);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        8,
        "Ctrl+Z undid"
    );
    wheel_far(&ui, 100_000.0);
    press(w, "-", false, true);
    assert_eq!(canvas.borrow().zoom(), 3, "Ctrl+- zoomed");
    wheel_far(&ui, 100_000.0);
    assert_eq!(
        rows_of(&ui)[0].size().width,
        0.0,
        "element 1 is out of view again"
    );
    press(w, Key::F6, false, false);
    type_text(w, "Q");
    assert_eq!(
        canvas
            .borrow()
            .doc()
            .staged()
            .map(|(id, _, t)| (id, t.to_string())),
        Some((1, "Q".to_string())),
        "F6 took the keyboard into the pane"
    );
}

#[test]
fn a_press_where_an_out_of_view_node_sits_selects_nothing() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let point = rows_of(&ui)[2].absolute_position();
    for d in [0.5, 2.0, 5.0] {
        for at in [
            LogicalPosition::new(point.x - d, point.y),
            LogicalPosition::new(point.x - d, point.y + d),
            LogicalPosition::new(point.x, point.y + d),
        ] {
            let w = ui.window();
            support::down(w, at);
            support::up(w, at);
            assert_eq!(selected(&canvas), None, "a press at {at:?}");
        }
    }
}

#[test]
fn the_canvas_list_stays_in_the_tree_at_every_pan_the_view_allows() {
    for zoom in [DEFAULT_ZOOM, 16] {
        let (ui, _canvas) = probe(hanging_label(), 1.0, Opening::At(zoom));
        for (dx, dy) in [
            (-1.0e6, -1.0e6),
            (1.0e6, -1.0e6),
            (-1.0e6, 1.0e6),
            (1.0e6, 1.0e6),
        ] {
            ui.invoke_pan_by(dx, dy);
            // The list itself: each item passes the query alone even once the list
            // that holds it has left the tree.
            let _list = canvas_of(&ui);
            assert_eq!(
                indices(&ui),
                vec![0, 1],
                "at {zoom}x after a pan of {dx},{dy}"
            );
        }
    }
}

#[test]
fn the_canvas_stays_in_view_when_the_view_shrinks_past_a_pan_at_its_limit() {
    let mut label = reference_label();
    label.elements = vec![rect(1, frame(-303, 8, 304, 24), Rot::D0, false)];
    let (ui, canvas) = probe(label, 1.5, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    wheel_far(&ui, -100_000.0);
    assert_eq!(
        ui.get_pan_x(),
        2 - ui.get_view_px_w(),
        "two pixels of canvas kept"
    );
    click_at(&ui, in_view(&ui, 500.0, 80.0));
    assert_eq!(selected(&canvas), Some(1));
    w.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    let _list = canvas_of(&ui);
    assert_eq!(
        ui.get_pan_x(),
        2 - ui.get_view_px_w(),
        "the narrower view keeps two pixels too"
    );
    press(w, Key::F6, false, false);
    press(w, Key::F6, false, false);
    press(w, Key::RightArrow, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, -302);
}

fn click_at(ui: &InsProbe, at: LogicalPosition) {
    support::down(ui.window(), at);
    support::up(ui.window(), at);
}

#[test]
fn a_keyboard_zoom_keeps_the_selected_element_where_it_was_on_screen() {
    // Element 2 still fits after the zoom: its centre stays within a dot of the new
    // zoom of where it was.
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    press(ui.window(), Key::Tab, false, false);
    press(ui.window(), Key::Tab, false, false);
    let before = rows_of(&ui)[1].clone();
    let was = before.absolute_position().x + before.size().width / 2.0;
    press(ui.window(), "=", false, true);
    assert_eq!(canvas.borrow().zoom(), 6);
    let after = rows_of(&ui)[1].clone();
    let is = after.absolute_position().x + after.size().width / 2.0;
    assert!(
        (was - is).abs() < 6.0,
        "element 2's centre moved from {was} to {is}"
    );
    assert!(wholly_in_view(&ui, &after));

    // Element 3 at the view's edge no longer fits where the anchor leaves it, so it
    // is brought in by the least distance from there and no further.
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    for _ in 0..3 {
        press(w, Key::Tab, false, false);
    }
    let view = (i64::from(ui.get_view_px_w()), i64::from(ui.get_view_px_h()));
    let pan = Pan {
        x: i64::from(ui.get_pan_x()),
        y: i64::from(ui.get_pan_y()),
    };
    let anchor = view::key_anchor(pan, Some(view::frame_px(240, 40, 40, 24, 4)), view);
    let placed = view::zoom_about(pan, 4, 6, anchor);
    let expected = view::clamp(
        view::reveal(placed, view::frame_px(240, 40, 40, 24, 6), view),
        view::scene(canvas.borrow().doc(), 6),
        (1920, 576),
        view,
    );
    press(w, "=", false, true);
    assert_eq!(
        (i64::from(ui.get_pan_x()), i64::from(ui.get_pan_y())),
        (expected.x, expected.y)
    );
    assert!(wholly_in_view(&ui, &rows_of(&ui)[2]));
}
