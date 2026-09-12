//! What the canvas publishes to an assistive technology, and what it accepts
//! back.
//!
//! There is no canvas or graphic role in `AccessibleRole`, so the surface is a
//! `list` and each element a `list-item` with an index. Asserted through the
//! accessibility tree rather than trusted, because nothing else in the
//! application is a drawing surface and there is no precedent in the toolkit to
//! copy: Slint's own canvas editor declares no accessible properties at all.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::AccessibleRole;
use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use relabel_render::document::{Element, ElementOrRaw, Rot};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use support::{canvas_of, frame, frame_of, rect, reference_label, rows_of};

include!(concat!(env!("OUT_DIR"), "/canvas-probe.rs"));

relabel_gui::impl_canvas_host!(CanvasProbe, ElRow);

fn probe(label: relabel_render::Label) -> (CanvasProbe, Rc<RefCell<Canvas<CanvasProbe>>>) {
    support::ensure_platform();
    let ui = CanvasProbe::new().expect("the probe window");
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

#[test]
fn the_canvas_is_a_list_with_one_item_per_element_it_draws() {
    let (ui, _canvas) = probe(reference_label());
    let surface = canvas_of(&ui);
    assert_eq!(surface.accessible_role(), Some(AccessibleRole::List));
    assert_eq!(surface.accessible_label().as_deref(), Some("Label canvas"));
    assert_eq!(surface.accessible_item_count(), Some(3));
    let rows = rows_of(&ui);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .map(|r| r.accessible_item_index())
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)]
    );
}

/// The sentence speaks the document's own feed and head, not the canvas's x and
/// y: those are the names in the file, in the inspector and in every message
/// preflight will print about the same element.
#[test]
fn an_element_reads_its_kind_its_name_and_its_placement_in_document_axes() {
    let (ui, _canvas) = probe(reference_label());
    let rows = rows_of(&ui);
    assert_eq!(
        rows[0].accessible_value().as_deref(),
        Some("Rectangle \u{2018}box 1\u{2019}, feed 8, head 8, 40 by 24 dots")
    );
    assert_eq!(rows[0].accessible_label(), rows[0].accessible_value());
}

/// The element's own box is the one the rasteriser will draw, so a quarter turn
/// swaps its sides on the canvas while the file keeps what the operator asked
/// for. The sentence reports the frame as stored; the row reports it as drawn.
#[test]
fn a_rotated_element_is_boxed_as_it_will_be_drawn() {
    let mut label = reference_label();
    label.elements = vec![rect(1, frame(8, 8, 40, 24), Rot::D90, false)];
    let (ui, canvas) = probe(label);
    let shown = canvas.borrow().doc().shown();
    assert_eq!((shown[0].w, shown[0].h), (24, 40));
    assert!(
        rows_of(&ui)[0]
            .accessible_value()
            .is_some_and(|v| v.ends_with("40 by 24 dots")),
        "the sentence states the frame the file holds"
    );
}

#[test]
fn a_locked_element_and_a_keepout_are_both_said_aloud() {
    let mut label = reference_label();
    label.canvas.inset = relabel_render::document::Insets {
        feed_start: 8,
        feed_end: 0,
        head_start: 0,
        head_end: 0,
    };
    label.elements = vec![rect(1, frame(0, 8, 40, 24), Rot::D0, true)];
    let (ui, _canvas) = probe(label);
    assert_eq!(
        rows_of(&ui)[0].accessible_value().as_deref(),
        Some(
            "Rectangle \u{2018}box 1\u{2019}, feed 0, head 8, 40 by 24 dots, \
             locked, outside the printable area"
        )
    );
}

#[test]
fn setting_the_accessible_value_moves_the_element_in_document_axes() {
    let (ui, canvas) = probe(reference_label());
    let rows = rows_of(&ui);
    rows[0].set_accessible_value("feed 40, head 16");
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(40, 16, 40, 24)
    );
    // And what it says now is what it was just told.
    assert_eq!(
        rows_of(&ui)[0].accessible_value().as_deref(),
        Some("Rectangle \u{2018}box 1\u{2019}, feed 40, head 16, 40 by 24 dots")
    );
}

/// The prompt an assistive technology puts up is prefilled with the whole
/// sentence, so the whole sentence has to be accepted back.
#[test]
fn the_sentence_it_published_is_one_it_accepts_back_edited() {
    let (ui, canvas) = probe(reference_label());
    rows_of(&ui)[0]
        .set_accessible_value("Rectangle \u{2018}box 1\u{2019}, feed 64, head 32, 40 by 24 dots");
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(64, 32, 40, 24)
    );
}

#[test]
fn an_unparseable_value_moves_nothing_and_says_what_to_type() {
    let (ui, canvas) = probe(reference_label());
    rows_of(&ui)[0].set_accessible_value("over there a bit");
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(8, 8, 40, 24)
    );
    assert_eq!(
        ui.get_status().as_str(),
        "\"over there a bit\" is not a placement: say \u{201c}feed 12, head 4\u{201d}"
    );
}

#[test]
fn a_hidden_element_is_not_on_the_canvas_at_all() {
    let mut label = reference_label();
    if let ElementOrRaw::Known(Element::Rect(r)) = &mut label.elements[1] {
        r.common.hidden = true;
    }
    let (ui, _canvas) = probe(label);
    assert_eq!(canvas_of(&ui).accessible_item_count(), Some(2));
    assert_eq!(
        rows_of(&ui)[1].accessible_value().as_deref(),
        Some("Rectangle \u{2018}box 3\u{2019}, feed 240, head 40, 40 by 24 dots")
    );
}

/// An element written by a later version has no frame this build can trust, so
/// it is not on the canvas - and it is still in the file after a save, which is
/// the promise the document format makes.
#[test]
fn an_element_this_build_cannot_draw_is_not_on_the_canvas_and_survives_a_save() {
    let mut label = reference_label();
    label.elements.push(ElementOrRaw::Raw(serde_json::json!({
        "type": "hologram",
        "id": 99,
        "frame": { "feed": 0, "head": 0, "feed_len": 8, "head_len": 8 },
        "shimmer": 3
    })));
    let (ui, canvas) = probe(label);
    assert_eq!(canvas_of(&ui).accessible_item_count(), Some(3));
    assert_eq!(canvas.borrow().doc().undrawable(), 1);
    let saved = canvas
        .borrow()
        .doc()
        .label()
        .to_json()
        .expect("a writable label");
    assert!(saved.contains("hologram") && saved.contains("shimmer"));
}
