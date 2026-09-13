//! The drag idiom the toolkit documents, kept as a fixture so that the reason
//! the real canvas does not use it is measured rather than remembered.

#[path = "support/mod.rs"]
mod support;

use slint::ComponentHandle;
use slint::platform::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;
use support::rows_of;

include!(concat!(env!("OUT_DIR"), "/naive-drag.rs"));

/// Physical pixels per dot, the canvas default.
const ZOOM: u32 = 4;

fn logical(dots: i32, scale: f32) -> f32 {
    dots as f32 * ZOOM as f32 / scale
}

/// The counter-example, measured rather than argued.
///
/// `ui/testing/naive-drag.slint` is the drag the toolkit's own documentation
/// shows, on a TouchArea that moves with its element. The element chases the
/// pointer, the TouchArea goes with it, and each interpolated move reports a
/// smaller `mouse-x - pressed-x` than the last, so the gesture undershoots. The
/// number is not the point; the direction is, and that the real canvas's form
/// gets it exactly right at the same scale is.
#[test]
fn the_documented_idiom_undershoots_on_a_touch_area_that_moves() {
    support::ensure_platform();
    let ui = NaiveProbe::new().expect("the counter-example window");
    ui.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
    let landed = Rc::new(RefCell::new((8i32, 8i32)));
    let rows = Rc::new(slint::VecModel::from(vec![ElRow {
        id: 1,
        x: 8,
        y: 8,
        w: 40,
        h: 24,
        say: "box".into(),
        locked: false,
        keepout: 0,
        clear_left: 0,
        clear_right: 0,
        clear_up: 0,
        clear_down: 0,
    }]));
    ui.set_rows(slint::ModelRc::from(rows.clone()));
    {
        let (landed, rows) = (landed.clone(), rows.clone());
        ui.on_move_to(move |_, x, y| {
            *landed.borrow_mut() = (x, y);
            let mut row = slint::Model::row_data(&*rows, 0).expect("one row");
            row.x = x;
            row.y = y;
            slint::Model::set_row_data(&*rows, 0, row);
        });
    }
    ui.show().expect("showing a headless window");

    let handles = rows_of(&ui);
    let at = handles[0].absolute_position();
    let size = handles[0].size();
    handles[0].mock_drag(
        slint::LogicalPosition::new(
            at.x + size.width / 2.0 + logical(10, 1.25),
            at.y + size.height / 2.0,
        ),
        slint::platform::PointerEventButton::Left,
    );
    let got = landed.borrow().0 - 8;
    assert!(
        got < 10,
        "the naive idiom is supposed to undershoot a ten-dot drag; it moved {got} dots"
    );
    println!("naive idiom at 1.25x: asked for 10 dots, got {got}");
}
