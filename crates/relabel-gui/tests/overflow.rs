//! What the view paints, measured with the software renderer rather than asserted.
//!
//! The reference label at zoom 4 is 1280 device pixels wide and the view beside the
//! pane is 709, so a canvas the view failed to clip would paint over the gap between
//! the view and the pane; and at Fit the far end of the label must be on screen,
//! which was the first thing an operator met before there was a fit.

use relabel_gui::canvas::{DEFAULT_ZOOM, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins;
use relabel_gui::view::{self, Opening};
use relabel_render::document::{Common, Element, ElementOrRaw, Label, RectDots, RectEl, Rot};
use relabel_render::target::Dpmm;
use slint::ComponentHandle;

use i_slint_backend_testing::ElementHandle;

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);

fn label() -> Label {
    let mut l = Label::new(320, 96, Dpmm::DPI_203);
    // A filled rectangle far along the feed: at zoom 4 its left edge is
    // 300 * 4 = 1200 device pixels into the canvas, well past a 709-pixel view.
    let mut c = Common::new(
        1,
        RectDots {
            feed: 300,
            head: 8,
            feed_len: 16,
            head_len: 24,
        },
    );
    c.rot = Rot::D0;
    c.name = "far".into();
    l.elements = vec![ElementOrRaw::Known(Element::Rect(RectEl {
        common: c,
        stroke_dots: 0,
        radius_dots: 0,
    }))];
    l
}

/// The software renderer, at most once for this thread: the testing backend is a
/// thread-local, and asking twice on one thread panics.
fn ensure_software_platform() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    DONE.with(|done| {
        if !done.replace(true) {
            slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
                i_slint_backend_testing::TestingBackendOptions {
                    mock_time: true,
                    threading: false,
                    renderer_name: Some("software".into()),
                },
            )))
            .expect("the platform is ours to set, once per thread");
        }
    });
}

fn shown_at(opening: Opening) -> InsProbe {
    ensure_software_platform();
    let ui = InsProbe::new().expect("the probe window");
    let doc = Doc::open(label(), relabel_proto::PrintDirection::Left).expect("a label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    view::wire(&ui, &canvas, opening);
    ui.show().expect("a headless window");
    ui
}

fn by_id(ui: &InsProbe, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, id)
        .next()
        .unwrap_or_else(|| panic!("no element {id}"))
}

/// Dark pixels of a snapshot in `xs` by `ys`, device pixels at scale 1.
fn ink(
    shot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    xs: std::ops::Range<u32>,
    ys: std::ops::Range<u32>,
) -> u32 {
    let (w, px) = (shot.width(), shot.as_slice());
    let mut n = 0;
    for y in ys {
        for x in xs.clone() {
            let p = px[(y * w + x) as usize];
            if u32::from(p.r) + u32::from(p.g) + u32::from(p.b) < 3 * 200 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_label_wider_than_its_view_paints_nothing_between_the_view_and_the_pane() {
    let ui = shown_at(Opening::At(DEFAULT_ZOOM));
    assert!(
        ui.get_plane_px_w() > ui.get_view_px_w(),
        "this test is only meaningful if the plane is wider than the view: plane {} vs view {}",
        ui.get_plane_px_w(),
        ui.get_view_px_w()
    );
    let shot = ui.window().take_snapshot().expect("software draws");
    let port = by_id(&ui, "Designer::port").absolute_position();
    let pane = by_id(&ui, "Designer::inspector").absolute_position().x;
    // The shell's 12 px of spacing, right of the vertical scroll bar and left of the
    // pane, over the rows the canvas covers: 96 dots at zoom 4.
    let (gap, rows) = (
        (pane - 12.0) as u32..pane as u32,
        port.y as u32..(port.y + 384.0) as u32,
    );
    assert_eq!(
        ink(&shot, gap.clone(), rows.clone()),
        0,
        "the canvas painted into the gap {gap:?} over rows {rows:?}"
    );
}

#[test]
fn a_label_opened_at_fit_is_drawn_whole() {
    let ui = shown_at(Opening::Fit);
    let zoom = ui.get_zoom();
    assert_eq!(zoom, 2, "the reference label fits the default window at 2x");
    let shot = ui.window().take_snapshot().expect("software draws");
    let port = by_id(&ui, "Designer::port").absolute_position();
    // The far rectangle: feed 300 to 316 and head 8 to 32 dots, at the zoom.
    let (x0, y0) = (port.x as u32, port.y as u32);
    let far = (x0 + 300 * 2..x0 + 316 * 2, y0 + 8 * 2..y0 + 32 * 2);
    let inked = ink(&shot, far.0.clone(), far.1.clone());
    assert_eq!(
        inked,
        16 * 2 * 24 * 2,
        "every device pixel of the far rectangle is inked, inside the snapshot"
    );
    assert!(
        far.0.end <= x0 + ui.get_view_px_w() as u32,
        "and inside the view"
    );
}
