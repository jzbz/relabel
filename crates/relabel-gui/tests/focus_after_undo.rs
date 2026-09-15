//! Does the keyboard still work after an undo?
//!
//! `Effect::Everything` calls `Canvas::publish`, which once called
//! `VecModel::set_vec`, which calls `notify.reset()` - and a model reset clears
//! every repeater instance (i-slint-core model/repeater.rs:508-511). The
//! per-element FocusScope holding the keyboard focus is one of those instances.
//! `Canvas::write_rows` now edits the model by element id and never resets it.

#[path = "support/mod.rs"]
mod support;

use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;
use std::rc::Rc;
use support::{frame_of, reference_label, rows_of};

include!(concat!(env!("OUT_DIR"), "/canvas-probe.rs"));

relabel_gui::impl_canvas_host!(CanvasProbe, ElRow);

fn probe() -> (CanvasProbe, Rc<RefCell<Canvas<CanvasProbe>>>) {
    support::ensure_platform();
    let ui = CanvasProbe::new().expect("the probe window");
    let doc = Doc::open(reference_label(), relabel_proto::PrintDirection::Left).expect("a label");
    let canvas = wire(&ui, doc);
    ui.show().expect("a headless window");
    (ui, canvas)
}

fn key(ui: &CanvasProbe, k: Key, ctrl: bool) {
    let w = ui.window();
    if ctrl {
        w.dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: k.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: k.into() });
    if ctrl {
        w.dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    }
}

fn ctrl_char(ui: &CanvasProbe, c: char) {
    let w = ui.window();
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    w.dispatch_event(WindowEvent::KeyPressed {
        text: c.to_string().into(),
    });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: c.to_string().into(),
    });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}

#[test]
fn the_arrow_keys_still_nudge_after_an_undo() {
    let (ui, canvas) = probe();
    rows_of(&ui)[0].mock_single_click(slint::platform::PointerEventButton::Left);

    key(&ui, Key::RightArrow, false);
    let after_one = frame_of(canvas.borrow().doc().label(), 1);
    eprintln!("after one nudge:  feed {}", after_one.feed);
    assert_eq!(after_one.feed, 9, "the first nudge works");

    ctrl_char(&ui, 'z');
    let after_undo = frame_of(canvas.borrow().doc().label(), 1);
    eprintln!("after ctrl+z:     feed {}", after_undo.feed);
    assert_eq!(after_undo.feed, 8, "the undo undid the nudge");
    eprintln!(
        "selection after undo: {:?}",
        canvas.borrow().doc().selected()
    );

    // The element is still selected, and nothing about the document changed its
    // ability to be nudged. If the keyboard focus survived the undo, this
    // second arrow key moves it again.
    key(&ui, Key::RightArrow, false);
    let after_two = frame_of(canvas.borrow().doc().label(), 1);
    eprintln!("after second arrow: feed {}", after_two.feed);
    assert_eq!(
        after_two.feed, 9,
        "an arrow key after an undo moved nothing: the undo's Effect::Everything \
         called publish -> set_vec -> model reset, and the reset cleared the \
         repeater instance that held the keyboard focus"
    );
}
