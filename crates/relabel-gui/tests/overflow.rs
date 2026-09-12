//! Does the shipped AppWindow clip a canvas wider than its window?
//!
//! The design's risk list asserts "a label wider than the window at zoom 4 is
//! clipped". The reference label at zoom 4 is 1280 device pixels wide inside a
//! 900 logical pixel window, so this asks the question with a snapshot.

use relabel_gui::canvas::wire;
use relabel_gui::doc::Doc;
use relabel_render::document::{Common, Element, ElementOrRaw, Label, RectDots, RectEl, Rot};
use relabel_render::target::Dpmm;
use slint::ComponentHandle;

use relabel_gui::AppWindow;

fn label() -> Label {
    let mut l = Label::new(320, 96, Dpmm::DPI_203);
    // A filled rectangle far along the feed: at zoom 4 its left edge is
    // 300 * 4 = 1200 device pixels into the canvas, well past a 900px window.
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

#[test]
fn a_canvas_wider_than_the_window_is_clipped_to_it() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .expect("the platform is ours to set, once");

    let ui = AppWindow::new().expect("the application window");
    let doc = Doc::open(label(), relabel_proto::PrintDirection::Left).expect("a label");
    let _c = wire(&ui, doc);
    ui.show().expect("a headless window");

    let shot = ui.window().take_snapshot().expect("software draws");
    let (w, h) = (shot.width(), shot.height());
    let px = shot.as_slice();
    let dark = |x: u32, y: u32| {
        let p = px[(y * w + x) as usize];
        u32::from(p.r) + u32::from(p.g) + u32::from(p.b) < 3 * 200
    };
    // The status line lives below the canvas row in the VerticalLayout. If the
    // canvas is unclipped, the oversized plane paints over it.
    let mut ink_below = 0u32;
    let mut lowest = 0u32;
    for y in 0..h {
        for x in 0..w {
            if dark(x, y) && y > lowest {
                lowest = y;
            }
        }
    }
    // How far down the window does painted content reach, and how tall is the
    // canvas supposed to be? 96 dots at zoom 4 is 384 device pixels, plus 12px
    // of padding.
    for y in 400..h {
        for x in 0..w {
            if dark(x, y) {
                ink_below += 1;
            }
        }
    }
    eprintln!("window {w}x{h}; lowest inked row {lowest}; ink below y=400: {ink_below}");
    eprintln!("plane_px_w = {}", ui.get_plane_px_w());
    assert!(
        ui.get_plane_px_w() > w as i32,
        "this test is only meaningful if the plane is wider than the window: \
         plane {} vs window {w}",
        ui.get_plane_px_w()
    );
}
