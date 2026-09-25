//! The print pane and the printer link, driven through the probe window as an
//! operator and an assistive technology drive them.
//!
//! Every probe is wired by `relabel_gui::wire_window`, the one wiring the
//! application runs, with a printer link that keeps every order and hears only
//! what the test says it heard: nothing here touches Bluetooth, and nothing
//! prints - a Print is an order kept, and its job only the news the test says.
//! Every file a test writes is under the build's scratch space.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleLiveness, AccessibleRole, ElementHandle};
use relabel_core::link::mock::{self, D110_M_NAME, MockDevice, MockOpener};
use relabel_core::link::{DeviceKey, Facts, Failure, Heard, LinkAfter, Order, Seen};
use relabel_core::{
    HandshakeStep, JobError, Reached, RunEnd, RunNews, RunReport, SessionError, TransportError,
};
use relabel_gui::choose::Chooser;
use relabel_gui::plug::{NativePlug, Plug};
use relabel_gui::printing::{MAX_CHOICES, failure_says, job_says};
use relabel_gui::view::Opening;
use relabel_gui::{Started, start, wire_window};
use relabel_proto::payload::IDENTITY_READS;
use relabel_proto::{LabelType, PrinterErrorCode};
use relabel_render::document::Label;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use support::{
    HeldPlug, Scripted, button_of, down, find_button, frame_of, group_of, press, reference_label,
    rows_of, status_of, tmp_dir, type_text,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);
relabel_gui::impl_print_host!(InsProbe, IssueRow, ChoiceRow);

type Wired = relabel_gui::Wired<InsProbe>;

/// The probe at scale 1 on the label at `path`, wired as the application wires it
/// with `plug` for the printer link, shown, with the change handlers the first
/// layout queued already run.
fn probe(path: &Path, plug: Rc<dyn Plug>) -> (InsProbe, Wired) {
    probe_on(start(Some(path)), plug, 1.0)
}

fn probe_on(started: Started, plug: Rc<dyn Plug>, scale: f32) -> (InsProbe, Wired) {
    probe_with(started, Rc::new(Scripted::default()), plug, scale)
}

fn probe_with(
    started: Started,
    chooser: Rc<dyn Chooser>,
    plug: Rc<dyn Plug>,
    scale: f32,
) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let wired = wire_window(&ui, started, chooser, plug, Opening::Fit);
    ui.show().expect("showing a headless window");
    settle();
    (ui, wired)
}

/// Run the change handlers a push queued, as the event loop would before the next
/// frame: the pane moves the keyboard in one (print.slint, `focus-serial`).
fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
}

/// A scratch directory holding the reference label as `a.relabel`, and the path of
/// each.
fn with_a(test: &str) -> (PathBuf, PathBuf) {
    with_label(test, &reference_label())
}

fn with_label(test: &str, label: &Label) -> (PathBuf, PathBuf) {
    let dir = tmp_dir(&format!("print_flow/{test}"));
    let a = dir.join("a.relabel");
    fs::write(&a, label.to_json().expect("the label saves")).expect("writing the label");
    (dir, a)
}

fn ctrl(ui: &InsProbe, k: &str) {
    press(ui.window(), k, false, true);
    settle();
}

fn key(ui: &InsProbe, k: Key) {
    press(ui.window(), k, false, false);
    settle();
}

/// A button's default action, which is how an assistive technology presses it.
fn activate(ui: &InsProbe, name: &str) {
    button_of(ui, name).invoke_accessible_default_action();
    settle();
}

fn hear(plug: &HeldPlug, heard: Heard) {
    plug.hear(heard);
    settle();
}

/// A text node's value, by the name it announces, inside the Preview pane.
fn pane_text(ui: &InsProbe, label: &str) -> String {
    let wanted = label.to_string();
    group_of(ui, "Print")
        .expect("the preview is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(move |h| h.accessible_label().is_some_and(|l| l == wanted.as_str()))
        .find_first()
        .unwrap_or_else(|| panic!("no text in the pane is called {label:?}"))
        .accessible_value()
        .unwrap_or_default()
        .to_string()
}

fn verdict(ui: &InsProbe) -> String {
    pane_text(ui, "Verdict")
}

fn heard_printer(key: u32, name: &str, heard: bool) -> Seen {
    Seen {
        key: DeviceKey(key),
        name: name.into(),
        heard,
    }
}

fn d110_m() -> Facts {
    mock::facts(2320, D110_M_NAME)
}

/// Control+P, Connect, one D110_M found and connected on the first ticket.
fn connected(ui: &InsProbe, plug: &HeldPlug, facts: Facts) {
    ctrl(ui, "p");
    activate(ui, "Connect");
    hear(
        plug,
        Heard::Found {
            ticket: 1,
            seen: vec![heard_printer(0, &facts.name, true)],
        },
    );
    hear(plug, Heard::Ready { ticket: 1, facts });
}

/// Return held down: auto-repeat's presses with no release between them, each after
/// the change handlers the last one queued, as the event loop runs them.
fn hold_return(ui: &InsProbe) {
    for _ in 0..2 {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
        settle();
    }
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    settle();
}

/// From Close back one Tab stop: to the primary slot, when nothing is connected.
/// Connected to a printer relabel prints on, the job slot and Copies come
/// between.
fn back_to_primary(ui: &InsProbe) {
    press(ui.window(), Key::Tab, true, false);
    settle();
}

fn feed_of(wired: &Wired, id: u32) -> i32 {
    frame_of(wired.canvas.borrow().doc().label(), id).feed
}

/// No Bluetooth before Connect: opening the pane orders nothing, and Connect holds
/// the keyboard, so Return presses it.
#[test]
fn control_p_opens_the_preview_with_connect_holding_the_keyboard() {
    let (_dir, a) = with_a("control_p");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    ctrl(&ui, "p");
    let pane = group_of(&ui, "Print").expect("the preview is shown");
    let verdict = pane
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Verdict"))
        .find_first()
        .expect("the verdict");
    assert_eq!(
        verdict.accessible_live_region(),
        Some(AccessibleLiveness::Polite)
    );
    assert!(
        verdict
            .accessible_value()
            .is_some_and(|v| v.starts_with("Nothing is refused.")),
        "{:?}",
        verdict.accessible_value()
    );
    assert_eq!(plug.orders(), [], "nothing is sent before Connect");
    key(&ui, Key::Return);
    assert_eq!(plug.orders(), [(1, Order::Find { secs: 4 })]);
    assert!(group_of(&ui, "Print").is_some());
}

/// The preview covers the digits in a field: the file command commits it first.
#[test]
fn control_p_from_a_field_commits_the_field_first() {
    let (_dir, a) = with_a("control_p_field");
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug);
    key(&ui, Key::Tab);
    key(&ui, Key::F6);
    key(&ui, Key::Tab);
    type_text(ui.window(), "37");
    ctrl(&ui, "p");
    assert_eq!(feed_of(&wired, 1), 37);
    let proof = wired.prints.proof().expect("a proof was drawn");
    assert_eq!(proof.rev, wired.canvas.borrow().doc().revision());
    assert!(group_of(&ui, "Print").is_some());
}

#[test]
fn connect_orders_a_scan_and_one_printer_is_opened() {
    let (_dir, a) = with_a("connect_one");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    ctrl(&ui, "p");
    activate(&ui, "Connect");
    assert_eq!(plug.orders(), [(1, Order::Find { secs: 4 })]);
    assert_eq!(ui.get_status(), "Looking for printers.");
    assert_eq!(ui.get_printer_line(), "Looking for printers");
    hear(
        &plug,
        Heard::Found {
            ticket: 1,
            seen: vec![heard_printer(7, D110_M_NAME, true)],
        },
    );
    assert_eq!(
        plug.orders().last(),
        Some(&(1, Order::Open { key: DeviceKey(7) }))
    );
    assert_eq!(ui.get_status(), "Connecting to D110_M-GB28010920.");
    assert_eq!(
        pane_text(&ui, "Printer link"),
        "Connecting to D110_M-GB28010920 (up to 15 seconds)"
    );
}

/// Return on Connect, then Return again or Return held down: the primary slot is
/// Stop waiting by then, and the keyboard has left it for Close, so the second
/// press closes the pane and the scan goes on behind it.
#[test]
fn return_twice_or_held_on_connect_closes_the_pane_and_never_stops_the_scan() {
    for held in [false, true] {
        let (_dir, a) = with_a(&format!("return_twice_{held}"));
        let plug = Rc::new(HeldPlug::default());
        let (ui, _wired) = probe(&a, plug.clone());
        ctrl(&ui, "p");
        if held {
            hold_return(&ui);
        } else {
            key(&ui, Key::Return);
            assert_eq!(plug.orders(), [(1, Order::Find { secs: 4 })]);
            key(&ui, Key::Return);
        }
        assert_eq!(
            plug.orders(),
            [(1, Order::Find { secs: 4 })],
            "held {held}: nothing was stopped"
        );
        assert!(
            group_of(&ui, "Print").is_none(),
            "held {held}: Return pressed Close"
        );
        assert_eq!(ui.get_printer_line(), "Looking for printers");
        assert_eq!(ui.get_status(), "Looking for printers.");
    }
}

/// One printer found is opened without asking, and the keyboard goes to Close as it
/// does on choosing. An operator who went back to Stop waiting to stop the scan is
/// not left holding a button that would now stop a connect nobody asked for.
#[test]
fn one_printer_opened_unasked_takes_the_keyboard_off_stop_waiting() {
    let (_dir, a) = with_a("opened_unasked");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    ctrl(&ui, "p");
    key(&ui, Key::Return);
    // Back to the primary slot, which is Stop waiting while it looks.
    back_to_primary(&ui);
    hear(
        &plug,
        Heard::Found {
            ticket: 1,
            seen: vec![heard_printer(7, D110_M_NAME, false)],
        },
    );
    assert_eq!(
        plug.orders().last(),
        Some(&(1, Order::Open { key: DeviceKey(7) }))
    );
    key(&ui, Key::Return);
    assert!(
        !plug.orders().iter().any(|(_, o)| *o == Order::StopWaiting),
        "{:?}",
        plug.orders()
    );
    assert!(group_of(&ui, "Print").is_none(), "Return pressed Close");
    assert_eq!(ui.get_printer_line(), "Connecting to D110_M-GB28010920");
}

/// Return on Stop waiting, then Return again or Return held down: the primary slot
/// is Connect by then, and the keyboard has left it for Close, so the second press
/// closes the pane and never starts the scan the first one stopped.
#[test]
fn return_twice_or_held_on_stop_waiting_closes_the_pane_and_never_looks_again() {
    for held in [false, true] {
        let (_dir, a) = with_a(&format!("stop_twice_{held}"));
        let plug = Rc::new(HeldPlug::default());
        let (ui, _wired) = probe(&a, plug.clone());
        ctrl(&ui, "p");
        key(&ui, Key::Return);
        back_to_primary(&ui);
        let stopped = [(1, Order::Find { secs: 4 }), (2, Order::StopWaiting)];
        if held {
            hold_return(&ui);
        } else {
            key(&ui, Key::Return);
            assert_eq!(plug.orders(), stopped);
            assert_eq!(ui.get_status(), "Stopped looking for printers.");
            key(&ui, Key::Return);
        }
        assert_eq!(plug.orders(), stopped, "held {held}: nothing more was sent");
        assert!(
            group_of(&ui, "Print").is_none(),
            "held {held}: Return pressed Close"
        );
        assert_eq!(ui.get_printer_line(), "No printer connected");
    }
}

/// Return on Cancel while choosing, then Return again or Return held down: Cancel is
/// Connect by then, and the keyboard has left it for Close.
#[test]
fn return_twice_or_held_on_cancel_closes_the_pane_and_never_looks_again() {
    for held in [false, true] {
        let (_dir, a) = with_a(&format!("cancel_twice_{held}"));
        let plug = Rc::new(HeldPlug::default());
        let (ui, _wired) = probe(&a, plug.clone());
        ctrl(&ui, "p");
        activate(&ui, "Connect");
        hear(
            &plug,
            Heard::Found {
                ticket: 1,
                seen: vec![
                    heard_printer(3, "B21-X", true),
                    heard_printer(5, D110_M_NAME, true),
                ],
            },
        );
        // From the first printer, past the second, to Cancel.
        key(&ui, Key::Tab);
        key(&ui, Key::Tab);
        if held {
            hold_return(&ui);
        } else {
            key(&ui, Key::Return);
            assert_eq!(ui.get_status(), "No printer was chosen.");
            key(&ui, Key::Return);
        }
        assert_eq!(
            plug.orders(),
            [(1, Order::Find { secs: 4 })],
            "held {held}: nothing more was sent"
        );
        assert!(
            group_of(&ui, "Print").is_none(),
            "held {held}: Return pressed Close"
        );
        assert_eq!(ui.get_printer_line(), "No printer connected");
    }
}

/// News that turns the primary slot into Connect under the keyboard - none found, a
/// failure while looking or connecting, a dropped link - moves the keyboard to
/// Close, so a Return meant to stop the wait, or to disconnect, starts no scan.
#[test]
fn news_that_turns_the_primary_slot_into_connect_takes_the_keyboard_off_it() {
    let one = || Heard::Found {
        ticket: 1,
        seen: vec![heard_printer(7, D110_M_NAME, true)],
    };
    let cases: [(&str, Vec<Heard>, Heard); 4] = [
        (
            "none found",
            vec![],
            Heard::Found {
                ticket: 1,
                seen: vec![],
            },
        ),
        (
            "a failure while looking",
            vec![],
            Heard::Failed {
                ticket: 1,
                failure: Failure::NoAdapter,
            },
        ),
        (
            "a failure while connecting",
            vec![one()],
            Heard::Failed {
                ticket: 1,
                failure: Failure::DroppedWhileConnecting {
                    name: D110_M_NAME.into(),
                },
            },
        ),
        (
            "a dropped link",
            vec![
                one(),
                Heard::Ready {
                    ticket: 1,
                    facts: d110_m(),
                },
            ],
            Heard::Dropped { ticket: 1 },
        ),
    ];
    for (i, (how, before, news)) in cases.into_iter().enumerate() {
        let (_dir, a) = with_a(&format!("news_moves_{i}"));
        let plug = Rc::new(HeldPlug::default());
        let (ui, _wired) = probe(&a, plug.clone());
        ctrl(&ui, "p");
        key(&ui, Key::Return);
        for h in before {
            hear(&plug, h);
        }
        // Stop waiting; or Disconnect once connected, past Print and Copies.
        back_to_primary(&ui);
        if how == "a dropped link" {
            back_to_primary(&ui);
            back_to_primary(&ui);
        }
        hear(&plug, news);
        assert_eq!(ui.get_printer_line(), "No printer connected", "{how}");
        let sent = plug.orders();
        key(&ui, Key::Return);
        assert_eq!(plug.orders(), sent, "{how}: nothing more was sent");
        assert!(
            group_of(&ui, "Print").is_none(),
            "{how}: Return pressed Close"
        );
    }
}

/// The pane's image is the proof's plane, dot for dot: every pixel read back
/// from what the window was handed, at the whole number of pixels a dot the
/// pane chose.
fn shows_the_plane(ui: &InsProbe, plane: &relabel_proto::BitPlane) {
    let px = ui.get_preview_plane().to_rgb8().expect("an RGB image");
    let k = px.width() as usize / usize::from(plane.width());
    assert!(k >= 1, "shown at a whole number of pixels a dot");
    assert_eq!(px.width() as usize, usize::from(plane.width()) * k);
    assert_eq!(px.height() as usize, usize::from(plane.height()) * k);
    let (w, bytes) = (px.width() as usize, px.as_bytes());
    let mut inked = 0;
    for y in 0..plane.height() {
        for x in 0..plane.width() {
            let at = (usize::from(y) * k * w + usize::from(x) * k) * 3;
            let ink = bytes[at] < 128;
            assert_eq!(ink, plane.get(x, y), "dot ({x}, {y})");
            inked += usize::from(ink);
        }
    }
    assert!(inked > 0, "the label prints something");
}

/// Connected, the label is checked against the printer, the Printer line says what
/// it is, and the keyboard is on Close: Return closes the pane and cannot
/// disconnect.
#[test]
fn ready_redraws_the_preview_for_the_printer() {
    let (_dir, a) = with_a("ready");
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug.clone());
    let before = wired.prints.proof();
    connected(&ui, &plug, d110_m());
    assert!(
        verdict(&ui)
            .starts_with("Nothing is refused. This is what would print on D110_M-GB28010920."),
        "{}",
        verdict(&ui)
    );
    assert!(
        ui.get_printer_line()
            .starts_with("D110_M-GB28010920: a D110_M"),
        "{}",
        ui.get_printer_line()
    );
    let proof = wired.prints.proof().expect("proved");
    assert_ne!(
        before.map(|p| p.generation),
        Some(proof.generation),
        "drawn again for the printer"
    );
    shows_the_plane(&ui, proof.plane.as_ref().expect("a plane"));
    let orders = plug.orders().len();
    key(&ui, Key::Return);
    assert!(group_of(&ui, "Print").is_none(), "Return pressed Close");
    assert_eq!(plug.orders().len(), orders, "and nothing was disconnected");
    assert!(
        ui.get_printer_line()
            .starts_with("D110_M-GB28010920: a D110_M")
    );
}

/// Two printers are offered in the pane itself, with the keyboard on the first,
/// and no other popup opens for the choice.
#[test]
fn several_printers_are_choices_inside_the_same_pane() {
    let found = || Heard::Found {
        ticket: 1,
        seen: vec![
            heard_printer(3, "B21-X", false),
            heard_printer(5, D110_M_NAME, true),
        ],
    };
    for (how, key_wanted) in [("Return", 5), ("the second button", 3)] {
        let (_dir, a) = with_a(&format!(
            "choices_{}",
            if key_wanted == 5 { "first" } else { "second" }
        ));
        let plug = Rc::new(HeldPlug::default());
        let (ui, _wired) = probe(&a, plug.clone());
        ctrl(&ui, "p");
        activate(&ui, "Connect");
        hear(&plug, found());
        let pane = group_of(&ui, "Print").expect("the preview is shown");
        let choices: Vec<ElementHandle> = pane
            .query_descendants()
            .match_accessible_role(AccessibleRole::Button)
            .match_predicate(|h| {
                h.accessible_description()
                    .is_some_and(|d| d.starts_with("Connect to "))
            })
            .find_all();
        let faces: Vec<String> = choices
            .iter()
            .filter_map(|h| h.accessible_label().map(|l| l.to_string()))
            .collect();
        assert_eq!(
            faces,
            ["D110_M-GB28010920", "B21-X, not heard just now"],
            "{how}"
        );
        assert!(group_of(&ui, "Unsaved changes").is_none());
        assert_eq!(ui.get_status(), "Found 2 printers: choose one.");
        if key_wanted == 5 {
            key(&ui, Key::Return);
        } else {
            choices[1].invoke_accessible_default_action();
            settle();
        }
        assert_eq!(
            plug.orders().last(),
            Some(&(
                1,
                Order::Open {
                    key: DeviceKey(key_wanted)
                }
            )),
            "{how}"
        );
        // The keyboard has left the choices for Close, not Stop waiting, so
        // a second Return, or Return held down, cannot stop the connect it
        // just started.
        key(&ui, Key::Return);
        assert!(
            !plug.orders().iter().any(|(_, o)| *o == Order::StopWaiting),
            "{how}: {:?}",
            plug.orders()
        );
        assert!(
            group_of(&ui, "Print").is_none(),
            "{how}: Return pressed Close"
        );
    }
}

/// A printer with another head: the label is not re-fitted, the verdict names
/// Control+N, and the New label pane then offers the connected printer.
#[test]
fn a_label_for_another_head_names_control_n() {
    let (_dir, a) = with_a("another_head");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    let b21 = mock::facts(768, "B21-AB12");
    assert_ne!(b21.head, 96, "a head other than the label's");
    assert!(b21.printable.is_ok());
    connected(&ui, &plug, b21);
    assert!(verdict(&ui).contains("Control+N"), "{}", verdict(&ui));
    assert!(!verdict(&ui).contains("re-fitted"));
    key(&ui, Key::Escape);
    assert!(group_of(&ui, "Print").is_none());
    ctrl(&ui, "n");
    let printer = group_of(&ui, "New label")
        .expect("the New label pane")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Printer"))
        .find_first()
        .expect("the pane's printer line")
        .accessible_value();
    assert_eq!(
        printer.as_deref(),
        Some("For the B21 relabel is connected to")
    );
    // Nothing was assumed, and the status line assumes nothing either.
    activate(&ui, "Create");
    assert!(
        ui.get_status().starts_with("new ") && ui.get_status().contains(" label for a B21: "),
        "{}",
        ui.get_status()
    );
}

/// Connected to a printer relabel cannot make a label for, the New label pane
/// assumes a D110_M and says why, rather than saying nothing is connected.
#[test]
fn a_printer_that_cannot_print_is_still_said_to_be_connected_in_new() {
    let (_dir, a) = with_a("unknown_connected");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    let unknown = mock::facts(9999, "X-1234");
    assert!(unknown.printable.is_err());
    connected(&ui, &plug, unknown);
    assert_eq!(
        ui.get_printer_line(),
        "X-1234: a printer relabel does not know (id 9999)"
    );
    key(&ui, Key::Escape);
    ctrl(&ui, "n");
    let printer = group_of(&ui, "New label")
        .expect("the New label pane")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Printer"))
        .find_first()
        .expect("the pane's printer line")
        .accessible_value();
    assert_eq!(
        printer.as_deref(),
        Some(
            "For a D110_M (assumed: relabel cannot make a label for the printer it is connected to)"
        )
    );
    // And the status line says why once the label is made, as the pane did.
    activate(&ui, "Create");
    assert!(
        ui.get_status().starts_with(
            "new 30 \u{d7} 12 mm label for a D110_M (assumed: relabel cannot make a label for \
             the printer it is connected to): "
        ),
        "{}",
        ui.get_status()
    );
}

#[test]
fn escape_closes_the_preview_and_frees_the_file_commands() {
    let (_dir, a) = with_a("escape");
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug);
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    ctrl(&ui, "p");
    assert!(group_of(&ui, "Print").is_some());
    key(&ui, Key::Escape);
    assert!(group_of(&ui, "Print").is_none());
    assert!(!wired.filing.borrow().previewing());
    ctrl(&ui, "s");
    assert_eq!(ui.get_status(), "saved a.relabel");
    assert_eq!(frame_of(&read(&a), 1).feed, 9);
}

/// An assistive technology can press a File-bar button behind the pane, and is
/// told why nothing happened.
#[test]
fn file_commands_behind_the_preview_are_refused() {
    let (_dir, a) = with_a("behind");
    let bytes = fs::read(&a).unwrap();
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug);
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    ctrl(&ui, "p");
    activate(&ui, "Save label");
    assert_eq!(ui.get_status(), "close the print pane first");
    assert_eq!(fs::read(&a).unwrap(), bytes, "nothing was written");
    assert!(group_of(&ui, "Print").is_some());
}

/// Closing the window is an ordinary close: the question takes the pane's place,
/// once, and answering it leaves nothing behind.
#[test]
fn closing_the_window_with_the_preview_open_asks_once() {
    let (_dir, a) = with_a("close_asks");
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug);
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    ctrl(&ui, "p");
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    settle();
    assert!(ui.window().is_visible());
    assert!(group_of(&ui, "Print").is_none());
    assert!(group_of(&ui, "Unsaved changes").is_some());
    assert!(!wired.filing.borrow().previewing());
    activate(&ui, "Cancel");
    assert!(group_of(&ui, "Unsaved changes").is_none(), "asked once");
    assert!(group_of(&ui, "Print").is_none());
    assert!(ui.window().is_visible());
}

/// An assistive technology's edit reaches the canvas behind the pane, and the pane
/// follows it.
#[test]
fn an_edit_behind_the_preview_redraws_it() {
    let (_dir, a) = with_a("edit_behind");
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug);
    // Taken before the pane opens: its issue rows are list items too.
    let row = rows_of(&ui)[0].clone();
    ctrl(&ui, "p");
    let before = wired.prints.proof().expect("proved");
    row.set_accessible_value("feed 40, head 16");
    settle();
    assert_eq!(feed_of(&wired, 1), 40);
    let after = wired.prints.proof().expect("proved");
    assert_eq!(after.rev, wired.canvas.borrow().doc().revision());
    assert_ne!(after.rev, before.rev);
    assert_ne!(after.stamp, before.stamp, "the image was drawn again");
}

#[test]
fn disconnect_orders_close_and_the_printer_line_returns() {
    let (_dir, a) = with_a("disconnect");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    activate(&ui, "Disconnect");
    assert_eq!(plug.orders().last(), Some(&(1, Order::Close)));
    assert_eq!(
        ui.get_printer_line(),
        "Disconnecting from D110_M-GB28010920"
    );
    assert!(
        find_button(&ui, "Disconnect").is_none(),
        "hidden while closing"
    );
    hear(&plug, Heard::Closed { ticket: 1 });
    assert_eq!(ui.get_printer_line(), "No printer connected");
    assert_eq!(ui.get_status(), "Disconnected from D110_M-GB28010920.");
    assert!(find_button(&ui, "Connect").is_some());
}

/// A fifth File-bar button like the other four: described by its chord, not a Tab
/// stop, and no row of its own, so the view keeps its pinned size.
#[test]
fn the_preview_button_is_on_the_file_bar_and_not_a_tab_stop() {
    let (_dir, a) = with_a("file_bar");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug);
    assert_eq!(
        button_of(&ui, "Print the label")
            .accessible_description()
            .as_deref(),
        Some("Control+P")
    );
    assert_eq!((ui.get_view_px_w(), ui.get_view_px_h()), (709, 525));
    key(&ui, Key::Tab);
    assert_eq!(
        ui.get_selected(),
        1,
        "Tab went from the surface to element 1"
    );
    activate(&ui, "Print the label");
    assert!(group_of(&ui, "Print").is_some());
}

/// Nothing is connected at launch, and nothing is sent, so the New label pane
/// offers the open label's own printer rather than a connected one.
#[test]
fn nothing_is_connected_at_launch_and_new_offers_the_labels_printer() {
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe_on(start(None), plug.clone(), 1.0);
    assert_eq!(ui.get_printer_line(), "No printer connected");
    ctrl(&ui, "n");
    let printer = group_of(&ui, "New label")
        .expect("the New label pane")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Printer"))
        .find_first()
        .expect("the pane's printer line")
        .accessible_value();
    assert_eq!(
        printer.as_deref(),
        Some("For a D110_M, the printer the open label was made for")
    );
    assert_eq!(plug.orders(), []);
}

fn read(path: &Path) -> Label {
    Label::from_json(&fs::read_to_string(path).expect("reading the label")).expect("a label")
}

/// The application's own plug with no event loop to hear the link on, which is
/// what a failed `spawn_local` looks like: the link is let go before it is asked
/// anything, the machine is told the link has ended, and it says so in words.
#[test]
fn a_link_that_cannot_be_heard_is_said_to_have_stopped() {
    let (_dir, a) = with_a("unheard");
    let (opener, mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let plug = Rc::new(NativePlug::with_opener(Box::new(opener)));
    let (ui, _wired) = probe(&a, plug.clone());
    ctrl(&ui, "p");
    activate(&ui, "Connect");
    let stopped =
        "The printer connection stopped working inside relabel; restart relabel to connect again.";
    assert_eq!(ui.get_status(), stopped);
    assert_eq!(pane_text(&ui, "Printer link"), stopped);
    assert_eq!(ui.get_printer_line(), "No printer connected");
    assert!(find_button(&ui, "Connect").is_none(), "no primary slot");
    assert_eq!(mock.opens(), 0);
    // Close holds the keyboard, so Return closes the pane.
    key(&ui, Key::Return);
    assert!(group_of(&ui, "Print").is_none());
    // Nothing is left to wait for.
    plug.finish(Duration::from_secs(5));
}

/// Twelve `{sku}` texts, each one of preflight's warnings: more than the pane
/// lists.
fn many_issues_label() -> Label {
    let elements: Vec<String> = (0..12)
        .map(|i| {
            format!(
                r#"{{"type": "text", "id": {}, "frame": {{"feed": {}, "head": 4, "feed_len": 20, "head_len": 40}}, "text": "{{sku}}", "family": "Noto Sans", "size": 12.0}}"#,
                i + 1,
                4 + 24 * i
            )
        })
        .collect();
    let json = format!(
        r#"{{"format": "relabel-label", "version": 1,
            "media": {{"feed_tenths_mm": 400, "head_tenths_mm": 120}},
            "canvas": {{"feed": 320, "head": 96, "dpmm": {{"num": 8, "den": 1}}}},
            "elements": [{}]}}"#,
        elements.join(",")
    );
    Label::from_json(&json).expect("a label of twelve texts")
}

/// The issue list is the part of the pane that gives way: at its longest the pane
/// still ends inside the window, with Close on screen, and Tab still reaches every
/// row and both buttons.
#[test]
fn a_long_issue_list_scrolls_inside_a_pane_that_fits_the_window() {
    let (_dir, a) = with_label("long_list", &many_issues_label());
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug.clone());
    ctrl(&ui, "p");
    let rows = wired.prints.proof().expect("proved").issue_rows();
    assert_eq!(rows.len(), 8, "{rows:?}");
    assert!(rows[7].starts_with("and "), "{}", rows[7]);
    let pane = group_of(&ui, "Print").expect("the preview is shown");
    let room = ui.window().size().height as f32 - 84.0 - 12.0;
    assert!(
        pane.size().height <= room,
        "the pane is {} tall with {room} to spare",
        pane.size().height
    );
    let close = button_of(&ui, "Close");
    let bottom = close.absolute_position().y + close.size().height;
    assert!(
        bottom <= pane.absolute_position().y + pane.size().height,
        "Close ends at {bottom}, inside the pane"
    );
    // From Connect, round Close, every row and Connect again, to Close: the rows the
    // scroll has clipped are stops too.
    for _ in 0..rows.len() + 3 {
        key(&ui, Key::Tab);
    }
    key(&ui, Key::Return);
    assert!(group_of(&ui, "Print").is_none(), "Return pressed Close");
    assert_eq!(plug.orders(), []);
}

/// Where shell.slint puts the pane's top, in the window: at the view's, below the
/// padding and the two bars' rows. The pane is a popup, and what is inside it is
/// measured from the popup's own top.
const PANE_TOP: f32 = 84.0;

/// Where an element inside the pane ends, down the window.
fn ends_at(h: &ElementHandle) -> f32 {
    PANE_TOP + h.absolute_position().y + h.size().height
}

/// Whether the pane shows its issue list.
fn lists_issues(ui: &InsProbe) -> bool {
    group_of(ui, "Print")
        .expect("the preview is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::List)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Issues"))
        .find_first()
        .is_some()
}

/// The longest step line the link's machine words for the author's printer: every
/// failure, which the step line keeps saying, at every handshake step, with the
/// platform's own words where a failure carries them, as the machine's unit tests
/// have them. Its longest is longer than any step while looking, choosing or
/// connecting.
fn longest_failure() -> Failure {
    let name = || D110_M_NAME.to_string();
    let steps = [HandshakeStep::Connect, HandshakeStep::Status]
        .into_iter()
        .chain(
            IDENTITY_READS
                .iter()
                .zip(1..)
                .map(|(&field, n)| HandshakeStep::Info { field, n }),
        )
        .chain([HandshakeStep::Heartbeat]);
    let failures = [
        Failure::NoBluetooth("org.freedesktop.DBus.Error.FileNotFound".into()),
        Failure::NoAdapter,
        Failure::BluetoothOff,
        Failure::NotAllowed,
        Failure::ScanFailed("Operation already in progress".into()),
        Failure::NotFound { name: name() },
        Failure::TimedOut {
            name: name(),
            secs: 15,
        },
        Failure::ConnectFailed {
            name: name(),
            detail: "le-connection-abort-by-local".into(),
        },
        Failure::NoChannel { name: name() },
        Failure::DroppedWhileConnecting { name: name() },
    ];
    failures
        .into_iter()
        .chain(steps.map(|step| Failure::Handshake { name: name(), step }))
        .max_by_key(|f| failure_says(f).chars().count())
        .expect("a failure")
}

/// The pane is a popup over the panes, and it ends above the status line at every
/// scale, with the longest step line wrapped and the longest issue list: the list is
/// what gives way, so the pane's border never runs through the status line's words.
#[test]
fn the_pane_ends_above_the_status_line_with_the_longest_step_line() {
    let failure = longest_failure();
    let words = failure_says(&failure);
    for (label, list) in [(reference_label(), "none"), (many_issues_label(), "eight")] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let (_dir, a) = with_label(&format!("above_status_{list}_{scale}"), &label);
            let plug = Rc::new(HeldPlug::default());
            let (ui, _wired) = probe_on(start(Some(&a)), plug.clone(), scale);
            ctrl(&ui, "p");
            activate(&ui, "Connect");
            hear(
                &plug,
                Heard::Failed {
                    ticket: 1,
                    failure: failure.clone(),
                },
            );
            assert_eq!(pane_text(&ui, "Printer link"), words);
            assert_eq!(ui.get_status(), words, "the status line says it too");
            let pane = group_of(&ui, "Print").expect("the preview is shown");
            let text = |name: &str| {
                pane.query_descendants()
                    .match_accessible_role(AccessibleRole::Text)
                    .match_predicate({
                        let name = name.to_string();
                        move |h| h.accessible_label().is_some_and(|l| l == name.as_str())
                    })
                    .find_first()
                    .unwrap_or_else(|| panic!("no {name:?} in the pane"))
            };
            let (step, facts) = (text("Printer link"), text("Size and sending"));
            let how = format!("{list} issues at scale {scale}");
            assert!(
                step.size().height >= 1.5 * facts.size().height,
                "{how}: the step line wraps: {} against {}",
                step.size().height,
                facts.size().height
            );
            let bottom = PANE_TOP + pane.size().height;
            let status = status_of(&ui).absolute_position().y;
            assert!(
                bottom <= status,
                "{how}: the pane ends at {bottom}, below the status line's top at {status}"
            );
            let close_end = ends_at(&button_of(&ui, "Close"));
            let step_end = ends_at(&step);
            assert!(
                close_end <= bottom && step_end <= bottom,
                "{how}: Close ends at {close_end} and the step line at {step_end}, inside a \
                 pane ending at {bottom}"
            );
        }
    }
}

/// Every printer the pane offers, each with the longest face a choice has, on the
/// longest issue list and at every scale: the choices and Close end inside the
/// pane, and the pane above the status line. The issue list steps aside while a
/// printer is chosen, since the choices and the list together are taller than the
/// room, and comes back once the choice is cancelled.
#[test]
fn the_pane_ends_above_the_status_line_with_every_choice_offered() {
    for (label, list) in [(reference_label(), "none"), (many_issues_label(), "eight")] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let (_dir, a) = with_label(&format!("choices_above_status_{list}_{scale}"), &label);
            let plug = Rc::new(HeldPlug::default());
            let (ui, _wired) = probe_on(start(Some(&a)), plug.clone(), scale);
            ctrl(&ui, "p");
            let listed = lists_issues(&ui);
            assert_eq!(listed, list == "eight", "{list} issues: the list is shown");
            activate(&ui, "Connect");
            // More than are offered, none heard, so every face is its longest.
            let seen = (0..MAX_CHOICES + 2)
                .map(|i| heard_printer(i as u32, &format!("{D110_M_NAME}{i}"), false))
                .collect();
            hear(&plug, Heard::Found { ticket: 1, seen });
            let how = format!("{list} issues at scale {scale}");
            let pane = group_of(&ui, "Print").expect("the preview is shown");
            let choices: Vec<ElementHandle> = pane
                .query_descendants()
                .match_accessible_role(AccessibleRole::Button)
                .match_predicate(|h| {
                    h.accessible_description()
                        .is_some_and(|d| d.starts_with("Connect to "))
                })
                .find_all();
            assert_eq!(choices.len(), MAX_CHOICES, "{how}");
            assert!(!lists_issues(&ui), "{how}: the list steps aside");
            let bottom = PANE_TOP + pane.size().height;
            let status = status_of(&ui).absolute_position().y;
            assert!(
                bottom <= status,
                "{how}: the pane ends at {bottom}, below the status line's top at {status}"
            );
            for (i, choice) in choices.iter().enumerate() {
                let end = ends_at(choice);
                assert!(
                    end <= bottom,
                    "{how}: choice {i} ends at {end}, past a pane ending at {bottom}"
                );
            }
            let close_end = ends_at(&button_of(&ui, "Close"));
            assert!(
                close_end <= bottom,
                "{how}: Close ends at {close_end}, past a pane ending at {bottom}"
            );
            activate(&ui, "Cancel");
            assert_eq!(lists_issues(&ui), listed, "{how}: the list is back");
        }
    }
}

// --- Printing ---

/// The corpus label with one element of every kind, two of them bound to a
/// `{sku}` the editor has no value for, one of which preflight refuses. Read,
/// never written: a test saves a copy of its own.
const KITCHEN_SINK: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../relabel-render/tests/corpus/v1/kitchen-sink.relabel"
));

/// A label whose one text prints the date, which the editor stops.
fn clock_label() -> Label {
    Label::from_json(
        r#"{"format": "relabel-label", "version": 1,
            "media": {"feed_tenths_mm": 400, "head_tenths_mm": 120},
            "canvas": {"feed": 320, "head": 96, "dpmm": {"num": 8, "den": 1}},
            "elements": [{"type": "text", "id": 1,
                "frame": {"feed": 8, "head": 8, "feed_len": 200, "head_len": 40},
                "text": "{dt}", "family": "Noto Sans", "size": 12.0}]}"#,
    )
    .expect("a label that prints the date")
}

/// What the link says of the job on the open's ticket.
fn news(plug: &HeldPlug, news: RunNews) {
    hear(plug, Heard::Job { ticket: 1, news });
}

/// The job ends, with the link still up.
fn ended(plug: &HeldPlug, printed: u16, of: u16, end: RunEnd) {
    hear(
        plug,
        Heard::JobEnded {
            ticket: 1,
            report: RunReport { printed, of, end },
            link: LinkAfter::Up,
        },
    );
}

/// The prints ordered so far, as (ticket, labels).
fn prints_of(plug: &HeldPlug) -> Vec<(u64, u16)> {
    plug.orders()
        .into_iter()
        .filter_map(|(t, o)| match o {
            Order::Print { labels, .. } => Some((t, labels)),
            _ => None,
        })
        .collect()
}

fn stops_of(plug: &HeldPlug) -> usize {
    plug.orders()
        .iter()
        .filter(|(_, o)| *o == Order::StopAfterLabel)
        .count()
}

/// The pane's Copies field.
fn copies_field(ui: &InsProbe) -> ElementHandle {
    group_of(ui, "Print")
        .expect("the print pane is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::TextInput)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Copies"))
        .find_first()
        .expect("the Copies field")
}

/// Copies written as an assistive technology writes it, which is also how the
/// field's `edited` is reached.
fn set_copies(ui: &InsProbe, text: &str) {
    copies_field(ui).set_accessible_value(text);
    settle();
}

/// The pane's image, and what it is called.
fn image_label(ui: &InsProbe) -> String {
    group_of(ui, "Print")
        .expect("the print pane is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Image)
        .find_first()
        .expect("the image")
        .accessible_label()
        .unwrap_or_default()
        .to_string()
}

/// Print from the keyboard: from Close, the job slot is the Tab stop before it.
fn print_by_keyboard(ui: &InsProbe) {
    back_to_primary(ui);
    key(ui, Key::Return);
}

/// Print sends the proof's own page, the one the pane shows, never encoded again:
/// one label from the job slot, then three once Copies says so.
#[test]
fn print_sends_the_proofs_own_page() {
    let (_dir, a) = with_a("print_page");
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    let proof = wired.prints.proof().expect("proved");
    assert!(proof.printable(), "{:?}", proof.stops);
    let page = proof.page.clone().expect("a page");
    print_by_keyboard(&ui);
    assert_eq!(
        plug.orders().last(),
        Some(&(
            1,
            Order::Print {
                page: page.clone(),
                labels: 1
            }
        ))
    );
    assert_eq!(ui.get_status(), "Printing on D110_M-GB28010920.");
    assert!(wired.prints.busy());
    ended(&plug, 1, 1, RunEnd::Done);
    assert_eq!(ui.get_status(), "Printed on D110_M-GB28010920.");
    assert!(!wired.prints.busy());

    // Past Print to Copies, whose text Tab selects; then back to Print.
    back_to_primary(&ui);
    back_to_primary(&ui);
    type_text(ui.window(), "3");
    settle();
    assert_eq!(copies_field(&ui).accessible_value().as_deref(), Some("3"));
    key(&ui, Key::Tab);
    key(&ui, Key::Return);
    assert_eq!(
        plug.orders().last(),
        Some(&(1, Order::Print { page, labels: 3 }))
    );
    assert_eq!(ui.get_status(), "Printing 3 labels on D110_M-GB28010920.");
}

/// The job slot's action, as an assistive technology can invoke it on a button
/// that is hidden, prints nothing whenever there is nothing the machine would
/// print, and the status line says why where the machine has something to say.
#[test]
fn an_action_on_a_hidden_print_prints_nothing() {
    let other: Label = Label::from_json(KITCHEN_SINK).expect("the kitchen sink");
    let cases: [(&str, Label, Option<Facts>); 4] = [
        ("while off", reference_label(), None),
        ("a {dt} label", clock_label(), Some(d110_m())),
        (
            "a label for another printer",
            reference_label(),
            Some(mock::facts(768, "B21-AB12")),
        ),
        ("a preflight refusal", other, Some(d110_m())),
    ];
    for (i, (how, label, facts)) in cases.into_iter().enumerate() {
        let (_dir, a) = with_label(&format!("hidden_print_{i}"), &label);
        let plug = Rc::new(HeldPlug::default());
        let (ui, wired) = probe(&a, plug.clone());
        match facts {
            Some(facts) => connected(&ui, &plug, facts),
            None => ctrl(&ui, "p"),
        }
        // Drawn for no printer, or stopped or refused.
        let proof = wired.prints.proof().expect("proved");
        assert!(
            proof.printer.is_none() || !proof.printable(),
            "{how}: nothing here prints"
        );
        ui.invoke_preview_act(2);
        settle();
        assert_eq!(prints_of(&plug), [], "{how}");
        assert!(
            ui.get_status().starts_with("Nothing printed"),
            "{how}: {}",
            ui.get_status()
        );
        assert!(!wired.prints.busy(), "{how}");
    }

    // While a job of one label runs, the slot is hidden, and its action is Stop
    // after this label, which one label leaves nothing to stop.
    let (_dir, a) = with_a("hidden_print_printing");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    print_by_keyboard(&ui);
    assert!(find_button(&ui, "Stop after this label").is_none());
    let sent = plug.orders();
    ui.invoke_preview_act(2);
    settle();
    assert_eq!(plug.orders(), sent, "no second Print, and no stop");
    assert_eq!(ui.get_status(), "Printing on D110_M-GB28010920.");
}

/// Return held on Print: the first press prints, the job slot is Stop's or hidden
/// by the second, and the keyboard is on Close, which the second press presses.
#[test]
fn holding_return_on_print_prints_once() {
    let (_dir, a) = with_a("hold_print");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    back_to_primary(&ui);
    hold_return(&ui);
    assert_eq!(prints_of(&plug), [(1, 1)]);
    assert_eq!(stops_of(&plug), 0);
    assert!(group_of(&ui, "Print").is_none(), "Return pressed Close");
}

/// Return in the Copies field does nothing: only the job slot prints.
#[test]
fn return_in_copies_never_prints() {
    let (_dir, a) = with_a("return_in_copies");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    back_to_primary(&ui);
    back_to_primary(&ui);
    let sent = plug.orders();
    for _ in 0..3 {
        key(&ui, Key::Return);
    }
    assert_eq!(plug.orders(), sent);
    assert!(group_of(&ui, "Print").is_some(), "the pane stays open");
}

/// Return held on Connect through the scan, the connect and the preview drawn
/// again for the printer: the keyboard has gone to Close, so the repeat closes the
/// pane, and nothing prints.
#[test]
fn the_return_that_connected_cannot_print() {
    let (_dir, a) = with_a("return_connected");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    ctrl(&ui, "p");
    let pressed = || {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
        settle();
    };
    pressed();
    hear(
        &plug,
        Heard::Found {
            ticket: 1,
            seen: vec![heard_printer(0, D110_M_NAME, true)],
        },
    );
    hear(
        &plug,
        Heard::Ready {
            ticket: 1,
            facts: d110_m(),
        },
    );
    assert!(find_button(&ui, "Print").is_some(), "Print is offered");
    pressed();
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    settle();
    assert_eq!(
        plug.orders(),
        [
            (1, Order::Find { secs: 4 }),
            (1, Order::Open { key: DeviceKey(0) })
        ]
    );
    assert!(
        group_of(&ui, "Print").is_none(),
        "the keyboard was on Close"
    );
}

/// News that changes the job slot under the keyboard moves the keyboard to Close:
/// a drop takes Print away, and the last label's start takes Stop away. Either
/// way the next Return presses Close and orders nothing.
#[test]
fn news_moves_the_keyboard_off_a_changed_slot() {
    // On Print when the link drops.
    let (_dir, a) = with_a("slot_moves_drop");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    back_to_primary(&ui);
    hear(&plug, Heard::Dropped { ticket: 1 });
    let sent = plug.orders();
    key(&ui, Key::Return);
    assert_eq!(plug.orders(), sent, "a drop: nothing more was sent");
    assert!(
        group_of(&ui, "Print").is_none(),
        "a drop: Return pressed Close"
    );

    // On Stop when the last label starts.
    let (_dir, a) = with_a("slot_moves_last");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    set_copies(&ui, "2");
    print_by_keyboard(&ui);
    assert_eq!(prints_of(&plug), [(1, 2)]);
    // Copies and Disconnect are hidden while printing: Stop is the stop before
    // Close.
    back_to_primary(&ui);
    news(&plug, RunNews::Checking);
    news(&plug, RunNews::Starting { n: 1, of: 2 });
    assert!(find_button(&ui, "Stop after this label").is_some());
    news(&plug, RunNews::Printed { n: 1, of: 2 });
    news(&plug, RunNews::Starting { n: 2, of: 2 });
    assert!(find_button(&ui, "Stop after this label").is_none());
    let sent = plug.orders();
    key(&ui, Key::Return);
    assert_eq!(plug.orders(), sent, "the last label: nothing more was sent");
    assert_eq!(stops_of(&plug), 0);
    assert!(
        group_of(&ui, "Print").is_none(),
        "the last label: Return pressed Close"
    );
}

/// Stop after this label is its own order, on the open's ticket: the slot hides
/// under the keyboard that pressed it, and the ending counts what printed.
#[test]
fn stop_after_this_label_stops_after_it() {
    let (_dir, a) = with_a("stop_after");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    set_copies(&ui, "3");
    print_by_keyboard(&ui);
    news(&plug, RunNews::Checking);
    news(&plug, RunNews::Starting { n: 1, of: 3 });
    back_to_primary(&ui);
    key(&ui, Key::Return);
    assert_eq!(plug.orders().last(), Some(&(1, Order::StopAfterLabel)));
    assert_eq!(
        ui.get_status(),
        "Stopping after the label in progress; no other label will start."
    );
    assert!(find_button(&ui, "Stop after this label").is_none());
    // The keyboard went to Close, so a second Return closes the pane.
    key(&ui, Key::Return);
    assert!(group_of(&ui, "Print").is_none());
    assert_eq!(stops_of(&plug), 1);
    ended(&plug, 1, 3, RunEnd::Stopped);
    assert_eq!(
        ui.get_status(),
        "Stopped after label 1 of 3: 1 printed, 2 not printed."
    );
    assert_eq!(prints_of(&plug), [(1, 3)]);
}

/// The job line counts labels printed, and says in words what the printer is
/// doing, while the status line keeps the job's start.
#[test]
fn the_job_line_counts_labels() {
    let (_dir, a) = with_a("job_line");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    set_copies(&ui, "3");
    activate(&ui, "Print");
    assert_eq!(prints_of(&plug), [(1, 3)]);
    assert_eq!(
        pane_text(&ui, "Print job"),
        "Checking that the printer answers."
    );
    news(&plug, RunNews::Checking);
    news(&plug, RunNews::Starting { n: 1, of: 3 });
    news(
        &plug,
        RunNews::Printer {
            n: 1,
            of: 3,
            print_percent: 60,
            feed_percent: 0,
        },
    );
    assert_eq!(
        pane_text(&ui, "Print job"),
        "Label 1 of 3: the printer says 60% printed, 0% fed."
    );
    news(&plug, RunNews::Printed { n: 1, of: 3 });
    news(&plug, RunNews::Starting { n: 2, of: 3 });
    assert_eq!(pane_text(&ui, "Print job"), "Label 2 of 3: sending.");
    let bar = group_of(&ui, "Print")
        .expect("the print pane is shown")
        .query_descendants()
        .match_accessible_role(AccessibleRole::ProgressIndicator)
        .find_first()
        .expect("the job bar");
    let done: f64 = bar
        .accessible_value()
        .expect("a value")
        .parse()
        .expect("a number");
    assert!((done - 1.0 / 3.0).abs() < 1e-6, "{done}");
    assert_eq!(
        ui.get_printer_line(),
        "D110_M-GB28010920: printing label 2 of 3"
    );
    assert_eq!(ui.get_status(), "Printing 3 labels on D110_M-GB28010920.");
    ended(&plug, 3, 3, RunEnd::Done);
    assert_eq!(ui.get_status(), "Printed 3 labels on D110_M-GB28010920.");
    assert_eq!(
        pane_text(&ui, "Printer link"),
        "Printed 3 labels on D110_M-GB28010920."
    );
    assert!(find_button(&ui, "Print").is_some(), "Print is back");
    assert!(
        ui.get_printer_line()
            .starts_with("D110_M-GB28010920: a D110_M")
    );
}

/// While a job runs the pane shows the job's own proof, whatever is edited behind
/// it, and the document's once the job has ended.
#[test]
fn the_pane_shows_what_is_printing() {
    let (_dir, a) = with_a("what_is_printing");
    let plug = Rc::new(HeldPlug::default());
    let (ui, wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    assert_eq!(image_label(&ui), "What will print");
    activate(&ui, "Print");
    let job = wired.prints.proof().expect("proved");
    assert_eq!(image_label(&ui), "What is printing");
    key(&ui, Key::Escape);
    assert!(group_of(&ui, "Print").is_none());
    let before = feed_of(&wired, 1);
    key(&ui, Key::Tab);
    key(&ui, Key::RightArrow);
    assert_ne!(feed_of(&wired, 1), before, "the element moved");
    ctrl(&ui, "p");
    let shown = wired.prints.proof().expect("proved");
    assert_eq!(shown.stamp, job.stamp, "the job's own proof");
    assert_eq!(shown.rev, job.rev);
    assert_eq!(image_label(&ui), "What is printing");
    ended(&plug, 1, 1, RunEnd::Done);
    let after = wired.prints.proof().expect("proved");
    assert_ne!(after.stamp, job.stamp, "drawn for the edited document");
    assert_eq!(after.rev, wired.canvas.borrow().doc().revision());
    assert_eq!(image_label(&ui), "What will print");
}

/// Closing the window during a job keeps it until the label in hand has printed,
/// then closes it as the close button would have: at once when nothing is
/// unsaved, with the question once when something is, and not at all while an
/// element is held.
#[test]
fn closing_the_window_during_a_job_waits_then_closes() {
    for case in ["clean", "dirty", "held"] {
        let (_dir, a) = with_a(&format!("close_during_{case}"));
        let plug = Rc::new(HeldPlug::default());
        let (ui, wired) = probe(&a, plug.clone());
        if case == "dirty" {
            key(&ui, Key::Tab);
            key(&ui, Key::RightArrow);
            key(&ui, Key::Escape);
        }
        connected(&ui, &plug, d110_m());
        set_copies(&ui, "2");
        activate(&ui, "Print");
        news(&plug, RunNews::Checking);
        news(&plug, RunNews::Starting { n: 1, of: 2 });
        let w = ui.window();
        let held_at = if case == "held" {
            key(&ui, Key::Escape);
            let row = &rows_of(&ui)[0];
            let (at, size) = (row.absolute_position(), row.size());
            let centre =
                slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
            down(w, centre);
            assert!(wired.canvas.borrow().doc().pressed());
            Some(centre)
        } else {
            None
        };
        w.dispatch_event(WindowEvent::CloseRequested);
        settle();
        assert!(w.is_visible(), "{case}: held for the job");
        assert_eq!(stops_of(&plug), 1, "{case}");
        assert_eq!(
            ui.get_status(),
            "Closing once the label in progress has printed; no other label will start.",
            "{case}"
        );
        // Asked again, it says the same again, and orders nothing more.
        w.dispatch_event(WindowEvent::CloseRequested);
        settle();
        assert_eq!(stops_of(&plug), 1, "{case}");
        assert!(w.is_visible(), "{case}");
        assert!(group_of(&ui, "Unsaved changes").is_none(), "{case}");
        ended(&plug, 1, 2, RunEnd::Stopped);
        match case {
            "clean" => assert!(!w.is_visible(), "closed once the label printed"),
            "dirty" => {
                assert!(w.is_visible());
                assert!(group_of(&ui, "Unsaved changes").is_some(), "asked once");
                assert!(group_of(&ui, "Print").is_none());
                activate(&ui, "Cancel");
                assert!(group_of(&ui, "Unsaved changes").is_none(), "asked once");
                assert!(w.is_visible());
            }
            _ => {
                assert!(w.is_visible());
                assert_eq!(
                    ui.get_status(),
                    "nothing was closed: let go of the element first"
                );
                support::up(w, held_at.expect("held"));
                settle();
                assert!(w.is_visible(), "the release closes nothing");
            }
        }
    }
}

/// With the pane closed, a job goes on behind the file commands: New and Open are
/// allowed, nothing is ordered for them, and the job keeps its own proof.
#[test]
fn new_and_open_are_allowed_during_a_job() {
    let (_dir, a) = with_a("new_open_during");
    let plug = Rc::new(HeldPlug::default());
    let chooser = Rc::new(Scripted::default());
    let (ui, wired) = probe_with(start(Some(&a)), chooser.clone(), plug.clone(), 1.0);
    connected(&ui, &plug, d110_m());
    activate(&ui, "Print");
    let job = wired.prints.proof().expect("proved");
    key(&ui, Key::Escape);
    let sent = plug.orders();
    ctrl(&ui, "n");
    assert!(group_of(&ui, "New label").is_some(), "{}", ui.get_status());
    activate(&ui, "Cancel");
    assert!(group_of(&ui, "New label").is_none());
    ctrl(&ui, "o");
    assert_eq!(chooser.asked().len(), 1, "the chooser was asked");
    assert_eq!(plug.orders(), sent, "nothing reached the plug");
    assert!(wired.prints.busy());
    let shown = wired.prints.proof().expect("proved");
    assert_eq!((shown.stamp, shown.rev), (job.stamp, job.rev));
}

/// Copies is kept for the life of the window: the pane opens again on what was
/// typed, and Print prints that many.
#[test]
fn copies_text_is_kept_for_the_run() {
    let (_dir, a) = with_a("copies_kept");
    let plug = Rc::new(HeldPlug::default());
    let (ui, _wired) = probe(&a, plug.clone());
    connected(&ui, &plug, d110_m());
    back_to_primary(&ui);
    back_to_primary(&ui);
    type_text(ui.window(), "3");
    settle();
    key(&ui, Key::Escape);
    assert!(group_of(&ui, "Print").is_none());
    ctrl(&ui, "p");
    assert_eq!(copies_field(&ui).accessible_value().as_deref(), Some("3"));
    print_by_keyboard(&ui);
    assert_eq!(prints_of(&plug), [(1, 3)]);
}

/// The longest ending a job of the author's printer can have on the step line:
/// every fault the printer can report and every other way a job fails, at every
/// point, for jobs of one label and of the most.
fn longest_job_failure() -> RunReport {
    let mut errors: Vec<JobError> = (0..=u8::MAX)
        .map(|b| JobError::Session(SessionError::Printer(PrinterErrorCode::from_byte(b))))
        .collect();
    errors.extend(
        [
            SessionError::Transport(TransportError::Disconnected),
            SessionError::Timeout {
                what: "PrintStatus",
                ms: 900,
            },
            SessionError::OneWayCommand(0x85),
            SessionError::Ended,
        ]
        .map(JobError::Session),
    );
    errors.extend(
        [
            "SetLabelType",
            "SetDensity",
            "PrintStart",
            "PrintClear",
            "PageStart",
            "PageEnd",
        ]
        .map(|step| JobError::Refused { step }),
    );
    errors.extend([
        JobError::Stalled {
            reached: 0,
            expected: 1,
        },
        JobError::DeadlineExceeded { ms: 25_000 },
        JobError::Cancelled,
    ]);
    let mut reports = Vec::new();
    for error in &errors {
        for reached in [
            Reached::Check,
            Reached::Settings,
            Reached::Rows,
            Reached::Waiting,
        ] {
            for (label, printed, of) in [(1, 0, 1), (1, 0, 99), (98, 97, 99)] {
                let label = if reached == Reached::Check { 1 } else { label };
                let printed = if reached == Reached::Check {
                    0
                } else {
                    printed
                };
                reports.push(RunReport {
                    printed,
                    of,
                    end: RunEnd::Failed {
                        label,
                        reached,
                        error: error.clone(),
                    },
                });
            }
        }
    }
    reports
        .into_iter()
        .max_by_key(|r| {
            job_says(D110_M_NAME, r, LinkAfter::Up, (3, LabelType::Gap))
                .chars()
                .count()
        })
        .expect("a failure")
}

/// The pane with its job line, and with the longest job failure on its step line,
/// ends above the status line at every scale, with the job line, the step line
/// and Close inside it, and the view keeps its pinned size.
#[test]
fn the_pane_fits_with_the_job_line() {
    let report = longest_job_failure();
    let words = job_says(D110_M_NAME, &report, LinkAfter::Up, (3, LabelType::Gap));
    for (label, list) in [(reference_label(), "none"), (many_issues_label(), "eight")] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for case in ["printing", "failed"] {
                let (_dir, a) = with_label(&format!("fits_{case}_{list}_{scale}"), &label);
                let plug = Rc::new(HeldPlug::default());
                let (ui, _wired) = probe_on(start(Some(&a)), plug.clone(), scale);
                let how = format!("{case} with {list} issues at scale {scale}");
                if scale == 1.0 {
                    assert_eq!((ui.get_view_px_w(), ui.get_view_px_h()), (709, 525));
                }
                connected(&ui, &plug, d110_m());
                set_copies(&ui, "99");
                activate(&ui, "Print");
                assert_eq!(prints_of(&plug), [(1, 99)], "{how}");
                news(
                    &plug,
                    RunNews::Printer {
                        n: 99,
                        of: 99,
                        print_percent: 100,
                        feed_percent: 100,
                    },
                );
                let line = "Label 99 of 99: the printer says 100% printed, 100% fed.";
                let watched = if case == "printing" {
                    assert_eq!(pane_text(&ui, "Print job"), line, "{how}");
                    "Print job"
                } else {
                    hear(
                        &plug,
                        Heard::JobEnded {
                            ticket: 1,
                            report: report.clone(),
                            link: LinkAfter::Up,
                        },
                    );
                    assert_eq!(pane_text(&ui, "Printer link"), words, "{how}");
                    assert!(find_button(&ui, "Print").is_some(), "{how}");
                    "Printer link"
                };
                let pane = group_of(&ui, "Print").expect("the print pane is shown");
                let bottom = PANE_TOP + pane.size().height;
                let status = status_of(&ui).absolute_position().y;
                assert!(
                    bottom <= status,
                    "{how}: the pane ends at {bottom}, below the status line's top at {status}"
                );
                let text = pane
                    .query_descendants()
                    .match_accessible_role(AccessibleRole::Text)
                    .match_predicate(move |h| h.accessible_label().is_some_and(|l| l == watched))
                    .find_first()
                    .expect("the watched line");
                for (name, end) in [
                    (watched, ends_at(&text)),
                    ("Close", ends_at(&button_of(&ui, "Close"))),
                ] {
                    assert!(
                        end <= bottom,
                        "{how}: {name} ends at {end}, past a pane ending at {bottom}"
                    );
                }
                // Across: the button row, at its widest once the job has ended,
                // stays inside the pane, measured from the pane's own left.
                if case == "failed" {
                    let width = pane.size().width;
                    let copies = copies_field(&ui);
                    let close = button_of(&ui, "Close");
                    let right = close.absolute_position().x + close.size().width;
                    assert!(
                        copies.absolute_position().x >= 0.0 && right <= width,
                        "{how}: the row runs from {} to {right} in a pane {width} wide",
                        copies.absolute_position().x
                    );
                }
            }
        }
    }
}
