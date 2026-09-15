//! What an assistive technology is given to add, delete and resize with: named
//! buttons with a default action, the length fields' numeric value and increment,
//! and a status line that says what came and went. And what the toolbar costs the
//! view beside it.
//!
//! Headless, so this pins the tree Slint builds, not what AT-SPI or Orca make of it.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleRole, ElementQuery};
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::edit;
use relabel_gui::ins::{self, Field};
use relabel_gui::view::{self, Opening};
use relabel_render::document::ElementId;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use support::{
    button_of, canvas_of, click, field_of, frame, frame_of, ids_of, kind_of, press,
    reference_label, rows_of, status_of, type_text,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

/// The six add buttons, by the name each announces, the chord it describes and the
/// kind it adds.
const ADDS: [(&str, &str, &str); 6] = [
    ("Add text", "Control+Shift+T", "text"),
    ("Add barcode", "Control+Shift+B", "barcode"),
    ("Add QR code", "Control+Shift+R", "qr"),
    ("Add DataMatrix", "Control+Shift+M", "data_matrix"),
    ("Add rectangle", "Control+Shift+O", "rect"),
    ("Add line", "Control+Shift+L", "line"),
];

fn probe_at(scale: f32, opening: Opening) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(reference_label(), relabel_proto::PrintDirection::Left)
        .expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    edit::wire(&ui, &canvas);
    view::wire(&ui, &canvas, opening);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

fn probe() -> (InsProbe, Wired) {
    probe_at(1.0, Opening::At(DEFAULT_ZOOM))
}

fn ids(canvas: &Wired) -> Vec<ElementId> {
    ids_of(canvas.borrow().doc().label())
}

fn said(ui: &InsProbe) -> String {
    status_of(ui)
        .accessible_value()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[test]
fn the_toolbar_buttons_are_named_and_described_and_none_is_a_tab_stop() {
    let (ui, canvas) = probe();
    for (name, keys, _) in ADDS {
        assert_eq!(
            button_of(&ui, name).accessible_description().as_deref(),
            Some(keys),
            "{name} says its chord"
        );
    }
    assert_eq!(
        button_of(&ui, "Delete element")
            .accessible_description()
            .as_deref(),
        Some("Delete")
    );
    // From the surface, three elements and then the pane, as with no toolbar.
    ui.invoke_focus_canvas();
    for _ in 0..4 {
        press(ui.window(), Key::Tab, false, false);
    }
    type_text(ui.window(), "X");
    assert_eq!(
        canvas
            .borrow()
            .doc()
            .staged()
            .map(|(id, f, t)| (id, f, t.to_string())),
        Some((3, Field::Name, "X".to_string()))
    );
}

#[test]
fn each_add_button_adds_its_kind_through_its_default_action() {
    for (name, _, kind) in ADDS {
        let (ui, canvas) = probe();
        button_of(&ui, name).invoke_accessible_default_action();
        assert_eq!(kind_of(canvas.borrow().doc().label(), 4), kind, "{name}");
        assert_eq!(canvas.borrow().doc().selected(), Some(4), "{name}");
        assert!(
            said(&ui).starts_with("added"),
            "{name} is announced: {}",
            said(&ui)
        );
        let feed = frame_of(canvas.borrow().doc().label(), 4).feed;
        press(ui.window(), Key::RightArrow, false, false);
        assert_eq!(
            frame_of(canvas.borrow().doc().label(), 4).feed,
            feed + 1,
            "{name}: an arrow moves what it added"
        );
    }
}

#[test]
fn the_delete_button_deletes_the_selection_and_says_why_when_there_is_none() {
    let (ui, canvas) = probe();
    click(&ui, &rows_of(&ui)[0]);
    button_of(&ui, "Delete element").invoke_accessible_default_action();
    assert_eq!(ids(&canvas), [2, 3]);
    button_of(&ui, "Delete element").invoke_accessible_default_action();
    assert_eq!(ui.get_status(), "nothing is selected to delete");
}

#[test]
fn a_refusal_said_again_changes_the_status_line_so_it_is_announced_again() {
    let (ui, _canvas) = probe();
    let delete = button_of(&ui, "Delete element");
    let mut heard = Vec::new();
    for _ in 0..3 {
        delete.invoke_accessible_default_action();
        heard.push(said(&ui));
    }
    // A live region is announced when its value changes, so each has to differ from
    // the one before it, and each still says what was refused.
    for (i, s) in heard.iter().enumerate() {
        assert!(s.starts_with("nothing is selected to delete"), "{heard:?}");
        if i > 0 {
            assert_ne!(*s, heard[i - 1], "{heard:?}");
        }
    }
}

#[test]
fn the_length_fields_resize_through_set_value_and_increment() {
    let (ui, canvas) = probe();
    click(&ui, &rows_of(&ui)[0]);
    field_of(&ui, "feed length").set_accessible_value("60");
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed_len, 60);
    field_of(&ui, "head length").invoke_accessible_increment_action();
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).head_len, 25);
    let feed_len = field_of(&ui, "feed length");
    assert_eq!(feed_len.accessible_value_minimum(), Some(1.0));
    assert_eq!(feed_len.accessible_value_maximum(), Some(65535.0));
    press(ui.window(), "z", false, true);
    press(ui.window(), "z", false, true);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(8, 8, 40, 24)
    );
}

#[test]
fn add_delete_and_a_structural_undo_are_announced() {
    let (ui, _canvas) = probe();
    button_of(&ui, "Add rectangle").invoke_accessible_default_action();
    assert!(
        said(&ui).starts_with("added Rectangle element 4"),
        "{}",
        said(&ui)
    );
    press(ui.window(), Key::Delete, false, false);
    assert!(
        said(&ui).starts_with("deleted Rectangle element 4"),
        "{}",
        said(&ui)
    );
    press(ui.window(), "z", false, true);
    assert!(
        said(&ui).starts_with("undo brought back Rectangle element 4"),
        "{}",
        said(&ui)
    );
}

#[test]
fn every_element_keeps_a_node_and_a_contiguous_index() {
    let (ui, canvas) = probe();
    let w = ui.window();
    let delete: slint::SharedString = Key::Delete.into();
    // What is pressed, then the key with Shift and Control.
    let steps = [
        ("Ctrl+Shift+T", "T", true, true),
        ("Delete", delete.as_str(), false, false),
        ("Ctrl+Z", "z", false, true),
        ("Ctrl+Shift+Z", "Z", true, true),
    ];
    let mut counts = Vec::new();
    for (what, key, shift, control) in steps {
        press(w, key, shift, control);
        let n = canvas.borrow().doc().shown().len();
        counts.push(n);
        let indices: Vec<usize> = rows_of(&ui)
            .iter()
            .filter_map(|r| r.accessible_item_index())
            .collect();
        assert_eq!(indices, (0..n).collect::<Vec<_>>(), "after {what}");
        assert_eq!(
            canvas_of(&ui).accessible_item_count(),
            Some(n),
            "after {what}"
        );
    }
    assert_eq!(counts, [4, 3, 4, 3], "each step added or took away one");
}

#[test]
fn the_toolbar_row_leaves_the_view_and_the_pane_in_the_window() {
    for (scale, want) in [(1.0, 2), (1.25, 2), (1.5, 3), (2.0, 4)] {
        let (ui, canvas) = probe_at(scale, Opening::Fit);
        i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
        assert_eq!(canvas.borrow().zoom(), want, "at {scale}x");
        ui.invoke_select(1);
        let (add, zoom_in) = (button_of(&ui, "Add text"), button_of(&ui, "Zoom in"));
        assert_eq!(
            add.absolute_position().y,
            zoom_in.absolute_position().y,
            "at {scale}x the toolbar shares the zoom bar's row"
        );
        assert_eq!(add.size().height, 28.0, "at {scale}x");
        assert_eq!(zoom_in.size().height, 28.0, "at {scale}x");
        let (delete, fit) = (
            button_of(&ui, "Delete element"),
            button_of(&ui, "Zoom to fit"),
        );
        assert!(
            delete.absolute_position().x + delete.size().width <= fit.absolute_position().x,
            "at {scale}x the edit bar ends before the zoom bar starts"
        );
        let pane = ElementQuery::from_root(&ui)
            .match_accessible_role(AccessibleRole::Groupbox)
            .find_first()
            .expect("the pane, shown with a selection");
        let right = pane.absolute_position().x + pane.size().width;
        let window = ui.global::<DefaultWindow>().get_width();
        assert!(
            right <= window + 0.01,
            "at {scale}x the pane ends at {right} in a window {window} wide"
        );
        eprintln!(
            "at {scale}x the zoom bar starts at {} and the pane at {}",
            fit.absolute_position().x,
            pane.absolute_position().x
        );
    }
}
