//! Pan against drag, and presses that change nothing about the view.
//!
//! A press on an element always moves that element: the rule is decided at the
//! press by what is under the pointer, with no threshold, timer or direction in it,
//! so the table below deliberately covers every threshold an interactive Flickable
//! would have used (i-slint-core flickable.rs:366-370) and every one must land
//! exactly. And nothing a press does may change the view's geometry under the
//! pointer, because the toolkit cancels a press whose clipping ancestor stops
//! containing it inside the dispatch that pressed (input.rs:1410-1441).

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::ElementHandle;
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins;
use relabel_gui::view::{self, Opening, Refusal, STEPS};
use relabel_render::document::Rot;
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use support::{
    canvas_of, down, drag_timed, every_kind_label, frame, frame_of, move_to, press, rect,
    reference_label, rows_of, type_text, up, wheel,
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

fn centre(h: &ElementHandle) -> LogicalPosition {
    let (at, size) = (h.absolute_position(), h.size());
    LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
}

fn feed(canvas: &Wired, id: u32) -> i32 {
    frame_of(canvas.borrow().doc().label(), id).feed
}

fn head(canvas: &Wired, id: u32) -> i32 {
    frame_of(canvas.borrow().doc().label(), id).head
}

fn selected(canvas: &Wired) -> Option<u32> {
    canvas.borrow().doc().selected()
}

/// A point on the canvas, given in device pixels from the dot grid's origin.
fn on_grid(ui: &InsProbe, x: f32, y: f32) -> LogicalPosition {
    let grid = canvas_of(ui).absolute_position();
    let s = ui.window().scale_factor();
    LogicalPosition::new(grid.x + x / s, grid.y + y / s)
}

/// The probe opened at Fit, as the application opens, with the change handlers the
/// first layout queued already run.
fn opened_at_fit(label: relabel_render::Label, scale: f32) -> (InsProbe, Wired) {
    let (ui, canvas) = probe(label, scale, Opening::Fit);
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    (ui, canvas)
}

fn view_size(ui: &InsProbe) -> (i32, i32) {
    (ui.get_view_px_w(), ui.get_view_px_h())
}

/// The dot a pointer at logical `x` is on, as `canvas.slint`'s `dot-of` reads it:
/// device pixels from the dot grid's origin, plus a sixteenth, over the zoom and
/// floored.
fn dot_at(ui: &InsProbe, x: f32, zoom: u32) -> i32 {
    let grid = canvas_of(ui).absolute_position().x;
    (((x - grid) * ui.window().scale_factor() + 0.0625) / zoom as f32).floor() as i32
}

fn four_lines() -> String {
    "the feed field was refused because a placement has to leave at least one dot of \
     the element on the label, and this sentence goes on to say so again and again. "
        .repeat(5)
        .trim_end()
        .to_string()
}

#[test]
fn a_drag_that_starts_on_an_element_moves_it_whatever_the_hold_the_speed_or_the_distance() {
    const PAN: i32 = 100;
    let mut cases = 0;
    for scale in [1.0, 1.25] {
        let (ui, canvas) = probe(reference_label(), scale, Opening::At(DEFAULT_ZOOM));
        for hold in [0, 50, 99, 101, 150, 499, 501, 600] {
            for travel in [2.0, 7.0, 8.0, 8.25, 9.0, 40.0, -40.0] {
                for (step, every) in [(5.0, 16), (1.0, 16)] {
                    if feed(&canvas, 1) != 8 {
                        ui.invoke_set_say(1, "feed 8, head 8".into());
                    }
                    ui.invoke_pan_to(PAN, 0);
                    assert_eq!((ui.get_pan_x(), feed(&canvas, 1)), (PAN, 8));
                    // Element 1 spans device pixels 32 to 192, so at a pan of 100 it
                    // straddles the view's left edge; the press is 3 px inside it.
                    let row = rows_of(&ui)[0].clone();
                    let from = LogicalPosition::new(
                        port_of(&ui).absolute_position().x + 3.0,
                        centre(&row).y,
                    );
                    let moved = dot_at(&ui, from.x + travel, DEFAULT_ZOOM)
                        - dot_at(&ui, from.x, DEFAULT_ZOOM);
                    drag_timed(ui.window(), from, (travel, 0.0), step, every, hold);
                    let case = format!(
                        "{scale}x, held {hold} ms, {travel} px at {step} px every {every} ms"
                    );
                    assert_eq!(feed(&canvas, 1), 8 + moved, "{case}");
                    assert_eq!(ui.get_pan_x(), PAN, "{case}: the view moved");
                    assert!(!ui.get_pointer_down(), "{case}");
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 224);
}

/// The defect present since the inspector, pinned. With nothing selected, the press
/// selects and the pane appears; a pane that changed width inside that press
/// narrowed the view's clip past the pointer and the drag rolled back. Element 3,
/// because it is the reference label's one element whose frame can reach past the
/// view's right edge at zoom 4.
#[test]
fn a_press_that_selects_an_element_near_the_right_edge_of_the_view_still_drags_it() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, canvas) = probe(reference_label(), scale, Opening::At(DEFAULT_ZOOM));
        let w = ui.window();
        let (vw, _) = view_size(&ui);
        ui.invoke_pan_to(1100 - vw, 0);
        assert_eq!(ui.get_pan_x(), 1100 - vw);
        assert_eq!(canvas.borrow().doc().selected(), None);
        let before = view_size(&ui);
        // Twelve device pixels in from the view's right edge, at element 3's middle.
        let from = in_view(&ui, (vw - 12) as f32, 208.0);
        let expected = dot_at(&ui, from.x + 40.0, DEFAULT_ZOOM) - dot_at(&ui, from.x, DEFAULT_ZOOM);
        down(w, from);
        assert_eq!(canvas.borrow().doc().selected(), Some(3), "at {scale}x");
        assert_eq!(view_size(&ui), before, "at {scale}x, during the press");
        for i in 1..=8 {
            move_to(w, LogicalPosition::new(from.x + 5.0 * i as f32, from.y));
        }
        assert_eq!(view_size(&ui), before, "at {scale}x, during the drag");
        up(w, LogicalPosition::new(from.x + 40.0, from.y));
        assert_eq!(feed(&canvas, 3), 240 + expected, "at {scale}x");
        if scale == 1.0 {
            assert_eq!(expected, 10, "forty device pixels at zoom 4");
        }
        assert_eq!(view_size(&ui), before, "at {scale}x, after");
    }
}

#[test]
fn a_drag_that_starts_at_the_bottom_edge_of_the_view_lands_while_the_press_writes_a_refusal_note() {
    let mut label = reference_label();
    label.elements[2] = rect(3, frame(240, 64, 40, 24), Rot::D0, false);
    let (ui, canvas) = probe(label, 1.0, Opening::At(6));
    let w = ui.window();
    // A number the feed field refuses, staged on element 1.
    press(w, Key::Tab, false, false);
    press(w, Key::F6, false, false);
    press(w, Key::Tab, false, false);
    type_text(w, "twelve");
    // Element 3 at 6x spans 1440 to 1680 across and 384 to 528 down: its frame's
    // bottom on the view's bottom and its end on the view's right.
    let (vw, vh) = view_size(&ui);
    ui.invoke_pan_to(1680 - vw, 528 - vh);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (1680 - vw, 528 - vh));
    let before = view_size(&ui);
    // Six device pixels above the view's bottom edge, well inside one line of text.
    let from = in_view(&ui, (vw - 120) as f32, (vh - 6) as f32);
    down(w, from);
    assert!(
        ui.get_status().contains("\"twelve\""),
        "the press committed the staged feed and it was refused: {}",
        ui.get_status()
    );
    assert_eq!(view_size(&ui), before);
    for i in 1..=3 {
        move_to(w, LogicalPosition::new(from.x + 4.0 * i as f32, from.y));
    }
    up(w, LogicalPosition::new(from.x + 12.0, from.y));
    assert_eq!(
        feed(&canvas, 3),
        242,
        "twelve device pixels at 6x is two dots"
    );
    assert_eq!(feed(&canvas, 1), 8);
}

#[test]
fn the_view_is_the_same_size_whatever_is_selected_and_whatever_the_status_line_says() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, _canvas) = probe(every_kind_label(), scale, Opening::At(DEFAULT_ZOOM));
        let size = view_size(&ui);
        for id in [-1, 1, 2, 3, 4, 5, 6, 7] {
            ui.invoke_select(id);
            for said in [String::new(), "one".to_string(), four_lines()] {
                ui.set_status(said.as_str().into());
                assert_eq!(
                    view_size(&ui),
                    size,
                    "at {scale}x with element {id} selected and {} characters of status",
                    said.len()
                );
            }
        }
    }
}

#[test]
fn clicking_an_element_that_straddles_the_edge_of_the_view_never_pans() {
    let (ui, canvas) = probe(reference_label(), 1.25, Opening::At(DEFAULT_ZOOM));
    ui.invoke_pan_to(100, 0);
    let grid = canvas_of(&ui).absolute_position();
    let row = rows_of(&ui)[0].clone();
    let at = LogicalPosition::new(port_of(&ui).absolute_position().x + 3.0, centre(&row).y);
    down(ui.window(), at);
    assert_eq!(canvas.borrow().doc().selected(), Some(1));
    assert_eq!(canvas_of(&ui).absolute_position(), grid, "during the press");
    assert_eq!(ui.get_pan_x(), 100);
    up(ui.window(), at);
    assert_eq!(
        canvas_of(&ui).absolute_position(),
        grid,
        "after the release"
    );
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (100, 0));
}

#[test]
fn a_drag_on_bare_canvas_never_pans() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    ui.invoke_pan_to(100, 0);
    let before = canvas.borrow().doc().shown();
    // Head 75 is below every element and still on the 96-dot label.
    let from = in_view(&ui, 300.0, 300.0);
    for by in [100.0, -100.0] {
        drag_timed(ui.window(), from, (by, 40.0), 5.0, 16, 0);
        assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (100, 0), "a drag of {by}");
        assert_eq!(canvas.borrow().doc().shown(), before);
    }
}

#[test]
fn a_drag_that_takes_an_element_wholly_out_of_view_and_back_follows_the_pointer_exactly() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, canvas) = probe(reference_label(), scale, Opening::At(DEFAULT_ZOOM));
        let w = ui.window();
        let from = centre(&rows_of(&ui)[0]);
        let at = |dots: i32| {
            LogicalPosition::new(from.x + (dots * DEFAULT_ZOOM as i32) as f32 / scale, from.y)
        };
        down(w, from);
        // Three hundred dots puts the frame past the view's right edge at every scale
        // here: 1232 device pixels against a view of at most 1064.
        for dots in [50, 100, 150, 200, 300] {
            move_to(w, at(dots));
            assert_eq!(feed(&canvas, 1), 8 + dots, "at {scale}x, {dots} dots out");
        }
        let node = rows_of(&ui)[0].size();
        assert_eq!(
            (node.width, node.height),
            (0.0, 0.0),
            "at {scale}x the frame is wholly out of view"
        );
        for dots in [200, 150, 100, 50, 7, 0] {
            move_to(w, at(dots));
            assert_eq!(feed(&canvas, 1), 8 + dots, "at {scale}x, back to {dots}");
        }
        up(w, at(0));
        assert_eq!(feed(&canvas, 1), 8, "at {scale}x");
        assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0), "at {scale}x");
    }
}

#[test]
fn an_element_dropped_out_of_view_stays_where_it_was_dropped_and_the_view_stays_put() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    let from = centre(&rows_of(&ui)[0]);
    let to = LogicalPosition::new(from.x + 1200.0, from.y);
    down(w, from);
    move_to(w, to);
    up(w, to);
    assert_eq!(feed(&canvas, 1), 308);
    assert_eq!(
        (ui.get_pan_x(), ui.get_pan_y()),
        (0, 0),
        "the view did not follow"
    );
    let rows = rows_of(&ui);
    assert_eq!(rows.len(), 3, "still in the tree");
    assert_eq!((rows[0].size().width, rows[0].size().height), (0.0, 0.0));
    // The next keyboard selection brings it into view.
    ui.invoke_focus_canvas();
    press(w, Key::Tab, false, false);
    assert_eq!(canvas.borrow().doc().selected(), Some(1));
    assert_eq!(ui.get_pan_x(), 348 * 4 - ui.get_view_px_w());
}

#[test]
fn a_wheel_during_a_drag_neither_pans_nor_disturbs_the_drag() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(8));
    let w = ui.window();
    let from = centre(&rows_of(&ui)[0]);
    let expected = dot_at(&ui, from.x + 40.0, 8) - dot_at(&ui, from.x, 8);
    down(w, from);
    let mid = LogicalPosition::new(from.x + 20.0, from.y);
    move_to(w, mid);
    wheel(w, mid, -120.0, -120.0, false, false);
    assert_eq!(
        (ui.get_pan_x(), ui.get_pan_y()),
        (0, 0),
        "straight after the wheel"
    );
    let to = LogicalPosition::new(from.x + 40.0, from.y);
    move_to(w, to);
    up(w, to);
    assert_eq!(feed(&canvas, 1), 8 + expected);
    assert_eq!(expected, 5);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0));
}

/// At 4x, where the view has no room down and a straight wheel would otherwise be
/// turned across: the pressed element answers GrabMouse and keeps the wheel from the
/// view (i-slint-core items/input_items.rs:193-199). The refusal of a held pointer
/// before the turn in `pan-by` itself is pinned by
/// `nothing_about_the_view_changes_while_an_element_is_held`.
#[test]
fn a_straight_wheel_during_a_drag_over_a_label_with_no_room_down_neither_pans_nor_disturbs_the_drag()
 {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    let from = centre(&rows_of(&ui)[0]);
    let expected = dot_at(&ui, from.x + 40.0, DEFAULT_ZOOM) - dot_at(&ui, from.x, DEFAULT_ZOOM);
    down(w, from);
    let mid = LogicalPosition::new(from.x + 20.0, from.y);
    move_to(w, mid);
    wheel(w, mid, 0.0, -120.0, false, false);
    assert_eq!(
        (ui.get_pan_x(), ui.get_pan_y()),
        (0, 0),
        "straight after the wheel"
    );
    let to = LogicalPosition::new(from.x + 40.0, from.y);
    move_to(w, to);
    up(w, to);
    assert_eq!(feed(&canvas, 1), 8 + expected);
    assert_eq!(expected, 10);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0));
}

#[test]
fn nothing_about_the_view_changes_while_an_element_is_held() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    let from = centre(&rows_of(&ui)[0]);
    down(w, from);
    assert!(ui.get_pointer_down());
    for key in ["=", "0", "1", "-"] {
        press(w, key, false, true);
        assert_eq!(canvas.borrow().zoom(), DEFAULT_ZOOM, "Ctrl+{key}");
        assert_eq!(ui.get_status(), Refusal::Held.to_string(), "Ctrl+{key}");
    }
    ui.invoke_pan_by(300.0, 0.0);
    ui.invoke_pan_by(0.0, 300.0);
    wheel(w, from, 0.0, -60.0, false, false);
    ui.invoke_pan_to(300, 0);
    ui.invoke_zoom_wheel(60.0, 10.0, 10.0);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0));
    assert_eq!(canvas.borrow().zoom(), DEFAULT_ZOOM);
    up(w, from);
    assert!(!ui.get_pointer_down());
}

#[test]
fn a_zoom_key_after_an_undo_during_a_drag_is_still_refused() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    let from = centre(&rows_of(&ui)[0]);
    down(w, from);
    move_to(w, LogicalPosition::new(from.x + 20.0, from.y));
    assert_eq!(feed(&canvas, 1), 13);
    press(w, "z", false, true);
    press(w, "=", false, true);
    assert!(ui.get_pointer_down(), "the pointer is still down");
    assert_eq!(canvas.borrow().zoom(), DEFAULT_ZOOM);
    assert_eq!(ui.get_status(), Refusal::Held.to_string());
    up(w, LogicalPosition::new(from.x + 20.0, from.y));
}

#[test]
fn the_dot_grid_lands_on_whole_device_pixels_at_every_pan() {
    for scale in [1.25, 1.5] {
        let (ui, _canvas) = probe(reference_label(), scale, Opening::At(DEFAULT_ZOOM));
        for pan in 0..=8 {
            ui.invoke_pan_to(pan, 0);
            assert_eq!(ui.get_pan_x(), pan);
            let grid = canvas_of(&ui).absolute_position();
            let row = rows_of(&ui)[0].absolute_position();
            for (what, logical) in [("grid x", grid.x), ("grid y", grid.y), ("frame x", row.x)] {
                let px = logical * scale;
                assert!(
                    (px - px.round()).abs() < 1.0e-3,
                    "at {scale}x and pan {pan} the {what} is {px} device pixels"
                );
            }
        }
    }
}

/// A rectangle, and one dot below it a one-dot rule drawn after it, as an underline
/// under a line of text is.
fn underlined_label() -> relabel_render::Label {
    let mut label = reference_label();
    label.elements = vec![
        rect(1, frame(8, 8, 40, 24), Rot::D0, false),
        rect(2, frame(8, 33, 40, 1), Rot::D0, false),
    ];
    label
}

#[test]
fn a_click_inside_an_elements_frame_selects_it_and_not_a_thin_neighbour_at_every_zoom() {
    let mut wrong = Vec::new();
    for zoom in STEPS {
        let (ui, canvas) = probe(underlined_label(), 1.0, Opening::At(zoom));
        assert_eq!(canvas.borrow().zoom(), zoom);
        let z = zoom as f32;
        // The middle of dot (28, 31): inside element 1, whose last row is dot 31, and
        // outside element 2, whose only row is dot 33.
        let at = on_grid(&ui, 28.0 * z + z / 2.0, 31.0 * z + z / 2.0);
        down(ui.window(), at);
        up(ui.window(), at);
        if selected(&canvas) != Some(1) {
            wrong.push((zoom, selected(&canvas)));
        }
    }
    assert!(
        wrong.is_empty(),
        "a click on dot 31 of element 1's frame, one dot clear of element 2, selected \
         (zoom, element): {wrong:?}"
    );
}

/// At 16x a dot is sixteen device pixels, wide enough to see which dot of the
/// element the pointer took. While the pointer stays inside that dot, the element
/// should stay where it is; once it enters the next dot, the element should be one
/// dot on.
#[test]
fn at_sixteen_pixels_to_the_dot_the_dot_the_pointer_took_stays_under_the_pointer() {
    let mut wrong = Vec::new();
    for scale in [1.0, 1.25] {
        let (ui, canvas) = probe(reference_label(), scale, Opening::At(16));
        let w = ui.window();
        // Element 1 spans dots 8 to 47 across and 8 to 31 down. Dot 10, two device
        // pixels in from its left side, and dot 20 down.
        let y = 20.0 * 16.0 + 8.0;
        let x0 = 10.0 * 16.0 + 2.0;
        down(w, on_grid(&ui, x0, y));
        assert_eq!(selected(&canvas), Some(1));
        // Nine and thirteen pixels on: still dot 10. Seventeen and twenty-nine on: dot
        // 11. Thirty-three on: dot 12.
        for (px, dot) in [(9.0, 10), (13.0, 10), (17.0, 11), (29.0, 11), (33.0, 12)] {
            move_to(w, on_grid(&ui, x0 + px, y));
            let (got, want) = (feed(&canvas, 1), 8 + dot - 10);
            if got != want {
                wrong.push((scale, px, got, want));
            }
        }
        up(w, on_grid(&ui, x0 + 33.0, y));
    }
    assert!(
        wrong.is_empty(),
        "(scale, device px moved, feed, feed that keeps the taken dot under the pointer): \
         {wrong:?}"
    );
}

/// Slice 12 keeps one dot of a dragged element on the label "so that there is something
/// left to grab it by" (canvas_drag.rs). At 12c's fixed 4x that dot was four device
/// pixels. A 100 mm label now opens at Fit, which is 1:1, and the view's origin is the
/// label's, so the one dot is the view's first column of device pixels.
#[test]
fn an_element_dragged_off_the_start_of_a_label_opened_at_one_to_one_can_be_taken_hold_of_again() {
    let mut label = relabel_render::Label::new(800, 96, relabel_render::target::Dpmm::DPI_203);
    label.elements = vec![
        rect(1, frame(100, 8, 40, 24), Rot::D0, false),
        rect(2, frame(200, 40, 40, 24), Rot::D0, false),
    ];
    let (ui, canvas) = opened_at_fit(label, 1.0);
    assert_eq!(canvas.borrow().zoom(), 1, "a 100 mm label opens at 1:1");
    let w = ui.window();

    // Off the start along the feed, as far as slice 12 lets it go.
    let from = centre(&rows_of(&ui)[0]);
    down(w, from);
    move_to(w, LogicalPosition::new(from.x - 400.0, from.y));
    up(w, LogicalPosition::new(from.x - 400.0, from.y));
    assert_eq!(feed(&canvas, 1), -39, "one dot left on the label");
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0));
    // The one dot, in the middle of its height, and back by ten dots.
    let at = in_view(&ui, 0.5, 20.0);
    down(w, at);
    move_to(w, LogicalPosition::new(at.x + 10.0, at.y));
    up(w, LogicalPosition::new(at.x + 10.0, at.y));
    let along = feed(&canvas, 1);

    // Off the start across the head, the same way.
    let from = centre(&rows_of(&ui)[1]);
    down(w, from);
    move_to(w, LogicalPosition::new(from.x, from.y - 400.0));
    up(w, LogicalPosition::new(from.x, from.y - 400.0));
    assert_eq!(head(&canvas, 2), -23, "one dot left on the label");
    let at = in_view(&ui, 220.0, 0.5);
    down(w, at);
    move_to(w, LogicalPosition::new(at.x, at.y + 10.0));
    up(w, LogicalPosition::new(at.x, at.y + 10.0));
    let across = head(&canvas, 2);

    assert_eq!(
        (along, across),
        (-29, -13),
        "a press on the one dot left on the label took hold of nothing (feed of element 1, \
         head of element 2)"
    );
}
