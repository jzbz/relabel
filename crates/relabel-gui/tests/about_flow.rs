//! The About pane, driven through the probe window as an operator and an
//! assistive technology drive it: opened from the File bar or F1, read line by
//! line, Slint's AboutSlint widget shown inside it, kept above the status line
//! in a short window, closed by Close, Return or Escape with the keyboard back
//! where it was, and kept in the file popups' one place.
//!
//! Every probe is wired by `relabel_gui::wire_window`, the one wiring the
//! application runs, with a scripted file dialog and a printer link that goes
//! nowhere. Nothing here prints or reaches Bluetooth.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use relabel_gui::ins::Field;
use relabel_gui::view::Opening;
use relabel_gui::{about, start, wire_window};
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use support::{
    HeldPlug, InlineImporter, Scripted, button_of, find_button, frame_of, group_of, press,
    reference_label, status_of, tmp_dir, type_text,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);
relabel_gui::impl_print_host!(InsProbe, IssueRow, ChoiceRow);

type Wired = relabel_gui::Wired<InsProbe>;

const PANE: &str = "About relabel";

/// The probe at scale 1 on the reference label saved as `a.relabel`, wired as the
/// application wires it and shown, and the label's path.
fn probe(test: &str) -> (InsProbe, Wired, PathBuf) {
    let dir = tmp_dir(&format!("about_flow/{test}"));
    let a = dir.join("a.relabel");
    fs::write(&a, reference_label().to_json().expect("the label saves")).expect("writing");
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let wired = wire_window(
        &ui,
        start(Some(&a)),
        Rc::new(Scripted::default()),
        Rc::new(HeldPlug::default()),
        Rc::new(InlineImporter),
        Opening::Fit,
    );
    ui.show().expect("showing a headless window");
    settle();
    (ui, wired, a)
}

fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
}

fn key(ui: &InsProbe, k: Key) {
    press(ui.window(), k, false, false);
    settle();
}

fn ctrl(ui: &InsProbe, k: &str) {
    press(ui.window(), k, false, true);
    settle();
}

fn activate(ui: &InsProbe, name: &str) {
    button_of(ui, name).invoke_accessible_default_action();
    settle();
}

fn pane(ui: &InsProbe) -> Option<ElementHandle> {
    group_of(ui, PANE)
}

/// A line of the pane, by the name it is read under.
fn line(ui: &InsProbe, label: &str) -> String {
    let wanted = label.to_string();
    pane(ui)
        .expect("the About pane is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(move |h| h.accessible_label().is_some_and(|l| l == wanted.as_str()))
        .find_first()
        .unwrap_or_else(|| panic!("no line is called {label:?}"))
        .accessible_value()
        .expect("a line has a value")
        .to_string()
}

fn feed_of(wired: &Wired, id: u32) -> i32 {
    frame_of(wired.canvas.borrow().doc().label(), id).feed
}

fn read_feed(path: &Path, id: u32) -> i32 {
    let label = relabel_render::document::Label::from_json(
        &fs::read_to_string(path).expect("reading the label"),
    )
    .expect("a label");
    frame_of(&label, id).feed
}

#[test]
fn f1_opens_the_about_pane_and_return_closes_it() {
    let (ui, wired, _a) = probe("f1");
    assert!(pane(&ui).is_none());
    key(&ui, Key::F1);
    assert!(pane(&ui).is_some());
    assert!(wired.filing.borrow().about_open());
    // Close has the keyboard.
    key(&ui, Key::Return);
    assert!(pane(&ui).is_none());
    assert!(!wired.filing.borrow().about_open());
}

/// The sixth File bar button, F1 in its description, opened by its default
/// action; not a Tab stop, so Tab still goes from the surface to element 1.
#[test]
fn the_file_bar_button_opens_it_and_carries_f1() {
    let (ui, wired, _a) = probe("button");
    assert_eq!(
        button_of(&ui, PANE).accessible_description().as_deref(),
        Some("F1")
    );
    key(&ui, Key::Tab);
    assert_eq!(
        ui.get_selected(),
        1,
        "Tab went from the surface to element 1"
    );
    activate(&ui, PANE);
    assert!(pane(&ui).is_some());
    assert!(wired.filing.borrow().about_open());
    activate(&ui, "Close");
    assert!(pane(&ui).is_none());
    assert!(!wired.filing.borrow().about_open());
}

#[test]
fn the_pane_says_relabels_version_copyright_licence_repository_and_notices() {
    let (ui, _wired, _a) = probe("lines");
    key(&ui, Key::F1);
    assert_eq!(
        line(&ui, "Version"),
        format!("Version {}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        line(&ui, "Copyright"),
        "Copyright \u{a9} 2026 Jonathan Zeppettini"
    );
    assert_eq!(line(&ui, "Licence"), "Released under the MIT licence");
    assert_eq!(line(&ui, "Repository"), "https://github.com/jzbz/relabel");
    assert_eq!(line(&ui, "Third-party notices"), about::NOTICES);
    assert!(line(&ui, "Third-party notices").contains("THIRD-PARTY.md"));
    assert_eq!(line(&ui, "Trademark"), about::TRADEMARK);
}

/// Slint's licence asks for its AboutSlint widget to be displayed, so it is
/// found inside the pane, at a size that shows it, and wholly inside the pane:
/// its logo under the name it announces, and its own line with Slint's address.
#[test]
fn the_about_slint_widget_is_shown_whole_inside_the_pane() {
    let (ui, _wired, _a) = probe("widget");
    key(&ui, Key::F1);
    let group = pane(&ui).expect("the About pane is shown");
    let logo = group
        .query_descendants()
        .match_accessible_role(AccessibleRole::Image)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "#MadeWithSlint"))
        .find_first()
        .expect("the AboutSlint widget's logo");
    let (at, size) = (logo.absolute_position(), logo.size());
    let (g_at, g_size) = (group.absolute_position(), group.size());
    assert!(
        size.width >= 200.0 && size.height >= 90.0,
        "the logo is {size:?}"
    );
    assert!(
        at.x >= g_at.x
            && at.y >= g_at.y
            && at.x + size.width <= g_at.x + g_size.width
            && at.y + size.height <= g_at.y + g_size.height,
        "the logo at {at:?} {size:?} is inside the pane at {g_at:?} {g_size:?}"
    );
    assert_eq!(logo.computed_opacity(), 1.0);
    let slint_line = group
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| {
            h.accessible_label()
                .is_some_and(|l| l.contains("https://slint.dev/"))
        })
        .find_first()
        .expect("the widget's version line");
    let line_at = slint_line.absolute_position();
    assert!(
        line_at.y + slint_line.size().height <= g_at.y + g_size.height,
        "the widget's line is inside the pane"
    );
    // The whole pane, the widget and Close with it, ends above the status line,
    // 84 logical pixels down where the window leaves it room (shell.slint).
    let status = status_of(&ui).absolute_position();
    assert_eq!(ui.get_about_top(), 84.0);
    assert!(
        bottom_in_window(&ui, &group) <= status.y,
        "the pane is {g_size:?}, the status line at {status:?}"
    );
}

/// The probe shortened to a window an operator can have: a 1366 x 768 screen at
/// 1.25 less a panel and a title bar, and a 1080p screen at 2.0.
fn shorten(ui: &InsProbe, width: f32, height: f32) {
    ui.set_probe_width(width);
    ui.set_probe_height(height);
    ui.window().set_size(slint::LogicalSize::new(width, height));
    settle();
}

/// The bottom of an element, from the window's top: a popup's elements are
/// measured from the popup, whose top the shell gives as `about-top`.
fn bottom_in_window(ui: &InsProbe, h: &ElementHandle) -> f32 {
    ui.get_about_top() + h.absolute_position().y + h.size().height
}

/// In a short window the pane stops above the status line, as the print pane
/// does, with Slint's widget and Close whole inside it; relabel's lines are what
/// give way, and the arrows and the page keys scroll them while Close keeps the
/// keyboard.
#[test]
fn in_a_short_window_the_pane_ends_above_the_status_line() {
    for (width, height) in [(1093.0, 560.0), (960.0, 480.0)] {
        let (ui, _wired, _a) = probe(&format!("short_{height}"));
        shorten(&ui, width, height);
        key(&ui, Key::F1);
        let group = pane(&ui).expect("the About pane is shown");
        let status = status_of(&ui).absolute_position();
        assert_eq!(ui.get_about_top(), 84.0, "{height}: where it opens");
        assert!(
            bottom_in_window(&ui, &group) <= status.y,
            "{height}: the pane ends at {}, the status line starts at {status:?}",
            bottom_in_window(&ui, &group)
        );
        let inside = |h: &ElementHandle| {
            let (at, size) = (h.absolute_position(), h.size());
            at.y >= group.absolute_position().y
                && at.y + size.height <= group.absolute_position().y + group.size().height
        };
        let close = button_of(&ui, "Close");
        assert!(inside(&close), "{height}: Close is inside the pane");
        let logo = group
            .query_descendants()
            .match_accessible_role(AccessibleRole::Image)
            .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "#MadeWithSlint"))
            .find_first()
            .expect("the AboutSlint widget's logo");
        assert!(inside(&logo), "{height}: the logo is inside the pane");
        assert!(logo.size().height >= 90.0, "{height}: {:?}", logo.size());
        // The last line is scrolled away, and End brings it into the pane. The
        // testing backend, as AccessKit does, leaves out a line scrolled wholly
        // away, so a line is shown when it is found and lies wholly inside the
        // lines' box.
        let lines = group
            .query_descendants()
            .match_id("AboutPane::lines")
            .find_first()
            .expect("the lines' box");
        let shown = |label: &str| {
            let wanted = label.to_string();
            group
                .query_descendants()
                .match_accessible_role(AccessibleRole::Text)
                .match_predicate(move |h| {
                    h.accessible_label().is_some_and(|l| l == wanted.as_str())
                })
                .find_first()
                .is_some_and(|h| {
                    let (at, size) = (h.absolute_position(), h.size());
                    let (b_at, b_size) = (lines.absolute_position(), lines.size());
                    at.y >= b_at.y && at.y + size.height <= b_at.y + b_size.height
                })
        };
        assert!(!shown("Trademark"), "{height}: the lines give way");
        assert!(shown("Version"), "{height}: the first line is shown");
        key(&ui, Key::End);
        assert!(shown("Trademark"), "{height}: End scrolls to the end");
        key(&ui, Key::Home);
        assert!(!shown("Trademark"), "{height}: Home scrolls back");
        key(&ui, Key::PageDown);
        key(&ui, Key::PageDown);
        key(&ui, Key::PageDown);
        assert!(shown("Trademark"), "{height}: so do the page keys");
        // Close still has the keyboard. While the lines do not all fit, Tab
        // stops on each, Version first, and brings it into view: a line
        // scrolled away is hidden from a screen reader until then. The seventh
        // Tab is back on Close.
        key(&ui, Key::Home);
        for _ in 0..5 {
            key(&ui, Key::Tab);
        }
        assert!(!shown("Trademark"), "{height}: five lines in");
        assert!(shown("Third-party notices"), "{height}: the fifth line");
        key(&ui, Key::Tab);
        assert!(shown("Trademark"), "{height}: Tab reached the last line");
        key(&ui, Key::Tab);
        key(&ui, Key::Return);
        assert!(pane(&ui).is_none());
    }
}

/// In a window too short for the widget's box and Close below a line of
/// relabel's at 84 pixels down - a 1080p screen at 2.0 less a panel, or a
/// half-height tile - the pane rises over the File bar, which nothing behind the
/// pane can be pressed on anyway, and still ends above the status line with
/// Close inside it. Where even that leaves too little room, everything above
/// Close scrolls as one: relabel's lines and the widget are Tab stops, which
/// bring each into view, and End, Home and the arrows reach the widget too. The
/// widget then shrinks towards its own least, so that down to a 250-pixel window
/// it can still be scrolled wholly into view; in a 220-pixel one its logo and its
/// own line, Slint's version and address, can each be; and in a 150-pixel one,
/// about the shortest relabel lays out, Close is still inside the pane.
#[test]
fn in_a_very_short_window_the_pane_still_ends_above_the_status_line() {
    // The window's height, and what can be seen of the widget at once or by the
    // keys: 2 the whole widget at once, 1 the whole widget by End, the arrows or
    // Tab, 0 its own line by End and its logo by Tab, -1 neither, though Tab
    // still stops on it.
    for (height, seen) in [(360.0, 2), (300.0, 1), (250.0, 1), (220.0, 0), (150.0, -1)] {
        let (ui, _wired, _a) = probe(&format!("very_short_{height}"));
        shorten(&ui, 960.0, height);
        // In the shortest window the canvas, which has the keyboard, is laid out
        // to nothing, and Slint takes the keyboard from an element out of view
        // before it delivers a key (i-slint-core window.rs:1306-1310), so F1
        // reaches nothing there; the File bar button still opens the pane.
        if height < 160.0 {
            activate(&ui, PANE);
        } else {
            key(&ui, Key::F1);
        }
        let group = pane(&ui).expect("the About pane is shown");
        let status = status_of(&ui).absolute_position();
        let top = ui.get_about_top();
        assert!(
            (8.0..=84.0).contains(&top),
            "{height}: the pane's top is {top}"
        );
        assert!(
            bottom_in_window(&ui, &group) <= status.y,
            "{height}: the pane ends at {}, the status line starts at {status:?}",
            bottom_in_window(&ui, &group)
        );
        let (g_at, g_size) = (group.absolute_position(), group.size());
        let close = button_of(&ui, "Close");
        let (c_at, c_size) = (close.absolute_position(), close.size());
        assert!(
            c_at.y >= g_at.y && c_at.y + c_size.height <= g_at.y + g_size.height,
            "{height}: Close at {c_at:?} {c_size:?} is inside the pane at {g_at:?} {g_size:?}"
        );
        // Whether an element of the widget, found by role and a test of its
        // name, lies wholly inside the part that scrolls. The testing backend,
        // as AccessKit does, leaves out an element scrolled wholly away.
        let whole = |role: AccessibleRole, named: fn(&str) -> bool| {
            let body = group
                .query_descendants()
                .match_id("AboutPane::body")
                .find_first()
                .expect("the part of the pane that scrolls");
            group
                .query_descendants()
                .match_accessible_role(role)
                .match_predicate(move |h| h.accessible_label().is_some_and(|l| named(&l)))
                .find_first()
                .is_some_and(|h| {
                    let (at, size) = (h.absolute_position(), h.size());
                    let (b_at, b_size) = (body.absolute_position(), body.size());
                    at.y >= b_at.y - 0.01 && at.y + size.height <= b_at.y + b_size.height + 0.01
                })
        };
        let logo = || whole(AccessibleRole::Image, |l| l == "#MadeWithSlint");
        let slint_line = || whole(AccessibleRole::Text, |l| l.contains("https://slint.dev/"));
        if seen == 2 {
            assert!(logo() && slint_line(), "{height}: the widget is whole");
            key(&ui, Key::Escape);
            assert!(pane(&ui).is_none());
            continue;
        }
        assert!(
            !slint_line(),
            "{height}: the widget's line is scrolled away"
        );
        if seen < 0 {
            // Too little shows for any line to be brought wholly into view, and
            // Slint moves the keyboard to nothing it cannot show; Close keeps it.
            key(&ui, Key::Return);
            assert!(pane(&ui).is_none(), "{height}: Close had the keyboard");
            continue;
        }
        key(&ui, Key::End);
        if seen >= 0 {
            assert!(
                slint_line(),
                "{height}: End brings the widget's line into view"
            );
        }
        if seen == 1 {
            assert!(logo(), "{height}: and the whole widget");
            key(&ui, Key::Home);
            assert!(!slint_line(), "{height}: Home scrolls back");
            // The down arrow reaches it too, once relabel's lines are at their
            // end, and the up arrow goes back.
            for _ in 0..40 {
                key(&ui, Key::DownArrow);
            }
            assert!(logo() && slint_line(), "{height}: so does the down arrow");
            for _ in 0..40 {
                key(&ui, Key::UpArrow);
            }
            assert!(!slint_line(), "{height}: and the up arrow goes back");
        }
        key(&ui, Key::Home);
        // Tab: the six lines, then the widget, brought into view; Close is the
        // next stop.
        for _ in 0..7 {
            key(&ui, Key::Tab);
        }
        if seen >= 0 {
            assert!(
                logo(),
                "{height}: Tab reached the widget and brought its logo into view"
            );
        }
        key(&ui, Key::Tab);
        key(&ui, Key::Return);
        assert!(pane(&ui).is_none(), "{height}: Close had the keyboard");
    }
}

/// Escape closes it, and the keyboard is on the element it was on: an arrow
/// moves that element.
#[test]
fn escape_closes_it_and_the_keyboard_is_where_it_was() {
    let (ui, wired, _a) = probe("escape");
    key(&ui, Key::Tab);
    let feed = feed_of(&wired, 1);
    key(&ui, Key::F1);
    assert!(pane(&ui).is_some());
    // The arrow stops at the popup.
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, 1), feed);
    key(&ui, Key::Escape);
    assert!(pane(&ui).is_none());
    assert!(!wired.filing.borrow().about_open());
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, 1), feed + 1);
}

/// Return, on Close, and Close's own action close it as Escape does, with the
/// keyboard back on the element it was on: an arrow moves that element.
#[test]
fn return_and_close_hand_the_keyboard_back_to_the_element() {
    let (ui, wired, _a) = probe("return_element");
    key(&ui, Key::Tab);
    let feed = feed_of(&wired, 1);
    key(&ui, Key::F1);
    key(&ui, Key::Return);
    assert!(pane(&ui).is_none());
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, 1), feed + 1, "after Return");
    key(&ui, Key::F1);
    activate(&ui, "Close");
    assert!(pane(&ui).is_none());
    key(&ui, Key::RightArrow);
    assert_eq!(feed_of(&wired, 1), feed + 2, "after Close");
}

/// The same from a field: Return and Close hand the keyboard back to it.
#[test]
fn return_and_close_hand_the_keyboard_back_to_the_field() {
    for (test, close) in [("return_field", true), ("close_field", false)] {
        let (ui, wired, _a) = probe(test);
        key(&ui, Key::Tab);
        key(&ui, Key::F6);
        key(&ui, Key::F1);
        assert!(pane(&ui).is_some());
        if close {
            key(&ui, Key::Return);
        } else {
            activate(&ui, "Close");
        }
        assert!(pane(&ui).is_none());
        type_text(ui.window(), "W");
        settle();
        let staged = wired
            .canvas
            .borrow()
            .doc()
            .staged()
            .map(|(id, f, t)| (id, f, t.to_string()));
        let Some((1, Field::Name, text)) = staged else {
            panic!("{test}: the name field had the keyboard back: {staged:?}");
        };
        assert!(text.contains('W'), "{test}: {text:?}");
    }
}

/// F1 reaches the shell from inside a field, as F6 does, and inserts nothing
/// there. Opening the pane finishes what was typed, as every file command does,
/// and closing it hands the keyboard back to the field.
#[test]
fn f1_from_inside_a_field_opens_it_and_closing_returns_to_the_field() {
    let (ui, wired, _a) = probe("field");
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    type_text(ui.window(), "Z");
    settle();
    key(&ui, Key::F1);
    assert!(pane(&ui).is_some());
    assert_eq!(wired.canvas.borrow().doc().staged(), None);
    key(&ui, Key::Escape);
    assert!(pane(&ui).is_none());
    type_text(ui.window(), "W");
    settle();
    let staged = wired
        .canvas
        .borrow()
        .doc()
        .staged()
        .map(|(id, f, t)| (id, f, t.to_string()));
    let Some((1, Field::Name, text)) = staged else {
        panic!("the name field had the keyboard back: {staged:?}");
    };
    assert!(text.contains('W') && !text.contains('\u{F704}'), "{text:?}");
}

/// Where the lines all fit, as in the default window, Close is the pane's only
/// Tab stop: Tab stays on it, and Return closes.
#[test]
fn tab_goes_round_inside_the_pane() {
    let (ui, _wired, _a) = probe("tab");
    key(&ui, Key::F1);
    key(&ui, Key::Tab);
    assert!(pane(&ui).is_some());
    key(&ui, Key::Tab);
    key(&ui, Key::Return);
    assert!(pane(&ui).is_none());
}

/// Control+F1 and Shift+F1 are left to the desktop.
#[test]
fn only_f1_alone_opens_it() {
    let (ui, wired, _a) = probe("modifiers");
    press(ui.window(), Key::F1, false, true);
    press(ui.window(), Key::F1, true, false);
    settle();
    assert!(pane(&ui).is_none());
    assert!(!wired.filing.borrow().about_open());
}

/// An assistive technology can press a File bar button behind the pane, and is
/// told why nothing happened; pressed again, About is shown once, not twice.
#[test]
fn file_commands_behind_the_about_pane_are_refused() {
    let (ui, wired, a) = probe("behind");
    let bytes = fs::read(&a).unwrap();
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    key(&ui, Key::F1);
    activate(&ui, "Save label");
    assert_eq!(ui.get_status(), "close the About pane first");
    assert_eq!(fs::read(&a).unwrap(), bytes, "nothing was written");
    // Said again, the status line marks it so that it is announced again.
    activate(&ui, "Print the label");
    assert_eq!(ui.get_status(), "close the About pane first (again)");
    assert!(group_of(&ui, "Print").is_none());
    activate(&ui, PANE);
    assert!(pane(&ui).is_some());
    assert!(wired.filing.borrow().about_open());
    key(&ui, Key::Escape);
    assert!(
        pane(&ui).is_none(),
        "one pane was open, and Escape closed it"
    );
    ctrl(&ui, "s");
    assert_eq!(ui.get_status(), "saved a.relabel");
    assert_eq!(read_feed(&a, 1), frame_of(&reference_label(), 1).feed + 1);
}

/// The print pane keeps the place too: F1 inside it stops at the popup, and the
/// About button behind it is refused in the print pane's words.
#[test]
fn about_is_refused_behind_the_print_pane() {
    let (ui, wired, _a) = probe("print_first");
    ctrl(&ui, "p");
    assert!(group_of(&ui, "Print").is_some());
    key(&ui, Key::F1);
    assert!(pane(&ui).is_none());
    activate(&ui, PANE);
    assert_eq!(ui.get_status(), "close the print pane first");
    assert!(pane(&ui).is_none());
    assert!(group_of(&ui, "Print").is_some());
    assert!(!wired.filing.borrow().about_open());
}

/// Closing the window is an ordinary close: a clean label closes at once, and
/// unsaved work is asked about in the pane's place.
#[test]
fn closing_the_window_with_the_about_pane_open() {
    let (ui, _wired, _a) = probe("close_clean");
    key(&ui, Key::F1);
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    settle();
    assert!(!ui.window().is_visible());

    let (ui, wired, _a) = probe("close_unsaved");
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    key(&ui, Key::F1);
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    settle();
    assert!(ui.window().is_visible());
    assert!(pane(&ui).is_none());
    assert!(group_of(&ui, "Unsaved changes").is_some());
    assert!(!wired.filing.borrow().about_open());
    activate(&ui, "Cancel");
    assert!(find_button(&ui, "Don't save").is_none());
    assert!(pane(&ui).is_none());
    assert!(ui.window().is_visible());
}
