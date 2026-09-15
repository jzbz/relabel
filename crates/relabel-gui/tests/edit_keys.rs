//! Adding and deleting from the keyboard, and where the keyboard is afterwards.
//!
//! A command that adds or removes an element adds or removes a row, and a row can be
//! what holds the keyboard. The rows are edited by element id, so a row that stays
//! keeps its focus; and when the row that held it goes, the shell hands the keyboard
//! to the canvas surface, because with no live focus item no capture runs and every
//! chord in the window would go dead (i-slint-core window.rs:1076-1082).

#[path = "support/mod.rs"]
mod support;

use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::view::{self, Opening};
use relabel_gui::{edit, ins};
use relabel_render::document::{Element, ElementId, ElementOrRaw, Rot};
use slint::ComponentHandle;
use slint::platform::Key;
use std::cell::RefCell;
use std::rc::Rc;
use support::{
    canvas_of, click, down, frame, frame_of, ids_of, kind_of, press, rect, reference_label,
    rows_of, type_text, up,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

/// The shell wired as `run_app` wires it, at the zoom the canvas tests state their
/// distances at.
fn probe(label: relabel_render::Label) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    edit::wire(&ui, &canvas);
    view::wire(&ui, &canvas, Opening::At(DEFAULT_ZOOM));
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

fn ids(canvas: &Wired) -> Vec<ElementId> {
    ids_of(canvas.borrow().doc().label())
}

fn selected(canvas: &Wired) -> Option<ElementId> {
    canvas.borrow().doc().selected()
}

fn feed_of(canvas: &Wired, id: ElementId) -> i32 {
    frame_of(canvas.borrow().doc().label(), id).feed
}

fn name_of(canvas: &Wired, id: ElementId) -> String {
    canvas
        .borrow()
        .doc()
        .label()
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .map(Element::common)
        .find(|c| c.id == id)
        .expect("the element is in the label")
        .name
        .clone()
}

/// The centre of a row, where a press takes the element.
fn centre_of(h: &i_slint_backend_testing::ElementHandle) -> slint::LogicalPosition {
    let (at, size) = (h.absolute_position(), h.size());
    slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
}

fn indices(ui: &InsProbe) -> Vec<usize> {
    rows_of(ui)
        .iter()
        .filter_map(|r| r.accessible_item_index())
        .collect()
}

#[test]
fn an_arrow_after_an_add_moves_the_new_element() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, "O", true, true);
    assert_eq!(selected(&canvas), Some(4));
    assert_eq!(rows_of(&ui).len(), 4);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 4),
        frame(56, 8, 40, 24)
    );
    press(w, Key::RightArrow, false, false);
    assert_eq!(feed_of(&canvas, 4), 57, "the new element moves");
    assert_eq!(
        feed_of(&canvas, 1),
        8,
        "and the one clicked before it does not"
    );
}

#[test]
fn an_add_chord_works_from_inside_a_field_and_commits_it_first() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, Key::F6, false, false);
    type_text(w, "x");
    press(w, "T", true, true);
    assert_eq!(
        name_of(&canvas, 1),
        "x",
        "what was typed landed where it was typed"
    );
    assert_eq!(kind_of(canvas.borrow().doc().label(), 4), "text");
    assert_eq!(selected(&canvas), Some(4));
    press(w, Key::Delete, false, false);
    assert_eq!(
        ids(&canvas),
        [1, 2, 3],
        "the keyboard left the field for the surface, where Delete deletes"
    );
}

#[test]
fn an_add_while_an_element_is_held_is_refused() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    let at = centre_of(&rows_of(&ui)[0]);
    down(w, at);
    press(w, "O", true, true);
    assert_eq!(ids(&canvas), [1, 2, 3]);
    assert!(
        ui.get_status().contains("let go"),
        "the status says why: {}",
        ui.get_status()
    );
    up(w, at);
}

#[test]
fn delete_removes_the_selection_and_the_keys_still_work() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[1]);
    press(w, Key::Delete, false, false);
    assert_eq!(ids(&canvas), [1, 3]);
    assert_eq!(selected(&canvas), None);
    assert_eq!(indices(&ui), [0, 1]);
    assert_eq!(canvas_of(&ui).accessible_item_count(), Some(2));
    assert!(
        ui.get_status().starts_with("deleted Rectangle"),
        "{}",
        ui.get_status()
    );
    press(w, "z", false, true);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 2),
        frame(120, 8, 40, 24)
    );
    press(w, Key::Tab, false, false);
    press(w, Key::RightArrow, false, false);
    assert_eq!(
        feed_of(&canvas, 1),
        9,
        "Tab and the arrows still reach an element"
    );
}

#[test]
fn delete_with_nothing_selected_says_so_as_the_button_does() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[1]);
    press(w, Key::Delete, false, false);
    assert_eq!(selected(&canvas), None);
    press(w, Key::Delete, false, false);
    assert_eq!(ids(&canvas), [1, 3]);
    assert_eq!(ui.get_status(), "nothing is selected to delete");
    press(w, Key::Backspace, false, false);
    assert!(
        ui.get_status().starts_with("nothing is selected to delete"),
        "{}",
        ui.get_status()
    );
    press(w, Key::Tab, false, false);
    press(w, Key::RightArrow, false, false);
    assert_eq!(
        feed_of(&canvas, 1),
        9,
        "and the keys still reach an element"
    );
}

#[test]
fn backspace_deletes_like_delete() {
    let (ui, canvas) = probe(reference_label());
    click(&ui, &rows_of(&ui)[1]);
    press(ui.window(), Key::Backspace, false, false);
    assert_eq!(ids(&canvas), [1, 3]);
}

#[test]
fn delete_and_backspace_inside_a_field_edit_the_text() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, Key::F6, false, false);
    press(w, Key::Delete, false, false);
    press(w, Key::Backspace, false, false);
    assert_eq!(ids(&canvas), [1, 2, 3]);
    assert_eq!(
        canvas.borrow().doc().staged().map(|(id, _, _)| id),
        Some(1),
        "the keys went to the field"
    );
}

#[test]
fn delete_from_a_locked_elements_read_only_field_deletes_nothing() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, 8, 40, 24), Rot::D0, true);
    let (ui, canvas) = probe(label);
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, Key::F6, false, false);
    press(w, Key::Delete, false, false);
    assert_eq!(ids(&canvas), [1, 2, 3]);
}

#[test]
fn the_focused_row_keeps_the_keyboard_when_a_row_before_it_comes_and_goes() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, Key::Delete, false, false);
    press(w, Key::Tab, false, false);
    assert_eq!(selected(&canvas), Some(2));
    press(w, "z", false, true);
    press(w, Key::RightArrow, false, false);
    assert_eq!(feed_of(&canvas, 2), 121);
    assert_eq!(
        feed_of(&canvas, 1),
        8,
        "the row put back before it took nothing"
    );
    press(w, "Z", true, true);
    press(w, Key::RightArrow, false, false);
    assert_eq!(feed_of(&canvas, 2), 122);
}

#[test]
fn an_undo_that_empties_the_selection_from_inside_a_field_leaves_the_keys_live() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, Key::RightArrow, false, false);
    assert_eq!(feed_of(&canvas, 1), 9);
    press(w, "O", true, true);
    press(w, Key::F6, false, false);
    press(w, "z", false, true);
    assert_eq!(ids(&canvas), [1, 2, 3]);
    assert_eq!(selected(&canvas), None);
    press(w, "z", false, true);
    assert_eq!(
        feed_of(&canvas, 1),
        8,
        "a second Ctrl+Z still reaches the document"
    );
}

#[test]
fn undo_of_an_add_from_its_own_row_leaves_the_keys_live() {
    let (ui, canvas) = probe(reference_label());
    let w = ui.window();
    click(&ui, &rows_of(&ui)[0]);
    press(w, Key::RightArrow, false, false);
    press(w, "O", true, true);
    click(&ui, &rows_of(&ui)[3]);
    assert_eq!(selected(&canvas), Some(4));
    press(w, "z", false, true);
    assert_eq!(selected(&canvas), None);
    press(w, "z", false, true);
    assert_eq!(
        feed_of(&canvas, 1),
        8,
        "a second Ctrl+Z still reaches the document"
    );
}
