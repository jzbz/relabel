//! The write path through the toolkit: a keystroke stages, Return or the focus
//! leaving commits, and after every outcome the document is pushed back into the
//! fields by assignment.
//!
//! Every path here is synchronous - a keystroke, a Return, a focus change by Tab
//! or by click, and an accessibility action all run to completion inside the
//! dispatch that caused them - so nothing below advances mock time. A test that
//! needed to would mean a `changed` handler had crept into the push.

#[path = "support/mod.rs"]
mod support;

use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins::{self, Field};
use relabel_render::document::{ElementId, Rot};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::rc::Rc;
use support::{
    click, editor_of, every_kind_label, field_of, frame, frame_of, press, rect, reference_label,
    rows_of, status_of, type_text,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

fn probe(label: relabel_render::Label) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

/// Two elements close to the label's start, so both are on screen beside the pane
/// and a click reaches either.
fn near_label() -> relabel_render::Label {
    let mut label = reference_label();
    label.elements = vec![
        rect(1, frame(4, 8, 24, 24), Rot::D0, false),
        rect(2, frame(40, 8, 24, 24), Rot::D0, false),
    ];
    label
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

/// Put the pointer in a field, the way an operator reaches it with a mouse.
fn click_into(ui: &InsProbe, label: &str) {
    click(ui, &editor_of(&field_of(ui, label)));
}

#[test]
fn a_field_shows_the_values_of_the_element_that_is_selected() {
    let (ui, _canvas) = probe(reference_label());
    ui.invoke_select(1);
    assert_eq!(
        (value(&ui, "name"), value(&ui, "feed"), value(&ui, "head")),
        ("box 1".into(), "8".into(), "8".into())
    );
    ui.invoke_select(3);
    assert_eq!(
        (value(&ui, "name"), value(&ui, "feed"), value(&ui, "head")),
        ("box 3".into(), "240".into(), "40".into())
    );
}

/// The named hazard. A field whose text had been bound to the pushed value would
/// lose that binding to the operator's first keystroke and never follow the
/// document again.
#[test]
fn a_field_the_operator_has_written_still_takes_the_next_push() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "37");
    assert_eq!(value(&ui, "feed"), "37", "the widget wrote its own text");
    ui.invoke_select(2);
    assert_eq!(
        value(&ui, "feed"),
        "120",
        "and still took the next element's"
    );
    ui.invoke_select(1);
    assert_eq!(value(&ui, "feed"), "37");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 37);
}

/// The document already holds 8, so the model row says "8" before and after; the
/// field saying "08" is set back anyway, because the push is an assignment and not
/// a comparison against the last row.
#[test]
fn the_push_lands_even_when_the_value_is_the_one_the_document_already_had() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "08");
    assert_eq!(value(&ui, "feed"), "08");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(value(&ui, "feed"), "8");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8);
    ui.invoke_undo();
    assert_eq!(
        ui.get_status().as_str(),
        "nothing to undo",
        "no edit happened"
    );
}

/// Every keystroke is pushed straight back into the field it came from. The
/// push hands the field the text it already holds, so the caret stays where the
/// operator left it rather than jumping to one end.
#[test]
fn pushing_the_text_a_field_already_holds_does_not_move_the_caret() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "name");
    press(ui.window(), Key::End, false, false);
    type_text(ui.window(), "XY");
    press(ui.window(), Key::Home, false, false);
    type_text(ui.window(), "AB");
    assert_eq!(
        staged(&canvas),
        Some((1, Field::Name, "ABbox 1XY".to_string()))
    );
    assert_eq!(value(&ui, "name"), "ABbox 1XY");
}

#[test]
fn a_value_written_through_the_accessibility_tree_commits_at_once() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    field_of(&ui, "feed").set_accessible_value("40");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 40);
    assert_eq!(staged(&canvas), None);
    assert_eq!(value(&ui, "feed"), "40");
}

#[test]
fn pressing_return_commits_what_is_in_the_field() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "37");
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        8,
        "staged, not written"
    );
    press(ui.window(), Key::Return, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 37);
    assert_eq!(staged(&canvas), None);
}

#[test]
fn typing_and_then_moving_the_focus_away_commits_once() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "37");
    press(ui.window(), Key::Tab, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 37);
    press(ui.window(), Key::Tab, false, false);
    ui.invoke_undo();
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8);
    ui.invoke_undo();
    assert_eq!(
        ui.get_status().as_str(),
        "nothing to undo",
        "one field visit, one undo step"
    );
}

/// The order the toolkit delivers is the press first - which focuses the second
/// element, selects it and pushes its values - and the first field's blur last.
/// The typed number still lands on the element it was typed into, because the
/// press flushed it before selecting, and the late blur finds nothing to commit.
#[test]
fn typing_into_one_element_and_then_clicking_another_writes_the_first_and_not_the_second() {
    let (ui, canvas) = probe(near_label());
    rows_of(&ui)[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(canvas.borrow().doc().selected(), Some(1));
    click_into(&ui, "feed");
    type_text(ui.window(), "37");
    rows_of(&ui)[1].mock_single_click(PointerEventButton::Left);
    assert_eq!(canvas.borrow().doc().selected(), Some(2));
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 37);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 2).feed, 40);
    assert_eq!(value(&ui, "feed"), "40");
    assert_eq!(staged(&canvas), None);
}

#[test]
fn a_refused_edit_puts_the_documents_own_value_back_in_the_field() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "twelve");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(value(&ui, "feed"), "8");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8);
    assert!(
        ui.get_status().contains("\"twelve\""),
        "{}",
        ui.get_status()
    );
}

#[test]
fn a_clamped_placement_puts_the_clamped_number_in_the_field_and_says_where_it_went() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "9000");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(value(&ui, "feed"), "319");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 319);
    let said = status_of(&ui)
        .accessible_value()
        .expect("a status")
        .to_string();
    assert!(
        said.contains("feed 9000") && said.contains("feed 319"),
        "{said}"
    );
    // Again, onto the value it already holds: nothing changes, and it is still
    // said, and the field still shows the document rather than the 9001.
    click_into(&ui, "feed");
    type_text(ui.window(), "9001");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(value(&ui, "feed"), "319");
    assert!(ui.get_status().contains("feed 9001"), "{}", ui.get_status());
}

#[test]
fn a_number_the_rules_refused_is_not_left_on_screen() {
    let (ui, canvas) = probe(every_kind_label());
    ui.invoke_select(1);
    click_into(&ui, "em size");
    type_text(ui.window(), "0");
    assert_eq!(value(&ui, "em size"), "0");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(value(&ui, "em size"), "12");
    assert_eq!(
        ui.get_status().as_str(),
        "element 1 has a size of 0, which cannot be saved"
    );
    assert!(canvas.borrow().doc().label().to_json().is_ok());
}

#[test]
fn the_increment_action_asks_for_one_step_and_writes_nothing_itself() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    field_of(&ui, "feed").invoke_accessible_increment_action();
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1).feed,
        9,
        "one dot"
    );
    assert_eq!(value(&ui, "feed"), "9");
    field_of(&ui, "rotation").invoke_accessible_increment_action();
    assert_eq!(value(&ui, "rotation"), "90", "a quarter turn");
    // What the field shows is what the document holds, and nothing else: undo
    // the turn and the field follows.
    ui.invoke_undo();
    assert_eq!(value(&ui, "rotation"), "0");
    field_of(&ui, "feed").invoke_accessible_decrement_action();
    assert_eq!(value(&ui, "feed"), "8");
}

#[test]
fn a_read_only_field_refuses_a_write_through_the_accessibility_tree() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, 8, 40, 24), Rot::D0, true);
    let (ui, canvas) = probe(label);
    ui.invoke_select(1);
    let before = canvas.borrow().doc().label().clone();
    field_of(&ui, "feed").set_accessible_value("40");
    field_of(&ui, "name").set_accessible_value("renamed");
    field_of(&ui, "head").invoke_accessible_increment_action();
    assert_eq!(canvas.borrow().doc().label(), &before);
    assert_eq!(value(&ui, "feed"), "8");
    assert_eq!(value(&ui, "name"), "box 1");
}

/// A std SpinBox erases a lone minus sign on the keystroke; a feed coordinate is
/// legitimately negative.
#[test]
fn a_minus_sign_typed_into_a_placement_field_reaches_the_document_as_a_negative_number() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "-");
    assert_eq!(value(&ui, "feed"), "-", "the minus sign was not erased");
    type_text(ui.window(), "20");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, -20);
    assert_eq!(value(&ui, "feed"), "-20");
}

#[test]
fn dragging_an_element_moves_the_numbers_in_the_feed_and_head_fields() {
    let (ui, _canvas) = probe(near_label());
    let row = &rows_of(&ui)[0];
    let (at, size) = (row.absolute_position(), row.size());
    // Four logical pixels to a dot at 1x and zoom 4.
    row.mock_drag(
        slint::LogicalPosition::new(
            at.x + size.width / 2.0 + 40.0,
            at.y + size.height / 2.0 + 16.0,
        ),
        PointerEventButton::Left,
    );
    assert_eq!(
        (value(&ui, "feed"), value(&ui, "head")),
        ("14".into(), "12".into())
    );
}

#[test]
fn an_undo_puts_the_old_value_back_in_the_field() {
    let (ui, _canvas) = probe(reference_label());
    ui.invoke_select(1);
    field_of(&ui, "feed").set_accessible_value("40");
    assert_eq!(value(&ui, "feed"), "40");
    ui.invoke_undo();
    assert_eq!(value(&ui, "feed"), "8");
    ui.invoke_redo();
    assert_eq!(value(&ui, "feed"), "40");
}

#[test]
fn a_name_typed_into_the_pane_reaches_the_sentence_the_canvas_reads() {
    let (ui, _canvas) = probe(near_label());
    ui.invoke_select(1);
    click_into(&ui, "name");
    press(ui.window(), Key::End, false, false);
    type_text(ui.window(), "!");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(
        rows_of(&ui)[0].accessible_value().as_deref(),
        Some("Rectangle \u{2018}box 1!\u{2019}, feed 4, head 8, 24 by 24 dots")
    );
}

/// An edit is `Effect::Row`, so the canvas resets no model and writes only what the
/// edit changed: the edited row, and of a neighbour drawn after it nothing but how far
/// its pointer padding may reach towards the frame that moved. The keyboard is still
/// in the field for the next number.
#[test]
fn an_edit_writes_only_what_it_changed_and_leaves_the_keyboard_where_it_was() {
    let (ui, canvas) = probe(near_label());
    ui.invoke_select(1);
    let other = ui.get_rows().row_data(1).expect("a second row");
    click_into(&ui, "feed");
    type_text(ui.window(), "5");
    press(ui.window(), Key::Return, false, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 5);
    assert_eq!(ui.get_rows().row_data(0).expect("a row").x, 5);
    assert_eq!(
        ui.get_rows().row_data(1),
        Some(ElRow {
            clear_left: other.clear_left - 1,
            ..other
        }),
        "the neighbour one dot nearer the frame that moved, and otherwise untouched"
    );
    type_text(ui.window(), "6");
    assert_eq!(staged(&canvas), Some((1, Field::Feed, "56".to_string())));
}

fn key(ui: &InsProbe, k: Key) {
    press(ui.window(), k, false, false);
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

/// What a real event loop does between two events: run the change handlers.
fn drain() {
    slint::platform::update_timers_and_animations();
}

fn label_of(canvas: &Wired) -> relabel_render::Label {
    canvas.borrow().doc().label().clone()
}

/// How many undo steps the document holds, counted by undoing until it says
/// there is nothing left, and then redoing them all back.
fn undo_depth(ui: &InsProbe) -> usize {
    let mut n = 0;
    loop {
        // A successful undo or redo says nothing, so the status line would still
        // hold the last "nothing to undo" and end the count at once.
        ui.set_status("".into());
        ui.invoke_undo();
        if ui.get_status().as_str() == "nothing to undo" {
            break;
        }
        n += 1;
        assert!(n < 100, "an undo stack that never empties");
    }
    for _ in 0..n {
        ui.invoke_redo();
    }
    n
}

/// An operator types the first digit, looks something up in another window, comes
/// back and types the second. The window's deactivation and reactivation reach the
/// field as a focus loss and a focus arrival, and neither is the operator leaving
/// or entering the field: nothing is committed, nothing is selected.
#[test]
fn switching_to_another_window_and_back_part_way_through_a_number_keeps_the_digits_typed() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "3");
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(false));
    drain();
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    drain();
    type_text(ui.window(), "7");
    assert_eq!(
        value(&ui, "feed"),
        "37",
        "the field the operator came back to"
    );
    key(&ui, Key::Return);
    assert_eq!(frame_of(&label_of(&canvas), 1).feed, 37);
    assert_eq!(undo_depth(&ui), 1, "one field visit, one undo step");
}

/// The same pair of focus events, from the field's own context menu: opening it
/// takes the focus into the popup, closing it gives it back.
#[test]
fn opening_and_closing_a_fields_context_menu_part_way_through_a_number_keeps_the_digits_typed() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "3");
    let ed = editor_of(&field_of(&ui, "feed"));
    let (at, size) = (ed.absolute_position(), ed.size());
    let position = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Right,
    });
    w.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Right,
    });
    drain();
    let committed_on_open = frame_of(&label_of(&canvas), 1).feed;
    key(&ui, Key::Escape);
    drain();
    type_text(ui.window(), "7");
    assert_eq!(
        value(&ui, "feed"),
        "37",
        "after the menu (the document held feed {committed_on_open} while it was open)"
    );
}

/// F6 decides from where the keyboard is, so a click that took the keyboard back
/// to the canvas without F6 does not leave the next F6 going the wrong way.
#[test]
fn f6_enters_the_pane_after_a_click_has_taken_the_keyboard_back_to_the_canvas() {
    let (ui, canvas) = probe(near_label());
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    rows_of(&ui)[1].mock_single_click(PointerEventButton::Left);
    assert_eq!(canvas.borrow().doc().selected(), Some(2));
    key(&ui, Key::F6);
    type_text(ui.window(), "Q");
    assert_eq!(
        staged(&canvas),
        Some((2, Field::Name, "Q".to_string())),
        "F6 put the keyboard in the name field"
    );
}

/// A field committed while the pointer is still down on the canvas lands after the
/// drag rather than inside it, so two undos are the label as it opened and never
/// a state in which the drag is applied without the name.
#[test]
fn undoing_a_field_commit_made_during_a_drag_walks_back_through_states_that_existed() {
    let original = near_label();
    let (ui, canvas) = probe(original.clone());
    let row = &rows_of(&ui)[0];
    let (at, size) = (row.absolute_position(), row.size());
    let start = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    let end = slint::LogicalPosition::new(start.x + 40.0, start.y);
    let w = ui.window();
    let button = PointerEventButton::Left;
    w.dispatch_event(WindowEvent::PointerMoved { position: start });
    w.dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button,
    });
    w.dispatch_event(WindowEvent::PointerMoved { position: end });
    assert_eq!(frame_of(&label_of(&canvas), 1).feed, 14, "dragged ten dots");
    key(&ui, Key::F6);
    type_text(ui.window(), "N");
    key(&ui, Key::Return);
    w.dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button,
    });
    let after = label_of(&canvas);
    assert_eq!(frame_of(&after, 1).feed, 14);
    ui.invoke_undo();
    let one = label_of(&canvas);
    ui.invoke_undo();
    let two = label_of(&canvas);
    assert_eq!(
        two,
        original,
        "two undos are the label as it opened; one undo left feed {}, two left feed {}",
        frame_of(&one, 1).feed,
        frame_of(&two, 1).feed
    );
}

/// Ctrl+Shift+Z as a real window delivers it: winit's `logical_key` is affected by
/// every modifier except Ctrl (winit-0.30.13/src/event.rs:557), so the text is "Z",
/// which Slint's own shortcut table expects for Redo (i-slint-core-1.17.1/input.rs:994).
#[test]
fn ctrl_shift_z_from_inside_a_field_redoes_the_document_as_a_window_delivers_it() {
    let (ui, canvas) = probe(near_label());
    ui.invoke_select(1);
    click_into(&ui, "feed");
    type_text(ui.window(), "30");
    key(&ui, Key::Return);
    ctrl_z(&ui);
    assert_eq!(frame_of(&label_of(&canvas), 1).feed, 4);
    let w = ui.window();
    for k in [Key::Control, Key::Shift] {
        w.dispatch_event(WindowEvent::KeyPressed { text: k.into() });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: "Z".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: "Z".into() });
    for k in [Key::Shift, Key::Control] {
        w.dispatch_event(WindowEvent::KeyReleased { text: k.into() });
    }
    assert_eq!(frame_of(&label_of(&canvas), 1).feed, 30, "redone");
    assert_eq!(value(&ui, "feed"), "30");
}
