//! What the inspector publishes: one node per field, each with a role, a name, a
//! value, a unit and a range, all of them Rust's.
//!
//! The fields are the primary keyboard path, so what an assistive technology is
//! told is asserted here field by field, for all seven element kinds, rather than
//! trusted to the widgets.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleLiveness, AccessibleRole, ElementQuery};
use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use relabel_gui::geom;
use relabel_gui::ins::{self, Field, Shape};
use relabel_render::document::Rot;
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::rc::Rc;
use support::{
    every_kind_label, field_of, fields_of, find_field, frame, rect, reference_label, status_of,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);

fn probe(label: relabel_render::Label) -> (InsProbe, Rc<RefCell<Canvas<InsProbe>>>) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

/// The slots the document says the pane is showing, without the unused ones.
fn slots(canvas: &Rc<RefCell<Canvas<InsProbe>>>) -> Vec<ins::Slot> {
    canvas
        .borrow()
        .doc()
        .inspected()
        .expect("something is selected")
        .into_iter()
        .filter(|s| s.shape != Shape::Unused)
        .collect()
}

fn role_of(shape: Shape) -> AccessibleRole {
    match shape {
        Shape::Number => AccessibleRole::Spinbox,
        Shape::Word => AccessibleRole::TextInput,
        Shape::Read => AccessibleRole::Text,
        Shape::Unused => AccessibleRole::None,
    }
}

#[test]
fn every_field_is_one_accessibility_node_with_a_name_a_value_and_a_role() {
    let (ui, canvas) = probe(every_kind_label());
    for id in 1..=7 {
        ui.invoke_select(id);
        let slots = slots(&canvas);
        for s in &slots {
            let h = field_of(&ui, s.field.label());
            assert_eq!(h.accessible_role(), Some(role_of(s.shape)), "{:?}", s.field);
            assert_eq!(
                h.accessible_value().as_deref(),
                Some(s.value.as_str()),
                "{:?} of element {id}",
                s.field
            );
        }
        // The editor inside each field publishes nothing of its own: a second
        // node would be an unlabelled text box whose own set-value writes text
        // the document never hears about.
        let inputs = ElementQuery::from_root(&ui)
            .match_accessible_role(AccessibleRole::TextInput)
            .find_all()
            .len();
        let words = slots.iter().filter(|s| s.shape == Shape::Word).count();
        assert_eq!(inputs, words, "element {id}");
    }
}

#[test]
fn a_number_field_announces_its_own_range_and_its_own_step() {
    let (ui, _canvas) = probe(every_kind_label());
    ui.invoke_select(1);
    let size = field_of(&ui, "em size");
    assert_eq!(size.accessible_value_minimum(), Some(0.0));
    assert_eq!(size.accessible_value_maximum(), Some(4096.0));
    assert_eq!(size.accessible_value_step(), Some(1.0));
    let line = field_of(&ui, "line height");
    let step = line.accessible_value_step().expect("a step");
    assert!((step - 0.1).abs() < 1e-6, "a tenth, not {step}");
    let rot = field_of(&ui, "rotation");
    assert_eq!(rot.accessible_value_step(), Some(90.0), "a quarter turn");
    assert_eq!(rot.accessible_value_maximum(), Some(359.0));
    ui.invoke_select(3);
    let version = field_of(&ui, "version");
    assert_eq!(
        (
            version.accessible_value_minimum(),
            version.accessible_value_maximum()
        ),
        (Some(1.0), Some(40.0))
    );
}

#[test]
fn a_placement_field_announces_the_placements_this_element_may_take() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, 8, 40, 24), Rot::D90, false);
    let (ui, canvas) = probe(label);
    ui.invoke_select(1);
    let canvas_rect = canvas.borrow().doc().label().canvas;
    // Turned a quarter, the drawn frame is 24 along the feed and 40 across.
    let feed = geom::reach(24, canvas_rect.feed);
    let head = geom::reach(40, canvas_rect.head);
    let f = field_of(&ui, "feed");
    let h = field_of(&ui, "head");
    assert_eq!(
        (f.accessible_value_minimum(), f.accessible_value_maximum()),
        (Some(feed.0 as f32), Some(feed.1 as f32))
    );
    assert_eq!(
        (h.accessible_value_minimum(), h.accessible_value_maximum()),
        (Some(head.0 as f32), Some(head.1 as f32))
    );
    assert_eq!((feed, head), ((-23, 319), (-39, 95)));
}

#[test]
fn the_pane_is_a_group_that_says_which_element_it_is_editing() {
    let (ui, _canvas) = probe(reference_label());
    ui.invoke_select(2);
    let group = ElementQuery::from_root(&ui)
        .match_accessible_role(AccessibleRole::Groupbox)
        .find_all();
    assert_eq!(group.len(), 1);
    assert_eq!(group[0].accessible_label().as_deref(), Some("Element"));
    assert_eq!(
        group[0].accessible_description().as_deref(),
        Some("Rectangle \u{2018}box 2\u{2019}")
    );
    let heading = ElementQuery::from_root(&ui)
        .match_predicate(|h| {
            h.accessible_label()
                .is_some_and(|l| l == "Selected element")
        })
        .find_first()
        .expect("the heading");
    assert_eq!(
        heading.accessible_value().as_deref(),
        Some("Rectangle \u{2018}box 2\u{2019}")
    );
}

#[test]
fn each_kind_publishes_its_own_fields_and_only_those() {
    let (ui, canvas) = probe(every_kind_label());
    for id in 1..=7 {
        ui.invoke_select(id);
        let expected: Vec<String> = slots(&canvas)
            .iter()
            .map(|s| s.field.label().to_string())
            .collect();
        let published: Vec<String> = fields_of(&ui)
            .iter()
            .filter_map(|h| h.accessible_label().map(|l| l.to_string()))
            .collect();
        assert_eq!(published, expected, "element {id}, in pane order");
    }
}

/// Sixteen slots at thirty-two pixels and their spacing is taller than the
/// column the pane has in this window, so a slot that kept its height when unused
/// would push the status line out of the window, where the accessibility tree
/// cannot see it.
#[test]
fn the_fields_a_kind_does_not_have_are_in_neither_the_tree_nor_the_layout() {
    let (ui, canvas) = probe(every_kind_label());
    ui.invoke_select(7);
    assert_eq!(fields_of(&ui).len(), 8, "a line has eight rows");
    assert!(find_field(&ui, "em size").is_none());
    assert!(find_field(&ui, "stroke").is_none());
    let status = status_of(&ui);
    let bottom = status.absolute_position().y + status.size().height;
    assert!(bottom <= 600.0, "the status line ends at {bottom}");
    // And the eight rows sit one under the other with nothing between them.
    let ys: Vec<f32> = fields_of(&ui)
        .iter()
        .map(|h| h.absolute_position().y)
        .collect();
    let pitch = ys[1] - ys[0];
    for w in ys.windows(2) {
        assert!((w[1] - w[0] - pitch).abs() < 0.01, "{ys:?}");
    }
    assert_eq!(slots(&canvas).len(), 8);
}

#[test]
fn the_pane_has_as_many_slots_as_the_widest_kind_needs() {
    let (ui, _canvas) = probe(every_kind_label());
    assert_eq!(ui.get_ins_slots().row_count(), ins::SLOTS);
    ui.invoke_select(1);
    assert_eq!(
        fields_of(&ui).len(),
        14,
        "the text element is the widest kind"
    );
    assert_eq!(
        ui.get_ins_slots().row_count(),
        ins::SLOTS,
        "the length never changes"
    );
}

#[test]
fn a_slot_changes_shape_when_the_selection_changes_kind() {
    let (ui, _canvas) = probe(every_kind_label());
    ui.invoke_select(1);
    assert_eq!(
        fields_of(&ui)[7].accessible_role(),
        Some(AccessibleRole::TextInput)
    );
    assert_eq!(
        fields_of(&ui)[7].accessible_label().as_deref(),
        Some("text")
    );
    ui.invoke_select(6);
    let eighth = &fields_of(&ui)[7];
    assert_eq!(eighth.accessible_role(), Some(AccessibleRole::Spinbox));
    assert_eq!(eighth.accessible_label().as_deref(), Some("stroke"));
    assert_eq!(eighth.accessible_value().as_deref(), Some("1"));
    let ninth = &fields_of(&ui)[8];
    assert_eq!(ninth.accessible_role(), Some(AccessibleRole::Text));
    assert!(find_field(&ui, "text").is_none());
}

#[test]
fn a_row_that_is_shown_and_not_edited_offers_no_value_to_set() {
    let (ui, canvas) = probe(reference_label());
    ui.invoke_select(1);
    let before = canvas.borrow().doc().label().clone();
    let status = ui.get_status();
    let extent = field_of(&ui, "extent");
    assert_eq!(extent.accessible_role(), Some(AccessibleRole::Text));
    assert_eq!(extent.accessible_value().as_deref(), Some("40 by 24 dots"));
    extent.set_accessible_value("1 by 1 dots");
    assert_eq!(extent.accessible_value().as_deref(), Some("40 by 24 dots"));
    assert_eq!(canvas.borrow().doc().label(), &before);
    assert_eq!(
        ui.get_status(),
        status,
        "not even a refusal: there was nothing to set"
    );
}

#[test]
fn a_locked_element_makes_every_field_announce_itself_read_only() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, 8, 40, 24), Rot::D0, true);
    let (ui, _canvas) = probe(label);
    ui.invoke_select(1);
    let editable: Vec<_> = fields_of(&ui)
        .into_iter()
        .filter(|h| h.accessible_role() != Some(AccessibleRole::Text))
        .collect();
    assert!(!editable.is_empty());
    for h in editable {
        assert_eq!(
            h.accessible_read_only(),
            Some(true),
            "{:?}",
            h.accessible_label()
        );
    }
    ui.invoke_select(2);
    assert_eq!(field_of(&ui, "feed").accessible_read_only(), Some(false));
}

#[test]
fn nothing_selected_leaves_the_whole_pane_out_of_the_tree_and_the_tab_order() {
    let (ui, _canvas) = probe(reference_label());
    assert!(fields_of(&ui).is_empty());
    assert!(
        ElementQuery::from_root(&ui)
            .match_accessible_role(AccessibleRole::Groupbox)
            .find_first()
            .is_none()
    );
    // Tab reaches what the tree reaches: both are gated on the item being
    // visible, so no editor in the pane is a stop.
    assert!(
        ElementQuery::from_root(&ui)
            .match_inherits("TextInput")
            .find_first()
            .is_none()
    );
    ui.invoke_select(1);
    assert!(!fields_of(&ui).is_empty());
    ui.invoke_select(-1);
    assert!(fields_of(&ui).is_empty());
}

#[test]
fn the_pane_names_its_fields_in_the_documents_own_axes() {
    let (ui, _canvas) = probe(reference_label());
    ui.invoke_select(1);
    let labels: Vec<String> = fields_of(&ui)
        .iter()
        .filter_map(|h| h.accessible_label().map(|l| l.to_string()))
        .collect();
    assert_eq!(&labels[1..3], ["feed", "head"]);
    for canvas_word in ["x", "y", "left", "top", "width", "height"] {
        assert!(!labels.iter().any(|l| l == canvas_word), "{labels:?}");
    }
}

#[test]
fn the_name_every_field_announces_is_the_name_rust_gives_it() {
    let (ui, canvas) = probe(every_kind_label());
    let mut seen = Vec::new();
    for id in 1..=7 {
        ui.invoke_select(id);
        for s in slots(&canvas) {
            field_of(&ui, s.field.label());
            seen.push(s.field);
        }
    }
    for f in Field::ALL {
        assert!(seen.contains(&f), "{f:?} is announced by no kind");
    }
}

#[test]
fn the_unit_every_number_announces_is_the_unit_rust_gives_it() {
    let (ui, canvas) = probe(every_kind_label());
    for id in 1..=7 {
        ui.invoke_select(id);
        for s in slots(&canvas)
            .into_iter()
            .filter(|s| s.shape == Shape::Number)
        {
            assert_eq!(
                field_of(&ui, s.field.label())
                    .accessible_description()
                    .as_deref(),
                Some(s.field.unit()),
                "{:?}",
                s.field
            );
        }
    }
}

#[test]
fn the_status_line_is_a_live_region_so_a_refusal_is_announced() {
    let (ui, _canvas) = probe(reference_label());
    assert_eq!(
        status_of(&ui).accessible_live_region(),
        Some(AccessibleLiveness::Polite)
    );
    ui.invoke_select(1);
    field_of(&ui, "feed").set_accessible_value("twelve");
    let said = status_of(&ui).accessible_value().expect("a status");
    assert!(said.contains("\"twelve\""), "{said}");
}

/// What an empty optional field stands for - the page's threshold, the smallest
/// version that fits - is `Field::hint`. The LineEdit that shows it as a
/// placeholder is `accessible-role: none` and flattened out of the tree, so the
/// node a reader lands on has to carry it too.
#[test]
fn an_empty_optional_field_tells_an_assistive_technology_what_empty_means() {
    let (ui, canvas) = probe(every_kind_label());
    let mut silent = Vec::new();
    let mut checked = 0;
    for id in 1..=7 {
        ui.invoke_select(id);
        let slots = canvas.borrow().doc().inspected().expect("selected");
        for s in slots.into_iter().filter(|s| {
            matches!(s.shape, Shape::Number | Shape::Word)
                && s.value.is_empty()
                && !s.field.hint().is_empty()
        }) {
            checked += 1;
            let node = field_of(&ui, s.field.label());
            let told = format!(
                "{} | {}",
                node.accessible_description().unwrap_or_default(),
                node.accessible_placeholder_text().unwrap_or_default()
            );
            if !told.contains(s.field.hint()) {
                silent.push(format!("{:?} on element {id}: told {told:?}", s.field));
            }
        }
    }
    assert!(
        checked > 0,
        "the label of every kind has no empty optional field, so this test checks nothing"
    );
    assert_eq!(
        silent,
        Vec::<String>::new(),
        "empty fields whose meaning never reaches the accessibility tree"
    );
}
