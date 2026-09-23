//! New, open, save, save as and the one question about unsaved work, driven
//! through the probe window as an operator and an assistive technology drive
//! them.
//!
//! Every probe is wired by `relabel_gui::wire_window`, the one wiring the
//! application runs, with a scripted file dialog in place of the platform's.
//! Every file a test writes is under the build's scratch space.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleLiveness, AccessibleRole};
use relabel_gui::choose::{Ask, Chooser, NativeChooser, NoDialog, Picked};
use relabel_gui::view::{self, Opening};
use relabel_gui::{Started, start, wire_window};
use relabel_render::document::{Common, Element, ElementOrRaw, Label, RectEl, Rot};
use relabel_render::target::Dpmm;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use support::{
    Held, Scripted, button_of, down, field_of, find_button, frame, frame_of, group_of, move_to,
    press, rect, reference_label, rows_of, tmp_dir, type_text, up,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);

type Wired = relabel_gui::Wired<InsProbe>;

/// The probe at scale 1, wired as the application wires it, shown, with the change
/// handlers the first layout queued already run.
fn probe(started: Started, chooser: Rc<dyn Chooser>) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let wired = wire_window(&ui, started, chooser, Opening::Fit);
    ui.show().expect("showing a headless window");
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    (ui, wired)
}

/// The probe on the label at `path`, as `relabel PATH` opens it.
fn on_file(path: &Path, chooser: Rc<dyn Chooser>) -> (InsProbe, Wired) {
    probe(start(Some(path)), chooser)
}

/// A scratch directory holding the reference label as `a.relabel`, and the path of
/// each.
fn with_a(test: &str) -> (PathBuf, PathBuf) {
    let dir = tmp_dir(&format!("file_flow/{test}"));
    let a = dir.join("a.relabel");
    write(&a, &reference_label());
    (dir, a)
}

fn write(path: &Path, label: &Label) {
    fs::write(path, label.to_json().expect("the label saves")).expect("writing the label");
}

fn read(path: &Path) -> Label {
    Label::from_json(&fs::read_to_string(path).expect("reading the label")).expect("a label")
}

/// Control and a key, as a keyboard sends it.
fn ctrl(ui: &InsProbe, k: &str) {
    press(ui.window(), k, false, true);
}

fn key(ui: &InsProbe, k: Key) {
    press(ui.window(), k, false, false);
}

/// Tab to element 1 and move it one dot along the feed, from 8 to 9.
fn nudge_first(ui: &InsProbe) {
    key(ui, Key::Tab);
    key(ui, Key::RightArrow);
}

fn close(ui: &InsProbe) {
    ui.window().dispatch_event(WindowEvent::CloseRequested);
}

fn answer(ui: &InsProbe, button: &str) {
    button_of(ui, button).invoke_accessible_default_action();
}

fn feed_of(wired: &Wired, id: u32) -> i32 {
    frame_of(wired.canvas.borrow().doc().label(), id).feed
}

/// A 100 mm label, longer than the view at any zoom but the first steps, with one
/// element.
fn long_label() -> Label {
    let mut l = Label::new(800, 96, Dpmm::DPI_203);
    l.elements = vec![rect(1, frame(8, 8, 40, 24), Rot::D0, false)];
    l
}

/// A 50 mm label whose second element is named "other".
fn b_label() -> Label {
    let mut other = Common::new(2, frame(200, 40, 40, 24));
    other.name = "other".into();
    let mut l = Label::new(400, 96, Dpmm::DPI_203);
    l.elements = vec![
        rect(1, frame(16, 8, 40, 24), Rot::D0, false),
        ElementOrRaw::Known(Element::Rect(RectEl {
            common: other,
            stroke_dots: 0,
            radius_dots: 0,
        })),
    ];
    l
}

/// The File bar takes a row of its own above the edit bar, and the default window
/// grows by exactly that row, so the view keeps the size every view test and
/// view.rs's own figures assume. Pinned before the bar went in, at 709 by 525.
#[test]
fn the_view_keeps_its_size_with_the_file_bar() {
    let (_dir, a) = with_a("view_size");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    assert_eq!((ui.get_view_px_w(), ui.get_view_px_h()), (709, 525));
}

/// Control+S from inside a field saves what the field holds: the shell's capture
/// takes the chord before the field sees it, and the file command commits the
/// field before it reads the document.
#[test]
fn ctrl_s_from_inside_a_field_saves_the_typed_text() {
    let (_dir, a) = with_a("ctrl_s_field");
    let scripted = Rc::new(Scripted::default());
    let (ui, wired) = on_file(&a, scripted.clone());
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "37");
    ctrl(&ui, "s");
    assert_eq!(frame_of(&read(&a), 1).feed, 37);
    assert!(!wired.canvas.borrow().doc().unsaved());
    assert_eq!(ui.get_doc_title(), "a.relabel \u{2014} relabel");
    assert_eq!(ui.get_status(), "saved a.relabel");
    assert_eq!(scripted.asked(), []);
}

/// Save as is told from Save by Shift, whichever case the letter arrives in, and
/// Control+Shift+O is still Add rectangle.
#[test]
fn ctrl_shift_s_asks_where_and_ctrl_shift_o_still_adds_a_rectangle() {
    let (dir, a) = with_a("ctrl_shift_s");
    let scripted = Rc::new(Scripted::default());
    let (ui, wired) = on_file(&a, scripted.clone());
    let save_as = Ask::SaveAs {
        folder: Some(dir.clone()),
        name: "a.relabel".into(),
    };
    press(ui.window(), "S", true, true);
    assert_eq!(scripted.asked(), std::slice::from_ref(&save_as));
    assert_eq!(ui.get_status(), "nothing was saved: no file was chosen");
    press(ui.window(), "s", true, true);
    assert_eq!(scripted.asked(), [save_as.clone(), save_as]);
    // Said again in other words, so a screen reader hears the repeat.
    assert_eq!(
        ui.get_status(),
        "nothing was saved: no file was chosen (again)"
    );
    let before = wired.canvas.borrow().doc().label().elements.len();
    press(ui.window(), "O", true, true);
    assert_eq!(
        wired.canvas.borrow().doc().label().elements.len(),
        before + 1
    );
    assert!(
        !scripted
            .asked()
            .iter()
            .any(|a| matches!(a, Ask::Open { .. }))
    );
}

#[test]
fn closing_unsaved_work_keeps_the_window_and_asks() {
    let (_dir, a) = with_a("close_asks");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    nudge_first(&ui);
    close(&ui);
    assert!(ui.window().is_visible());
    let question = "Save the changes to a.relabel before closing?";
    for name in ["Save", "Don't save", "Cancel"] {
        assert_eq!(
            button_of(&ui, name).accessible_description().as_deref(),
            Some(question),
            "{name}"
        );
    }
    let group = group_of(&ui, "Unsaved changes").expect("the question is shown");
    let text = group
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Question"))
        .find_first()
        .expect("the question's text");
    assert_eq!(text.accessible_value().as_deref(), Some(question));
    assert_eq!(
        text.accessible_live_region(),
        Some(AccessibleLiveness::Polite)
    );
}

#[test]
fn cancel_keeps_everything_and_dont_save_closes() {
    let (_dir, a) = with_a("cancel_then_dont_save");
    let bytes = fs::read(&a).unwrap();
    let (ui, wired) = on_file(&a, Rc::new(Scripted::default()));
    nudge_first(&ui);
    close(&ui);
    answer(&ui, "Cancel");
    assert!(ui.window().is_visible());
    assert!(wired.canvas.borrow().doc().dirty());
    assert_eq!(ui.get_status(), "nothing was closed");
    assert!(ui.get_doc_title().starts_with('*'));
    close(&ui);
    answer(&ui, "Don't save");
    assert!(!ui.window().is_visible());
    assert_eq!(fs::read(&a).unwrap(), bytes);
}

#[test]
fn escape_in_the_question_is_cancel_and_the_keyboard_is_live_after() {
    let (_dir, a) = with_a("escape_question");
    let (ui, wired) = on_file(&a, Rc::new(Scripted::default()));
    nudge_first(&ui);
    let label = wired.canvas.borrow().doc().label().clone();
    close(&ui);
    key(&ui, Key::Escape);
    assert!(find_button(&ui, "Don't save").is_none());
    assert!(ui.window().is_visible());
    assert_eq!(wired.canvas.borrow().doc().label(), &label);
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, 1), frame_of(&label, 1).feed + 1);
}

/// Save has the keyboard, and Tab goes round the three answers.
#[test]
fn tab_and_return_answer_the_question() {
    let (_dir, a) = with_a("tab_return");
    let bytes = fs::read(&a).unwrap();
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    nudge_first(&ui);
    close(&ui);
    key(&ui, Key::Tab);
    key(&ui, Key::Return);
    assert!(!ui.window().is_visible(), "Don't save closed the window");
    assert_eq!(fs::read(&a).unwrap(), bytes);
}

#[test]
fn closing_a_clean_document_closes_at_once() {
    let (_dir, a) = with_a("close_clean");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    close(&ui);
    assert!(!ui.window().is_visible());
    assert!(find_button(&ui, "Don't save").is_none());
}

#[test]
fn save_from_the_question_writes_and_closes() {
    let (_dir, a) = with_a("save_then_close");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    nudge_first(&ui);
    close(&ui);
    answer(&ui, "Save");
    assert!(!ui.window().is_visible());
    assert_eq!(frame_of(&read(&a), 1).feed, 9);
}

/// A label opened in place of another is seen afresh: fitted, at the origin,
/// nothing selected, and the keyboard on the canvas surface, where Tab reaches its
/// first element.
#[test]
fn open_replaces_the_document_refits_and_gives_the_canvas_the_keyboard() {
    let (dir, a) = with_a("open_refits");
    let long = dir.join("long.relabel");
    write(&long, &long_label());
    let scripted = Rc::new(Scripted::new([Picked::Path(long.clone())]));
    let (ui, wired) = on_file(&a, scripted.clone());
    ctrl(&ui, "=");
    ctrl(&ui, "=");
    ctrl(&ui, "o");
    assert_eq!(scripted.asked(), [Ask::Open { folder: Some(dir) }]);
    assert_eq!(
        wired.canvas.borrow().zoom(),
        view::fit((800, 96), (ui.get_view_px_w(), ui.get_view_px_h()))
    );
    assert!(
        ui.get_zoom_says().starts_with("Fit"),
        "{}",
        ui.get_zoom_says()
    );
    assert_eq!((ui.get_pan_x(), ui.get_pan_y()), (0, 0));
    assert_eq!(ui.get_selected(), -1);
    assert!(
        ui.get_status().starts_with("opened long.relabel: "),
        "{}",
        ui.get_status()
    );
    assert_eq!(ui.get_doc_title(), "long.relabel \u{2014} relabel");
    assert_eq!(wired.filing.borrow().path(), Some(long.as_path()));
    key(&ui, Key::Tab);
    assert_eq!(ui.get_selected(), 1);
}

/// Every row of the old document goes, so the row that held the keyboard cannot
/// carry it, or its element's name, into the new one.
#[test]
fn no_row_carries_the_keyboard_across_an_open() {
    let (dir, a) = with_a("open_rows");
    let b = dir.join("b.relabel");
    write(&b, &b_label());
    let (ui, wired) = on_file(&a, Rc::new(Scripted::new([Picked::Path(b.clone())])));
    key(&ui, Key::Tab);
    key(&ui, Key::Tab);
    assert_eq!(ui.get_selected(), 2);
    ctrl(&ui, "o");
    assert_eq!(ui.get_selected(), -1);
    let says: Vec<String> = wired
        .canvas
        .borrow()
        .doc()
        .shown()
        .into_iter()
        .map(|s| s.say)
        .collect();
    let labels: Vec<String> = rows_of(&ui)
        .iter()
        .map(|r| r.accessible_label().unwrap_or_default().to_string())
        .collect();
    assert_eq!(labels, says);
    key(&ui, Key::RightArrow);
    assert_eq!(wired.canvas.borrow().doc().label(), &read(&b));
    key(&ui, Key::Tab);
    assert_eq!(ui.get_selected(), 1);
}

#[test]
fn a_failed_open_keeps_the_current_label_and_its_unsaved_state() {
    let (dir, a) = with_a("open_fails");
    let bad = dir.join("bad.relabel");
    fs::write(&bad, "{}").unwrap();
    let (ui, wired) = on_file(&a, Rc::new(Scripted::new([Picked::Path(bad)])));
    nudge_first(&ui);
    ctrl(&ui, "o");
    answer(&ui, "Don't save");
    let status = ui.get_status();
    assert!(status.contains("could not open bad.relabel:"), "{status}");
    assert!(status.contains("a.relabel is still open"), "{status}");
    assert_eq!(feed_of(&wired, 1), 9);
    assert!(wired.canvas.borrow().doc().dirty());
    assert!(ui.get_doc_title().starts_with('*'));
    assert_eq!(wired.filing.borrow().path(), Some(a.as_path()));
}

/// "Don't save" let go of the work as it stood; a nudge made while the dialog was
/// open is work it never covered, so the question comes back, with the file
/// already chosen kept.
#[test]
fn an_edit_made_behind_a_held_dialog_is_asked_about_again() {
    let (dir, a) = with_a("held_edit");
    let b = dir.join("b.relabel");
    write(&b, &b_label());
    let held = Rc::new(Held::default());
    let (ui, wired) = on_file(&a, held.clone());
    nudge_first(&ui);
    ctrl(&ui, "o");
    answer(&ui, "Don't save");
    key(&ui, Key::RightArrow);
    held.release(Picked::Path(b));
    assert!(find_button(&ui, "Don't save").is_some(), "asked again");
    assert_eq!(feed_of(&wired, 1), 10, "both nudges are still there");
    answer(&ui, "Don't save");
    assert_eq!(ui.get_dots_x(), 400);
    assert_eq!(held.asked().len(), 1, "no second dialog");
}

#[test]
fn a_second_ctrl_o_while_a_dialog_is_held_is_refused() {
    let (_dir, a) = with_a("held_twice");
    let held = Rc::new(Held::default());
    let (ui, _wired) = on_file(&a, held.clone());
    ctrl(&ui, "o");
    ctrl(&ui, "o");
    assert_eq!(held.asked().len(), 1);
    assert_eq!(ui.get_status(), "a file dialog is already open");
    held.release(Picked::Nothing);
    assert_eq!(ui.get_status(), "nothing was opened: no file was chosen");
}

/// Nothing is written or closed half-way through a drag, nothing is kept for the
/// release, and the drag is still one undo step once it is let go.
#[test]
fn file_commands_are_refused_while_an_element_is_held() {
    let (_dir, a) = with_a("held_pointer");
    let bytes = fs::read(&a).unwrap();
    let (ui, wired) = on_file(&a, Rc::new(Scripted::default()));
    let before = feed_of(&wired, 1);
    let row = &rows_of(&ui)[0];
    let (at, size) = (row.absolute_position(), row.size());
    let centre = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    let moved = slint::LogicalPosition::new(centre.x + 24.0, centre.y);
    let w = ui.window();
    down(w, centre);
    move_to(w, moved);
    assert!(wired.canvas.borrow().doc().pressed());
    ctrl(&ui, "s");
    assert_eq!(
        ui.get_status(),
        "nothing was saved: let go of the element first"
    );
    assert_eq!(fs::read(&a).unwrap(), bytes);
    close(&ui);
    assert_eq!(
        ui.get_status(),
        "nothing was closed: let go of the element first"
    );
    assert!(ui.window().is_visible());
    up(w, moved);
    assert_ne!(feed_of(&wired, 1), before, "the drag moved the element");
    // The release runs nothing that was refused.
    assert_eq!(fs::read(&a).unwrap(), bytes);
    assert!(ui.window().is_visible());
    assert!(find_button(&ui, "Don't save").is_none());
    ctrl(&ui, "z");
    assert_eq!(feed_of(&wired, 1), before);
}

/// A file chosen while an element is held is let go of, and the note says which
/// and how to have it after all; the next Control+O opens a dialog afresh.
#[test]
fn a_file_chosen_mid_drag_is_named_and_not_opened() {
    let (dir, a) = with_a("held_chosen");
    let b = dir.join("b.relabel");
    write(&b, &b_label());
    let held = Rc::new(Held::default());
    let (ui, wired) = on_file(&a, held.clone());
    ctrl(&ui, "o");
    let row = &rows_of(&ui)[0];
    let (at, size) = (row.absolute_position(), row.size());
    let centre = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    let moved = slint::LogicalPosition::new(centre.x + 24.0, centre.y);
    let w = ui.window();
    down(w, centre);
    move_to(w, moved);
    held.release(Picked::Path(b));
    assert_eq!(
        ui.get_status(),
        "nothing was opened: b.relabel was chosen while an element was held; let go, then \
         choose Open again"
    );
    up(w, moved);
    assert_eq!(wired.filing.borrow().path(), Some(a.as_path()));
    // The drag is unsaved work, asked about first; then a fresh dialog, where a
    // pending one would have been refused.
    ctrl(&ui, "o");
    answer(&ui, "Don't save");
    assert_eq!(held.asked().len(), 2);
}

/// The question about closing is asked over a dialog still on screen. Cancelled,
/// the window stays and so does the dialog: a second is refused, and the first
/// one's answer is carried out.
#[test]
fn a_close_cancelled_over_a_held_dialog_keeps_that_dialog_the_one() {
    let (dir, a) = with_a("close_over_dialog");
    let held = Rc::new(Held::default());
    let (ui, wired) = on_file(&a, held.clone());
    nudge_first(&ui);
    press(ui.window(), "s", true, true);
    close(&ui);
    assert!(find_button(&ui, "Don't save").is_some());
    answer(&ui, "Cancel");
    ctrl(&ui, "o");
    assert_eq!(ui.get_status(), "a file dialog is already open");
    assert_eq!(held.asked().len(), 1);
    let c = dir.join("c.relabel");
    held.release(Picked::Path(c.clone()));
    assert_eq!(ui.get_status(), "saved c.relabel");
    assert_eq!(frame_of(&read(&c), 1).feed, 9);
    assert_eq!(wired.filing.borrow().path(), Some(c.as_path()));
    assert_eq!(ui.get_doc_title(), "c.relabel \u{2014} relabel");
}

/// Answered while the question about closing is showing, the dialog's file is
/// dropped out loud, and the question stays to be answered.
#[test]
fn a_dialog_answered_under_the_close_question_is_said_to_be_dropped() {
    let (dir, a) = with_a("dialog_under_close");
    let held = Rc::new(Held::default());
    let (ui, _wired) = on_file(&a, held.clone());
    nudge_first(&ui);
    press(ui.window(), "s", true, true);
    close(&ui);
    let c = dir.join("c.relabel");
    held.release(Picked::Path(c.clone()));
    assert_eq!(
        ui.get_status(),
        "nothing was saved: c.relabel was chosen while the window was asking whether to \
         close; choose Save as again"
    );
    assert!(!c.exists());
    assert!(
        find_button(&ui, "Don't save").is_some(),
        "the question stays"
    );
    answer(&ui, "Cancel");
    press(ui.window(), "s", true, true);
    assert_eq!(held.asked().len(), 2, "no dialog is left pending");
}

/// New replaces the document and its history, so Undo straight after has nothing
/// to take back: the question before it is what protects the work.
#[test]
fn new_with_a_preset_makes_a_40_mm_d110_label_that_undo_cannot_take_back() {
    let (_dir, a) = with_a("new_preset");
    let (ui, wired) = on_file(&a, Rc::new(Scripted::default()));
    ctrl(&ui, "n");
    assert!(group_of(&ui, "New label").is_some());
    answer(&ui, "New 40 by 12 millimetre label");
    assert_eq!((ui.get_dots_x(), ui.get_dots_y()), (320, 96));
    {
        let c = wired.canvas.borrow();
        let label = c.doc().label();
        assert!(label.elements.is_empty(), "not the reference label");
        let model = label
            .meta
            .authored_for
            .as_ref()
            .and_then(|f| f.model.as_deref());
        assert_eq!(model, Some("D110"));
        assert_eq!(label.media.feed_tenths_mm, 400);
    }
    assert_eq!(ui.get_doc_title(), "Untitled label \u{2014} relabel");
    assert!(group_of(&ui, "New label").is_none());
    assert!(
        ui.get_status()
            .starts_with("new 40 \u{d7} 12 mm label for a D110"),
        "{}",
        ui.get_status()
    );
    assert_eq!(wired.filing.borrow().path(), None);
    ctrl(&ui, "z");
    assert_eq!(ui.get_status(), "nothing to undo");
}

#[test]
fn new_refuses_a_feed_past_the_printer_and_stays_open() {
    let (_dir, a) = with_a("new_too_long");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    ctrl(&ui, "n");
    type_text(ui.window(), "150");
    assert_eq!(
        ui.get_new_note(),
        "100 mm is the longest label the D110 takes"
    );
    // Return is refused out loud: the note already held the refusal, so it is
    // said again in other words, as the status line says a repeat.
    key(&ui, Key::Return);
    assert_eq!(
        ui.get_new_note(),
        "100 mm is the longest label the D110 takes (again)"
    );
    assert!(find_button(&ui, "Create").is_some());
    assert_eq!((ui.get_dots_x(), ui.get_dots_y()), (320, 96));
    key(&ui, Key::Escape);
    assert!(group_of(&ui, "New label").is_none());
    assert_eq!(ui.get_status(), "no new label was started");
}

/// The printer and the head are plain lines no key reaches, so the field that
/// takes the keyboard when the pane opens carries them as its description.
#[test]
fn the_feed_field_describes_the_printer_and_the_head() {
    let (_dir, a) = with_a("new_described");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    ctrl(&ui, "n");
    assert_eq!(
        field_of(&ui, "Along the feed, in millimetres")
            .accessible_description()
            .as_deref(),
        Some(
            "For a D110 (assumed: relabel is not connected to a printer). Across the head: \
             12 mm, 96 dots, the whole print head"
        )
    );
}

#[test]
fn new_takes_a_length_written_by_an_assistive_technology() {
    let (_dir, a) = with_a("new_assistive");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    ctrl(&ui, "n");
    field_of(&ui, "Along the feed, in millimetres").set_accessible_value("55");
    assert_eq!(ui.get_new_note(), "55 mm along the feed is 440 dots");
    answer(&ui, "Create");
    assert_eq!((ui.get_dots_x(), ui.get_dots_y()), (440, 96));
}

#[test]
fn the_file_bar_buttons_carry_their_chords_and_are_not_tab_stops() {
    let (_dir, a) = with_a("file_bar");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    for (name, keys) in [
        ("New label", "Control+N"),
        ("Open label", "Control+O"),
        ("Save label", "Control+S"),
        ("Save label as", "Control+Shift+S"),
    ] {
        assert_eq!(
            button_of(&ui, name).accessible_description().as_deref(),
            Some(keys)
        );
    }
    key(&ui, Key::Tab);
    assert_eq!(
        ui.get_selected(),
        1,
        "Tab went from the surface to element 1"
    );
    key(&ui, Key::RightArrow);
    answer(&ui, "Save label");
    assert_eq!(frame_of(&read(&a), 1).feed, 9);
}

/// Without an event loop no dialog can be started, and the answer it was given is
/// dropped uncalled rather than called with something made up.
#[test]
fn the_native_chooser_refuses_without_an_event_loop_and_never_answers() {
    let (_dir, a) = with_a("native_chooser");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    let called = Rc::new(Cell::new(false));
    let answered = called.clone();
    let asked = NativeChooser::new(&ui).ask(
        Ask::Open { folder: None },
        Box::new(move |_| answered.set(true)),
    );
    assert!(matches!(asked, Err(NoDialog::Spawn(_))), "{asked:?}");
    assert!(!called.get());
}

#[test]
fn the_title_follows_unsaved_work() {
    let (_dir, a) = with_a("title");
    let (ui, _wired) = on_file(&a, Rc::new(Scripted::default()));
    let clean = "a.relabel \u{2014} relabel";
    let marked = "*a.relabel \u{2014} relabel";
    assert_eq!(ui.get_doc_title(), clean);
    nudge_first(&ui);
    assert_eq!(ui.get_doc_title(), marked);
    ctrl(&ui, "z");
    assert_eq!(ui.get_doc_title(), clean);
    key(&ui, Key::F6);
    type_text(ui.window(), "Q");
    assert_eq!(ui.get_doc_title(), marked, "typed and not yet committed");
    ctrl(&ui, "s");
    assert_eq!(ui.get_doc_title(), clean);
}

#[test]
fn a_bad_path_at_launch_opens_the_default_label_and_says_why() {
    let dir = tmp_dir("file_flow/launch_bad");
    let bad = dir.join("bad.relabel");
    fs::write(&bad, "{}").unwrap();
    let started = start(Some(&bad));
    assert_eq!(started.path, None);
    assert_eq!(started.doc.canvas_dots(), (240, 96));
    assert!(
        started.note.starts_with("could not open bad.relabel: "),
        "{}",
        started.note
    );
    let blank = start(None);
    assert!(
        blank
            .note
            .starts_with("new 30 \u{d7} 12 mm label for a D110 (assumed"),
        "{}",
        blank.note
    );
    assert_eq!(blank.path, None);
}

/// The editor convention: a path that does not exist yet names the new label's
/// file, and Control+S writes it there with no dialog - which is also how a label
/// is saved on a desktop that can show no file dialog.
#[test]
fn a_missing_path_at_launch_becomes_the_new_labels_file() {
    let dir = tmp_dir("file_flow/launch_missing");
    let new = dir.join("new.relabel");
    let started = start(Some(&new));
    let whole = std::path::absolute(&new).unwrap();
    assert_eq!(started.path.as_deref(), Some(whole.as_path()));
    assert_eq!(started.doc.canvas_dots(), (240, 96));
    assert!(
        started.note.contains("new.relabel does not exist yet"),
        "{}",
        started.note
    );
    let scripted = Rc::new(Scripted::default());
    let (ui, _wired) = probe(started, scripted.clone());
    assert_eq!(ui.get_doc_title(), "new.relabel \u{2014} relabel");
    ctrl(&ui, "s");
    assert_eq!(scripted.asked(), []);
    let saved = read(&new);
    assert_eq!(saved.canvas.feed, 240);
    let model = saved
        .meta
        .authored_for
        .as_ref()
        .and_then(|f| f.model.as_deref());
    assert_eq!(model, Some("D110"));
    assert_eq!(ui.get_doc_title(), "new.relabel \u{2014} relabel");
}

/// A label a later version wrote is edited and saved with everything this build
/// does not understand still in it: the needles relabel-render's corpus test
/// checks, after a real edit and a real save through the window.
#[test]
fn saving_a_future_label_keeps_every_stranger() {
    let dir = tmp_dir("file_flow/future");
    let future = dir.join("from-the-future.relabel");
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../relabel-render/tests/corpus/v1/from-the-future.relabel");
    fs::copy(&corpus, &future).expect("copying the corpus label");
    let (ui, wired) = on_file(&future, Rc::new(Scripted::default()));
    key(&ui, Key::Tab);
    assert_eq!(ui.get_selected(), 1);
    key(&ui, Key::RightArrow);
    assert!(wired.canvas.borrow().doc().dirty());
    ctrl(&ui, "s");
    assert_eq!(ui.get_status(), "saved from-the-future.relabel");
    let text = fs::read_to_string(&future).unwrap();
    for needle in [
        "colour_profile",
        "something from 2027",
        "hologram",
        "632",
        "layers",
        "dash_dots",
        "oval",
        "mirror",
    ] {
        assert!(text.contains(needle), "{needle:?} was lost on save");
    }
    let again = Label::from_json(&text).unwrap().to_json().unwrap();
    assert_eq!(again, text);
}
