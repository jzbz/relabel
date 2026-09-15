//! The keyboard path: arrow keys, Tab between elements, and undo.
//!
//! The arrow keys are the accelerator and the numeric inspector fields will be
//! the primary path, but the inspector is not in this slice - so these are
//! every keyboard route the first commit ships, and they are the reason it can
//! claim to be usable without a pointer at all.

#[path = "support/mod.rs"]
mod support;

use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use relabel_render::document::Rot;
use relabel_render::target::Dpmm;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;
use std::rc::Rc;
use support::{frame, frame_of, rect, reference_label, rows_of};

include!(concat!(env!("OUT_DIR"), "/canvas-probe.rs"));

relabel_gui::impl_canvas_host!(CanvasProbe, ElRow);

fn probe(label: relabel_render::Label) -> (CanvasProbe, Rc<RefCell<Canvas<CanvasProbe>>>) {
    support::ensure_platform();
    let ui = CanvasProbe::new().expect("the probe window");
    // A fractional scale changes nothing about a key press, and running the
    // keyboard tests at 1.25x as well says so rather than assuming it.
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

/// Click an element, which focuses it, which selects it.
fn focus_row(ui: &CanvasProbe, row: usize) {
    rows_of(ui)[row].mock_single_click(slint::platform::PointerEventButton::Left);
}

fn key(ui: &CanvasProbe, k: Key, shift: bool) {
    let w = ui.window();
    if shift {
        w.dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: k.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: k.into() });
    if shift {
        w.dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    }
}

/// The arrow keys are spatial, so they move the element the way the operator
/// pressed - and on this hardware the canvas's horizontal axis is the feed.
#[test]
fn an_arrow_key_nudges_the_selected_element_one_dot_the_way_it_points() {
    let (ui, canvas) = probe(reference_label());
    focus_row(&ui, 0);
    key(&ui, Key::RightArrow, false);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(9, 8, 40, 24)
    );
    key(&ui, Key::DownArrow, false);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(9, 9, 40, 24)
    );
    key(&ui, Key::LeftArrow, false);
    key(&ui, Key::UpArrow, false);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(8, 8, 40, 24)
    );
}

/// Eight dots, because eight dots is a millimetre at 203 dpi - the unit the
/// label's own size and the printer's unprintable margin are both quoted in.
#[test]
fn shift_and_an_arrow_nudges_one_millimetre_of_dots() {
    let (ui, canvas) = probe(reference_label());
    focus_row(&ui, 0);
    key(&ui, Key::RightArrow, true);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 16);
}

/// The same millimetre on a 300 dpi label is twelve dots, which is why the
/// coarse step is computed from the document's pitch instead of being 8.
#[test]
fn a_coarse_nudge_is_still_a_millimetre_on_a_three_hundred_dpi_label() {
    let mut label = relabel_render::Label::new(472, 142, Dpmm::DPI_300);
    label.elements = vec![rect(1, frame(8, 8, 40, 24), Rot::D0, false)];
    let (ui, canvas) = probe(label);
    focus_row(&ui, 0);
    key(&ui, Key::RightArrow, true);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 20);
}

/// Focus and selection are one thing, so Tab walks the canvas and the
/// highlighted element is always the one the arrows will move.
#[test]
fn tab_moves_the_selection_to_the_next_element() {
    let (ui, canvas) = probe(reference_label());
    focus_row(&ui, 0);
    assert_eq!(canvas.borrow().doc().selected(), Some(1));
    key(&ui, Key::Tab, false);
    assert_eq!(canvas.borrow().doc().selected(), Some(2));
    key(&ui, Key::Tab, false);
    assert_eq!(canvas.borrow().doc().selected(), Some(3));
}

#[test]
fn an_arrow_key_on_a_locked_element_refuses_and_says_which_element_it_was() {
    let mut label = reference_label();
    label.elements[0] = rect(1, frame(8, 8, 40, 24), Rot::D0, true);
    let (ui, canvas) = probe(label);
    focus_row(&ui, 0);
    // The click's press is refused first, in the same words.
    assert_eq!(ui.get_status().as_str(), "\u{2018}box 1\u{2019} is locked");
    key(&ui, Key::RightArrow, false);
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(8, 8, 40, 24)
    );
    // Said again as a different string, so the live region announces it again.
    assert_eq!(
        ui.get_status().as_str(),
        "\u{2018}box 1\u{2019} is locked (again)"
    );
}

#[test]
fn each_nudge_is_its_own_undo_step() {
    let (ui, canvas) = probe(reference_label());
    focus_row(&ui, 0);
    key(&ui, Key::RightArrow, false);
    key(&ui, Key::RightArrow, false);
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 10);
    ui.invoke_undo();
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 9);
    ui.invoke_undo();
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8);
}

/// A key an element does not handle reaches the canvas scope that surrounds it,
/// which is how one undo binding serves every element.
#[test]
fn control_z_reaches_the_canvas_from_a_focused_element() {
    let (ui, canvas) = probe(reference_label());
    focus_row(&ui, 0);
    key(&ui, Key::RightArrow, false);
    let w = ui.window();
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    w.dispatch_event(WindowEvent::KeyPressed { text: "z".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: "z".into() });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(
        frame_of(canvas.borrow().doc().label(), 1),
        frame(8, 8, 40, 24),
        "Ctrl+Z was pressed while an element had focus"
    );
}

/// A keyboard sends Ctrl+Shift+Z with the shifted character: winit's logical key
/// keeps every modifier but Ctrl, so the text is "Z", and Slint's own shortcut
/// table matches both cases for Redo for that reason (i-slint-core input.rs:991-996).
#[test]
fn control_shift_z_as_a_keyboard_sends_it_redoes_on_the_canvas() {
    let (ui, canvas) = probe(reference_label());
    focus_row(&ui, 0);
    key(&ui, Key::RightArrow, false);
    let w = ui.window();
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    w.dispatch_event(WindowEvent::KeyPressed { text: "z".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: "z".into() });
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 8, "undone");
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    w.dispatch_event(WindowEvent::KeyPressed { text: "Z".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: "Z".into() });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(frame_of(canvas.borrow().doc().label(), 1).feed, 9, "redone");
}
