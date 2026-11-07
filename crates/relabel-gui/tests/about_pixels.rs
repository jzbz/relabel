//! Slint's logo, as device pixels, in the About pane.
//!
//! One `#[test]` in this file and no more, for `canvas_pixels`'s reason: a
//! snapshot needs the software rasteriser, set by hand, once per process. It is
//! the rasteriser the application ships.
//!
//! What it is for: Slint's licence asks for its AboutSlint widget to be
//! displayed, and the accessibility tree shows only that the widget's logo is
//! there and how big its box is. This shows that the logo is drawn: its SVG is
//! embedded, rasterised, and painted on the pane - the pill in Slint's near-black
//! and the mark in Slint's blue - in the light scheme the probe runs in.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::AccessibleRole;
use relabel_gui::view::Opening;
use relabel_gui::{start, wire_window};
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};
use std::rc::Rc;
use std::time::Duration;
use support::{HeldPlug, InlineImporter, Scripted, group_of, press};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);
relabel_gui::impl_edit_host!(InsProbe, Tool, Grips);
relabel_gui::impl_file_host!(InsProbe, SizePreset);
relabel_gui::impl_print_host!(InsProbe, IssueRow, ChoiceRow);

/// The pill behind the logo, `#151D21`, and the mark on it, `#2379F4`
/// (i-slint-compiler widgets/common/MadeWithSlint-logo-light.svg).
const PILL: (u8, u8, u8) = (0x15, 0x1d, 0x21);
const MARK: (u8, u8, u8) = (0x23, 0x79, 0xf4);

fn near(p: (u8, u8, u8), c: (u8, u8, u8)) -> bool {
    p.0.abs_diff(c.0) <= 8 && p.1.abs_diff(c.1) <= 8 && p.2.abs_diff(c.2) <= 8
}

#[test]
fn the_about_pane_draws_slints_logo() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .expect("the platform is ours to set, once");

    let ui = InsProbe::new().expect("the probe window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let _wired = wire_window(
        &ui,
        start(None),
        Rc::new(Scripted::default()),
        Rc::new(HeldPlug::default()),
        Rc::new(InlineImporter),
        Opening::Fit,
    );
    ui.show().expect("showing a headless window");
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    press(ui.window(), Key::F1, false, false);
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);

    let group = group_of(&ui, "About relabel").expect("the About pane is shown");
    let logo = group
        .query_descendants()
        .match_accessible_role(AccessibleRole::Image)
        .match_predicate(|h| h.accessible_label().is_some_and(|l| l == "#MadeWithSlint"))
        .find_first()
        .expect("the AboutSlint widget's logo");
    // A popup's elements are measured from the popup, which the shell centres
    // across the window and places at `about-top`, 84 logical pixels down in the
    // default window (shell.slint).
    let shot = ui
        .window()
        .take_snapshot()
        .expect("the software renderer draws");
    let (w, h) = (shot.width(), shot.height());
    let pane_x = (w as f32 - group.size().width) / 2.0;
    let (at, size) = (logo.absolute_position(), logo.size());
    let x0 = (pane_x + at.x).round() as u32;
    assert_eq!(ui.get_about_top(), 84.0);
    let y0 = (ui.get_about_top() + at.y).round() as u32;
    let (x1, y1) = (x0 + size.width as u32, y0 + size.height as u32);
    assert!(x1 <= w && y1 <= h, "the logo's box is inside the snapshot");

    let px = shot.as_slice();
    let (mut pill, mut mark) = (0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = px[(y * w + x) as usize];
            let p = (p.r, p.g, p.b);
            pill += u32::from(near(p, PILL));
            mark += u32::from(near(p, MARK));
        }
    }
    let area = (x1 - x0) * (y1 - y0);
    // The logo keeps its shape inside the box, so the pill covers a share of it:
    // 212 by 97 fitted to the box's height, less the white lettering on it.
    assert!(
        pill * 5 > area,
        "{pill} of {area} pixels in the logo's box are the pill's near-black"
    );
    assert!(mark > 100, "{mark} pixels are the mark's blue");
}
