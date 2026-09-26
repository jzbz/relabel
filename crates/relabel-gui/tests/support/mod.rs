//! Labels and handles the canvas tests share. Nothing here touches the
//! toolkit except the element-tree helpers, each about one trap, the two
//! choosers the file tests stand in for a file dialog, the two importers they
//! stand in for the worker thread, and the two plugs the file and print tests
//! stand in for the printer link.

// Shared by several test binaries, each of which uses part of it.
#![allow(dead_code)]

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use relabel_core::link::mock::{MockLink, MockOpener};
use relabel_core::link::{Events, Finished, Heard, Link, Order};
use relabel_gui::choose::{Answer, Ask, Chooser, NoDialog, Picked};
use relabel_gui::import::{self, Done, Importer, Job, NoWorker};
use relabel_gui::plug::{Deliver, News, Plug};
use relabel_render::document::{
    Common, Element, ElementId, ElementOrRaw, Label, RectDots, RectEl, Rot,
};
use relabel_render::target::Dpmm;
use slint::platform::{Key, WindowEvent};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// The labels of the four text nodes that are not fields: the status line, the
/// pane's heading, the zoom readout and the Printer line beside the File bar.
const NOT_FIELDS: [&str; 4] = ["Status", "Selected element", "Zoom", "Printer"];

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

/// Every inspector field in the tree, in tree order, which is pane and Tab order:
/// the three roles a value has, and the buttons an action slot is, which are the
/// only buttons inside the pane. Empty when the pane is not shown.
pub fn fields_of(probe: &impl slint::ComponentHandle) -> Vec<ElementHandle> {
    let Some(pane) = group_of(probe, "Element") else {
        return Vec::new();
    };
    pane.query_descendants()
        .match_predicate(|h| {
            (is_field_role(h) || h.accessible_role() == Some(AccessibleRole::Button))
                && !h
                    .accessible_label()
                    .is_some_and(|l| NOT_FIELDS.contains(&l.as_str()))
        })
        .find_all()
}

/// An action slot's button in the inspector, by its face, which is the name it
/// announces. Only inside the pane, so an edit bar button that happened to share
/// the face could not be taken for it; and at most one, for [`find_field`]'s
/// reason.
pub fn action_of(probe: &impl slint::ComponentHandle, face: &str) -> ElementHandle {
    let pane = group_of(probe, "Element").expect("the pane, shown with a selection");
    let wanted = face.to_string();
    let mut found = pane
        .query_descendants()
        .match_accessible_role(AccessibleRole::Button)
        .match_predicate(move |h| h.accessible_label().is_some_and(|l| l == wanted.as_str()))
        .find_all();
    assert!(
        found.len() <= 1,
        "{} buttons in the pane announce themselves as {face:?}",
        found.len()
    );
    found
        .pop()
        .unwrap_or_else(|| panic!("no button in the pane is called {face:?}"))
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

/// The ids of the label's elements in z-order, known and unknown alike, as numbers:
/// an unknown element's id is read from its JSON, and one that has none is left out.
pub fn ids_of(label: &Label) -> Vec<ElementId> {
    label
        .elements
        .iter()
        .filter_map(|e| match e {
            ElementOrRaw::Known(k) => Some(k.common().id),
            ElementOrRaw::Raw(v) => v
                .get("id")
                .and_then(|i| i.as_u64())
                .and_then(|i| i.try_into().ok()),
        })
        .collect()
}

/// The file format's name for one element's kind: "text", "rect" and so on.
pub fn kind_of(label: &Label, id: ElementId) -> &str {
    label
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .find(|e| e.common().id == id)
        .map(Element::kind)
        .expect("the element is in the label")
}

/// One of the selected element's grips, by its name in canvas.slint: "grip-e",
/// "grip-s" or "grip-se". They have no accessibility node, so they are found by
/// element id, which is there only with debug info.
pub fn grip_of(probe: &impl slint::ComponentHandle, name: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(probe, &format!("LabelCanvas::{name}"))
        .next()
        .unwrap_or_else(|| {
            panic!("no grip is called {name:?}; the probe was compiled without debug info")
        })
}

/// A button, by the name it announces, or `None` when there is none: on a popup
/// that has closed, say. Only a match that has a description, for [`button_of`]'s
/// reason.
pub fn find_button(probe: &impl slint::ComponentHandle, name: &str) -> Option<ElementHandle> {
    let wanted = name.to_string();
    ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::Button)
        .match_predicate(move |h| {
            h.accessible_label().is_some_and(|l| l == wanted.as_str())
                && h.accessible_description().is_some()
        })
        .find_first()
}

/// A group box, by the name it announces - a popup, since Slint has no dialog
/// role - or `None` when it is not shown. By name, never the first group box: the
/// inspector pane is one too.
pub fn group_of(probe: &impl slint::ComponentHandle, label: &str) -> Option<ElementHandle> {
    let wanted = label.to_string();
    ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::Groupbox)
        .match_predicate(move |h| h.accessible_label().is_some_and(|l| l == wanted.as_str()))
        .find_first()
}

/// A directory of the test's own under the build's scratch space, emptied first
/// so a run never sees what an earlier one left. Never in the source tree.
pub fn tmp_dir(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clearing the test's directory");
    }
    std::fs::create_dir_all(&dir).expect("making the test's directory");
    dir
}

/// A file dialog that answers at once, from a queue, and remembers every question.
/// Once the queue is empty it answers as a cancelled dialog does.
#[derive(Default)]
pub struct Scripted {
    answers: RefCell<VecDeque<Picked>>,
    asked: RefCell<Vec<Ask>>,
}

impl Scripted {
    pub fn new(answers: impl IntoIterator<Item = Picked>) -> Scripted {
        Scripted {
            answers: RefCell::new(answers.into_iter().collect()),
            asked: RefCell::default(),
        }
    }

    /// The next answer, after any already queued.
    pub fn then(&self, picked: Picked) {
        self.answers.borrow_mut().push_back(picked);
    }

    pub fn asked(&self) -> Vec<Ask> {
        self.asked.borrow().clone()
    }
}

impl Chooser for Scripted {
    fn ask(&self, ask: Ask, answer: Answer) -> Result<(), NoDialog> {
        self.asked.borrow_mut().push(ask);
        // Taken before the answer runs, which asks again if the wiring asks again.
        let picked = self.answers.borrow_mut().pop_front();
        answer(picked.unwrap_or(Picked::Nothing));
        Ok(())
    }
}

/// A file dialog that stays open until the test answers it, as an operator's does
/// while they go on editing behind it.
#[derive(Default)]
pub struct Held {
    answer: RefCell<Option<Answer>>,
    asked: RefCell<Vec<Ask>>,
}

impl Held {
    pub fn asked(&self) -> Vec<Ask> {
        self.asked.borrow().clone()
    }

    /// Answer the dialog held open.
    pub fn release(&self, picked: Picked) {
        let answer = self.answer.borrow_mut().take().expect("a dialog is open");
        answer(picked);
    }
}

impl Chooser for Held {
    fn ask(&self, ask: Ask, answer: Answer) -> Result<(), NoDialog> {
        self.asked.borrow_mut().push(ask);
        *self.answer.borrow_mut() = Some(answer);
        Ok(())
    }
}

/// An importer that reads and decodes on the test's own thread and answers at once,
/// from inside `start`, as a test's chooser does: the application's own read and
/// decode, with no worker and no event loop.
pub struct InlineImporter;

impl Importer for InlineImporter {
    fn start(&self, job: Job, done: Done) -> Result<(), NoWorker> {
        done(import::run(&job).map(Box::new));
        Ok(())
    }
}

/// An importer whose job waits until the test lets it run, as a slow decode does
/// while the operator goes on working.
#[derive(Default)]
pub struct HeldImporter {
    held: RefCell<Option<(Job, Done)>>,
    jobs: RefCell<Vec<Job>>,
}

impl HeldImporter {
    /// Every job started so far, oldest first.
    pub fn jobs(&self) -> Vec<Job> {
        self.jobs.borrow().clone()
    }

    /// Whether a job is waiting to be let run.
    pub fn holds(&self) -> bool {
        self.held.borrow().is_some()
    }

    /// Run the job held, and deliver its answer.
    pub fn release(&self) {
        let (job, done) = self.held.borrow_mut().take().expect("a job is held");
        done(import::run(&job).map(Box::new));
    }
}

impl Importer for HeldImporter {
    fn start(&self, job: Job, done: Done) -> Result<(), NoWorker> {
        self.jobs.borrow_mut().push(job.clone());
        let already = self.held.borrow_mut().replace((job, done));
        assert!(already.is_none(), "one import at a time");
        Ok(())
    }
}

/// A toolbar button, by the name it announces.
///
/// Only a match that has a description, for `rows_of`'s trap in another shape: a
/// button repeated from a model is found once per source element merged into its
/// item, and only the first of those, `element_index` 0, answers any accessible
/// property but its role (search_api.rs:701). Every button here describes its chord.
pub fn button_of(probe: &impl slint::ComponentHandle, name: &str) -> ElementHandle {
    let wanted = name.to_string();
    ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::Button)
        .match_predicate(move |h| {
            h.accessible_label().is_some_and(|l| l == wanted.as_str())
                && h.accessible_description().is_some()
        })
        .find_first()
        .unwrap_or_else(|| panic!("no button is called {name:?}"))
}

/// A printer link that goes nowhere: it keeps every order, and the test says what
/// the link heard. Nothing is delivered unless the test delivers it, so no test
/// using it touches Bluetooth or a thread.
#[derive(Default)]
pub struct HeldPlug {
    orders: RefCell<Vec<(u64, Order)>>,
    deliver: RefCell<Option<Deliver>>,
    finished: Cell<bool>,
}

impl HeldPlug {
    /// Every order so far, with its ticket, oldest first.
    pub fn orders(&self) -> Vec<(u64, Order)> {
        self.orders.borrow().clone()
    }

    /// The link says `news`, as the application's loop would deliver it.
    pub fn deliver(&self, news: News) {
        let deliver = self
            .deliver
            .borrow()
            .clone()
            .expect("the wiring listens before anything is heard");
        deliver(news);
    }

    pub fn hear(&self, heard: Heard) {
        self.deliver(News::Heard(heard));
    }

    pub fn finished(&self) -> bool {
        self.finished.get()
    }
}

impl Plug for HeldPlug {
    fn listen(&self, deliver: Deliver) {
        *self.deliver.borrow_mut() = Some(deliver);
    }

    fn order(&self, ticket: u64, order: Order) {
        self.orders.borrow_mut().push((ticket, order));
    }

    fn finish(&self, _within: Duration) {
        self.finished.set(true);
    }
}

/// relabel-core's real link, on its own thread and runtime, over an in-memory
/// printer. The test thread has no event loop, so what the link says is
/// delivered by [`LivePlug::pump_until`] rather than by `slint::spawn_local`.
pub struct LivePlug {
    opener: RefCell<Option<MockOpener>>,
    /// The test's view of the printer the link opens.
    pub mock: MockLink,
    link: RefCell<Option<Link>>,
    events: RefCell<Option<Events>>,
    deliver: RefCell<Option<Deliver>>,
    finished: Cell<Option<Finished>>,
}

impl LivePlug {
    pub fn new((opener, mock): (MockOpener, MockLink)) -> LivePlug {
        LivePlug {
            opener: RefCell::new(Some(opener)),
            mock,
            link: RefCell::new(None),
            events: RefCell::new(None),
            deliver: RefCell::new(None),
            finished: Cell::new(None),
        }
    }

    /// Deliver what the link says until `until` holds, for at most `within` of
    /// real time. True when it held.
    pub fn pump_until(&self, within: Duration, until: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if until() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            // Released before delivering: the delivery can order again.
            let heard = self
                .events
                .borrow_mut()
                .as_mut()
                .and_then(|e| e.wait(Duration::from_millis(20)));
            match heard {
                Some(h) => {
                    let deliver = self.deliver.borrow().clone().expect("listening");
                    deliver(News::Heard(h));
                }
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    }

    /// How `finish` went, once it has run.
    pub fn finished(&self) -> Option<Finished> {
        self.finished.get()
    }
}

impl Plug for LivePlug {
    fn listen(&self, deliver: Deliver) {
        *self.deliver.borrow_mut() = Some(deliver);
    }

    fn order(&self, ticket: u64, order: Order) {
        if self.link.borrow().is_none() {
            let opener = self.opener.borrow_mut().take().expect("started once");
            let (link, events) = Link::start(Box::new(opener)).expect("the link starts");
            *self.link.borrow_mut() = Some(link);
            *self.events.borrow_mut() = Some(events);
        }
        let link = self.link.borrow();
        link.as_ref()
            .expect("started")
            .send(ticket, order)
            .expect("the link takes orders");
    }

    fn finish(&self, within: Duration) {
        if let Some(link) = self.link.borrow_mut().take() {
            self.finished.set(Some(link.shutdown(within)));
        }
    }
}
