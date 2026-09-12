//! Labels and handles the canvas tests share. Nothing here touches the
//! toolkit except the two element-tree helpers at the bottom, which are about
//! one trap each.

// Shared by several test binaries, each of which uses part of it.
#![allow(dead_code)]

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use relabel_render::document::{
    Common, Element, ElementId, ElementOrRaw, Label, RectDots, RectEl, Rot,
};
use relabel_render::target::Dpmm;

/// The 40 by 12 mm label at 203 dpi this project measures everything against:
/// 320 dots along the feed, 96 across the head.
pub fn reference_label() -> Label {
    let mut label = Label::new(320, 96, Dpmm::DPI_203);
    label.elements = vec![
        rect(1, frame(8, 8, 40, 24), Rot::D0, false),
        rect(2, frame(120, 8, 40, 24), Rot::D0, false),
        rect(3, frame(240, 40, 40, 24), Rot::D0, false),
    ];
    label
}

pub fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

pub fn rect(id: ElementId, frame: RectDots, rot: Rot, locked: bool) -> ElementOrRaw {
    let mut common = Common::new(id, frame);
    common.rot = rot;
    common.locked = locked;
    common.name = format!("box {id}");
    ElementOrRaw::Known(Element::Rect(RectEl {
        common,
        stroke_dots: 0,
        radius_dots: 0,
    }))
}

/// The frame of one element, straight out of the document.
pub fn frame_of(label: &Label, id: ElementId) -> RectDots {
    label
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .map(Element::common)
        .find(|c| c.id == id)
        .expect("the element is in the label")
        .frame
}

/// The canvas's element rows, one handle each, in model order.
///
/// `find_all` returns a repeated element once per *source* element merged into
/// an ItemRc, so a three-row canvas answers a role query nine times over.
/// `accessible-item-index` is the one key that is one per model row, so that is
/// what this deduplicates on - and indexing into the raw result instead
/// silently addresses the wrong element.
pub fn rows_of(probe: &impl slint::ComponentHandle) -> Vec<ElementHandle> {
    let mut found: Vec<(usize, ElementHandle)> = ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .filter_map(|h| h.accessible_item_index().map(|i| (i, h)))
        .collect();
    found.sort_by_key(|(i, _)| *i);
    found.dedup_by_key(|(i, _)| *i);
    found.into_iter().map(|(_, h)| h).collect()
}

/// The canvas surface itself: the one element with the list role.
pub fn canvas_of(probe: &impl slint::ComponentHandle) -> ElementHandle {
    ElementQuery::from_root(probe)
        .match_accessible_role(AccessibleRole::List)
        .find_first()
        .expect(
            "the canvas publishes itself as a list; if this fails the probe was \
             compiled without slint debug info and every other assertion here is vacuous",
        )
}

/// Install the headless platform, at most once for this thread.
///
/// The testing backend lives in a thread-local, so every `#[test]` in a binary
/// gets its own - and asking twice on one thread panics with
/// `platform already initialized`. A test that opens several windows, which is
/// what comparing display scales needs, must therefore ask once and then build
/// as many windows as it likes.
pub fn ensure_platform() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    DONE.with(|done| {
        if !done.replace(true) {
            i_slint_backend_testing::init_no_event_loop();
        }
    });
}
