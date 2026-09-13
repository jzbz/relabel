//! Labels and handles the canvas tests share. Nothing here touches the
//! toolkit except the two element-tree helpers at the bottom, which are about
//! one trap each.

// Shared by several test binaries, each of which uses part of it.
#![allow(dead_code)]

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use relabel_render::document::{
    Common, Element, ElementId, ElementOrRaw, Label, RectDots, RectEl, Rot,
};
use relabel_render::target::Dpmm;
use slint::platform::{Key, WindowEvent};

/// The 40 by 12 mm label at 203 dpi this project measures everything against:
/// 320 dots along the feed, 96 across the head.
pub fn reference_label() -> Label {
    let mut label = Label::new(320, 96, Dpmm::DPI_203);
    label.elements = vec![
        rect(1, frame(8, 8, 40, 24), Rot::D0, false),
        rect(2, frame(120, 8, 40, 24), Rot::D0, false),
        rect(3, frame(240, 40, 40, 24), Rot::D0, false),
    ];
    label
}

pub fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

pub fn rect(id: ElementId, frame: RectDots, rot: Rot, locked: bool) -> ElementOrRaw {
    let mut common = Common::new(id, frame);
    common.rot = rot;
    common.locked = locked;
    common.name = format!("box {id}");
    ElementOrRaw::Known(Element::Rect(RectEl {
        common,
        stroke_dots: 0,
        radius_dots: 0,
    }))
}

/// The frame of one element, straight out of the document.
pub fn frame_of(label: &Label, id: ElementId) -> RectDots {
    label
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .map(Element::common)
        .find(|c| c.id == id)
        .expect("the element is in the label")
        .frame
}

/// The canvas's element rows, one handle each, in model order.
///
/// `find_all` returns a repeated element once per *source* element merged into
/// an ItemRc, so a three-row canvas answers a role query nine times over.
/// `accessible-item-index` is the one key that is one per model row, so that is
/// what this deduplicates on - and indexing into the raw result instead
/// silently addresses the wrong element.
pub fn rows_of(probe: &impl slint::ComponentHandle) -> Vec<ElementHandle> {
    let mut found: Vec<(usize, ElementHandle)> = ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .filter_map(|h| h.accessible_item_index().map(|i| (i, h)))
        .collect();
    found.sort_by_key(|(i, _)| *i);
    found.dedup_by_key(|(i, _)| *i);
    found.into_iter().map(|(_, h)| h).collect()
}

/// The canvas surface itself: the one element with the list role.
pub fn canvas_of(probe: &impl slint::ComponentHandle) -> ElementHandle {
    ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::List)
        .find_first()
        .expect(
            "the canvas publishes itself as a list; if this fails the probe was \
             compiled without slint debug info and every other assertion here is vacuous",
        )
}

/// Install the headless platform, at most once for this thread.
///
/// The testing backend lives in a thread-local, so every `#[test]` in a binary
/// gets its own - and asking twice on one thread panics with
/// `platform already initialized`. A test that opens several windows, which is
/// what comparing display scales needs, must therefore ask once and then build
/// as many windows as it likes.
pub fn ensure_platform() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    DONE.with(|done| {
        if !done.replace(true) {
            i_slint_backend_testing::init_no_event_loop();
        }
    });
}

/// A label holding one element of each of the seven kinds, ids 1 to 7 in the
/// order the document model declares them, each at its own place along the feed
/// so a click reaches the one it means.
pub fn every_kind_label() -> Label {
    let frame = |feed: i32| {
        format!(r#""frame": {{"feed": {feed}, "head": 4, "feed_len": 12, "head_len": 12}}"#)
    };
    let json = format!(
        r#"{{"format": "relabel-label", "version": 1,
            "media": {{"feed_tenths_mm": 400, "head_tenths_mm": 120}},
            "canvas": {{"feed": 320, "head": 96, "dpmm": {{"num": 8, "den": 1}}}},
            "elements": [
              {{"type": "text", "id": 1, "name": "SKU", {}, "text": "{{sku}}", "family": "Noto Sans", "size": 12.0}},
              {{"type": "barcode", "id": 2, {}, "symbology": "code128", "data": "12", "module_dots": 2, "bar_dots": 0}},
              {{"type": "qr", "id": 3, {}, "data": "x", "module_dots": 2}},
              {{"type": "data_matrix", "id": 4, {}, "data": "x", "module_dots": 2}},
              {{"type": "image", "id": 5, {}, "path": "logo.png"}},
              {{"type": "rect", "id": 6, {}, "stroke_dots": 1, "radius_dots": 2}},
              {{"type": "line", "id": 7, {}}}
            ]}}"#,
        frame(4),
        frame(20),
        frame(36),
        frame(52),
        frame(68),
        frame(84),
        frame(100)
    );
    Label::from_json(&json).expect("a label of every kind")
}

/// The roles an inspector field publishes, one per shape.
fn is_field_role(h: &ElementHandle) -> bool {
    matches!(
        h.accessible_role(),
        Some(AccessibleRole::Spinbox | AccessibleRole::TextInput | AccessibleRole::Text)
    )
}

/// The labels of the three text nodes that are not fields: the status line, the
/// pane's heading and the zoom readout.
const NOT_FIELDS: [&str; 3] = ["Status", "Selected element", "Zoom"];

/// One inspector field, by the name it announces, or `None` when it is not in
/// the accessibility tree at all.
///
/// Matched on role as well as label, and required to be at most one: a widget
/// that publishes its label on two nodes would make a bare label lookup return
/// both - the trap `rows_of` documents for repeated rows - and a test taking the
/// first would drive whichever node happened to come first.
pub fn find_field(probe: &impl slint::ComponentHandle, label: &str) -> Option<ElementHandle> {
    let wanted = label.to_string();
    let mut found = ElementQuery::from_root(probe)
        .match_predicate(move |h| {
            is_field_role(h) && h.accessible_label().is_some_and(|l| l == wanted.as_str())
        })
        .find_all();
    assert!(
        found.len() <= 1,
        "{} nodes announce themselves as {label:?}; a field is one node",
        found.len()
    );
    found.pop()
}

/// One inspector field, which must be there.
pub fn field_of(probe: &impl slint::ComponentHandle, label: &str) -> ElementHandle {
    find_field(probe, label).unwrap_or_else(|| {
        panic!(
            "no field announces itself as {label:?}; if none at all is found the probe was \
             compiled without slint debug info"
        )
    })
}

/// Every inspector field in the tree, in tree order, which is pane and Tab order.
pub fn fields_of(probe: &impl slint::ComponentHandle) -> Vec<ElementHandle> {
    ElementQuery::from_root(probe)
        .match_predicate(|h| {
            is_field_role(h)
                && !h
                    .accessible_label()
                    .is_some_and(|l| NOT_FIELDS.contains(&l.as_str()))
        })
        .find_all()
}

/// The status line.
pub fn status_of(probe: &impl slint::ComponentHandle) -> ElementHandle {
    ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Status"))
        .find_first()
        .expect("the status line")
}

/// A key pressed and released, with Shift or Control held around it: a named
/// [`Key`], or the character a keyboard produces with those modifiers, such as `"="`
/// or `"_"`.
pub fn press(
    w: &slint::Window,
    k: impl Into<slint::SharedString> + Copy,
    shift: bool,
    control: bool,
) {
    let held = modifiers(shift, control);
    for h in &held {
        w.dispatch_event(WindowEvent::KeyPressed { text: (*h).into() });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: k.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: k.into() });
    for h in held.iter().rev() {
        w.dispatch_event(WindowEvent::KeyReleased { text: (*h).into() });
    }
}

fn modifiers(shift: bool, control: bool) -> Vec<Key> {
    [(shift, Key::Shift), (control, Key::Control)]
        .into_iter()
        .filter_map(|(on, k)| on.then_some(k))
        .collect()
}

/// A wheel or trackpad scroll at `position`, in logical pixels of travel, with
/// Shift or Control held around it. The toolkit reads the modifiers from the
/// window's key state rather than from the event, so they are pressed first.
pub fn wheel(
    w: &slint::Window,
    position: slint::LogicalPosition,
    dx: f32,
    dy: f32,
    shift: bool,
    control: bool,
) {
    let held = modifiers(shift, control);
    for h in &held {
        w.dispatch_event(WindowEvent::KeyPressed { text: (*h).into() });
    }
    w.dispatch_event(WindowEvent::PointerScrolled {
        position,
        delta_x: dx,
        delta_y: dy,
    });
    for h in held.iter().rev() {
        w.dispatch_event(WindowEvent::KeyReleased { text: (*h).into() });
    }
}

/// The primary button going down at `position`, after the pointer arrived there.
pub fn down(w: &slint::Window, position: slint::LogicalPosition) {
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
}

/// The pointer moving to `position`, button state unchanged.
pub fn move_to(w: &slint::Window, position: slint::LogicalPosition) {
    w.dispatch_event(WindowEvent::PointerMoved { position });
}

/// The primary button coming up at `position`.
pub fn up(w: &slint::Window, position: slint::LogicalPosition) {
    w.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
}

/// A drag on mock time: a press at `from`, a hold of `hold` milliseconds, then
/// moves of at most `step` logical pixels every `every` milliseconds until `by` is
/// covered, and the release where the last move left the pointer.
///
/// `ElementHandle::mock_drag` moves at a fixed pace from the press on
/// (search_api.rs:17, :22), so it cannot say how long a pointer was held before it
/// moved or how fast it went, and both are what a Flickable's thresholds read.
pub fn drag_timed(
    w: &slint::Window,
    from: slint::LogicalPosition,
    by: (f32, f32),
    step: f32,
    every: u64,
    hold: u64,
) {
    down(w, from);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(hold));
    let steps = ((by.0.hypot(by.1)) / step).ceil().max(1.0) as u32;
    let mut at = from;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        at = slint::LogicalPosition::new(from.x + by.0 * t, from.y + by.1 * t);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(every));
        move_to(w, at);
    }
    up(w, at);
}

/// Text typed one character at a time, as a person types it.
pub fn type_text(w: &slint::Window, s: &str) {
    for ch in s.chars() {
        w.dispatch_event(WindowEvent::KeyPressed { text: ch.into() });
        w.dispatch_event(WindowEvent::KeyReleased { text: ch.into() });
    }
}

/// The editor inside a field, which is what a pointer clicks and a keyboard
/// types into; the field's own node is the wrapper round it.
pub fn editor_of(field: &ElementHandle) -> ElementHandle {
    field
        .query_descendants()
        .match_inherits("TextInput")
        .find_first()
        .expect("a field with an editor in it")
}

/// A click at the centre of an element, as a window receives one from a pointer
/// that did not move while the button was down: a move there, a press, a release.
///
/// Not `ElementHandle::mock_single_click`, which advances mock time between the
/// press and the release and then moves the pointer to the same spot again
/// (search_api.rs:929-950). A TextInput that is pressed reads any move as a
/// drag-selection from where it was pressed, so that second move replaces the
/// selection a number field makes on arrival with an empty one at the click - a
/// field that selected itself correctly would type 378 where an operator asked
/// for 37, and the test would blame the field.
pub fn click(probe: &impl slint::ComponentHandle, h: &ElementHandle) {
    let (at, size) = (h.absolute_position(), h.size());
    let position = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    let w = probe.window();
    let button = slint::platform::PointerEventButton::Left;
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed { position, button });
    w.dispatch_event(WindowEvent::PointerReleased { position, button });
}
