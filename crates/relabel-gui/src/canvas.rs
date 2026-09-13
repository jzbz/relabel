//! The canvas pane's wiring: the one place the document and the toolkit meet.
//!
//! Every callback follows the same shape - upgrade the weak handle or return,
//! apply one [`Cmd`], then refresh exactly what the outcome says changed.
//! Unwrapping the handle instead would panic when the window closes while a
//! gesture is in flight.

use crate::doc::{Cmd, Doc, Effect, Outcome};
use crate::preview::Pane;
use crate::rows::{CanvasHost, CanvasRow};
use slint::{Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// How the canvas tells a pane beside it that the document may have changed: the
/// window, and the document to read the pane's values from.
///
/// A closure rather than a trait bound on `Canvas`, so the canvas needs to know
/// nothing about the inspector's own trait and no existing probe changes.
pub(crate) type PanePush<H> = Rc<dyn Fn(&H, &Doc)>;

/// The canvas's own state: the document, the rows on screen and the zoom.
pub struct Canvas<H: CanvasHost> {
    doc: Doc,
    /// One model for the life of the window, mutated with `set_row_data`.
    ///
    /// Never replaced while a gesture is in flight: handing Slint a new
    /// `ModelRc` destroys and recreates every repeater instance, including the
    /// TouchArea holding the pointer grab, so the drag dies after its first
    /// interpolated move. Measured: a twelve-dot drag moved one dot.
    rows: Rc<VecModel<H::Row>>,
    zoom: u32,
    pane: Pane,
    /// Told after every outcome, when there is a pane above to tell.
    ///
    /// Monomorphic in `H`, so the canvas needs to know nothing about the
    /// inspector's own trait, `CanvasHost` does not grow a pane-shaped hole, and
    /// no existing probe changes.
    push_ins: Option<PanePush<H>>,
}

/// Whole device pixels per dot. Four, because one dot of a 203 dpi head is an
/// eighth of a millimetre and nobody can place that by eye at 1:1 - and because
/// the zoom is also the nudge the arrow keys make visible. A zoom control is the
/// next slice; the clamp is `preview::Scale`'s.
pub const DEFAULT_ZOOM: u32 = 4;

impl<H: CanvasHost> Canvas<H> {
    pub fn new(doc: Doc) -> Canvas<H> {
        Canvas {
            doc,
            rows: Rc::new(VecModel::default()),
            zoom: DEFAULT_ZOOM,
            pane: Pane::default(),
            push_ins: None,
        }
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// For `ins::wire`, which applies its own commands to the same document.
    pub(crate) fn doc_mut(&mut self) -> &mut Doc {
        &mut self.doc
    }

    /// Install the pane's push. `publish` does not call it: the `Everything` arm
    /// of `refresh` calls `publish`, and the pane would then be pushed twice for
    /// one outcome.
    pub(crate) fn tell_pane(&mut self, push: PanePush<H>) {
        self.push_ins = Some(push);
    }

    pub fn zoom(&self) -> u32 {
        self.zoom
    }

    /// Push everything: the rows, the selection, the size and the dots. Called
    /// once at startup and after any edit that may have moved more than one row.
    pub fn publish(&mut self, host: &H) {
        let rows: Vec<H::Row> = self.doc.shown().iter().map(H::Row::of).collect();
        // Written one row at a time rather than through `set_vec`, which emits
        // a model *reset*: a reset drops every repeater instance, and with them
        // the keyboard focus, so an arrow key after an undo moved nothing at
        // all. Only a change in how many rows there are needs the reset.
        if self.rows.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                self.rows.set_row_data(i, row);
            }
        } else {
            self.rows.set_vec(rows);
        }
        host.set_rows(ModelRc::from(self.rows.clone()));
        host.set_selected(self.doc.selected().map_or(-1, |id| id as i32));
        host.set_zoom(self.zoom as i32);
        let (dots_x, dots_y) = self.doc.canvas_dots();
        host.set_dots_x(dots_x);
        host.set_dots_y(dots_y);
        self.repaint(host);
    }

    /// Set the zoom, in whole device pixels per dot.
    pub fn set_zoom(&mut self, host: &H, zoom: u32) {
        self.zoom = zoom;
        self.pane.invalidate();
        host.set_zoom(zoom as i32);
        self.repaint(host);
    }

    /// What one outcome costs: at most one row written, one repaint, and the
    /// pane's sixteen slots.
    pub(crate) fn refresh(&mut self, host: &H, out: Outcome) {
        // Unconditional, and no longer the `Selection` arm's job: a commit flushed
        // by a selection yields `Row(a)`, and an arm that only drew the row would
        // leave the border on the element the operator just left. Setting the
        // same id again marks nothing dirty.
        host.set_selected(self.doc.selected().map_or(-1, |id| id as i32));
        match out.effect {
            Effect::Nothing => {}
            // The selection was pushed above; the variant means only that there
            // is nothing to redraw.
            Effect::Selection => {}
            Effect::Row(id) => {
                if let Some(s) = self.doc.shown_one(id) {
                    // The row is rebuilt whole in Rust and written with
                    // set_row_data. Nothing in .slint assigns to a row, so the
                    // compiler's read / rebuild / assign-whole-row rewrite never
                    // happens at all.
                    if let Some(i) = self.index_of(id) {
                        self.rows.set_row_data(i, H::Row::of(&s));
                    }
                }
                self.repaint(host);
            }
            Effect::Everything => self.publish(host),
        }
        // Also unconditional, and deliberately not keyed to the effect.
        // `Effect::Nothing` is exactly what a refused edit and a placement clamped
        // onto the value the element already held return, and those are the two
        // cases where a field is holding something the document does not. After
        // the rows, so the numbers describe the rows the canvas now shows.
        if let Some(push) = self.push_ins.clone() {
            push(host, &self.doc);
        }
        if let Some(note) = out.note {
            host.set_status(note.as_str().into());
        }
    }

    fn index_of(&self, id: relabel_render::document::ElementId) -> Option<usize> {
        // By id, never by the index the gesture came from: the row order is the
        // document's z-order and the moment anything can be added, deleted or
        // raised, an index captured a frame ago addresses a different element.
        self.doc.shown().iter().position(|s| s.id == id)
    }

    fn repaint(&mut self, host: &H) {
        match self.pane.draw(&self.doc, self.zoom) {
            Ok(None) => {}
            Ok(Some(p)) => {
                host.set_plane_px_w(p.px_w);
                host.set_plane_px_h(p.px_h);
                host.set_plane(p.image);
            }
            Err(why) => {
                host.set_plane(slint::Image::default());
                host.set_plane_px_w(0);
                host.set_plane_px_h(0);
                host.set_status(why.to_string().as_str().into());
            }
        }
    }
}

/// Wire one canvas to one window, and return the state so that a caller - the
/// application, or a test - can read the document back.
pub fn wire<H: CanvasHost>(host: &H, doc: Doc) -> Rc<RefCell<Canvas<H>>> {
    let canvas = Rc::new(RefCell::new(Canvas::new(doc)));
    canvas.borrow_mut().publish(host);

    macro_rules! on {
        ($setter:ident, || $cmd:expr) => {{
            let (weak, canvas) = (host.as_weak(), canvas.clone());
            host.$setter(move || {
                let Some(host) = weak.upgrade() else { return };
                let mut c = canvas.borrow_mut();
                let out = c.doc.apply($cmd);
                c.refresh(&host, out);
            });
        }};
        ($setter:ident, |$($arg:ident),*| $cmd:expr) => {{
            let (weak, canvas) = (host.as_weak(), canvas.clone());
            host.$setter(move |$($arg),*| {
                let Some(host) = weak.upgrade() else { return };
                let mut c = canvas.borrow_mut();
                let out = c.doc.apply($cmd);
                c.refresh(&host, out);
            });
        }};
    }

    on!(on_select, |id| Cmd::Select(id.try_into().ok()));
    on!(on_begin_move, |id| Cmd::BeginMove(id as u32));
    on!(on_move_to, |id, x, y| Cmd::MoveTo {
        id: id as u32,
        x,
        y
    });
    on!(on_end_move, |committed| Cmd::EndMove { committed });
    on!(on_nudge, |id, dx, dy, coarse| Cmd::Nudge {
        id: id as u32,
        dx,
        dy,
        coarse
    });
    on!(on_set_say, |id, value| Cmd::SetPlacement {
        id: id as u32,
        value: value.to_string()
    });
    on!(on_undo, || Cmd::Undo);
    on!(on_redo, || Cmd::Redo);

    canvas
}
