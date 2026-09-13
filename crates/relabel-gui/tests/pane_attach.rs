//! The pane beside the canvas: what a second column costs the canvas, and what it
//! must not cost.
//!
//! The canvas is routinely wider than the window, so every pixel the pane takes
//! is a pixel of label out of view. Since slice 12b an element wholly outside the
//! view keeps its place in the accessibility tree and the Tab order as a point just
//! inside the view's edge, so out of view no longer means unreachable; these pin
//! what the pane costs the view rather than leave it to be noticed.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;
use std::rc::Rc;
use support::{
    canvas_of, editor_of, every_kind_label, field_of, fields_of, frame_of, press, reference_label,
    rows_of, type_text,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);

/// The shell's padding on either side of the window.
const PADDING: f32 = 12.0;

/// The probe window's width, which is the application's, read from the one global
/// both are sized from.
fn window_width(ui: &InsProbe) -> f32 {
    ui.global::<DefaultWindow>().get_width()
}

fn probe(label: relabel_render::Label, scale: f32) -> (InsProbe, Rc<RefCell<Canvas<InsProbe>>>) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

fn pane_of(ui: &InsProbe) -> ElementHandle {
    ElementQuery::from_root(ui)
        .match_accessible_role(AccessibleRole::Groupbox)
        .find_first()
        .expect("the pane, which is shown only with a selection")
}

fn is_whole(px: f32) -> bool {
    (px - px.round()).abs() < 1.0e-3
}

#[test]
fn the_dot_grid_still_lands_on_whole_device_pixels_beside_the_pane() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, _canvas) = probe(every_kind_label(), scale);
        for id in [-1, 1, 6] {
            ui.invoke_select(id);
            let grid = canvas_of(&ui).absolute_position();
            for (axis, logical) in [("x", grid.x), ("y", grid.y)] {
                assert!(
                    is_whole(logical * scale),
                    "at {scale}x with element {id} selected the dot grid's {axis} is {} \
                     device pixels",
                    logical * scale
                );
            }
            let row = rows_of(&ui)[0].size();
            assert_eq!(
                (row.width * scale).round() as i32,
                12 * DEFAULT_ZOOM as i32,
                "at {scale}x an element is still a whole number of dots across"
            );
        }
    }
}

#[test]
fn every_field_is_inside_the_pane_rather_than_clipped_by_it() {
    let (ui, _canvas) = probe(every_kind_label(), 1.0);
    for id in 1..=7 {
        ui.invoke_select(id);
        let pane = pane_of(&ui);
        let (left, right) = (
            pane.absolute_position().x,
            pane.absolute_position().x + pane.size().width,
        );
        assert!(
            right <= window_width(&ui) - PADDING + 0.01,
            "the pane ends at {right}"
        );
        for field in fields_of(&ui) {
            let at = field.absolute_position().x;
            let end = at + field.size().width;
            assert!(
                at >= left && end <= right + 0.01,
                "{:?} of element {id} spans {at}..{end} in a pane spanning {left}..{right}",
                field.accessible_label()
            );
            if field.accessible_role() != Some(AccessibleRole::Text) {
                let editor = editor_of(&field);
                let editor_end = editor.absolute_position().x + editor.size().width;
                assert!(
                    editor_end <= end + 0.01,
                    "{:?}'s editor ends at {editor_end}, past its field at {end}",
                    field.accessible_label()
                );
            }
        }
    }
}

#[test]
fn the_leftmost_element_of_the_reference_label_is_still_in_the_accessibility_tree() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, _canvas) = probe(reference_label(), scale);
        ui.invoke_select(2);
        let rows = rows_of(&ui);
        assert!(
            rows.first().and_then(ElementHandle::accessible_item_index) == Some(0),
            "at {scale}x the first element is not in the tree beside the pane"
        );
        assert_eq!(
            rows[0].accessible_value().as_deref(),
            Some("Rectangle \u{2018}box 1\u{2019}, feed 8, head 8, 40 by 24 dots")
        );
    }
}

#[test]
fn the_canvas_does_not_take_the_panes_width() {
    let (ui, _canvas) = probe(every_kind_label(), 1.0);
    ui.invoke_select(1);
    let pane = pane_of(&ui);
    let canvas = canvas_of(&ui);
    assert_eq!(
        canvas.size().width,
        (320 * DEFAULT_ZOOM) as f32,
        "the label at zoom 4 is far wider than the window"
    );
    let pane_end = pane.absolute_position().x + pane.size().width;
    assert!(
        (pane_end - (window_width(&ui) - PADDING)).abs() < 0.01,
        "the pane ends at {pane_end}, not at the window's padding"
    );
    // Every field's editor is the style's own minimum width, which a pane squeezed
    // by the canvas could not give it.
    let feed = editor_of(&field_of(&ui, "feed"));
    assert!(feed.size().width >= 100.0, "{}", feed.size().width);
}

/// An element the view does not show is still in the accessibility tree, as a
/// point just inside the view's edge, and still editable through the pane. Before
/// slice 12b this asserted the opposite: element 3 was out of the tree until its
/// feed was typed back into view, which was the defect.
#[test]
fn an_element_out_of_view_is_in_the_tree_and_editable_through_the_pane() {
    let (ui, canvas) = probe(reference_label(), 1.0);
    ui.invoke_select(1);
    let present: Vec<usize> = rows_of(&ui)
        .iter()
        .filter_map(ElementHandle::accessible_item_index)
        .collect();
    assert!(
        present.contains(&2),
        "element 3 at feed 240 is out of view at zoom 4 and still in the tree: {present:?}"
    );
    ui.invoke_select(3);
    assert_eq!(
        field_of(&ui, "name").accessible_value().as_deref(),
        Some("box 3")
    );
    field_of(&ui, "feed").set_accessible_value("20");
    assert_eq!(support::frame_of(canvas.borrow().doc().label(), 3).feed, 20);
    let present: Vec<usize> = rows_of(&ui)
        .iter()
        .filter_map(ElementHandle::accessible_item_index)
        .collect();
    assert!(present.contains(&2), "still in the tree: {present:?}");
}

/// A feed the field announces and accepts must not cost the element its Tab stop
/// on the canvas. At feed -1 the reference label's first element has 39 of its 40
/// dots on screen, and a keyboard-only operator who leaves it has no other way back
/// to it: Tab is the only keyboard route to a selection.
#[test]
fn an_element_typed_to_a_negative_feed_is_still_a_tab_stop_on_the_canvas() {
    for typed in ["0", "-1"] {
        let (ui, canvas) = probe(reference_label(), 1.0);
        press(ui.window(), Key::Tab, false, false);
        assert_eq!(canvas.borrow().doc().selected(), Some(1));
        press(ui.window(), Key::F6, false, false);
        press(ui.window(), Key::Tab, false, false);
        type_text(ui.window(), typed);
        press(ui.window(), Key::Return, false, false);
        let feed: i32 = typed.parse().expect("a number");
        assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, feed);
        // Back to the canvas, where the first Tab lands on the first element.
        press(ui.window(), Key::F6, false, false);
        let row = rows_of(&ui)[0].clone();
        assert!(
            row.absolute_position().x >= 0.0 && row.size().width == 160.0,
            "element 1 is on screen at feed {feed}: {:?}",
            row.absolute_position()
        );
        press(ui.window(), Key::Tab, false, false);
        assert_eq!(
            canvas.borrow().doc().selected(),
            Some(1),
            "after typing feed {typed}, Tab from the canvas skipped element 1"
        );
    }
}
