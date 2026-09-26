//! The obvious way to fill a field, kept as a fixture so that the reason the
//! inspector does not use it is measured rather than remembered.
//!
//! `ui/testing/naive-field.slint` holds a LineEdit whose text is bound one way
//! to a pushed value, beside the shipped Inspector, which is pushed the same
//! numbers by assignment. The widget's own first write removes the binding for
//! good (i-slint-core-1.17.1/properties.rs:1005-1020), and nothing at build time
//! says so.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use relabel_gui::ins::{Field, Shape, Slot};
use slint::platform::{Key, PointerEventButton};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use support::{editor_of, field_of, press, type_text};

include!(concat!(env!("OUT_DIR"), "/naive-field.rs"));

fn probe() -> NaiveFieldProbe {
    support::ensure_platform();
    let ui = NaiveFieldProbe::new().expect("the counter-example window");
    ui.show().expect("showing a headless window");
    ui
}

fn naive(ui: &NaiveFieldProbe) -> ElementHandle {
    ElementQuery::from_root(ui)
        .match_accessible_role(AccessibleRole::TextInput)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "naive"))
        .find_first()
        .expect("the naive field")
}

fn reads(h: &ElementHandle) -> String {
    h.accessible_value()
        .map(|v| v.to_string())
        .unwrap_or_default()
}

/// The shipped pane's model, one feed slot and fifteen unused, as `ins::wire`
/// builds it.
fn slots_with_feed(value: &str) -> Vec<InsSlot> {
    let mut slots = vec![as_row(&Slot::unused()); 16];
    slots[0] = as_row(&Slot {
        field: Field::Feed,
        shape: Shape::Number,
        value: value.to_string(),
        min: -39.0,
        max: 319.0,
        step: 1.0,
        read_only: false,
        face: String::new(),
    });
    slots
}

fn as_row(s: &Slot) -> InsSlot {
    InsSlot {
        shape: s.shape.ordinal(),
        field: s.field.token(),
        label: s.field.label().into(),
        unit: s.field.unit().into(),
        hint: s.field.hint().into(),
        value: s.value.as_str().into(),
        minimum: s.min,
        maximum: s.max,
        step: s.step,
        read_only: s.read_only,
        face: s.face.as_str().into(),
    }
}

#[test]
fn a_field_bound_to_the_pushed_value_stops_following_the_document_after_one_keystroke() {
    let ui = probe();
    ui.set_pushed("12".into());
    assert_eq!(
        reads(&naive(&ui)),
        "12",
        "the binding works until the widget writes"
    );
    editor_of(&naive(&ui)).mock_single_click(PointerEventButton::Left);
    press(ui.window(), Key::End, false, false);
    type_text(ui.window(), "3");
    assert_eq!(reads(&naive(&ui)), "123");
    ui.set_pushed("99".into());
    assert_eq!(
        reads(&naive(&ui)),
        "123",
        "one keystroke removed the binding: the push of 99 never arrived"
    );
    ui.set_pushed("7".into());
    assert_eq!(reads(&naive(&ui)), "123", "and no later push ever will");
}

#[test]
fn the_stale_value_is_what_a_screen_reader_is_told_too() {
    let ui = probe();
    let edits = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let edits = edits.clone();
        ui.on_naive_edited(move |t| edits.borrow_mut().push(t.to_string()));
    }
    ui.set_pushed("12".into());
    naive(&ui).set_accessible_value("40");
    assert_eq!(*edits.borrow(), ["40"]);
    ui.set_pushed("99".into());
    assert_eq!(
        naive(&ui).accessible_value().as_deref(),
        Some("40"),
        "an assistive technology reads the number the document no longer holds"
    );
}

#[test]
fn the_field_this_slice_ships_takes_the_same_push_and_keeps_following() {
    let ui = probe();
    let model = Rc::new(VecModel::from(slots_with_feed("12")));
    ui.set_slots(ModelRc::from(model.clone()));
    ui.set_shown(true);
    ui.invoke_show_fields();
    assert_eq!(reads(&field_of(&ui, "feed")), "12");

    editor_of(&field_of(&ui, "feed")).mock_single_click(PointerEventButton::Left);
    press(ui.window(), Key::End, false, false);
    type_text(ui.window(), "3");
    assert_eq!(
        reads(&field_of(&ui, "feed")),
        "123",
        "the widget wrote its own text"
    );

    for pushed in ["99", "7", "7"] {
        model.set_row_data(0, slots_with_feed(pushed)[0].clone());
        ui.invoke_show_fields();
        assert_eq!(reads(&field_of(&ui, "feed")), pushed);
    }
    assert_eq!(model.row_count(), 16);
}
