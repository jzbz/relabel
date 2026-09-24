//! The editor over relabel-core's real printer link: the actor on its own thread
//! and runtime, as the application starts it, over an in-memory D110_M in place of
//! Bluetooth. The test thread has no event loop, so what the link says is pumped
//! to the wiring by the test rather than by `slint::spawn_local`; the one test that
//! runs that loop is print_native.rs.
//!
//! Real time throughout, with caps a healthy run is far inside: the in-memory scan
//! takes 10 ms and the handshake a few more.

#[path = "support/mod.rs"]
mod support;

use relabel_core::link::Finished;
use relabel_core::link::mock::{MockDevice, MockOpener};
use relabel_gui::plug::Plug;
use relabel_gui::view::Opening;
use relabel_gui::{start, wire_window};
use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::rc::Rc;
use std::time::{Duration, Instant};
use support::{LivePlug, Scripted, button_of, press};

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
    support::ensure_platform();
    let (opener, mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let plug = Rc::new(LivePlug::new((opener.with_scan(SCAN), mock)));
    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let wired = wire_window(
        &ui,
        start(None),
        Rc::new(Scripted::default()),
        plug.clone(),
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
