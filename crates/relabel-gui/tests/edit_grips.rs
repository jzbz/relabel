//! The grips on the selected element's far edges: a drag resizes in whole dots and
//! is one undo step, a cancel rolls it back, and a grip is offered only where it
//! covers no other element, so a press on an element still moves it.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::ElementHandle;
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::view::{self, Opening};
use relabel_gui::{edit, ins};
use relabel_render::document::{Element, ElementOrRaw, Rot};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::cell::RefCell;
use std::rc::Rc;
use support::{
    canvas_of, click, down, frame, frame_of, grip_of, move_to, press, rect, reference_label,
    rows_of, type_text, up,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

fn probe_at(label: relabel_render::Label, scale: f32, zoom: u32) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    edit::wire(&ui, &canvas);
    view::wire(&ui, &canvas, Opening::At(zoom));
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

fn probe(label: relabel_render::Label) -> (InsProbe, Wired) {
    probe_at(label, 1.0, DEFAULT_ZOOM)
}

/// A point `dx`, `dy` logical pixels from the dot grid's origin.
fn at(ui: &InsProbe, dx: f32, dy: f32) -> LogicalPosition {
    let g = canvas_of(ui).absolute_position();
    LogicalPosition::new(g.x + dx, g.y + dy)
}

fn frame_1(canvas: &Wired) -> relabel_render::document::RectDots {
    frame_of(canvas.borrow().doc().label(), 1)
}

/// At 4x, element 1's east grip is at 198 to 206 across and 76 to 84 down.
fn drag_east_by_ten_dots(ui: &InsProbe) {
    let w = ui.window();
    down(w, at(ui, 202.0, 80.0));
    move_to(w, at(ui, 222.0, 80.0));
    move_to(w, at(ui, 242.0, 80.0));
    up(w, at(ui, 242.0, 80.0));
}

#[test]
fn a_grip_drag_resizes_in_one_undo_step() {
    let (ui, canvas) = probe(reference_label());
    click(&ui, &rows_of(&ui)[0]);
    drag_east_by_ten_dots(&ui);
    assert_eq!(frame_1(&canvas), frame(8, 8, 50, 24));
    assert!(!ui.get_pointer_down());
    press(ui.window(), "z", false, true);
    assert_eq!(frame_1(&canvas).feed_len, 40);
    press(ui.window(), "z", false, true);
    assert_eq!(ui.get_status(), "nothing to undo");
}

#[test]
fn a_press_inside_the_frame_still_moves_after_a_resize() {
    let (ui, canvas) = probe(reference_label());
    click(&ui, &rows_of(&ui)[0]);
    drag_east_by_ten_dots(&ui);
    let w = ui.window();
    down(w, at(&ui, 190.0, 80.0));
    move_to(w, at(&ui, 230.0, 80.0));
    up(w, at(&ui, 230.0, 80.0));
    assert_eq!(frame_1(&canvas), frame(18, 8, 50, 24));
}

#[test]
fn a_grip_is_offered_only_where_it_covers_no_other_element() {
    for (feed, want) in [(52, 0.0), (60, 8.0)] {
        let mut label = reference_label();
        label.elements = vec![
            rect(1, frame(8, 8, 40, 24), Rot::D0, false),
            rect(2, frame(feed, 8, 40, 24), Rot::D0, false),
        ];
        let (ui, _canvas) = probe(label);
        ui.invoke_select(1);
        assert_eq!(
            grip_of(&ui, "grip-e").size().width,
            want,
            "with the next element at feed {feed}"
        );
    }
}

#[test]
fn grips_are_eight_logical_pixels_at_every_zoom() {
    for zoom in [1, 2, 8] {
        let (ui, _canvas) = probe_at(reference_label(), 1.0, zoom);
        ui.invoke_select(1);
        for name in ["grip-e", "grip-s", "grip-se"] {
            let size = grip_of(&ui, name).size();
            assert_eq!((size.width, size.height), (8.0, 8.0), "{name} at {zoom}x");
        }
    }
}

#[test]
fn a_locked_element_offers_no_grip() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, 8, 40, 24), Rot::D0, true);
    let (ui, canvas) = probe(label);
    click(&ui, &rows_of(&ui)[0]);
    assert_eq!(grip_of(&ui, "grip-e").size().width, 0.0);
    drag_east_by_ten_dots(&ui);
    assert_eq!(frame_1(&canvas), frame(8, 8, 40, 24));
}

#[test]
fn a_cancelled_grip_drag_restores_the_frame_and_clears_the_press() {
    let (ui, canvas) = probe(reference_label());
    click(&ui, &rows_of(&ui)[0]);
    let w = ui.window();
    down(w, at(&ui, 202.0, 80.0));
    move_to(w, at(&ui, 242.0, 80.0));
    assert_eq!(frame_1(&canvas).feed_len, 50);
    w.dispatch_event(WindowEvent::PointerExited);
    assert_eq!(frame_1(&canvas), frame(8, 8, 40, 24));
    assert!(!ui.get_pointer_down());
    press(w, "z", false, true);
    assert_eq!(ui.get_status(), "nothing to undo");
}

#[test]
fn grips_follow_a_quarter_turn() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, 8, 40, 24), Rot::D90, false);
    let (ui, canvas) = probe(label);
    click(&ui, &rows_of(&ui)[0]);
    // Drawn 24 across and 40 down, so the east grip is at 134 to 142 and 108 to 116.
    let w = ui.window();
    down(w, at(&ui, 138.0, 112.0));
    move_to(w, at(&ui, 178.0, 112.0));
    up(w, at(&ui, 178.0, 112.0));
    assert_eq!(frame_1(&canvas), frame(8, 8, 40, 34));
}

#[test]
fn a_grip_drag_lands_on_whole_dots_at_a_fractional_scale() {
    let (ui, canvas) = probe_at(reference_label(), 1.5, 2);
    click(&ui, &rows_of(&ui)[0]);
    let grip = grip_of(&ui, "grip-s");
    let (g, size) = (grip.absolute_position(), grip.size());
    let from = LogicalPosition::new(g.x + size.width / 2.0, g.y + size.height / 2.0);
    // Twenty device pixels at 1.5x: ten dots at 2x.
    let to = LogicalPosition::new(from.x, from.y + 40.0 / 3.0);
    let w = ui.window();
    down(w, from);
    move_to(w, to);
    up(w, to);
    assert_eq!(frame_1(&canvas).head_len, 34);
}

#[test]
fn grips_add_nothing_to_the_accessibility_tree() {
    let (ui, _canvas) = probe(reference_label());
    let labels = |ui: &InsProbe| {
        rows_of(ui)
            .iter()
            .map(|r| r.accessible_label().map(|s| s.to_string()))
            .collect::<Vec<_>>()
    };
    let before = labels(&ui);
    ui.invoke_select(1);
    assert_eq!(grip_of(&ui, "grip-e").size().width, 8.0, "the grips are up");
    assert_eq!(rows_of(&ui).len(), 3);
    assert_eq!(canvas_of(&ui).accessible_item_count(), Some(3));
    assert_eq!(labels(&ui), before);
}

/// A second button pressed and released at `position` while the first is held.
fn other_button(ui: &InsProbe, position: LogicalPosition) {
    let button = PointerEventButton::Right;
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerPressed { position, button });
    w.dispatch_event(WindowEvent::PointerReleased { position, button });
}

#[test]
fn a_second_buttons_release_ends_a_grip_drag_as_its_own_undo_step() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[1]);
    press(w, Key::RightArrow, false, false);
    click(&ui, &rows_of(&ui)[0]);
    down(w, at(&ui, 202.0, 80.0));
    move_to(w, at(&ui, 222.0, 80.0));
    assert_eq!(frame_1(&canvas).feed_len, 45);
    // The second button's press opens nothing; its release is the release of the
    // grab, which the toolkit gives up at any button's release.
    other_button(&ui, at(&ui, 222.0, 80.0));
    assert!(!ui.get_pointer_down());
    assert!(!canvas.borrow().doc().pressed());
    move_to(w, at(&ui, 242.0, 80.0));
    up(w, at(&ui, 242.0, 80.0));
    assert_eq!(
        frame_1(&canvas).feed_len,
        45,
        "the drag ended at the release"
    );
    assert!(!canvas.borrow().doc().pressed());
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas).feed_len, 40);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 2).feed,
        121,
        "the nudge before the drag is a step of its own"
    );
}

#[test]
fn a_second_buttons_press_during_a_move_opens_nothing_and_its_release_ends_the_move() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[1]);
    press(w, Key::RightArrow, false, false);
    down(w, at(&ui, 112.0, 80.0));
    move_to(w, at(&ui, 120.0, 80.0));
    assert_eq!(frame_1(&canvas).feed, 10);
    other_button(&ui, at(&ui, 120.0, 80.0));
    assert!(!ui.get_pointer_down());
    assert!(!canvas.borrow().doc().pressed());
    move_to(w, at(&ui, 300.0, 300.0));
    up(w, at(&ui, 300.0, 300.0));
    assert!(!ui.get_pointer_down(), "nothing is left held");
    assert!(!canvas.borrow().doc().pressed());
    assert_eq!(frame_1(&canvas).feed, 10);
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas).feed, 8);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 2).feed, 121);
}

#[test]
fn a_grip_drag_resizes_from_a_length_still_being_typed() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, Key::F6, false, false);
    for _ in 0..3 {
        press(w, Key::Tab, false, false);
    }
    type_text(w, "60");
    assert_eq!(
        canvas
            .borrow()
            .doc()
            .staged()
            .map(|(id, f, t)| (id, f, t.to_string())),
        Some((1, ins::Field::FeedLen, "60".to_string()))
    );
    // On the grip of the frame as it stands, 40 dots long.
    down(w, at(&ui, 202.0, 80.0));
    assert_eq!(
        frame_1(&canvas).feed_len,
        60,
        "the typed length lands first"
    );
    assert!(ui.get_pointer_down(), "and the press still holds the grip");
    assert!(canvas.borrow().doc().pressed());
    move_to(w, at(&ui, 222.0, 80.0));
    move_to(w, at(&ui, 242.0, 80.0));
    up(w, at(&ui, 242.0, 80.0));
    assert_eq!(
        frame_1(&canvas).feed_len,
        70,
        "ten dots of drag are ten dots longer"
    );
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas).feed_len, 60);
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas).feed_len, 40);
}

/// The view's clipping port, whose clip decides whether a grip can be seen and pressed.
fn port_of(ui: &InsProbe) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "Designer::port")
        .next()
        .expect("the port; the probe was compiled without debug info")
}

/// A press at the middle of `g`, moved in four equal steps by `by` logical pixels,
/// and released there.
fn drag_grip(ui: &InsProbe, g: &ElementHandle, by: (f32, f32)) {
    let (p, size) = (g.absolute_position(), g.size());
    let from = LogicalPosition::new(p.x + size.width / 2.0, p.y + size.height / 2.0);
    let w = ui.window();
    down(w, from);
    for k in 1..=4 {
        let t = k as f32 / 4.0;
        move_to(
            w,
            LogicalPosition::new(from.x + by.0 * t, from.y + by.1 * t),
        );
    }
    up(w, LogicalPosition::new(from.x + by.0, from.y + by.1));
}

#[test]
fn the_grip_reach_is_the_grips_far_edge_in_whole_device_pixels_and_one_more() {
    for (scale, want) in [(1.0, 15), (1.25, 19), (1.5, 22), (2.0, 29)] {
        let (ui, _canvas) = probe_at(reference_label(), scale, 4);
        assert_eq!(ui.get_grip_reach_px(), want, "at scale {scale}");
    }
}

/// At 4 pixels to the dot a run of the binary found the east and corner grips of an
/// element on the label's end outside the view's clip, where the pan stopped at the
/// canvas's edge: a drag on them changed nothing.
#[test]
fn the_grips_of_an_element_on_the_labels_far_edges_can_be_panned_into_view_and_dragged() {
    let mut label = reference_label();
    label.elements = vec![
        rect(1, frame(0, 0, 320, 96), Rot::D0, false),
        rect(2, frame(8, 8, 40, 24), Rot::D0, false),
    ];
    for (scale, zoom, e, se, s) in [
        (1.0, 4, (310, 96), (310, 86), (320, 86)),
        (1.0, 8, (315, 96), (315, 91), (320, 91)),
        (1.5, 4, (305, 96), (305, 81), (320, 81)),
    ] {
        let (ui, canvas) = probe_at(label.clone(), scale, zoom);
        let ctx = format!("{zoom}x at scale {scale}");
        ui.invoke_select(1);
        let reach = ui.get_grip_reach_px();
        let zoom = zoom as i32;
        let (vw, vh) = (ui.get_view_px_w(), ui.get_view_px_h());
        assert_eq!(ui.get_scene_w(), 320 * zoom + reach, "{ctx}");
        ui.invoke_pan_by(1.0e6, 0.0);
        assert_eq!(ui.get_pan_x(), 320 * zoom + reach - vw, "{ctx}");
        for (name, by, want) in [
            ("grip-e", (-40.0, 0.0), e),
            ("grip-se", (-40.0, -40.0), se),
            ("grip-s", (0.0, -40.0), s),
        ] {
            let ctx = format!("{name} at {ctx}");
            if name == "grip-s" {
                ui.invoke_pan_to(160 * zoom - vw / 2, i32::MAX);
            } else {
                ui.invoke_pan_to(i32::MAX, i32::MAX);
            }
            assert_eq!(rows_of(&ui).len(), 2, "{ctx}");
            let g = grip_of(&ui, name);
            assert_eq!((g.size().width, g.size().height), (8.0, 8.0), "{ctx}");
            let (at, p) = (g.absolute_position(), port_of(&ui).absolute_position());
            assert!(
                at.x >= p.x - 0.01 && at.y >= p.y - 0.01,
                "{ctx}: {at:?} {p:?}"
            );
            assert!(
                at.x + 8.0 <= p.x + vw as f32 / scale + 0.01
                    && at.y + 8.0 <= p.y + vh as f32 / scale + 0.01,
                "{ctx}: the grip at {at:?} is not wholly inside the port at {p:?}"
            );
            drag_grip(&ui, &g, by);
            let label = canvas.borrow().doc().label().clone();
            assert_eq!(frame_of(&label, 1), frame(0, 0, want.0, want.1), "{ctx}");
            assert_eq!(frame_of(&label, 2), frame(8, 8, 40, 24), "{ctx}");
            assert!(!ui.get_pointer_down(), "{ctx}");
            press(ui.window(), "z", false, true);
            assert_eq!(frame_1(&canvas), frame(0, 0, 320, 96), "{ctx}");
        }
    }
}

/// A label with a two-dot border drawn first, round the reference label's elements.
fn bordered_label() -> relabel_render::Label {
    let mut label = reference_label();
    let mut border = rect(4, frame(0, 0, 320, 96), Rot::D0, false);
    if let ElementOrRaw::Known(Element::Rect(r)) = &mut border {
        r.stroke_dots = 2;
    }
    label.elements.insert(0, border);
    label
}

/// A run of the binary found no element inside the kitchen-sink label's border
/// offered a grip, because the border's frame enclosed them.
#[test]
fn an_element_inside_a_border_offers_grips_and_a_press_on_the_border_elsewhere_still_moves_it() {
    let (ui, canvas) = probe(bordered_label());
    ui.invoke_select(1);
    for name in ["grip-e", "grip-s", "grip-se"] {
        let size = grip_of(&ui, name).size();
        assert_eq!((size.width, size.height), (8.0, 8.0), "{name}");
    }
    let border = |c: &Wired| frame_of(c.borrow().doc().label(), 4);
    drag_east_by_ten_dots(&ui);
    assert_eq!(frame_1(&canvas), frame(8, 8, 50, 24));
    assert_eq!(border(&canvas), frame(0, 0, 320, 96));
    press(ui.window(), "z", false, true);
    assert_eq!(frame_1(&canvas).feed_len, 40);

    let w = ui.window();
    down(w, at(&ui, 112.0, 80.0));
    move_to(w, at(&ui, 152.0, 80.0));
    up(w, at(&ui, 152.0, 80.0));
    assert_eq!(frame_1(&canvas), frame(18, 8, 40, 24));
    assert_eq!(border(&canvas), frame(0, 0, 320, 96));

    // Dot 100, 70: on the border's frame, on no other element and on no grip.
    down(w, at(&ui, 400.0, 280.0));
    move_to(w, at(&ui, 440.0, 280.0));
    up(w, at(&ui, 440.0, 280.0));
    assert_eq!(border(&canvas).feed, 10);
    assert_eq!(canvas.borrow().doc().selected(), Some(4));
    assert_eq!(frame_1(&canvas), frame(18, 8, 40, 24));
}

/// Rooms 36, 52 and 62 dots, which are 72, 104 and 124 pixels at 2x. Strips the
/// length of each edge would have left 6, 6 and 62.
#[test]
fn an_element_a_millimetre_inside_a_border_offers_all_three_grips_at_two_pixels_to_the_dot() {
    let (ui, _canvas) = probe_at(bordered_label(), 1.0, 2);
    ui.invoke_select(1);
    for name in ["grip-e", "grip-s", "grip-se"] {
        let size = grip_of(&ui, name).size();
        assert_eq!((size.width, size.height), (8.0, 8.0), "{name}");
    }
}

/// Types `text` into the pane field `tabs` Tab presses past element 1's first field.
fn stage_on_element_1(ui: &InsProbe, canvas: &Wired, tabs: usize, text: &str) {
    let w = ui.window();
    click(ui, &rows_of(ui)[0]);
    press(w, Key::F6, false, false);
    for _ in 0..tabs {
        press(w, Key::Tab, false, false);
    }
    type_text(w, text);
    assert!(canvas.borrow().doc().staged().is_some(), "{text} is staged");
}

#[test]
fn a_move_pressed_on_the_frame_a_typed_feed_takes_away_still_moves_from_where_it_lands() {
    let (ui, canvas) = probe(reference_label());
    stage_on_element_1(&ui, &canvas, 1, "100");
    // On the frame as it stands, at feed 8; the typed feed puts it far from here.
    let w = ui.window();
    down(w, at(&ui, 160.0, 80.0));
    assert_eq!(frame_1(&canvas).feed, 100, "the typed feed lands first");
    assert!(
        ui.get_pointer_down(),
        "and the press still holds the element"
    );
    assert!(canvas.borrow().doc().pressed());
    move_to(w, at(&ui, 170.0, 80.0));
    move_to(w, at(&ui, 180.0, 80.0));
    up(w, at(&ui, 180.0, 80.0));
    assert_eq!(
        frame_1(&canvas).feed,
        105,
        "five dots of drag are five dots on from the typed feed"
    );
    assert!(!ui.get_pointer_down());
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas).feed, 100);
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas).feed, 8);
}

#[test]
fn a_move_pressed_on_the_part_of_a_frame_a_typed_length_takes_away_still_moves() {
    let (ui, canvas) = probe(reference_label());
    stage_on_element_1(&ui, &canvas, 3, "10");
    // Thirty dots into a frame forty long, which the typed length makes ten.
    let w = ui.window();
    down(w, at(&ui, 160.0, 80.0));
    assert_eq!(
        frame_1(&canvas).feed_len,
        10,
        "the typed length lands first"
    );
    assert!(
        ui.get_pointer_down(),
        "and the press still holds the element"
    );
    assert!(canvas.borrow().doc().pressed());
    move_to(w, at(&ui, 170.0, 80.0));
    move_to(w, at(&ui, 180.0, 80.0));
    up(w, at(&ui, 180.0, 80.0));
    assert_eq!(frame_1(&canvas), frame(13, 8, 10, 24));
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas), frame(8, 8, 10, 24));
    press(w, "z", false, true);
    assert_eq!(frame_1(&canvas), frame(8, 8, 40, 24));
}
