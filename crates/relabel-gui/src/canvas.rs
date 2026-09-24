//! The canvas pane's wiring: the one place the document and the toolkit meet.
//!
//! Every callback follows the same shape - upgrade the weak handle or return,
//! apply one [`Cmd`], then refresh exactly what the outcome says changed.
//! Unwrapping the handle instead would panic when the window closes while a
//! gesture is in flight.

use crate::doc::{Cmd, Doc, Effect, Outcome};
use crate::geom;
use crate::preview::{NoDots, Pane};
use crate::rows::{CanvasHost, CanvasRow, RowEdit, row_edits};
use relabel_render::preview::{Extent, Scale};
use slint::{Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// How the canvas tells a pane beside it that the document may have changed: the
/// window, and the document to read the pane's values from.
///
/// A closure rather than a trait bound on `Canvas`, so the canvas needs to know
/// nothing about the inspector's own trait and no existing probe changes.
pub(crate) type PanePush<H> = Rc<dyn Fn(&H, &Doc)>;

/// Why an outcome is being refreshed, as far as a view is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// Focus arrived on an element, by Tab, by an assistive technology or by a
    /// caller invoking `select`. A request to see it, even when it was already the
    /// selection.
    Selection,
    /// Anything else. Shown only when it changed the selection or moved the
    /// selected element's frame.
    Edit,
}

/// How the canvas tells a view an outcome has been drawn: the window, the
/// document, the zoom the rows were drawn at, and why. A closure for `PanePush`'s
/// reason: the canvas probes have no view and must not grow a view-shaped hole.
pub(crate) type ViewTell<H> = Rc<dyn Fn(&H, &Doc, u32, Cause)>;

/// The canvas's own state: the document, the rows on screen and the zoom.
pub struct Canvas<H: CanvasHost> {
    doc: Doc,
    /// One model for the life of the window, mutated row by row: `remove` and
    /// `insert` keyed by element id, then `set_row_data`, and never `set_vec`.
    ///
    /// Never replaced and never reset: handing Slint a new `ModelRc` or a reset
    /// destroys and recreates every repeater instance, including the TouchArea
    /// holding the pointer grab, so the drag dies after its first interpolated
    /// move (measured: a twelve-dot drag moved one dot), and the FocusScope holding
    /// the keyboard, so an arrow after an undo moved nothing.
    rows: Rc<VecModel<H::Row>>,
    zoom: u32,
    pane: Pane,
    /// Told after every outcome, when there is a pane above to tell.
    ///
    /// Monomorphic in `H`, so the canvas needs to know nothing about the
    /// inspector's own trait, `CanvasHost` does not grow a pane-shaped hole, and
    /// no existing probe changes.
    push_ins: Option<PanePush<H>>,
    /// Told after every outcome, after the pane, when there is an edit bar and grips
    /// to tell. A closure for `push_ins`'s reason.
    push_edit: Option<PanePush<H>>,
    /// Told after every outcome, after the grips, when there is a window title to
    /// keep: whether the work is unsaved can change with any outcome, a typed digit
    /// included. A closure for `push_ins`'s reason.
    push_file: Option<PanePush<H>>,
    /// Told after every outcome, after the title, when there is a Preview pane: an
    /// assistive technology can edit behind the open pane, and what it shows must
    /// follow. A closure for `push_ins`'s reason.
    push_print: Option<PanePush<H>>,
    /// Told after every outcome, after the pane and the grips: a view reads the
    /// pane's effects.
    view: Option<ViewTell<H>>,
}

/// Whole device pixels per dot that `canvas::wire` starts at, and that every canvas
/// probe keeps for its life: its drag, grid and pixel tests state their distances
/// at four. One dot of a 203 dpi head is an eighth of a millimetre, and at four an
/// arrow nudge is a visible step. The application does not open here: it opens at
/// Fit (`view::Opening::Fit`), which in its default window is two pixels to the dot
/// for a 40 mm label at scale 1.
pub const DEFAULT_ZOOM: u32 = 4;

impl<H: CanvasHost> Canvas<H> {
    pub fn new(doc: Doc) -> Canvas<H> {
        Canvas {
            doc,
            rows: Rc::new(VecModel::default()),
            zoom: DEFAULT_ZOOM,
            pane: Pane::default(),
            push_ins: None,
            push_edit: None,
            push_file: None,
            push_print: None,
            view: None,
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

    /// Install the grips' push, which `refresh_because` calls after the pane's.
    pub(crate) fn tell_edit(&mut self, push: PanePush<H>) {
        self.push_edit = Some(push);
    }

    /// Install the title's push, which `refresh_because` calls after the grips'.
    pub(crate) fn tell_file(&mut self, push: PanePush<H>) {
        self.push_file = Some(push);
    }

    /// Install the Preview pane's push, which `refresh_because` calls after the
    /// title's. It is handed the document and must not borrow the canvas, which is
    /// borrowed while it runs.
    pub(crate) fn tell_print(&mut self, push: PanePush<H>) {
        self.push_print = Some(push);
    }

    /// Install the view's hook, which `refresh_because` calls last.
    pub(crate) fn tell_view(&mut self, tell: ViewTell<H>) {
        self.view = Some(tell);
    }

    pub fn zoom(&self) -> u32 {
        self.zoom
    }

    /// Push everything: the rows, the selection, the size and the dots. Called
    /// once at startup and after any edit that may have moved more than one row.
    pub fn publish(&mut self, host: &H) {
        self.write_rows();
        host.set_rows(ModelRc::from(self.rows.clone()));
        host.set_selected(self.doc.selected().map_or(-1, |id| id as i32));
        host.set_zoom(self.zoom as i32);
        let (dots_x, dots_y) = self.doc.canvas_dots();
        host.set_dots_x(dots_x);
        host.set_dots_y(dots_y);
        self.repaint(host);
    }

    /// Set the zoom, in whole device pixels per dot.
    ///
    /// The stored and pushed zoom is `Scale::up(zoom).pixels_per_dot()`, and a zoom
    /// at which this label has no `Extent` is refused with nothing changed. So `dot`
    /// in `canvas.slint` and the plane under it cannot disagree, which a zoom stored
    /// raw and clamped only inside the paint allowed: a zoom of 0 drew frames of no
    /// size over a 1x plane, and 27 on a 40 mm label drew frames over no plane at all.
    ///
    /// No `Pane::invalidate`: `plane_stamp` mixes the scale in (relabel-render
    /// preview.rs:595-597), so a real change of zoom already repaints.
    ///
    /// The canvas cannot see the pointer. A caller must not change the zoom while a
    /// pointer holds an element, because the grab offset is in dots of the zoom at
    /// the press; `view::wire` refuses there.
    pub fn set_zoom(&mut self, host: &H, zoom: u32) -> Result<(), NoDots> {
        let scale = Scale::up(zoom);
        let (w, h) = crate::view::dims(self.doc.canvas_dots());
        if Extent::new(w, h, scale).is_none() {
            return Err(NoDots::TooLarge);
        }
        let zoom = scale.pixels_per_dot();
        if zoom != self.zoom {
            self.zoom = zoom;
            host.set_zoom(zoom as i32);
            self.repaint(host);
        }
        Ok(())
    }

    /// Put another document in this one's place: a label opened or a new one made.
    ///
    /// Every row goes, from the end, and the new document's rows come in fresh, so no
    /// row instance survives to carry the keyboard, a hover or a half-finished drag
    /// into a document it does not belong to; removed one at a time rather than by
    /// `set_vec` or a reset, for `rows`' reason. The pane forgets what it drew, since
    /// two labels can share a stamp and not a picture, and a zoom this label has no
    /// `Extent` at comes down to the largest it has. The caller refits the view.
    pub(crate) fn replace_doc(&mut self, host: &H, doc: Doc) {
        while self.rows.row_count() > 0 {
            self.rows.remove(self.rows.row_count() - 1);
        }
        self.doc = doc;
        self.pane.invalidate();
        self.zoom = self.zoom.min(crate::view::top(self.doc.canvas_dots()));
        self.refresh(
            host,
            Outcome {
                effect: Effect::Everything,
                note: None,
            },
        );
    }

    /// `refresh_because(host, out, Cause::Edit)`.
    pub(crate) fn refresh(&mut self, host: &H, out: Outcome) {
        self.refresh_because(host, out, Cause::Edit);
    }

    /// What one outcome costs: at most one row written, one repaint, the pane's
    /// sixteen slots, and then the view's hook, last.
    pub(crate) fn refresh_because(&mut self, host: &H, out: Outcome, cause: Cause) {
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
            // Every row that changed, not only the one named: a frame that moved
            // changes how far the padding of each element drawn after it may reach.
            Effect::Row(_) => {
                self.write_rows();
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
        // For the pane's reason: whether an outcome moved the selected frame, changed
        // the selection or refused, the grips are pushed, and setting the same value
        // again marks nothing dirty.
        if let Some(push) = self.push_edit.clone() {
            push(host, &self.doc);
        }
        if let Some(push) = self.push_file.clone() {
            push(host, &self.doc);
        }
        if let Some(push) = self.push_print.clone() {
            push(host, &self.doc);
        }
        if let Some(note) = out.note {
            host.set_status(said_again(&host.get_status(), note).as_str().into());
        }
        // Last, and for every outcome: the selection and its frame are only final
        // once the rows and the pane have been pushed, and the view decides from
        // those whether anything needs bringing into view.
        if let Some(tell) = self.view.clone() {
            tell(host, &self.doc, self.zoom, cause);
        }
    }

    /// Every row as the document stands, with each frame's pointer clearances, in
    /// z-order.
    fn rows_now(&self) -> Vec<H::Row> {
        let shown = self.doc.shown();
        let frames: Vec<_> = shown.iter().map(|s| (s.x, s.y, s.w, s.h)).collect();
        shown
            .iter()
            .zip(geom::clearances(&frames))
            .map(|(s, clear)| H::Row::of(s, clear))
            .collect()
    }

    /// Bring the rows on screen to the document, in z-order: rows whose element has
    /// gone are removed and rows for new elements inserted, keyed by id through
    /// [`row_edits`], and then every row that differs is written in place.
    ///
    /// Each row is rebuilt whole in Rust and written with `set_row_data`. Nothing in
    /// .slint assigns to a row, so the compiler's read / rebuild / assign-whole-row
    /// rewrite never happens at all. Never through `set_vec`, which emits a model
    /// *reset*: a reset drops every repeater instance, and with them the keyboard
    /// focus, so an arrow key after an undo moved nothing at all. And not by position
    /// when the count changes, which would write a deleted element's neighbour into
    /// the instance that held the keyboard: a `row_removed` or `row_added` elsewhere
    /// keeps that instance and moves it with its element (i-slint-core
    /// model/repeater.rs:443-506). An inserted row carries its data already, so the
    /// pass that follows finds nothing to write there.
    fn write_rows(&mut self) {
        let rows = self.rows_now();
        let have: Vec<i32> = self.rows.iter().map(|r| r.id()).collect();
        let want: Vec<i32> = rows.iter().map(CanvasRow::id).collect();
        for edit in row_edits(&have, &want) {
            match edit {
                RowEdit::Remove(i) => {
                    self.rows.remove(i);
                }
                RowEdit::Insert(k) => self.rows.insert(k, rows[k].clone()),
            }
        }
        debug_assert_eq!(self.rows.row_count(), rows.len());
        for (i, row) in rows.into_iter().enumerate() {
            if self.rows.row_data(i).as_ref() != Some(&row) {
                self.rows.set_row_data(i, row);
            }
        }
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

/// The end of a pointer gesture, as `wire` installs it.
///
/// A function of its own because `view::wire` installs the handler again - Slint's
/// `on_end_move` replaces a handler rather than chaining one - and has a refit to pay
/// once this has returned and released the canvas. One code path either way, so the
/// canvas probes and the shell run the same end of a drag.
pub(crate) fn end_move<H: CanvasHost>(host: &H, canvas: &RefCell<Canvas<H>>, committed: bool) {
    let mut c = canvas.borrow_mut();
    let out = c.doc.apply(Cmd::EndMove { committed });
    c.refresh(host, out);
}

/// The status line for `note`, given what it holds now.
///
/// A note the line already holds is written as a different string. A live region is
/// announced when its value changes (accesskit_atspi_common node.rs:567-579), and
/// `Property::set` marks nothing dirty for an equal value (i-slint-core
/// properties.rs:1023-1029), so a refusal repeated word for word - a second Delete
/// with nothing selected, a second add while an element is held - was silent to a
/// screen reader the second time. Clearing the line first is no cure inside one
/// dispatch, for `ins::wire`'s reason. Alternating, so a third repeat differs from
/// the second.
pub(crate) fn said_again(shown: &str, note: String) -> String {
    if shown == note {
        format!("{note} (again)")
    } else {
        note
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

    // Not through `on!`: focus arriving on an element is a request to see it, even
    // when the element was already the selection and the outcome changes nothing.
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_select(move |id| {
            let Some(host) = weak.upgrade() else { return };
            let mut c = canvas.borrow_mut();
            let out = c.doc.apply(Cmd::Select(id.try_into().ok()));
            c.refresh_because(&host, out, Cause::Selection);
        });
    }
    on!(on_begin_move, |id| Cmd::BeginMove(id as u32));
    on!(on_move_to, |id, x, y| Cmd::MoveTo {
        id: id as u32,
        x,
        y
    });
    {
        let (weak, canvas) = (host.as_weak(), canvas.clone());
        host.on_end_move(move |committed| {
            let Some(host) = weak.upgrade() else { return };
            end_move(&host, &canvas, committed);
        });
    }
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

#[cfg(test)]
mod tests {
    use super::said_again;

    #[test]
    fn a_note_said_again_differs_from_the_line_it_replaces_and_says_the_same_thing() {
        let note = "nothing is selected to delete";
        assert_eq!(said_again("", note.into()), note);
        let second = said_again(note, note.into());
        assert_eq!(second, "nothing is selected to delete (again)");
        assert_eq!(said_again(&second, note.into()), note);
        assert_eq!(said_again("undo brought back box 1", note.into()), note);
    }
}
