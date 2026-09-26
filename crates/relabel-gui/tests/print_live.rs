//! The editor over relabel-core's real printer link: the actor on its own thread
//! and runtime, as the application starts it, over an in-memory D110_M in place of
//! Bluetooth. The test thread has no event loop, so what the link says is pumped
//! to the wiring by the test rather than by `slint::spawn_local`; the one test that
//! runs that loop is print_native.rs.
//!
//! Real time throughout, with caps a healthy run is far inside: the in-memory scan
//! takes 10 ms, the handshake a few more, and the in-memory printer a tenth of a
//! second a label. Its jobs print nothing anywhere: the printer is a responder.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleRole, ElementQuery};
use relabel_core::link::Finished;
use relabel_core::link::mock::{Behaviour, D110_M_NAME, MockDevice, MockOpener};
use relabel_core::transport::mock::{FakePrinter, Responder, d110_m_stuck};
use relabel_gui::plug::Plug;
use relabel_gui::view::Opening;
use relabel_gui::{start, wire_window};
use relabel_proto::RequestId;
use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use support::{InlineImporter, LivePlug, Scripted, button_of, find_button, press};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);
relabel_gui::impl_print_host!(InsProbe, IssueRow, ChoiceRow);

type Wired = relabel_gui::Wired<InsProbe>;

/// The link finds its printer in this long, whatever the scan asks for.
const SCAN: Duration = Duration::from_millis(10);

/// The probe on a new label, wired to a live link over one in-memory D110_M.
fn probe() -> (InsProbe, Wired, Rc<LivePlug>) {
    probe_over(MockDevice::d110_m())
}

fn probe_over(device: MockDevice) -> (InsProbe, Wired, Rc<LivePlug>) {
    support::ensure_platform();
    let (opener, mock) = MockOpener::new(vec![device]);
    let plug = Rc::new(LivePlug::new((opener.with_scan(SCAN), mock)));
    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let wired = wire_window(
        &ui,
        start(None),
        Rc::new(Scripted::default()),
        plug.clone(),
        Rc::new(InlineImporter),
        Opening::Fit,
    );
    ui.show().expect("showing a headless window");
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    (ui, wired, plug)
}

/// Control+P, Connect, and the link's news pumped until the Printer line names the
/// D110_M.
fn connect(ui: &InsProbe, plug: &LivePlug) {
    press(ui.window(), "p", false, true);
    button_of(ui, "Connect").invoke_accessible_default_action();
    let named = plug.pump_until(Duration::from_secs(3), || {
        ui.get_printer_line()
            .starts_with("D110_M-GB28010920: a D110_M")
    });
    assert!(
        named,
        "connected within 3 s; the Printer line says {:?} and the status {:?}",
        ui.get_printer_line(),
        ui.get_status()
    );
}

#[test]
fn a_real_link_connects_and_identifies_a_d110_m() {
    let (ui, wired, plug) = probe();
    connect(&ui, &plug);
    assert_eq!(
        ui.get_printer_line(),
        "D110_M-GB28010920: a D110_M, 96-dot head at 203 dpi, firmware 4.17, battery about \
         three quarters"
    );
    assert_eq!(ui.get_status(), "Connected to D110_M-GB28010920, a D110_M.");
    let proof = wired.prints.proof().expect("proved");
    assert!(proof.printable(), "{:?}", proof.stops);
    assert_eq!(proof.printer.as_ref().map(|p| p.1), Some("D110_M"));

    button_of(&ui, "Disconnect").invoke_accessible_default_action();
    let closed = plug.pump_until(Duration::from_secs(3), || {
        ui.get_printer_line() == "No printer connected"
    });
    assert!(closed, "the Printer line says {:?}", ui.get_printer_line());
    assert_eq!(ui.get_status(), "Disconnected from D110_M-GB28010920.");
    let transport = plug.mock.transport().expect("a printer was opened");
    assert!(transport.is_closed());
    plug.finish(Duration::from_secs(5));
    assert_eq!(plug.finished(), Some(Finished::Clean));
}

/// The window closing with a printer still connected: finishing closes the link
/// before the process would go, well inside its five seconds.
#[test]
fn finishing_closes_a_live_link() {
    let (ui, _wired, plug) = probe();
    connect(&ui, &plug);
    let transport = plug.mock.transport().expect("a printer was opened");
    assert!(!transport.is_closed());
    let began = Instant::now();
    plug.finish(Duration::from_secs(5));
    assert!(began.elapsed() < Duration::from_secs(5));
    assert_eq!(plug.finished(), Some(Finished::Clean));
    assert!(transport.is_closed());
}

/// Copies as an assistive technology writes it.
fn set_copies(ui: &InsProbe, text: &str) {
    ElementQuery::from_root(ui)
        .match_accessible_role(AccessibleRole::TextInput)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "Copies"))
        .find_first()
        .expect("the Copies field")
        .set_accessible_value(text);
}

/// Three labels over the real link: the loop's own jobs, one a label, each on
/// the in-memory printer, and the ending said once they have all printed.
#[test]
fn a_print_over_the_threaded_link_finishes() {
    let (ui, _wired, plug) = probe();
    connect(&ui, &plug);
    set_copies(&ui, "3");
    button_of(&ui, "Print").invoke_accessible_default_action();
    let printed = plug.pump_until(Duration::from_secs(10), || {
        ui.get_status() == "Printed 3 labels on D110_M-GB28010920."
    });
    assert!(
        printed,
        "printed within 10 s; the status says {:?}",
        ui.get_status()
    );
    let transport = plug.mock.transport().expect("a printer was opened");
    let starts = transport
        .received_ids()
        .into_iter()
        .filter(|&id| id == RequestId::PRINT_START.0)
        .count();
    assert_eq!(starts, 3, "one job a label");
    assert!(!transport.is_closed(), "the link stays up");
    plug.finish(Duration::from_secs(5));
    assert_eq!(plug.finished(), Some(Finished::Clean));
}

/// The platform says the printer went during label 1: the job ends at once, it
/// is said once, and the pane offers Connect again.
#[test]
fn a_drop_mid_print_over_the_threaded_link_is_said_once() {
    // Takes the job and never prints, so the drop is inside label 1 however
    // slowly the test runs.
    let stuck = MockDevice::new(D110_M_NAME, true, Behaviour::Answers(d110_m_stuck));
    let (ui, _wired, plug) = probe_over(stuck);
    connect(&ui, &plug);
    button_of(&ui, "Print").invoke_accessible_default_action();
    let sending = plug.pump_until(Duration::from_secs(3), || {
        let t = plug.mock.transport().expect("a printer was opened");
        t.received_ids().contains(&RequestId::PRINT_START.0)
    });
    assert!(
        sending,
        "label 1 started; the status says {:?}",
        ui.get_status()
    );
    let said = RefCell::new(vec![ui.get_status().to_string()]);
    let note = || {
        let now = ui.get_status().to_string();
        let mut said = said.borrow_mut();
        if said.last() != Some(&now) {
            said.push(now);
        }
    };
    plug.mock.drop_link();
    let ended = plug.pump_until(Duration::from_secs(5), || {
        note();
        ui.get_printer_line() == "No printer connected"
    });
    assert!(ended, "the Printer line says {:?}", ui.get_printer_line());
    // Anything more the link had to say.
    plug.pump_until(Duration::from_millis(300), || {
        note();
        false
    });
    let dropped: Vec<String> = said
        .borrow()
        .iter()
        .filter(|s| s.contains("dropped during"))
        .cloned()
        .collect();
    assert_eq!(dropped.len(), 1, "{:?}", said.borrow());
    assert!(
        dropped[0].starts_with("The connection to D110_M-GB28010920 dropped during the label."),
        "{}",
        dropped[0]
    );
    assert_eq!(ui.get_status(), dropped[0], "and nothing after it");
    assert!(
        find_button(&ui, "Connect").is_some(),
        "Connect is offered again"
    );
    plug.finish(Duration::from_secs(5));
}

/// A D110_M taking a second a label: time enough for a press to land within
/// label 1 however slowly the test runs.
fn d110_m_second_a_label() -> Responder {
    FakePrinter::d110_m()
        .per_label(Duration::from_secs(1))
        .responder()
}

/// How many jobs the in-memory printer was sent.
fn starts_of(plug: &LivePlug) -> usize {
    let transport = plug.mock.transport().expect("a printer was opened");
    transport
        .received_ids()
        .into_iter()
        .filter(|&id| id == RequestId::PRINT_START.0)
        .count()
}

/// Waits, pumping nothing to the editor, until label 1's job has reached the
/// printer: the link is then a label ahead of all the editor has heard.
fn label_1_sent_unheard(plug: &LivePlug) {
    let began = Instant::now();
    while starts_of(plug) == 0 {
        assert!(
            began.elapsed() < Duration::from_secs(3),
            "label 1 started within 3 s"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Connected to a D110_M taking a second a label, with Print pressed for three
/// and label 1 on its way before the editor has heard of it.
fn label_1_unheard_of_3() -> (InsProbe, Wired, Rc<LivePlug>) {
    let slow = MockDevice::new(D110_M_NAME, true, Behaviour::Answers(d110_m_second_a_label));
    let (ui, wired, plug) = probe_over(slow);
    connect(&ui, &plug);
    set_copies(&ui, "3");
    button_of(&ui, "Print").invoke_accessible_default_action();
    label_1_sent_unheard(&plug);
    (ui, wired, plug)
}

/// Stop pressed once label 1 has gone to the printer but before the editor has
/// heard it start. The link reads the stop only before each label, so label 1
/// prints: the note promises no more than that, and the ending counts it.
#[test]
fn a_stop_ahead_of_the_news_promises_only_what_the_link_keeps() {
    let (ui, _wired, plug) = label_1_unheard_of_3();
    button_of(&ui, "Stop after this label").invoke_accessible_default_action();
    assert_eq!(
        ui.get_status(),
        "Stopping after the label in progress, if any; no other label will start."
    );
    let stopped = plug.pump_until(Duration::from_secs(10), || {
        ui.get_status().starts_with("Stopped")
    });
    assert!(stopped, "the status says {:?}", ui.get_status());
    assert_eq!(
        ui.get_status(),
        "Stopped after label 1 of 3: 1 printed, 2 not printed."
    );
    assert_eq!(starts_of(&plug), 1);
    plug.finish(Duration::from_secs(5));
    assert_eq!(plug.finished(), Some(Finished::Clean));
}

/// The window closed at the same moment: the close is held for label 1, which
/// the note does not claim will not print, and the window closes once it has.
#[test]
fn a_close_ahead_of_the_news_promises_only_what_the_link_keeps() {
    let (ui, _wired, plug) = label_1_unheard_of_3();
    let w = ui.window();
    w.dispatch_event(WindowEvent::CloseRequested);
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    assert!(w.is_visible(), "held for the job");
    assert_eq!(
        ui.get_status(),
        "Closing once the label in progress, if any, has printed; no other label will start."
    );
    let closed = plug.pump_until(Duration::from_secs(10), || !w.is_visible());
    assert!(closed, "the status says {:?}", ui.get_status());
    assert_eq!(
        ui.get_status(),
        "Stopped after label 1 of 3: 1 printed, 2 not printed."
    );
    assert_eq!(starts_of(&plug), 1);
    plug.finish(Duration::from_secs(5));
    assert_eq!(plug.finished(), Some(Finished::Clean));
}
