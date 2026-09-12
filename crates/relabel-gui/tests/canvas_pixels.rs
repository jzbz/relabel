//! The dots, as device pixels, at a fractional display scale.
//!
//! One `#[test]` in this file and no more: a snapshot needs a real rasteriser,
//! which means setting the platform by hand with
//! `TestingBackendOptions::renderer_name`, and a platform can be set once per
//! process. The rasteriser it asks for is `software`, which is the one the
//! application ships - so unlike a golden image taken from a GPU renderer, what
//! this measures is what an operator will see.
//!
//! What it is for: `i-slint-renderer-femtovg` translates by an unrounded
//! physical distance (`itemrenderer.rs:897`) while the software renderer rounds
//! its geometry, so "the dots land on whole device pixels" is a claim about a
//! particular rasteriser and has to be asserted against one. At 1.25x a dot is
//! four device pixels and 3.2 logical ones; if anything in the chain worked in
//! logical units, the edges below would be a pixel out or smeared across two.

#[path = "support/mod.rs"]
mod support;

use relabel_gui::canvas::{Canvas, DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;
use support::{canvas_of, reference_label};

include!(concat!(env!("OUT_DIR"), "/canvas-probe.rs"));

relabel_gui::impl_canvas_host!(CanvasProbe, ElRow);

const SCALE: f32 = 1.25;

#[test]
fn a_dot_is_four_device_pixels_with_hard_edges_at_one_and_a_quarter_scale() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .expect("the platform is ours to set, once");

    let ui = CanvasProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE,
    });
    let doc = Doc::open(reference_label(), relabel_proto::PrintDirection::Left).expect("a label");
    let _canvas: Rc<RefCell<Canvas<CanvasProbe>>> = wire(&ui, doc);
    ui.show().expect("showing a headless window");

    let origin = canvas_of(&ui).absolute_position();
    let ox = (origin.x * SCALE).round() as u32;
    let oy = (origin.y * SCALE).round() as u32;

    let shot = ui
        .window()
        .take_snapshot()
        .expect("the software renderer draws");
    let (w, h) = (shot.width(), shot.height());
    let px = shot.as_slice();
    let dark = |x: u32, y: u32| -> bool {
        let p = px[(y * w + x) as usize];
        u32::from(p.r) + u32::from(p.g) + u32::from(p.b) < 3 * 128
    };

    // The first element of the reference label is a filled rectangle at feed 8,
    // head 8, 40 by 24 dots. At four device pixels to the dot that is a black
    // block 160 by 96 device pixels, starting 32 device pixels into the canvas.
    let zoom = DEFAULT_ZOOM;
    let (x0, y0) = (ox + 8 * zoom, oy + 8 * zoom);
    let (x1, y1) = (x0 + 40 * zoom, y0 + 24 * zoom);
    assert!(
        w > x1 + 4 && h > y1 + 4,
        "the snapshot is {w} by {h} device pixels"
    );

    let mid_y = y0 + 12 * zoom;
    assert!(!dark(x0 - 1, mid_y), "the dot before the element is paper");
    assert!(dark(x0, mid_y), "the element's first dot is inked");
    assert!(dark(x1 - 1, mid_y), "the element's last dot is inked");
    assert!(!dark(x1, mid_y), "the dot after the element is paper");

    let mid_x = x0 + 20 * zoom;
    assert!(!dark(mid_x, y0 - 1), "the row before the element is paper");
    assert!(dark(mid_x, y0), "the element's first row is inked");
    assert!(dark(mid_x, y1 - 1), "the element's last row is inked");
    assert!(!dark(mid_x, y1), "the row after the element is paper");

    // Hard edges: no half-lit pixel anywhere along the boundary, which is what a
    // dot grid sitting between two device pixels would produce.
    for y in y0..y1 {
        let p = px[(y * w + x0 - 1) as usize];
        assert_eq!(
            (p.r, p.g, p.b),
            (255, 255, 255),
            "the device pixel left of the element at row {y} is not clean paper"
        );
    }
}
