//! The edit bar's wiring and the grips': adding an element, deleting one, and
//! resizing one by pointer.
//!
//! Called after [`crate::canvas::wire`] and [`crate::ins::wire`], and before
//! [`crate::view::wire`], which replaces the canvas's `end-move` handler with one
//! that runs the same end of a gesture; a resize ends through it like a move.
//! Every closure has the canvas's shape - upgrade the weak handle or return, apply
//! one [`Cmd`], refresh what the outcome says changed.

use crate::canvas::{Canvas, PanePush};
use crate::doc::{Cmd, Doc};
use crate::geom::Grip;
use crate::make::Kind;
use crate::rows::{EditHost, GripsRow, ToolRow};
use relabel_render::document::ElementId;
use slint::{ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The element whose grips were last offered, and the rooms they were offered with.
type Offered = Option<(ElementId, [i32; 3])>;

/// Wire the edit bar and the grips to the document the canvas already has.
pub fn wire<H: EditHost>(host: &H, canvas: &Rc<RefCell<Canvas<H>>>) {
    // Once: the kinds are this build's, and a model that never changes is never
    // reset.
    let tools: Vec<H::Tool> = Kind::ALL.into_iter().map(H::Tool::of).collect();
    host.set_tools(ModelRc::new(VecModel::from(tools)));

    // After every outcome. While a pointer is down the rooms are the ones the press
    // was offered, for the same element: a grip dragged toward a neighbour would
    // otherwise shrink its own room to nothing and vanish under the pointer, and a
    // move would make the grips flicker as other frames came and went. The box still
    // follows every outcome, so the grips travel with the edges they drag.
    let push: PanePush<H> = {
        let held: Rc<Cell<Offered>> = Rc::new(Cell::new(None));
        Rc::new(move |host: &H, doc: &Doc| {
            let grips = doc.grips().map(|mut g| {
                match held.get() {
                    Some((id, room)) if doc.pressed() && id == g.id => g.room = room,
                    _ => held.set(Some((g.id, g.room))),
                }
                g
            });
            host.set_grips(H::Grips::of(grips.as_ref()));
        })
    };
    canvas.borrow_mut().tell_edit(push.clone());
    push(host, canvas.borrow().doc());

    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_add(move |token| {
            let (Some(host), Some(kind)) = (weak.upgrade(), Kind::from_token(token)) else {
                return false;
            };
            let mut c = canvas.borrow_mut();
            let before = c.doc().label().elements.len();
            let out = c.doc_mut().apply(Cmd::Create(kind));
            c.refresh(&host, out);
            // The count rather than the outcome: a refusal and an add both carry a
            // note, and only an add leaves one more element.
            c.doc().label().elements.len() > before
        });
    }
    // Needs no document: which letters are chords is `Kind`'s alone.
    host.on_kind_of_key(|text| Kind::from_letter(&text).map_or(-1, Kind::token));
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_delete_element(move |id| {
            let Some(host) = weak.upgrade() else {
                return false;
            };
            let mut c = canvas.borrow_mut();
            // -1 is no selection, which is said rather than ignored.
            let out = c.doc_mut().apply(Cmd::Delete {
                id: ElementId::try_from(id).ok(),
            });
            c.refresh(&host, out);
            // The shell's to act on: it cannot read the selection back reliably in
            // the function that called this (shell.slint, `remove-selected`).
            c.doc().selected().is_none()
        });
    }
    // Not through a macro: an id or a grip token this build has no meaning for must
    // send no command at all, as `ins::wire`'s field tokens do.
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_begin_resize(move |id, grip| {
            let (Some(host), Ok(id), Some(grip)) = (
                weak.upgrade(),
                ElementId::try_from(id),
                Grip::from_token(grip),
            ) else {
                return;
            };
            let mut c = canvas.borrow_mut();
            let out = c.doc_mut().apply(Cmd::BeginResize { id, grip });
            c.refresh(&host, out);
        });
    }
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_resize_to(move |id, right, bottom| {
            let (Some(host), Ok(id)) = (weak.upgrade(), ElementId::try_from(id)) else {
                return;
            };
            let mut c = canvas.borrow_mut();
            let out = c.doc_mut().apply(Cmd::ResizeTo { id, right, bottom });
            c.refresh(&host, out);
        });
    }
}
