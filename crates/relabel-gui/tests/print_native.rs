//! The application's own plug, end to end on a real event loop: relabel-core's
//! link on its thread and runtime, and its news delivered by the one
//! `slint::spawn_local` loop `NativePlug` runs, over an in-memory D110_M in place
//! of Bluetooth.
//!
//! A binary of its own with a single test, because an event loop needs the testing
//! backend's integration mode, which can be installed once per process
//! (i-slint-backend-testing lib.rs:48-56). Every other print test runs without a
//! loop, where `spawn_local` cannot start. Mock time, so no Slint timer runs; the
//! link runs on real time, capped at five seconds.

#[path = "support/mod.rs"]
mod support;

use relabel_core::link::mock::{D110_M_NAME, MockDevice, MockOpener};
use relabel_gui::plug::{NativePlug, Plug};
use relabel_gui::view::Opening;
use relabel_gui::{start, wire_window};
use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use support::{Scripted, button_of, press};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);
relabel_gui::impl_print_host!(InsProbe, IssueRow, ChoiceRow);

#[test]
fn the_native_plug_delivers_through_spawn_local() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let (opener, mock) = MockOpener::new(vec![MockDevice::d110_m()]);
    let plug = Rc::new(NativePlug::with_opener(Box::new(
        opener.with_scan(Duration::from_millis(10)),
    )));
    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let _wired = wire_window(
        &ui,
        start(None),
        Rc::new(Scripted::default()),
        plug.clone(),
        Opening::Fit,
    );
    ui.show().expect("showing a headless window");
    press(ui.window(), "p", false, true);
    button_of(&ui, "Connect").invoke_accessible_default_action();
    assert_eq!(ui.get_printer_line(), "Looking for printers");

    // Every 50 ms, a look posted to the loop, which quits it once the Printer line
    // names the printer, or at the cap either way.
    let weak = ui.as_weak();
    let stop = Arc::new(AtomicBool::new(false));
    let looker = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(50));
                let (weak, stop) = (weak.clone(), stop.clone());
                let late = Instant::now() >= deadline;
                let posted = slint::invoke_from_event_loop(move || {
                    let named = weak
                        .upgrade()
                        .is_some_and(|ui| ui.get_printer_line().starts_with(D110_M_NAME));
                    if (named || late) && !stop.swap(true, Ordering::SeqCst) {
                        let _ = slint::quit_event_loop();
                    }
                });
                if posted.is_err() || late {
                    break;
                }
            }
        })
    };
    slint::run_event_loop_until_quit().expect("the event loop runs");
    looker.join().expect("the looker ends");

    assert_eq!(
        ui.get_printer_line(),
        "D110_M-GB28010920: a D110_M, 96-dot head at 203 dpi, firmware 4.17, battery about \
         three quarters"
    );
    assert_eq!(ui.get_status(), "Connected to D110_M-GB28010920, a D110_M.");
    let transport = mock.transport().expect("a printer was opened");
    assert!(!transport.is_closed());
    let began = Instant::now();
    plug.finish(Duration::from_secs(5));
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(transport.is_closed(), "finishing closed the link");
}
