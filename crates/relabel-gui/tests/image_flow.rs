//! Adding an image from a file and importing an image's artwork again, driven
//! through the probe window as an operator and an assistive technology drive them:
//! the edit bar's Image tool, Control+Shift+I and the inspector's artwork button.
//!
//! Every probe is wired by `relabel_gui::wire_window`, the one wiring the
//! application runs, with a scripted or held file dialog in place of the
//! platform's, and an importer that reads and decodes on the test's own thread,
//! at once or when the test lets it. The worker thread the application uses is
//! `import_native`'s. Every file a test writes is under the build's scratch space.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use relabel_gui::choose::{Ask, Chooser, Picked};
use relabel_gui::import::Importer;
use relabel_gui::ins::Field;
use relabel_gui::view::Opening;
use relabel_gui::{start, wire_window};
use relabel_render::artwork::{Artwork, MAX_INPUT};
use relabel_render::document::{Element, ElementId, ElementOrRaw, Label};
use slint::ComponentHandle;
use slint::platform::Key;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use support::{
    Held, HeldImporter, HeldPlug, InlineImporter, Scripted, action_of, button_of, down,
    every_kind_label, frame_of, group_of, press, reference_label, rows_of, tmp_dir, type_text, up,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);
relabel_gui::impl_print_host!(InsProbe, IssueRow, ChoiceRow);

type Wired = relabel_gui::Wired<InsProbe>;

const IMPORT: &str = "Import\u{2026}";
const IMPORT_AGAIN: &str = "Import again\u{2026}";

/// The probe at scale 1 on the label at `path`, wired as the application wires it
/// with this dialog and this importer, shown, with the change handlers the first
/// layout queued already run.
fn probe(path: &Path, chooser: Rc<dyn Chooser>, importer: Rc<dyn Importer>) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let plug = Rc::new(HeldPlug::default());
    let wired = wire_window(
        &ui,
        start(Some(path)),
        chooser,
        plug,
        importer,
        Opening::Fit,
    );
    ui.show().expect("showing a headless window");
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    (ui, wired)
}

/// A scratch directory holding `label` as `a.relabel` and a 32 by 32 `logo.png`,
/// and the paths of the directory, the label and the logo.
fn with_files(test: &str, label: &Label) -> (PathBuf, PathBuf, PathBuf) {
    let dir = tmp_dir(&format!("image_flow/{test}"));
    let a = dir.join("a.relabel");
    fs::write(&a, label.to_json().expect("the label saves")).expect("writing the label");
    let logo = dir.join("logo.png");
    let art = Artwork {
        w: 32,
        h: 32,
        px: (0..32 * 32)
            .map(|i| if (i / 32 + i % 32) % 4 == 0 { 0 } else { 255 })
            .collect(),
    };
    fs::write(&logo, art.to_png().expect("encoded")).expect("writing the logo");
    (dir, a, logo)
}

/// The label of every kind, with element 5 - the image, which has no artwork -
/// locked when asked.
fn every_kind(locked: bool) -> Label {
    let mut label = every_kind_label();
    for e in &mut label.elements {
        if let ElementOrRaw::Known(Element::Image(i)) = e {
            i.common.locked = locked;
        }
    }
    label
}

/// Control+Shift+I, as a keyboard sends it.
fn chord(ui: &InsProbe) {
    press(ui.window(), "I", true, true);
}

fn ctrl(ui: &InsProbe, k: &str) {
    press(ui.window(), k, false, true);
}

fn key(ui: &InsProbe, k: Key) {
    press(ui.window(), k, false, false);
}

fn count(wired: &Wired) -> usize {
    wired.canvas.borrow().doc().label().elements.len()
}

/// Element `id`, when the label has it.
fn element(wired: &Wired, id: ElementId) -> Option<Element> {
    wired
        .canvas
        .borrow()
        .doc()
        .label()
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .find(|e| e.common().id == id)
        .cloned()
}

/// Element `id`, which must be an image.
fn image(wired: &Wired, id: ElementId) -> relabel_render::document::ImageEl {
    wired
        .canvas
        .borrow()
        .doc()
        .label()
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .find_map(|e| match e {
            Element::Image(i) if i.common.id == id => Some(i.clone()),
            _ => None,
        })
        .expect("that image is in the label")
}

/// The id of the last element in the label, which a new one is.
fn last_id(wired: &Wired) -> ElementId {
    wired
        .canvas
        .borrow()
        .doc()
        .label()
        .elements
        .last()
        .and_then(ElementOrRaw::known)
        .map(|e| e.common().id)
        .expect("a known element last")
}

/// The centre of element 1's row, where a press holds it.
fn first_row(ui: &InsProbe) -> slint::LogicalPosition {
    let row = &rows_of(ui)[0];
    let (at, size) = (row.absolute_position(), row.size());
    slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
}

/// Select element 5 and put the keyboard on its artwork button: F6 into its
/// name, then Tab past feed, head, both lengths, rotation, ink and source. The
/// state row is only read, so it is no stop.
fn onto_artwork(ui: &InsProbe) {
    ui.invoke_select(5);
    key(ui, Key::F6);
    for _ in 0..8 {
        key(ui, Key::Tab);
    }
}

fn space(ui: &InsProbe) {
    key(ui, Key::Space);
}

#[test]
fn control_shift_i_adds_the_chosen_png_and_hands_the_canvas_the_keyboard() {
    let (dir, a, logo) = with_files("chord", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let (ui, wired) = probe(&a, chooser.clone(), Rc::new(InlineImporter));
    let before = count(&wired);
    chord(&ui);
    assert_eq!(
        chooser.asked(),
        [Ask::Artwork {
            folder: Some(dir),
            again: None
        }]
    );
    assert_eq!(count(&wired), before + 1);
    let id = last_id(&wired);
    let added = image(&wired, id);
    assert!(!added.png.is_empty());
    assert_eq!(added.path, "logo.png");
    assert_eq!(wired.canvas.borrow().doc().selected(), Some(id));
    assert!(
        ui.get_status().starts_with("added Image element"),
        "{}",
        ui.get_status()
    );
    // The keyboard is on the canvas, where an arrow moves the new image.
    let feed = frame_of(wired.canvas.borrow().doc().label(), id).feed;
    key(&ui, Key::RightArrow);
    assert_eq!(
        frame_of(wired.canvas.borrow().doc().label(), id).feed,
        feed + 1
    );
    ctrl(&ui, "z");
    ctrl(&ui, "z");
    assert_eq!(count(&wired), before);
    assert_eq!(
        ui.get_status(),
        format!("undo took away Image element {id}")
    );
}

#[test]
fn the_image_button_is_the_same_route_for_an_assistive_technology() {
    let (dir, a, logo) = with_files("button", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let (ui, wired) = probe(&a, chooser.clone(), Rc::new(InlineImporter));
    let before = count(&wired);
    let button = button_of(&ui, "Add image");
    assert_eq!(
        button.accessible_description().as_deref(),
        Some("Control+Shift+I")
    );
    button.invoke_accessible_default_action();
    assert_eq!(
        chooser.asked(),
        [Ask::Artwork {
            folder: Some(dir),
            again: None
        }]
    );
    assert_eq!(count(&wired), before + 1);
    let added = image(&wired, last_id(&wired));
    assert_eq!(added.path, "logo.png");
    assert!(!added.png.is_empty());
}

#[test]
fn text_typed_into_a_field_lands_before_the_image() {
    let (_dir, a, logo) = with_files("typed", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let (ui, wired) = probe(&a, chooser, Rc::new(InlineImporter));
    let before = count(&wired);
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    type_text(ui.window(), "x");
    chord(&ui);
    let name = |wired: &Wired| element(wired, 1).map(|e| e.common().name.clone());
    assert_eq!(name(&wired).as_deref(), Some("x"));
    assert_eq!(count(&wired), before + 1);
    // The chord committed the name, so nothing is staged and the new image is
    // selected with the keyboard on the canvas, where an arrow moves it.
    let id = last_id(&wired);
    assert_eq!(wired.canvas.borrow().doc().selected(), Some(id));
    let feed = feed_of(&wired, id);
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, id), feed + 1, "the canvas has the keyboard");
    ctrl(&ui, "z");
    ctrl(&ui, "z");
    assert_eq!(count(&wired), before, "the image is the last step");
    assert_eq!(name(&wired).as_deref(), Some("x"));
    ctrl(&ui, "z");
    assert_eq!(name(&wired).as_deref(), Some("box 1"));
}

/// A field that was left with Return holds nothing, so an import that lands while
/// the keyboard is still in it selects the new image and hands the canvas the
/// keyboard, as adding any other element does: left in a field of a pane that
/// now shows the image, it would type into a row the operator did not choose.
#[test]
fn an_import_that_lands_after_a_field_was_left_with_return_hands_the_canvas_the_keyboard() {
    let (_dir, a, logo) = with_files("after_return", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser, importer.clone());
    let before = count(&wired);
    chord(&ui);
    key(&ui, Key::Tab);
    onto_feed(&ui);
    type_text(ui.window(), "20");
    key(&ui, Key::Return);
    assert_eq!(feed_of(&wired, 1), 20);
    assert_eq!(staged(&wired), None);
    importer.release();
    assert_eq!(count(&wired), before + 1);
    let id = last_id(&wired);
    assert_eq!(wired.canvas.borrow().doc().selected(), Some(id));
    let feed = feed_of(&wired, id);
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, id), feed + 1, "the canvas has the keyboard");
    assert_eq!(feed_of(&wired, 1), 20, "and the field is left alone");
}

/// A pointer on the edit bar's Image button, after a visit to a field, is the
/// same add as the chord from the canvas: the new image has the keyboard.
#[test]
fn an_image_added_by_the_pointer_after_a_field_visit_has_the_keyboard() {
    let (_dir, a, logo) = with_files("pointer_after_field", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser, importer.clone());
    let before = count(&wired);
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    type_text(ui.window(), "x");
    key(&ui, Key::Return);
    support::click(&ui, &button_of(&ui, "Add image"));
    assert!(importer.holds());
    importer.release();
    assert_eq!(count(&wired), before + 1);
    let id = last_id(&wired);
    assert_eq!(wired.canvas.borrow().doc().selected(), Some(id));
    let feed = feed_of(&wired, id);
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, id), feed + 1, "the canvas has the keyboard");
}

#[test]
fn a_file_chosen_mid_drag_is_dropped_and_named() {
    let (_dir, a, logo) = with_files("held_chosen", &reference_label());
    let held = Rc::new(Held::default());
    let (ui, wired) = probe(&a, held.clone(), Rc::new(InlineImporter));
    let (before, feed) = (
        count(&wired),
        frame_of(wired.canvas.borrow().doc().label(), 1).feed,
    );
    chord(&ui);
    assert_eq!(held.asked().len(), 1);
    let centre = first_row(&ui);
    let moved = slint::LogicalPosition::new(centre.x + 24.0, centre.y);
    let w = ui.window();
    down(w, centre);
    support::move_to(w, moved);
    held.release(Picked::Path(logo));
    assert_eq!(
        ui.get_status(),
        "nothing was added: logo.png was chosen while an element was held; let go, then \
         choose Image again"
    );
    up(w, moved);
    assert_eq!(count(&wired), before);
    assert_ne!(frame_of(wired.canvas.borrow().doc().label(), 1).feed, feed);
    ctrl(&ui, "z");
    assert_eq!(
        frame_of(wired.canvas.borrow().doc().label(), 1).feed,
        feed,
        "the drag was one undo step"
    );
}

#[test]
fn the_window_goes_on_answering_while_an_import_runs() {
    let (_dir, a, logo) = with_files("running", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser.clone(), importer.clone());
    let before = count(&wired);
    chord(&ui);
    assert!(importer.holds());
    assert_eq!(ui.get_status(), "importing logo.png");
    let feed = frame_of(wired.canvas.borrow().doc().label(), 1).feed;
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    assert_eq!(
        frame_of(wired.canvas.borrow().doc().label(), 1).feed,
        feed + 1
    );
    chord(&ui);
    assert_eq!(
        ui.get_status(),
        "logo.png is still being imported; nothing else was started"
    );
    assert_eq!(chooser.asked().len(), 1);
    ctrl(&ui, "s");
    let saved = Label::from_json(&fs::read_to_string(&a).expect("read")).expect("a label");
    assert_eq!(saved.elements.len(), before, "saved without the image");
    assert_eq!(frame_of(&saved, 1).feed, feed + 1);
    assert!(!ui.get_doc_title().starts_with('*'));
    importer.release();
    assert_eq!(count(&wired), before + 1);
    assert!(
        ui.get_doc_title().starts_with('*'),
        "{}",
        ui.get_doc_title()
    );
    ctrl(&ui, "z");
    assert_eq!(count(&wired), before, "the image is taken back first");
    assert_eq!(
        frame_of(wired.canvas.borrow().doc().label(), 1).feed,
        feed + 1
    );
    ctrl(&ui, "z");
    assert_eq!(frame_of(wired.canvas.borrow().doc().label(), 1).feed, feed);
}

/// Where the keyboard is in the inspector: F6 from the canvas into the selected
/// element's name, then Tab onto its feed.
fn onto_feed(ui: &InsProbe) {
    key(ui, Key::F6);
    key(ui, Key::Tab);
}

fn feed_of(wired: &Wired, id: ElementId) -> i32 {
    frame_of(wired.canvas.borrow().doc().label(), id).feed
}

fn staged(wired: &Wired) -> Option<(ElementId, Field, String)> {
    wired
        .canvas
        .borrow()
        .doc()
        .staged()
        .map(|(id, f, t)| (id, f, t.to_string()))
}

/// An answer comes from the importer, not from the operator, so it finishes
/// nothing they were typing: the half number stays in its field and out of the
/// label, the new image leaves the selection and the keyboard where they were,
/// and the rest of the number lands where it was going, as a step of its own.
#[test]
fn an_import_that_lands_while_a_number_is_typed_leaves_it_half_typed() {
    let (_dir, a, logo) = with_files("typing", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser, importer.clone());
    let (before, feed) = (count(&wired), feed_of(&wired, 1));
    chord(&ui);
    key(&ui, Key::Tab);
    onto_feed(&ui);
    type_text(ui.window(), "1");
    assert_eq!(staged(&wired), Some((1, Field::Feed, "1".into())));
    importer.release();
    assert_eq!(count(&wired), before + 1);
    assert_eq!(feed_of(&wired, 1), feed, "nothing half typed was written");
    assert_eq!(staged(&wired), Some((1, Field::Feed, "1".into())));
    assert_eq!(wired.canvas.borrow().doc().selected(), Some(1));
    let said = ui.get_status().to_string();
    assert!(
        said.starts_with("added Image element") && said.contains(" is not selected, since"),
        "{said}"
    );
    type_text(ui.window(), "20");
    key(&ui, Key::Return);
    assert_eq!(feed_of(&wired, 1), 120, "the keyboard stayed in the field");
    ctrl(&ui, "z");
    assert_eq!(
        (feed_of(&wired, 1), count(&wired)),
        (feed, before + 1),
        "the number was one step"
    );
    ctrl(&ui, "z");
    assert_eq!(count(&wired), before);
}

/// An import let go by an Open answers quietly, and writes nothing that is being
/// typed into the label opened in its place.
#[test]
fn a_let_go_answer_writes_nothing_that_is_being_typed() {
    let (dir, a, logo) = with_files("let_go_typing", &reference_label());
    let b = dir.join("b.relabel");
    let mut b_label = reference_label();
    b_label.elements.truncate(1);
    fs::write(&b, b_label.to_json().expect("saves")).expect("writing b");
    let chooser = Rc::new(Scripted::new([Picked::Path(logo), Picked::Path(b)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser, importer.clone());
    chord(&ui);
    ctrl(&ui, "o");
    let feed = feed_of(&wired, 1);
    key(&ui, Key::Tab);
    onto_feed(&ui);
    type_text(ui.window(), "1");
    let status = ui.get_status();
    importer.release();
    assert_eq!(ui.get_status(), status, "nothing was said");
    assert_eq!(feed_of(&wired, 1), feed, "nothing half typed was written");
    assert_eq!(staged(&wired), Some((1, Field::Feed, "1".into())));
    type_text(ui.window(), "20");
    key(&ui, Key::Return);
    assert_eq!(feed_of(&wired, 1), 120);
    ctrl(&ui, "z");
    assert_eq!(feed_of(&wired, 1), feed, "one undo takes the number back");
}

#[test]
fn an_import_that_lands_while_an_element_is_held_is_dropped() {
    let (_dir, a, logo) = with_files("held_read", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser, importer.clone());
    let (before, feed) = (
        count(&wired),
        frame_of(wired.canvas.borrow().doc().label(), 1).feed,
    );
    chord(&ui);
    let centre = first_row(&ui);
    let moved = slint::LogicalPosition::new(centre.x + 24.0, centre.y);
    let w = ui.window();
    down(w, centre);
    support::move_to(w, moved);
    importer.release();
    assert_eq!(
        ui.get_status(),
        "nothing was added: logo.png was read while an element was held; let go, then \
         choose Image again"
    );
    assert_eq!(count(&wired), before);
    up(w, moved);
    assert!(!wired.canvas.borrow().doc().pressed());
    assert_ne!(frame_of(wired.canvas.borrow().doc().label(), 1).feed, feed);
    assert_eq!(count(&wired), before);
}

#[test]
fn an_import_that_lands_behind_the_print_pane_leaves_the_keyboard_in_the_pane() {
    let (_dir, a, logo) = with_files("behind_pane", &reference_label());
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser, importer.clone());
    let before = count(&wired);
    chord(&ui);
    ctrl(&ui, "p");
    assert!(group_of(&ui, "Print").is_some());
    importer.release();
    assert_eq!(count(&wired), before + 1);
    assert!(group_of(&ui, "Print").is_some(), "the pane is still open");
    key(&ui, Key::Escape);
    assert!(
        group_of(&ui, "Print").is_none(),
        "Escape reached the pane, so the keyboard stayed in it"
    );
}

#[test]
fn opening_a_label_during_an_import_lets_it_go() {
    let (dir, a, logo) = with_files("let_go", &reference_label());
    let b = dir.join("b.relabel");
    let mut b_label = reference_label();
    b_label.elements.truncate(1);
    fs::write(&b, b_label.to_json().expect("saves")).expect("writing b");
    let chooser = Rc::new(Scripted::new([Picked::Path(logo), Picked::Path(b)]));
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, chooser.clone(), importer.clone());
    chord(&ui);
    assert!(importer.holds());
    ctrl(&ui, "o");
    let said = ui.get_status().to_string();
    assert!(said.starts_with("opened b.relabel"), "{said}");
    assert!(
        said.ends_with("; the import of logo.png was let go"),
        "{said}"
    );
    chord(&ui);
    assert_eq!(
        ui.get_status(),
        "logo.png is still being imported; nothing else was started"
    );
    let status = ui.get_status();
    importer.release();
    assert_eq!(count(&wired), 1, "nothing was added to b");
    assert_eq!(ui.get_status(), status, "and nothing was said");
    let asked = chooser.asked().len();
    chord(&ui);
    assert_eq!(chooser.asked().len(), asked + 1);
    assert!(matches!(
        chooser.asked().last(),
        Some(Ask::Artwork { again: None, .. })
    ));
}

#[test]
fn import_again_keeps_the_keyboard_on_its_button_and_mends_missing_artwork() {
    let (_dir, a, logo) = with_files("again", &every_kind(false));
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let (ui, wired) = probe(&a, chooser.clone(), Rc::new(InlineImporter));
    onto_artwork(&ui);
    action_of(&ui, IMPORT);
    space(&ui);
    assert!(matches!(
        chooser.asked().as_slice(),
        [Ask::Artwork { again: Some(e), .. }] if e == "Image element 5"
    ));
    let said = ui.get_status().to_string();
    assert!(
        said.starts_with("imported logo.png into Image element 5"),
        "{said}"
    );
    assert!(said.contains("Image element 5 is drawn again"), "{said}");
    assert!(!image(&wired, 5).png.is_empty());
    assert_eq!(image(&wired, 5).path, "logo.png");
    // Imported for its 12 by 12 frame, so drawn as imported.
    let button = action_of(&ui, IMPORT_AGAIN);
    let told = button.accessible_description().unwrap_or_default();
    assert!(
        told.starts_with("12 by 12 dots") && told.ends_with("; drawn as imported"),
        "{told}"
    );
    // The keyboard stayed on the button: a second Space asks again.
    space(&ui);
    assert_eq!(chooser.asked().len(), 2);
    ctrl(&ui, "p");
    let rows = wired.prints.proof().expect("proved").issue_rows();
    assert!(
        !rows.iter().any(|r| r.contains("no embedded artwork")),
        "{rows:?}"
    );
}

#[test]
fn a_target_deleted_behind_the_dialog_is_said_and_nothing_is_read() {
    let (dir, a, logo) = with_files("deleted", &every_kind(false));
    let held = Rc::new(Held::default());
    let importer = Rc::new(HeldImporter::default());
    let (ui, wired) = probe(&a, held.clone(), importer.clone());
    ui.invoke_select(5);
    action_of(&ui, IMPORT).invoke_accessible_default_action();
    assert_eq!(
        held.asked(),
        [Ask::Artwork {
            folder: Some(dir),
            again: Some("Image element 5".into())
        }]
    );
    ui.invoke_delete_element(5);
    assert!(element(&wired, 5).is_none());
    held.release(Picked::Path(logo));
    assert_eq!(
        ui.get_status(),
        "could not import logo.png: element 5 is gone; nothing was imported"
    );
    assert!(importer.jobs().is_empty());
}

#[test]
fn a_locked_image_announces_its_button_unavailable_and_asks_nothing() {
    let (_dir, a, logo) = with_files("locked", &every_kind(true));
    let chooser = Rc::new(Scripted::new([Picked::Path(logo)]));
    let (ui, wired) = probe(&a, chooser.clone(), Rc::new(InlineImporter));
    let before = wired.canvas.borrow().doc().label().clone();
    onto_artwork(&ui);
    let button = action_of(&ui, IMPORT);
    assert_eq!(button.accessible_enabled(), Some(false));
    // Tab reached it: Space is answered, with the refusal.
    space(&ui);
    assert!(chooser.asked().is_empty());
    assert_eq!(
        ui.get_status(),
        "Image element 5 is locked; nothing was imported"
    );
    ui.set_status("".into());
    button.invoke_accessible_default_action();
    assert!(chooser.asked().is_empty());
    assert_eq!(
        ui.get_status(),
        "Image element 5 is locked; nothing was imported"
    );
    assert_eq!(wired.canvas.borrow().doc().label(), &before);
}

#[test]
fn delete_and_arrows_on_the_import_button_move_and_delete_nothing() {
    let (_dir, a, _logo) = with_files("keys", &every_kind(false));
    let chooser = Rc::new(Scripted::default());
    let (ui, wired) = probe(&a, chooser.clone(), Rc::new(InlineImporter));
    let before = count(&wired);
    onto_artwork(&ui);
    let frame = frame_of(wired.canvas.borrow().doc().label(), 5);
    for k in [Key::Delete, Key::Backspace, Key::RightArrow] {
        key(&ui, k);
        assert_eq!(count(&wired), before, "{k:?}");
        assert_eq!(
            frame_of(wired.canvas.borrow().doc().label(), 5),
            frame,
            "{k:?}"
        );
    }
    // The keyboard was on the button all along, and is still: Space asks.
    space(&ui);
    assert_eq!(chooser.asked().len(), 1);
    key(&ui, Key::F6);
    key(&ui, Key::RightArrow);
    assert_eq!(
        frame_of(wired.canvas.borrow().doc().label(), 5).feed,
        frame.feed + 1
    );
}

#[test]
fn hostile_files_are_refused_and_the_window_goes_on() {
    let (dir, a, logo) = with_files("hostile", &reference_label());
    let empty = dir.join("empty.png");
    fs::write(&empty, b"").expect("written");
    let huge = dir.join("huge.png");
    // Sparse: no block of it is written.
    fs::File::create(&huge)
        .and_then(|f| f.set_len(MAX_INPUT as u64 + 1))
        .expect("a sparse file");
    let folder = dir.join("folder.png");
    fs::create_dir(&folder).expect("a folder");
    let page = dir.join("page.png");
    fs::write(
        &page,
        b"<!DOCTYPE html><html><body>not a picture</body></html>",
    )
    .expect("written");
    let cut = dir.join("cut.png");
    fs::write(&cut, &fs::read(&logo).expect("the logo")[..60]).expect("written");
    let text = dir.join("text.svg");
    fs::write(
        &text,
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="5mm"><text x="1" y="4">hi</text></svg>"#,
    )
    .expect("written");
    // A JPEG's start and its JFIF header, and nothing of its picture.
    let photo = dir.join("photo.jpg");
    fs::write(
        &photo,
        b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00",
    )
    .expect("written");
    let archive = dir.join("archive.png");
    fs::write(&archive, [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3]).expect("written");
    let gone = dir.join("gone.png");
    let cases = [
        (&empty, "the file is empty"),
        (&huge, "limited to 64 MiB"),
        (&folder, "it is not a regular file"),
        (&page, "starts like a web page"),
        (&cut, "could not be decoded"),
        (&text, "convert text to outlines"),
        (&photo, "the JPEG has no end marker"),
        (&archive, "it is compressed data"),
        (&gone, "it does not exist"),
    ];
    let chooser = Rc::new(Scripted::new(
        cases.iter().map(|(p, _)| Picked::Path((*p).clone())),
    ));
    let (ui, wired) = probe(&a, chooser, Rc::new(InlineImporter));
    let before = count(&wired);
    for (path, why) in cases {
        chord(&ui);
        let name = path.file_name().expect("a name").to_string_lossy();
        let said = ui.get_status().to_string();
        assert!(
            said.starts_with(&format!("could not import {name}: ")),
            "{said}"
        );
        assert!(said.contains(why), "{said}");
        assert!(said.ends_with("; nothing was added"), "{said}");
        assert_eq!(count(&wired), before, "{name}");
    }
    press(ui.window(), "T", true, true);
    assert_eq!(count(&wired), before + 1, "the window goes on");
}

#[test]
fn the_pane_is_one_width_whatever_is_selected() {
    let (_dir, a, _logo) = with_files("width", &every_kind(false));
    let (ui, _wired) = probe(&a, Rc::new(Scripted::default()), Rc::new(InlineImporter));
    let width = |id: i32| {
        ui.invoke_select(id);
        let pane: ElementHandle = group_of(&ui, "Element").expect("the pane");
        assert_eq!(pane.accessible_role(), Some(AccessibleRole::Groupbox));
        pane.size().width
    };
    let text = width(1);
    let image = width(5);
    assert_eq!(text, image);
    action_of(&ui, IMPORT);
}
