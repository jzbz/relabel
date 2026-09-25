//! What a screen reader is told about the printer link and the print pane: a
//! connect said three times on the status line and once more by the verdict,
//! every control in the pane named and reached by Tab, the issues as list
//! items, the Printer line as plain text, and a job's progress as a number that
//! is read, never announced.
//!
//! Through the probe window, wired by `relabel_gui::wire_window` with a printer
//! link that keeps every order and hears only what the test says it heard.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleLiveness, AccessibleRole, ElementHandle, ElementQuery};
use relabel_core::link::mock::{self, D110_M_NAME};
use relabel_core::link::{DeviceKey, Heard, Order, Seen};
use relabel_core::{HandshakeStep, RunNews};
use relabel_gui::view::Opening;
use relabel_gui::{start, wire_window};
use relabel_proto::payload::IDENTITY_READS;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use support::{
    HeldPlug, Scripted, button_of, group_of, press, reference_label, status_of, tmp_dir,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);
relabel_gui::impl_print_host!(InsProbe, IssueRow, ChoiceRow);

type Wired = relabel_gui::Wired<InsProbe>;

/// The corpus label with one element of every kind, two of them bound to a
/// `{sku}` the editor has no value for. Read, never written: each test saves a
/// copy of its own.
const KITCHEN_SINK: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../relabel-render/tests/corpus/v1/kitchen-sink.relabel"
));

fn probe(path: &Path, plug: Rc<HeldPlug>) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let wired = wire_window(
        &ui,
        start(Some(path)),
        Rc::new(Scripted::default()),
        plug,
        Opening::Fit,
    );
    ui.show().expect("showing a headless window");
    settle();
    (ui, wired)
}

/// Run the change handlers a push queued, as the event loop would before the next
/// frame.
fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
}

/// A scratch copy of `json` as `a.relabel`, and its folder.
fn saved(test: &str, json: &str) -> (PathBuf, PathBuf) {
    let dir = tmp_dir(&format!("print_a11y/{test}"));
    let a = dir.join("a.relabel");
    fs::write(&a, json).expect("writing the label");
    (dir, a)
}

fn key(ui: &InsProbe, k: Key) {
    press(ui.window(), k, false, false);
    settle();
}

fn open_preview(ui: &InsProbe) {
    press(ui.window(), "p", false, true);
    settle();
}

fn hear(plug: &HeldPlug, heard: Heard) {
    plug.hear(heard);
    settle();
}

/// Every step a level-4 printer's handshake reports, in order.
fn handshake_steps() -> Vec<HandshakeStep> {
    let mut steps = vec![HandshakeStep::Connect, HandshakeStep::Status];
    steps.extend(
        IDENTITY_READS
            .iter()
            .zip(1..)
            .map(|(&field, n)| HandshakeStep::Info { field, n }),
    );
    steps.push(HandshakeStep::Heartbeat);
    steps
}

/// The issue list's items in index order, for `rows_of`'s trap: a repeated item
/// is found once per source element merged into it.
fn issue_items(ui: &InsProbe) -> Vec<ElementHandle> {
    let list = group_of(ui, "Print")
        .expect("the preview is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::List)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Issues"))
        .find_first()
        .expect("the issue list");
    let mut found: Vec<(usize, ElementHandle)> = list
        .query_descendants()
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .filter_map(|h| h.accessible_item_index().map(|i| (i, h)))
        .collect();
    found.sort_by_key(|(i, _)| *i);
    found.dedup_by_key(|(i, _)| *i);
    found.into_iter().map(|(_, h)| h).collect()
}

/// The status line is live, so every value it takes is an announcement: over a
/// whole connect it takes three, and the eleven handshake steps reach only the
/// pane's step line. The pane's verdict is live too, and changes once, when the
/// printer connects and the preview is drawn for it: four announcements in all.
#[test]
fn connect_is_announced_three_times_and_the_verdict_once() {
    let (_dir, a) = saved("announced", &reference_label().to_json().unwrap());
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    open_preview(&ui);
    let status = || {
        status_of(&ui)
            .accessible_value()
            .unwrap_or_default()
            .to_string()
    };
    let verdict = || {
        group_of(&ui, "Print")
            .expect("the preview is shown")
            .query_descendants()
            .match_accessible_role(AccessibleRole::Text)
            .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Verdict"))
            .find_first()
            .expect("the verdict")
            .accessible_value()
            .unwrap_or_default()
            .to_string()
    };
    let first = status();
    let first_verdict = verdict();
    let mut said: Vec<String> = Vec::new();
    let mut verdicts: Vec<String> = Vec::new();
    let note = |said: &mut Vec<String>, verdicts: &mut Vec<String>| {
        let now = status();
        if now != first && said.last() != Some(&now) {
            said.push(now);
        }
        let now = verdict();
        if now != first_verdict && verdicts.last() != Some(&now) {
            verdicts.push(now);
        }
    };
    button_of(&ui, "Connect").invoke_accessible_default_action();
    settle();
    note(&mut said, &mut verdicts);
    hear(
        &plug,
        Heard::Found {
            ticket: 1,
            seen: vec![Seen {
                key: DeviceKey(0),
                name: D110_M_NAME.into(),
                heard: true,
            }],
        },
    );
    note(&mut said, &mut verdicts);
    let steps = handshake_steps();
    assert_eq!(steps.len(), 11);
    for step in steps {
        hear(&plug, Heard::Step { ticket: 1, step });
        note(&mut said, &mut verdicts);
        let line = group_of(&ui, "Print")
            .expect("the preview is shown")
            .query_descendants()
            .match_accessible_role(AccessibleRole::Text)
            .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Printer link"))
            .find_first()
            .expect("the step line")
            .accessible_value()
            .unwrap_or_default();
        assert_eq!(
            line,
            format!("Connecting to {D110_M_NAME}: {}", step.says()).as_str()
        );
    }
    hear(
        &plug,
        Heard::Ready {
            ticket: 1,
            facts: mock::facts(2320, D110_M_NAME),
        },
    );
    note(&mut said, &mut verdicts);
    assert_eq!(
        said,
        [
            "Looking for printers.",
            "Connecting to D110_M-GB28010920.",
            "Connected to D110_M-GB28010920, a D110_M.",
        ]
    );
    assert_eq!(
        verdicts,
        ["Nothing is refused. This is what would print on D110_M-GB28010920."]
    );
}

/// Tab goes round the issue rows, the primary slot and Close, and stays inside
/// the pane; every stop is named; the image says what it shows in dots.
#[test]
fn every_pane_control_is_named_and_reachable_by_tab() {
    let (_dir, a) = saved("tab_round", KITCHEN_SINK);
    let rows = {
        let plug = Rc::new(HeldPlug::default());
        let (ui, wired) = probe(&a, plug);
        open_preview(&ui);
        let rows = wired.prints.proof().expect("proved").issue_rows().len();
        assert!(rows >= 2, "the kitchen sink has issues to list");
        let items = issue_items(&ui);
        assert_eq!(items.len(), rows);
        for item in &items {
            assert!(item.accessible_label().is_some_and(|l| !l.is_empty()));
        }
        for name in ["Connect", "Close"] {
            assert!(
                button_of(&ui, name)
                    .accessible_description()
                    .is_some_and(|d| !d.is_empty()),
                "{name}"
            );
        }
        let image = group_of(&ui, "Print")
            .unwrap()
            .query_descendants()
            .match_accessible_role(AccessibleRole::Image)
            .find_first()
            .expect("the image");
        assert_eq!(image.accessible_label().as_deref(), Some("What will print"));
        assert!(
            image
                .accessible_description()
                .is_some_and(|d| d.contains("dots")),
            "{:?}",
            image.accessible_description()
        );
        rows
    };

    // The keyboard opens on Connect. Tab by Tab, where Return lands tells which
    // stop has the keyboard: Connect orders a scan, Close closes the pane, and an
    // issue row does neither.
    let stops = rows + 2;
    for tabs in [0, 1, 2, rows + 1, stops, stops + 1] {
        let plug = Rc::new(HeldPlug::default());
        let (ui, _wired) = probe(&a, plug.clone());
        open_preview(&ui);
        for _ in 0..tabs {
            key(&ui, Key::Tab);
        }
        key(&ui, Key::Return);
        let at = tabs % stops;
        let (ordered, open) = (!plug.orders().is_empty(), group_of(&ui, "Print").is_some());
        match at {
            0 => assert_eq!(
                (ordered, open),
                (true, true),
                "{tabs} Tabs is back on Connect"
            ),
            1 => assert_eq!((ordered, open), (false, false), "{tabs} Tabs is on Close"),
            _ => assert_eq!(
                (ordered, open),
                (false, true),
                "{tabs} Tabs is on an issue row, inside the pane"
            ),
        }
        if at == 0 {
            assert_eq!(plug.orders(), [(1, Order::Find { secs: 4 })]);
        }
    }
}

/// Preflight's issues, severity first, in its own order, as items of a list a
/// screen reader can count.
#[test]
fn issues_are_list_items_with_severity_first() {
    let (_dir, a) = saved("issues", KITCHEN_SINK);
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug);
    open_preview(&ui);
    let rows = wired.prints.proof().expect("proved").issue_rows();
    let items = issue_items(&ui);
    let said: Vec<String> = items
        .iter()
        .map(|h| h.accessible_label().unwrap_or_default().to_string())
        .collect();
    assert_eq!(said, rows, "the list is the proof's, in its order");
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item.accessible_item_index(), Some(i));
        assert_eq!(item.accessible_value(), item.accessible_label());
    }
    let refused = said
        .iter()
        .position(|r| r.starts_with("Refused: element 4") && r.contains("{sku}"))
        .expect("the {sku} barcode is refused");
    let warned = said
        .iter()
        .position(|r| r.starts_with("Warning: element 3") && r.contains("{sku}"))
        .expect("the {sku} text is a warning");
    assert_ne!(refused, warned);
    assert!(
        said.iter().all(|r| r.starts_with("Refused: ")
            || r.starts_with("Warning: ")
            || r.starts_with("Note: ")),
        "{said:?}"
    );
}

/// What is connected is read, not announced, and not a Tab stop.
#[test]
fn the_printer_line_is_plain_text() {
    let (_dir, a) = saved("printer_line", &reference_label().to_json().unwrap());
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug);
    let lines: Vec<ElementHandle> = ElementQuery::from_root(&ui)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Printer"))
        .find_all();
    assert_eq!(
        lines.len(),
        1,
        "one Printer line, and no New label pane open"
    );
    let line = &lines[0];
    assert_eq!(line.accessible_role(), Some(AccessibleRole::Text));
    assert_eq!(
        line.accessible_value().as_deref(),
        Some("No printer connected")
    );
    assert!(
        !matches!(
            line.accessible_live_region(),
            Some(AccessibleLiveness::Polite | AccessibleLiveness::Assertive)
        ),
        "{:?}",
        line.accessible_live_region()
    );
    // Tab from the surface reaches the first element, and from there each element
    // in turn: the line above the canvas is never a stop on the way.
    for id in 1..=3 {
        key(&ui, Key::Tab);
        assert_eq!(ui.get_selected(), id);
    }
}

/// Control+P, Connect, and the author's D110_M found and connected on the first
/// ticket, with the keyboard on Close.
fn connected(ui: &InsProbe, plug: &HeldPlug) {
    open_preview(ui);
    button_of(ui, "Connect").invoke_accessible_default_action();
    settle();
    hear(
        plug,
        Heard::Found {
            ticket: 1,
            seen: vec![Seen {
                key: DeviceKey(0),
                name: D110_M_NAME.into(),
                heard: true,
            }],
        },
    );
    hear(
        plug,
        Heard::Ready {
            ticket: 1,
            facts: mock::facts(2320, D110_M_NAME),
        },
    );
}

fn in_pane(ui: &InsProbe, role: AccessibleRole, label: &str) -> ElementHandle {
    let wanted = label.to_string();
    group_of(ui, "Print")
        .expect("the print pane is shown")
        .query_descendants()
        .match_accessible_role(role)
        .match_predicate(move |h| h.accessible_label().is_some_and(|l| l == wanted.as_str()))
        .find_first()
        .unwrap_or_else(|| panic!("no {role:?} in the pane is called {label:?}"))
}

fn news(plug: &HeldPlug, news: RunNews) {
    hear(plug, Heard::Job { ticket: 1, news });
}

fn is_live(h: &ElementHandle) -> bool {
    matches!(
        h.accessible_live_region(),
        Some(AccessibleLiveness::Polite | AccessibleLiveness::Assertive)
    )
}

/// The job bar is a progress indicator with a number a screen reader reads on
/// request: labels printed as a share of the job, from 0 to 1, described by the
/// job line's words. Neither the bar nor the words are live; the status line is.
#[test]
fn the_job_bar_is_a_numeric_progress_indicator() {
    let (_dir, a) = saved("job_bar", &reference_label().to_json().unwrap());
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug);
    in_pane(&ui, AccessibleRole::TextInput, "Copies").set_accessible_value("3");
    settle();
    button_of(&ui, "Print").invoke_accessible_default_action();
    settle();
    news(&plug, RunNews::Checking);
    news(&plug, RunNews::Starting { n: 1, of: 3 });
    news(&plug, RunNews::Printed { n: 1, of: 3 });
    let bar = group_of(&ui, "Print")
        .expect("the print pane is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::ProgressIndicator)
        .find_first()
        .expect("the job bar");
    let value: f64 = bar
        .accessible_value()
        .expect("a value")
        .parse()
        .expect("a number");
    assert!((value - 1.0 / 3.0).abs() < 1e-6, "{value}");
    assert_eq!(bar.accessible_value_minimum(), Some(0.0));
    assert_eq!(bar.accessible_value_maximum(), Some(1.0));
    assert_eq!(bar.accessible_label().as_deref(), Some("Print job"));
    let text = in_pane(&ui, AccessibleRole::Text, "Print job");
    let says = text.accessible_value().unwrap_or_default();
    assert!(!says.is_empty());
    assert_eq!(bar.accessible_description(), Some(says));
    assert!(!is_live(&bar), "{:?}", bar.accessible_live_region());
    assert!(!is_live(&text), "{:?}", text.accessible_live_region());
    assert_eq!(
        status_of(&ui).accessible_live_region(),
        Some(AccessibleLiveness::Polite)
    );
}

/// Copies, Print and Stop after this label each say what they do, and Shift+Tab
/// from Close goes to Print, then Copies, then Disconnect. Where Return lands
/// tells which stop has the keyboard: Print orders a print, Copies does nothing,
/// and Disconnect orders a close.
#[test]
fn copies_print_and_stop_say_what_they_do() {
    let (_dir, a) = saved("copies_print_stop", &reference_label().to_json().unwrap());
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug);
    let copies = in_pane(&ui, AccessibleRole::TextInput, "Copies");
    assert_eq!(
        copies.accessible_description().as_deref(),
        Some("How many labels to print, from 1 to 99")
    );
    assert_eq!(copies.accessible_value().as_deref(), Some("1"));
    copies.set_accessible_value("3");
    settle();
    let print = button_of(&ui, "Print");
    assert_eq!(
        print.accessible_description().as_deref(),
        Some(
            "Print 3 labels, 40 \u{d7} 12 mm, on D110_M-GB28010920 at density 3 on labels with gaps"
        )
    );
    print.invoke_accessible_default_action();
    settle();
    news(&plug, RunNews::Checking);
    news(&plug, RunNews::Starting { n: 1, of: 3 });
    assert_eq!(
        button_of(&ui, "Stop after this label")
            .accessible_description()
            .as_deref(),
        Some("Finish the label in progress, then print no more")
    );

    for back in 1..=3 {
        let plug = Rc::new(HeldPlug::default());
        let (ui, _wired) = probe(&a, plug.clone());
        connected(&ui, &plug);
        let sent = plug.orders().len();
        for _ in 0..back {
            press(ui.window(), Key::Tab, true, false);
            settle();
        }
        key(&ui, Key::Return);
        let new: Vec<Order> = plug.orders()[sent..]
            .iter()
            .map(|(_, o)| o.clone())
            .collect();
        let open = group_of(&ui, "Print").is_some();
        match back {
            1 => assert!(
                matches!(new.as_slice(), [Order::Print { labels: 1, .. }]),
                "one Shift+Tab is Print: {new:?}"
            ),
            2 => assert!(new.is_empty() && open, "two is Copies: {new:?}"),
            _ => assert_eq!(new, [Order::Close], "three is Disconnect"),
        }
    }
}
