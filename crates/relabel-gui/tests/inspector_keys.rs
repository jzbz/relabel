//! The inspector by keyboard alone: no pointer event anywhere in this file.
//!
//! The fields are the primary keyboard path and drag the accelerator, so every
//! route into, through and out of the pane is walked here with keys: Tab from the
//! canvas, F6 between the panes, Up and Down in a number field, and Ctrl+Z from
//! inside a field.

#[path = "support/mod.rs"]
mod support;

use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins::{self, Field};
use relabel_render::document::{ElementId, Rot};
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;
use std::rc::Rc;
use support::{field_of, frame, frame_of, press, rect, reference_label, status_of, type_text};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

/// The window as `run_app` opens it, keyboard in the canvas - or, with
/// `focused` false, as it would open without that call.
fn probe_at(label: relabel_render::Label, focused: bool) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    if focused {
        ui.invoke_focus_canvas();
    }
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

fn probe() -> (InsProbe, Wired) {
    probe_at(near_label(), true)
}

fn near_label() -> relabel_render::Label {
    let mut label = reference_label();
    label.elements = vec![
        rect(1, frame(4, 8, 24, 24), Rot::D0, false),
        rect(2, frame(40, 8, 24, 24), Rot::D0, false),
    ];
    label
}

fn key(ui: &InsProbe, k: Key) {
    press(ui.window(), k, false, false);
}

fn back_tab(ui: &InsProbe) {
    press(ui.window(), Key::Tab, true, false);
}

fn ctrl_z(ui: &InsProbe) {
    let w = ui.window();
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    w.dispatch_event(WindowEvent::KeyPressed { text: "z".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: "z".into() });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}

fn staged(canvas: &Wired) -> Option<(ElementId, Field, String)> {
    canvas
        .borrow()
        .doc()
        .staged()
        .map(|(id, f, t)| (id, f, t.to_string()))
}

fn value(ui: &InsProbe, label: &str) -> String {
    field_of(ui, label)
        .accessible_value()
        .map(|v| v.to_string())
        .unwrap_or_default()
}

#[test]
fn tabbing_from_the_last_element_on_the_canvas_reaches_the_first_field() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    assert_eq!(
        canvas.borrow().doc().selected(),
        Some(2),
        "the last element"
    );
    key(&ui, Key::Tab);
    type_text(ui.window(), "Q");
    assert_eq!(staged(&canvas), Some((2, Field::Name, "Q".to_string())));
}

#[test]
fn f6_moves_the_keyboard_into_the_pane_and_back_to_the_canvas() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    type_text(ui.window(), "Z");
    assert_eq!(
        staged(&canvas),
        Some((1, Field::Name, "Z".to_string())),
        "the name field had the keyboard, with its contents selected"
    );
    key(&ui, Key::F6);
    assert_eq!(staged(&canvas), None, "leaving the field committed it");
    assert_eq!(value(&ui, "name"), "Z");
    type_text(ui.window(), "W");
    assert_eq!(
        staged(&canvas),
        None,
        "the keyboard is no longer in a field"
    );
    assert_eq!(value(&ui, "name"), "Z");
}

#[test]
fn f6_back_to_the_canvas_leaves_the_selection_alone() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::F6);
    assert_eq!(canvas.borrow().doc().selected(), Some(2));
    assert!(
        ui.get_ins_shown(),
        "the pane the operator left is still there"
    );
}

/// The capture that sees F6 is offered along the focused item's ancestors, and
/// at startup nothing is focused - so the window focuses the canvas as it opens,
/// and without that the same key does nothing at all.
#[test]
fn f6_does_something_before_anything_has_been_clicked() {
    let (ui, canvas) = probe_at(near_label(), true);
    ui.invoke_select(1);
    key(&ui, Key::F6);
    type_text(ui.window(), "Q");
    assert_eq!(staged(&canvas), Some((1, Field::Name, "Q".to_string())));

    let (ui, canvas) = probe_at(near_label(), false);
    ui.invoke_select(1);
    key(&ui, Key::F6);
    type_text(ui.window(), "Q");
    assert_eq!(
        staged(&canvas),
        None,
        "with nothing focused, F6 reached nothing"
    );
}

#[test]
fn neither_the_pane_cycling_scope_nor_the_stepping_scope_costs_a_tab_stop() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    ui.invoke_select(1);
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    // Now in the name field of element 2, and every Tab from here is one field.
    let walk = [
        (Field::Name, "b"),
        (Field::Feed, "40"),
        (Field::Head, "8"),
        (Field::Rot, "0"),
        (Field::Ink, "black"),
        (Field::StrokeDots, "0"),
    ];
    for (i, (field, typed)) in walk.into_iter().enumerate() {
        if i > 0 {
            key(&ui, Key::Tab);
        }
        type_text(ui.window(), typed);
        assert_eq!(
            staged(&canvas),
            Some((2, field, typed.to_string())),
            "stop {i}"
        );
    }
}

#[test]
fn a_number_field_selects_its_contents_however_the_keyboard_arrived() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    // By Tab, forwards.
    key(&ui, Key::Tab);
    type_text(ui.window(), "3");
    assert_eq!(staged(&canvas), Some((1, Field::Feed, "3".to_string())));
    // By Tab, backwards, into a field whose number has just changed.
    key(&ui, Key::Tab);
    back_tab(&ui);
    type_text(ui.window(), "7");
    assert_eq!(staged(&canvas), Some((1, Field::Feed, "7".to_string())));
    key(&ui, Key::Return);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 7);
}

#[test]
fn the_up_and_down_arrows_step_a_number_field_by_its_own_unit() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    key(&ui, Key::UpArrow);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 5);
    assert_eq!(value(&ui, "feed"), "5");
    key(&ui, Key::DownArrow);
    key(&ui, Key::DownArrow);
    assert_eq!(value(&ui, "feed"), "3");
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    key(&ui, Key::UpArrow);
    assert_eq!(value(&ui, "rotation"), "90", "a quarter turn");
}

#[test]
fn a_key_the_field_wants_still_reaches_it_through_the_stepping_scope() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "12");
    key(&ui, Key::Home);
    type_text(ui.window(), "-");
    key(&ui, Key::End);
    key(&ui, Key::Backspace);
    assert_eq!(staged(&canvas), Some((1, Field::Feed, "-1".to_string())));
    key(&ui, Key::Return);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, -1);
}

/// Every push after an edit writes rows one at a time and resets no model, so the
/// element's own focus scope is still there to take the arrows when the keyboard
/// comes back from the pane.
#[test]
fn the_arrow_keys_still_nudge_after_the_keyboard_has_been_through_the_inspector() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    type_text(ui.window(), "renamed");
    back_tab(&ui);
    assert_eq!(value(&ui, "name"), "renamed");
    key(&ui, Key::RightArrow);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 2).feed, 41);
    key(&ui, Key::RightArrow);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 2).feed, 42);
}

#[test]
fn an_element_is_placed_by_keyboard_alone_from_the_canvas_to_the_field_and_back() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "30");
    key(&ui, Key::Tab);
    type_text(ui.window(), "20");
    key(&ui, Key::F6);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(30, 20, 24, 24)
    );
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 31);
}

#[test]
fn ctrl_z_undoes_while_the_keyboard_is_in_the_inspector() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "30");
    key(&ui, Key::Return);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 30);
    ctrl_z(&ui);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 4);
    assert_eq!(value(&ui, "feed"), "4");
}

fn selected(canvas: &Wired) -> Option<ElementId> {
    canvas.borrow().doc().selected()
}

fn status(ui: &InsProbe) -> String {
    status_of(ui)
        .accessible_value()
        .map(|v| v.to_string())
        .unwrap_or_default()
}

/// Ctrl and one key, the way a window delivers the chord: with Shift held the
/// key's text is the shifted character, because winit's logical key keeps every
/// modifier but Ctrl.
fn ctrl_chord(ui: &InsProbe, shift: bool, text: &str) {
    let w = ui.window();
    let held: &[Key] = if shift {
        &[Key::Control, Key::Shift]
    } else {
        &[Key::Control]
    };
    for k in held {
        w.dispatch_event(WindowEvent::KeyPressed { text: (*k).into() });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    for k in held.iter().rev() {
        w.dispatch_event(WindowEvent::KeyReleased { text: (*k).into() });
    }
}

/// F6 decides from where the keyboard is rather than from a toggle only F6 writes.
/// Shift+Tab out of the first field is a way out of the pane that F6 never saw,
/// and the next F6 from the canvas still has to reach the pane at the first press.
#[test]
fn f6_from_the_canvas_reaches_the_pane_after_shift_tab_took_the_keyboard_out_of_it() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    back_tab(&ui);
    assert_eq!(
        selected(&canvas),
        Some(2),
        "Shift+Tab left the pane for the last element on the canvas"
    );
    key(&ui, Key::F6);
    type_text(ui.window(), "Q");
    assert_eq!(
        staged(&canvas),
        Some((2, Field::Name, "Q".to_string())),
        "F6 from the canvas should put the keyboard in the name field"
    );
}

/// The other way round: a field reached by Tab was never entered through F6, and
/// F6 from inside it still has to leave the pane and finish the typed name.
#[test]
fn f6_from_a_field_reached_by_tab_returns_the_keyboard_to_the_canvas_and_commits() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    type_text(ui.window(), "Q");
    assert_eq!(staged(&canvas), Some((2, Field::Name, "Q".to_string())));
    key(&ui, Key::F6);
    assert_eq!(staged(&canvas), None, "leaving the pane committed the name");
    type_text(ui.window(), "W");
    assert_eq!(
        staged(&canvas),
        None,
        "and the keyboard is on the canvas, not in a field"
    );
    assert_eq!(value(&ui, "name"), "Q");
}

/// F6 back hands the keyboard to the canvas surface rather than to the selected
/// element's own scope, and the surface's handler binds no arrows. The arrows still
/// have to move the element the pane was editing, because Tab, the only key that
/// reaches an element's scope, starts again from the canvas's first element.
#[test]
fn f6_back_to_the_canvas_leaves_the_arrow_keys_nudging_the_element_being_edited() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    assert_eq!(selected(&canvas), Some(2));
    key(&ui, Key::F6);
    key(&ui, Key::F6);
    assert_eq!(selected(&canvas), Some(2), "F6 back kept the selection");
    key(&ui, Key::RightArrow);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 2).feed,
        41,
        "the arrow should nudge the element the pane was editing"
    );
}

/// The status line is the only channel a refusal has, and a polite live region is
/// announced when its content changes. The same mistake made twice has to be a
/// change the second time too, so typing clears what the last refusal said.
#[test]
fn a_second_identical_refusal_changes_the_live_region_so_it_is_announced_again() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "x");
    key(&ui, Key::Return);
    let first = status(&ui);
    assert!(
        first.contains("feed"),
        "the first refusal was said: {first}"
    );
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 4);

    // Out of the field and back in, so it arrives selected, and the same mistake.
    back_tab(&ui);
    key(&ui, Key::Tab);
    type_text(ui.window(), "x");
    assert_eq!(staged(&canvas), Some((1, Field::Feed, "x".to_string())));
    let before = status(&ui);
    key(&ui, Key::Return);
    let after = status(&ui);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 4);
    assert_ne!(
        before, after,
        "the second refusal left the live region exactly as it was, so nothing announces it"
    );
}

/// A real keyboard delivers Ctrl+Shift+Z with the shifted character, and Slint's
/// own `KeyEvent::shortcut` matches `"z" | "Z"` for Redo for exactly that reason
/// (i-slint-core input.rs:991-996). A capture that knew only "z" would let the key
/// fall through to the LineEdit's own text-level redo.
#[test]
fn ctrl_shift_z_as_a_keyboard_sends_it_redoes_while_the_keyboard_is_in_the_inspector() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "30");
    key(&ui, Key::Return);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 30);
    ctrl_z(&ui);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        4,
        "Ctrl+Z undid the edit"
    );
    ctrl_chord(&ui, true, "Z");
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        30,
        "Ctrl+Shift+Z should redo the edit it just undid"
    );
}

/// Ctrl+Y is Redo on Windows (i-slint-core input.rs:986-987), and from inside a
/// field it would otherwise reach the LineEdit, whose own redo stack is not the
/// document's.
#[test]
fn ctrl_y_redoes_while_the_keyboard_is_in_the_inspector() {
    let (ui, canvas) = probe();
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "30");
    key(&ui, Key::Return);
    ctrl_z(&ui);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 4);
    ctrl_chord(&ui, false, "y");
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        30,
        "redone"
    );
}
