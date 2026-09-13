//! The zoom: the steps, the keys and buttons that walk them, Fit and 1:1, what a
//! zoom keeps in place, and what it must not touch.
//!
//! Every path here is synchronous - a key, a button's action and a click all run
//! to completion inside their dispatch - so nothing below advances mock time.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleLiveness, AccessibleRole, ElementHandle, ElementQuery};
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins::{self, Field};
use relabel_gui::view::{self, Opening, Refusal, STEPS};
use relabel_render::document::{ElementId, Rot};
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;
use std::rc::Rc;
use support::{click, frame, frame_of, press, rect, reference_label, rows_of, type_text};

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

fn zoom_of(canvas: &Wired) -> u32 {
    canvas.borrow().zoom()
}

fn staged(canvas: &Wired) -> Option<(ElementId, Field, String)> {
    canvas
        .borrow()
        .doc()
        .staged()
        .map(|(id, f, t)| (id, f, t.to_string()))
}

fn button_of(ui: &InsProbe, name: &str) -> ElementHandle {
    let wanted = name.to_string();
    ElementQuery::from_root(ui)
        .match_accessible_role(AccessibleRole::Button)
        .match_predicate(move |h| h.accessible_label().is_some_and(|l| l == wanted.as_str()))
        .find_first()
        .unwrap_or_else(|| panic!("no button is called {name:?}"))
}

fn readout_of(ui: &InsProbe) -> ElementHandle {
    ElementQuery::from_root(ui)
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Zoom"))
        .find_first()
        .expect("the zoom readout")
}

/// A label as long as a 100 mm D110 label: 800 dots, too long for the view at 1:1.
fn long_label() -> relabel_render::Label {
    let mut label = relabel_render::Label::new(800, 96, relabel_render::target::Dpmm::DPI_203);
    label.elements = vec![rect(1, frame(8, 8, 40, 24), Rot::D0, false)];
    label
}

#[test]
fn control_equals_and_control_plus_zoom_in_through_the_steps_and_control_minus_and_underscore_zoom_out()
 {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    for (i, want) in [6, 8, 12, 16].into_iter().enumerate() {
        if i % 2 == 0 {
            press(w, "=", false, true);
        } else {
            // Ctrl+Shift+= and the keypad's plus both arrive as "+".
            press(w, "+", true, true);
        }
        assert_eq!((zoom_of(&canvas), ui.get_zoom()), (want, want as i32));
    }
    press(w, "=", false, true);
    assert_eq!(zoom_of(&canvas), 16);
    assert_eq!(ui.get_status(), Refusal::AtTop(16).to_string());
    for (i, want) in [12, 8, 6, 4, 3, 2, 1].into_iter().enumerate() {
        if i % 2 == 0 {
            press(w, "-", false, true);
        } else {
            press(w, "_", true, true);
        }
        assert_eq!((zoom_of(&canvas), ui.get_zoom()), (want, want as i32));
    }
    press(w, "-", false, true);
    assert_eq!(zoom_of(&canvas), 1);
    assert_eq!(ui.get_status(), Refusal::AtOne.to_string());
}

#[test]
fn the_zoom_keys_work_from_the_canvas_and_from_inside_a_field_and_type_nothing() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, "=", false, true);
    assert_eq!(zoom_of(&canvas), 6, "from the canvas surface");
    press(w, Key::Tab, false, false);
    press(w, Key::F6, false, false);
    type_text(w, "Q");
    press(w, "=", false, true);
    type_text(w, "R");
    assert_eq!(zoom_of(&canvas), 8, "from inside the name field");
    assert_eq!(staged(&canvas), Some((1, Field::Name, "QR".to_string())));
}

#[test]
fn control_one_draws_one_device_pixel_to_the_dot_at_every_display_scale() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, canvas) = probe(reference_label(), scale, Opening::At(DEFAULT_ZOOM));
        press(ui.window(), "1", false, true);
        assert_eq!(zoom_of(&canvas), 1);
        assert_eq!(
            (ui.get_plane_px_w(), ui.get_plane_px_h()),
            (320, 96),
            "at {scale}x"
        );
        let row = rows_of(&ui)[0].size();
        assert_eq!((row.width * scale).round() as i32, 40, "at {scale}x");
    }
}

#[test]
fn control_zero_fits_the_whole_label_from_its_origin() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    ui.invoke_pan_to(300, 0);
    assert_eq!(ui.get_pan_x(), 300);
    press(ui.window(), "0", false, true);
    assert_eq!(zoom_of(&canvas), 2);
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0));
    assert!(ui.get_plane_px_w() <= ui.get_view_px_w());
    let shown = canvas.borrow().doc().shown();
    let rows = rows_of(&ui);
    assert_eq!(rows.len(), 3);
    for (row, s) in rows.iter().zip(&shown) {
        assert_eq!(
            (
                row.size().width.round() as i32,
                row.size().height.round() as i32
            ),
            (s.w * 2, s.h * 2),
            "element {} at its full frame",
            s.id
        );
    }
    assert_eq!(
        readout_of(&ui).accessible_value().as_deref(),
        Some("Fit, 2x")
    );
}

#[test]
fn fit_on_a_label_larger_than_the_view_at_one_to_one_says_so() {
    let (ui, canvas) = probe(long_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    press(ui.window(), "0", false, true);
    assert_eq!(zoom_of(&canvas), 1);
    assert_eq!(ui.get_status(), Refusal::StillLarger.to_string());
}

#[test]
fn selecting_any_element_after_a_fit_changes_neither_the_zoom_nor_the_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::Fit);
    let before = (
        zoom_of(&canvas),
        ui.get_view_px_w(),
        ui.get_view_px_h(),
        ui.get_pan_x(),
        ui.get_pan_y(),
    );
    let now = |ui: &InsProbe| {
        (
            zoom_of(&canvas),
            ui.get_view_px_w(),
            ui.get_view_px_h(),
            ui.get_pan_x(),
            ui.get_pan_y(),
        )
    };
    for id in 1..=3 {
        press(ui.window(), Key::Tab, false, false);
        assert_eq!(canvas.borrow().doc().selected(), Some(id));
        assert_eq!(now(&ui), before, "Tab to element {id}");
    }
    for (i, row) in rows_of(&ui).iter().enumerate() {
        click(&ui, row);
        assert_eq!(canvas.borrow().doc().selected(), Some(i as ElementId + 1));
        assert_eq!(now(&ui), before, "a click on element {}", i + 1);
    }
}

#[test]
fn the_dots_and_the_frames_agree_at_every_step_up_to_the_top() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    press(ui.window(), "1", false, true);
    for z in STEPS {
        assert_eq!(zoom_of(&canvas), z);
        // A zoom with nothing selected keeps the view's centre, which leaves element 1
        // out of view and a point; from the origin its whole frame is in view.
        ui.invoke_pan_to(0, 0);
        assert_eq!(ui.get_plane_px_w(), 320 * z as i32, "at {z}x");
        let row = rows_of(&ui)[0].size();
        assert_eq!(row.width.round() as i32, 40 * z as i32, "at {z}x");
        press(ui.window(), "=", false, true);
    }
}

#[test]
fn clicking_zoom_in_zooms_and_leaves_the_keyboard_where_it_was() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    press(w, Key::F6, false, false);
    type_text(w, "Q");
    click(&ui, &button_of(&ui, "Zoom in"));
    assert_eq!(zoom_of(&canvas), 6);
    type_text(w, "R");
    assert_eq!(staged(&canvas), Some((1, Field::Name, "QR".to_string())));
}

#[test]
fn the_zoom_buttons_are_named_buttons_an_assistive_technology_can_press() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    for (name, keys) in [
        ("Zoom to fit", "Control+0"),
        ("Zoom to one device pixel per dot", "Control+1"),
        ("Zoom out", "Control+Minus"),
        ("Zoom in", "Control+Plus"),
    ] {
        assert_eq!(
            button_of(&ui, name).accessible_description().as_deref(),
            Some(keys),
            "{name} says its chord"
        );
    }
    button_of(&ui, "Zoom in").invoke_accessible_default_action();
    assert_eq!(zoom_of(&canvas), 6);
    button_of(&ui, "Zoom out").invoke_accessible_default_action();
    assert_eq!(zoom_of(&canvas), 4);
    button_of(&ui, "Zoom to one device pixel per dot").invoke_accessible_default_action();
    assert_eq!(zoom_of(&canvas), 1);
    button_of(&ui, "Zoom to fit").invoke_accessible_default_action();
    assert_eq!(zoom_of(&canvas), 2);
    // No button is a Tab stop: from the surface, three elements and then the pane.
    ui.invoke_focus_canvas();
    for _ in 0..4 {
        press(ui.window(), Key::Tab, false, false);
    }
    type_text(ui.window(), "X");
    assert_eq!(staged(&canvas), Some((3, Field::Name, "X".to_string())));
}

#[test]
fn the_zoom_readout_is_a_polite_live_region_that_says_the_zoom() {
    let (ui, _canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let readout = readout_of(&ui);
    assert_eq!(readout.accessible_value().as_deref(), Some("4x"));
    assert_eq!(
        readout.accessible_live_region(),
        Some(AccessibleLiveness::Polite)
    );
    press(ui.window(), "0", false, true);
    assert_eq!(
        readout_of(&ui).accessible_value().as_deref(),
        Some("Fit, 2x")
    );
    press(ui.window(), "1", false, true);
    assert_eq!(readout_of(&ui).accessible_value().as_deref(), Some("1x"));
}

#[test]
fn zooming_is_not_an_undo_step_and_does_not_commit_a_typed_field() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(DEFAULT_ZOOM));
    let w = ui.window();
    press(w, Key::Tab, false, false);
    press(w, Key::F6, false, false);
    press(w, Key::Tab, false, false);
    type_text(w, "50");
    assert_eq!(staged(&canvas), Some((1, Field::Feed, "50".to_string())));
    press(w, "=", false, true);
    assert_eq!(zoom_of(&canvas), 6);
    assert_eq!(
        staged(&canvas),
        Some((1, Field::Feed, "50".to_string())),
        "still staged, not committed"
    );
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8);
    assert!(!canvas.borrow().doc().dirty());
    press(w, Key::Return, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 50);
    press(w, "=", false, true);
    assert_eq!(zoom_of(&canvas), 8);
    press(w, "z", false, true);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        8,
        "the one undo step is the typed feed"
    );
    assert_eq!(zoom_of(&canvas), 8, "and the zoom is not undone with it");
}

#[test]
fn zooming_with_nothing_selected_keeps_the_dot_at_the_centre_of_the_view() {
    let (ui, canvas) = probe(reference_label(), 1.0, Opening::At(8));
    ui.invoke_pan_to(600, 100);
    let (px, py) = (f64::from(ui.get_pan_x()), f64::from(ui.get_pan_y()));
    assert_eq!((px, py), (600.0, 100.0));
    let (cx, cy) = (
        f64::from(ui.get_view_px_w()) / 2.0,
        f64::from(ui.get_view_px_h()) / 2.0,
    );
    press(ui.window(), "=", false, true);
    assert_eq!(zoom_of(&canvas), 12);
    let (qx, qy) = (f64::from(ui.get_pan_x()), f64::from(ui.get_pan_y()));
    for (axis, before, after) in [
        ("across", (px + cx) / 8.0, (qx + cx) / 12.0),
        ("down", (py + cy) / 8.0, (qy + cy) / 12.0),
    ] {
        assert!(
            (before - after).abs() <= 0.5 / 12.0 + 1e-9,
            "the dot at the view's centre {axis} was {before} and is {after}"
        );
    }
}
