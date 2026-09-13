//! The shell round the view: the pane beside it, the status line under it, and
//! the size the view is left with.
//!
//! Nothing a press does may change the view's size, because the toolkit cancels a
//! press whose clipping ancestor stops containing the pointer inside the dispatch
//! that pressed (i-slint-core input.rs:1410-1441). So the chrome round the view is
//! pinned here to one size whatever is selected and whatever the status line says.

#[path = "support/mod.rs"]
mod support;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use relabel_gui::canvas::{Canvas, wire};
use relabel_gui::doc::Doc;
use relabel_gui::ins;
use relabel_gui::view::{self, Opening};
use relabel_render::document::Rot;
use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use support::{
    canvas_of, every_kind_label, fields_of, frame, press, rect, reference_label, rows_of, status_of,
};

include!(concat!(env!("OUT_DIR"), "/ins-probe.rs"));

relabel_gui::impl_canvas_host!(InsProbe, ElRow);
relabel_gui::impl_ins_host!(InsProbe, InsSlot);
relabel_gui::impl_view_host!(InsProbe);

type Wired = Rc<RefCell<Canvas<InsProbe>>>;

fn probe(label: relabel_render::Label, scale: f32) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    (ui, canvas)
}

/// The probe with the view wired as the application wires it, and the change
/// handlers the first layout queued already run.
fn probe_viewed(label: relabel_render::Label, scale: f32, opening: Opening) -> (InsProbe, Wired) {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let doc = Doc::open(label, relabel_proto::PrintDirection::Left).expect("a printable label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    view::wire(&ui, &canvas, opening);
    ui.invoke_focus_canvas();
    ui.show().expect("showing a headless window");
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    (ui, canvas)
}

/// A label as long as a 100 mm D110 label: 800 dots, too long for the view at 1:1.
fn long_label() -> relabel_render::Label {
    let mut label = relabel_render::Label::new(800, 96, relabel_render::target::Dpmm::DPI_203);
    label.elements = vec![rect(1, frame(8, 8, 40, 24), Rot::D0, false)];
    label
}

/// How many scroll bar thumbs are drawn; an undrawn one is not in the tree.
fn thumbs(ui: &InsProbe) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(ui, "Scroller::thumb").collect()
}

/// The inspector component itself, which is in the element tree whether or not its
/// pane is shown, so its width can be read with nothing selected.
fn inspector_of(ui: &InsProbe) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "Designer::inspector")
        .next()
        .expect("the inspector; if this fails the probe was compiled without debug info")
}

/// The view's port: the clipped rectangle the canvas is seen through.
fn port_of(ui: &InsProbe) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "Designer::port")
        .next()
        .expect("the view's port")
}

/// The pane's heading, one line of the style's text: the height a status line is
/// measured in. The shell's own measuring line is invisible, and the testing
/// backend's queries skip what is not visible (search_api.rs:373-376).
fn heading_of(ui: &InsProbe) -> ElementHandle {
    ElementQuery::from_root(ui)
        .match_accessible_role(AccessibleRole::Text)
        .match_predicate(|h| {
            h.accessible_label()
                .is_some_and(|l| l == "Selected element")
        })
        .find_first()
        .expect("the pane's heading, which is shown only with a selection")
}

/// A note long enough to wrap to four lines across the default window.
fn four_lines() -> String {
    "the feed field was refused because a placement has to leave at least one dot of \
     the element on the label, and this sentence goes on to say so again and again. "
        .repeat(5)
        .trim_end()
        .to_string()
}

#[test]
fn the_pane_is_as_wide_with_nothing_selected_as_with_every_kind_of_element() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, _canvas) = probe(every_kind_label(), scale);
        let empty = inspector_of(&ui).size().width;
        for id in 1..=7 {
            ui.invoke_select(id);
            let pane = inspector_of(&ui);
            let (left, width) = (pane.absolute_position().x, pane.size().width);
            assert_eq!(
                width, empty,
                "at {scale}x the pane is {width} wide with element {id} selected and {empty} \
                 with nothing selected"
            );
            for field in fields_of(&ui) {
                let (at, end) = (
                    field.absolute_position().x,
                    field.absolute_position().x + field.size().width,
                );
                assert!(
                    at >= left && end <= left + width + 0.01,
                    "at {scale}x {:?} of element {id} spans {at}..{end} in a pane spanning \
                     {left}..{}",
                    field.accessible_label(),
                    left + width
                );
            }
        }
        eprintln!("at {scale}x the pane is {empty} logical pixels wide");
    }
}

#[test]
fn the_status_line_is_two_lines_tall_whatever_it_says_and_ends_inside_the_window() {
    let (ui, _canvas) = probe(every_kind_label(), 1.0);
    // A text element has the most fields of any kind, so its pane is the tallest.
    ui.invoke_select(1);
    let line = heading_of(&ui).size().height;
    let window = ui.global::<DefaultWindow>().get_height();
    for said in [String::new(), "one".to_string(), four_lines()] {
        ui.set_status(said.as_str().into());
        let status = status_of(&ui);
        let (top, height) = (status.absolute_position().y, status.size().height);
        assert!(
            (height - 2.0 * line).abs() < 0.01,
            "the status line is {height} tall saying {} characters, and one line is {line}",
            said.len()
        );
        assert!(
            top + height <= window + 0.01,
            "the status line ends at {} in a window {window} tall",
            top + height
        );
    }
}

#[test]
fn the_whole_status_sentence_is_announced_even_when_the_screen_elides_it() {
    let (ui, _canvas) = probe(reference_label(), 1.0);
    ui.set_status("short".into());
    let short = status_of(&ui).size().height;
    let long = four_lines();
    ui.set_status(long.as_str().into());
    let status = status_of(&ui);
    assert_eq!(status.accessible_value().as_deref(), Some(long.as_str()));
    assert_eq!(
        status.size().height,
        short,
        "the screen shows two lines of it and no more"
    );
}

#[test]
fn the_canvas_sits_at_the_top_left_of_its_view_at_every_display_scale() {
    for scale in [1.0, 1.25, 1.5] {
        let (ui, _canvas) = probe(reference_label(), scale);
        let port = port_of(&ui).absolute_position();
        let grid = canvas_of(&ui).absolute_position();
        for (axis, logical) in [("x", grid.x - port.x), ("y", grid.y - port.y)] {
            assert!(
                (logical * scale).abs() <= 0.5 + 1.0e-3,
                "at {scale}x the dot grid is {} device pixels from the view's {axis} edge; only \
                 the grid anchor's half pixel is allowed",
                logical * scale
            );
        }
    }
}

#[test]
fn the_reference_label_opens_whole_at_fit_in_the_default_window_at_every_display_scale() {
    for (scale, want) in [(1.0, 2), (1.25, 2), (1.5, 3), (2.0, 4)] {
        let (ui, canvas) = probe_viewed(reference_label(), scale, Opening::Fit);
        let (vw, vh) = (ui.get_view_px_w(), ui.get_view_px_h());
        eprintln!("at {scale}x the view is {vw} by {vh} device pixels");
        assert_eq!(
            canvas.borrow().zoom(),
            want,
            "at {scale}x in a view {vw} by {vh}"
        );
        assert!(ui.get_plane_px_w() <= vw && ui.get_plane_px_h() <= vh);
        assert_eq!(ui.get_zoom_says(), format!("Fit, {want}x"));
        let shown = canvas.borrow().doc().shown();
        let rows = rows_of(&ui);
        assert_eq!(rows.len(), 3, "at {scale}x every element is in the tree");
        for (row, s) in rows.iter().zip(&shown) {
            assert_eq!(
                (
                    (row.size().width * scale).round() as i32,
                    (row.size().height * scale).round() as i32
                ),
                (s.w * want as i32, s.h * want as i32),
                "at {scale}x element {} at its full frame",
                s.id
            );
        }
    }
}

#[test]
fn a_label_too_long_for_the_view_opens_at_one_to_one_and_writes_no_note() {
    support::ensure_platform();
    let ui = InsProbe::new().expect("the probe window");
    let doc = Doc::open(long_label(), relabel_proto::PrintDirection::Left).expect("a label");
    let canvas = wire(&ui, doc);
    ins::wire(&ui, &canvas);
    ui.set_status("the orientation note".into());
    view::wire(&ui, &canvas, Opening::Fit);
    ui.show().expect("showing a headless window");
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    assert_eq!(canvas.borrow().zoom(), 1);
    assert_eq!(ui.get_plane_px_w(), 800);
    assert_eq!(ui.get_status(), "the orientation note");
}

#[test]
fn the_scroll_bars_appear_only_when_the_label_continues_past_the_view_and_the_view_keeps_its_size()
{
    let (ui, canvas) = probe_viewed(reference_label(), 1.0, Opening::Fit);
    let size = (ui.get_view_px_w(), ui.get_view_px_h());
    let across = |ui: &InsProbe| {
        thumbs(ui)
            .iter()
            .map(|t| t.size().width > t.size().height)
            .collect::<Vec<bool>>()
    };
    assert_eq!(canvas.borrow().zoom(), 2);
    assert!(thumbs(&ui).is_empty(), "the whole label is in view");
    for (want, bars) in [(3, vec![true]), (4, vec![true]), (6, vec![true, false])] {
        press(ui.window(), "=", false, true);
        assert_eq!(canvas.borrow().zoom(), want);
        let mut drawn = across(&ui);
        drawn.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(drawn, bars, "at {want}x");
        assert_eq!(
            (ui.get_view_px_w(), ui.get_view_px_h()),
            size,
            "at {want}x the view is the size it was"
        );
    }
    press(ui.window(), "0", false, true);
    assert!(thumbs(&ui).is_empty());
    assert_eq!((ui.get_view_px_w(), ui.get_view_px_h()), size);
}
