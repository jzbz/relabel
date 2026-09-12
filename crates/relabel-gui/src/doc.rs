//! The document being edited, the selection, and the undo stack: the editor's
//! rules as a synchronous state machine with no toolkit in it.
//!
//! # Why the rules are here and not in `.slint`
//!
//! Not taste. A rule written as a `.slint` expression can only be reached
//! through a simulated gesture, which needs the testing backend, a build
//! script that emits debug info and a probe window; the same rule here is
//! reached by an ordinary unit test. It also means nothing in `.slint` ever
//! assigns to a repeater row - the compiler rewrites such an assignment into
//! read-whole-struct / rebuild / assign-whole-row - and it gives the five panes
//! DESIGN.md section 6 asks for one owner of the document instead of five.
//!
//! # What a gesture is
//!
//! A drag is one undo step, not one per pointer event, so the pointer opens a
//! transaction and closes it: [`Cmd::BeginMove`], any number of
//! [`Cmd::MoveTo`], then [`Cmd::EndMove`]. That shape is decided here rather
//! than later because it is the callback signature every canvas test is
//! written against.

use crate::a11y;
use crate::geom;
use relabel_proto::PrintDirection;
use relabel_render::document::{Canvas, Element, ElementId, ElementOrRaw, Label, RectDots, Rot};
use relabel_render::media::{Bounds, Keepout};
use relabel_render::target::{Dpmm, Target};

/// How deep the undo stack goes.
///
/// Each step is a whole-document clone, which is 0.3 us and a few kilobytes for
/// an ordinary label and a megabyte for one carrying a base64 image - there is
/// no structural sharing inside `ImageEl::png`. Fifty steps of a label with
/// artwork is the one case this bound exists for; an edit journal is the only
/// thing that would remove it and it is not worth its inverses yet.
const UNDO_DEPTH: usize = 50;

/// One edit. Every gesture the canvas can make is one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// Selection follows focus, so this arrives from a click and from Tab alike.
    Select(Option<ElementId>),
    /// Pointer down: open one undo step for the gesture that follows.
    BeginMove(ElementId),
    /// An absolute placement in *canvas* dots, as the pointer reports it.
    MoveTo {
        id: ElementId,
        x: i32,
        y: i32,
    },
    /// Pointer up, or a cancelled gesture - the one that has to roll back.
    EndMove {
        committed: bool,
    },
    /// One step per press, in canvas directions. `coarse` is a millimetre.
    Nudge {
        id: ElementId,
        dx: i32,
        dy: i32,
        coarse: bool,
    },
    /// A placement written through the accessibility tree, in the document's
    /// own axes, in any spelling [`a11y::parse_say`] accepts.
    SetPlacement {
        id: ElementId,
        value: String,
    },
    Undo,
    Redo,
}

/// What the view has to refresh, and what to tell the operator.
///
/// Narrow on purpose: a move at pointer speed must not ask for a whole model
/// rebuild, because handing Slint a new model mid-gesture destroys the
/// TouchArea holding the grab and the drag dies after its first event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub effect: Effect,
    /// Pre-formatted for the status line. The UI never composes a message.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing moved. A move event that lands on the dot it was already on is
    /// the common case at pointer speed, and it must cost nothing.
    Nothing,
    Selection,
    /// One row changed, and the label has to be drawn again.
    Row(ElementId),
    /// Any row may have changed: an undo, a redo, or a rolled-back gesture.
    Everything,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("the label's canvas is {feed} by {head} dots, which no printer takes")]
    Unprintable { feed: i32, head: i32 },
    #[error("the label is drawn at {num}/{den} dots per millimetre, which no printer uses")]
    UnknownPitch { num: u32, den: u32 },
    #[error(transparent)]
    Target(#[from] relabel_render::RenderError),
}

/// One element as the canvas needs it: already placed, already measured,
/// already described.
///
/// Built here because this is what owns the [`Target`] and the [`Bounds`], so
/// the axis swap and the keep-out test happen once per element per rebuild and
/// the row mapping has nothing left to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub id: ElementId,
    /// Canvas dots: the picture as the operator sees it.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    /// Off the label altogether. Asked before `keepout`, which reads an
    /// off-canvas frame as the tail.
    pub off_canvas: bool,
    pub keepout: Option<Keepout>,
    pub say: String,
}

pub struct Doc {
    label: Label,
    target: Target,
    bounds: Bounds,
    direction: PrintDirection,
    /// Whether the direction above came from the file or from a fallback. The
    /// canvas is transposed if this is wrong, and a transposed rectangle is
    /// still a rectangle, so the operator is told which it was.
    direction_assumed: bool,
    selected: Option<ElementId>,
    undo: Vec<Label>,
    redo: Vec<Label>,
    /// The document as it was when the pointer went down, and whether anything
    /// has actually moved since.
    gesture: Option<(Label, bool)>,
    dirty: bool,
}

impl Doc {
    /// Open a label for editing.
    ///
    /// `fallback` is the print direction to assume when the file does not
    /// record a model this build knows. It cannot be inferred from the canvas:
    /// a 320 by 96 label is landscape either way, and guessing wrong transposes
    /// every element on screen without anything looking wrong.
    pub fn open(label: Label, fallback: PrintDirection) -> Result<Doc, OpenError> {
        let Canvas { feed, head, .. } = label.canvas;
        let (Ok(feed_px), Ok(head_px)) = (u16::try_from(feed), u16::try_from(head)) else {
            return Err(OpenError::Unprintable { feed, head });
        };
        let dpmm = label.canvas.dpmm;
        let dpi = [203u16, 300]
            .into_iter()
            .find(|&d| Dpmm::for_dpi(d) == dpmm)
            .ok_or(OpenError::UnknownPitch {
                num: dpmm.num,
                den: dpmm.den,
            })?;
        let from_file = label
            .meta
            .authored_for
            .as_ref()
            .and_then(|a| a.model.as_deref())
            .and_then(relabel_proto::models::by_name)
            .map(|m| m.direction);
        let direction = from_file.unwrap_or(fallback);
        let target = Target::for_label(head_px, feed_px, dpi, direction)?;
        Ok(Doc {
            bounds: Bounds::of(&label),
            label,
            target,
            direction,
            direction_assumed: from_file.is_none(),
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            gesture: None,
            dirty: false,
        })
    }

    pub fn label(&self) -> &Label {
        &self.label
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn selected(&self) -> Option<ElementId> {
        self.selected
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// The canvas's own extent in canvas dots, which is what sizes the pane.
    ///
    /// From the document rather than from the rendered plane: a label whose
    /// render failed still has to show its element frames, and a pane sized
    /// from an image that is not there would be a pane of nothing.
    pub fn canvas_dots(&self) -> (i32, i32) {
        self.target
            .extent(self.label.canvas.feed, self.label.canvas.head)
    }

    /// How the operator is told which way round the canvas is.
    pub fn orientation_note(&self) -> String {
        let axis = match self.direction {
            PrintDirection::Left => "the head runs down the side, so the feed is across the canvas",
            PrintDirection::Top => "the head runs across the top of the canvas",
        };
        if self.direction_assumed {
            format!("{axis} (assumed: the file records no model this build knows)")
        } else {
            axis.to_string()
        }
    }

    /// Elements the canvas draws, in document z-order. Hidden elements and
    /// elements written by a later version are not among them: the first cannot
    /// be drawn and the second has no frame this build can trust, and both are
    /// carried through a save untouched.
    pub fn shown(&self) -> Vec<Shown> {
        self.label
            .drawable()
            .map(|e| self.shown_of_element(e))
            .collect()
    }

    pub fn shown_one(&self, id: ElementId) -> Option<Shown> {
        self.label
            .drawable()
            .find(|e| e.common().id == id)
            .map(|e| self.shown_of_element(e))
    }

    /// Elements this build cannot draw, so the operator can be told they are
    /// there rather than wondering where they went.
    pub fn undrawable(&self) -> usize {
        self.label
            .elements
            .iter()
            .filter(|e| matches!(e, ElementOrRaw::Raw(_)))
            .count()
    }

    fn shown_of_element(&self, e: &Element) -> Shown {
        let c = e.common();
        let drawn = geom::as_drawn(c.frame, c.rot);
        let (x, y, w, h) = geom::on_canvas(&self.target, c.frame, c.rot);
        let off_canvas = !geom::inside_canvas(&self.bounds.canvas, &drawn);
        // Only a frame on the canvas has a keep-out worth naming: `keepout`
        // reads one that runs off the label as the tail, which would tell the
        // operator to move it for the wrong reason.
        let keepout = if off_canvas {
            None
        } else {
            self.bounds.keepout(&drawn)
        };
        Shown {
            id: c.id,
            x,
            y,
            w,
            h,
            locked: c.locked,
            off_canvas,
            keepout,
            say: a11y::say(
                kind_name(e),
                &c.name,
                c.frame,
                c.locked,
                off_canvas,
                keepout,
            ),
        }
    }

    /// The one entry point. Every gesture, every key and every accessibility
    /// action arrives here.
    pub fn apply(&mut self, cmd: Cmd) -> Outcome {
        match cmd {
            Cmd::Select(id) => {
                if self.selected == id {
                    return Outcome::nothing();
                }
                self.selected = id;
                Outcome::plain(Effect::Selection)
            }
            Cmd::BeginMove(id) => self.begin_move(id),
            Cmd::MoveTo { id, x, y } => {
                let (feed, head) = geom::to_document(&self.target, x, y);
                self.place(id, feed, head)
            }
            Cmd::EndMove { committed } => self.end_move(committed),
            Cmd::Nudge { id, dx, dy, coarse } => self.nudge(id, dx, dy, coarse),
            Cmd::SetPlacement { id, value } => match a11y::parse_say(&value) {
                Some((feed, head)) => self.one_step(id, feed, head),
                None => Outcome::note(
                    Effect::Nothing,
                    format!("{value:?} is not a placement: say \u{201c}feed 12, head 4\u{201d}"),
                ),
            },
            Cmd::Undo => self.step_history(true),
            Cmd::Redo => self.step_history(false),
        }
    }

    /// Pointer down. Refuses once, here, rather than on every move event: a
    /// locked element that complained sixty times a second would make the
    /// status line unreadable and is the same refusal each time.
    fn begin_move(&mut self, id: ElementId) -> Outcome {
        self.gesture = None;
        let Some(c) = self.common(id) else {
            return Outcome::nothing();
        };
        if c.locked {
            let name = describe(c.id, &c.name);
            return Outcome::note(Effect::Nothing, format!("{name} is locked"));
        }
        self.gesture = Some((self.label.clone(), false));
        Outcome::nothing()
    }

    /// A placement during an open gesture. Silent when nothing is open, which
    /// is how a refused press stays refused for the rest of the drag.
    fn place(&mut self, id: ElementId, feed: i32, head: i32) -> Outcome {
        if self.gesture.is_none() {
            return Outcome::nothing();
        }
        match self.write_frame(id, feed, head) {
            Some(effect) => {
                if let Some((_, moved)) = &mut self.gesture {
                    *moved = true;
                }
                Outcome::plain(effect)
            }
            None => Outcome::nothing(),
        }
    }

    fn end_move(&mut self, committed: bool) -> Outcome {
        let Some((before, moved)) = self.gesture.take() else {
            return Outcome::nothing();
        };
        if !moved {
            // A click that selected and did not drag is not an undo step.
            return Outcome::nothing();
        }
        if !committed {
            self.label = before;
            self.bounds = Bounds::of(&self.label);
            return Outcome::plain(Effect::Everything);
        }
        self.push_undo(before);
        Outcome::nothing()
    }

    fn nudge(&mut self, id: ElementId, dx: i32, dy: i32, coarse: bool) -> Outcome {
        let step = if coarse {
            geom::millimetre(self.label.canvas.dpmm)
        } else {
            1
        };
        let Some(s) = self.shown_one(id) else {
            return Outcome::nothing();
        };
        let (feed, head) = geom::to_document(&self.target, s.x + dx * step, s.y + dy * step);
        self.one_step(id, feed, head)
    }

    /// An edit that is its own undo step: a nudge, or a placement set through
    /// the accessibility tree.
    fn one_step(&mut self, id: ElementId, feed: i32, head: i32) -> Outcome {
        let Some(c) = self.common(id) else {
            return Outcome::nothing();
        };
        if c.locked {
            let name = describe(c.id, &c.name);
            return Outcome::note(Effect::Nothing, format!("{name} is locked"));
        }
        let before = self.label.clone();
        match self.write_frame(id, feed, head) {
            Some(effect) => {
                self.push_undo(before);
                Outcome::plain(effect)
            }
            None => Outcome::nothing(),
        }
    }

    /// The only place a frame is written.
    ///
    /// `None` when nothing changed, so a move event that lands on the dot the
    /// element is already on costs one comparison and no redraw - which is most
    /// of them, at pointer speed.
    fn write_frame(&mut self, id: ElementId, feed: i32, head: i32) -> Option<Effect> {
        let drawn = {
            let c = self.common(id)?;
            if c.locked {
                return None;
            }
            geom::as_drawn(c.frame, c.rot)
        };
        let (feed, head) = geom::keep_reachable(&self.bounds, drawn, feed, head);
        let c = self.common_mut(id)?;
        if c.frame.feed == feed && c.frame.head == head {
            return None;
        }
        c.frame.feed = feed;
        c.frame.head = head;
        self.dirty = true;
        Some(Effect::Row(id))
    }

    fn step_history(&mut self, undo: bool) -> Outcome {
        // A gesture in flight is abandoned rather than half-undone: the
        // snapshot it is holding is older than the one about to be restored.
        self.gesture = None;
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(other) = from.pop() else {
            return Outcome::note(
                Effect::Nothing,
                if undo {
                    "nothing to undo".into()
                } else {
                    "nothing to redo".into()
                },
            );
        };
        to.push(std::mem::replace(&mut self.label, other));
        self.bounds = Bounds::of(&self.label);
        self.dirty = true;
        // The selected element may have been one that does not exist in the
        // restored document. Not possible yet - nothing adds or deletes - but
        // the rule belongs with the restore rather than with whatever adds one.
        if self.selected.is_some_and(|id| self.shown_one(id).is_none()) {
            self.selected = None;
        }
        Outcome::plain(Effect::Everything)
    }

    fn push_undo(&mut self, before: Label) {
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn common(&self, id: ElementId) -> Option<&relabel_render::document::Common> {
        self.label
            .elements
            .iter()
            .filter_map(ElementOrRaw::known)
            .map(Element::common)
            .find(|c| c.id == id)
    }

    /// The mutable half of [`Element::common`], which `relabel-render` does not
    /// have: the crate is read-only about geometry and preflight hand-writes
    /// the same match to set one flag. Exhaustive on purpose - a new element
    /// type in the document model fails to compile here rather than quietly
    /// becoming unmovable.
    fn common_mut(&mut self, id: ElementId) -> Option<&mut relabel_render::document::Common> {
        self.label
            .elements
            .iter_mut()
            .filter_map(|e| match e {
                ElementOrRaw::Known(e) => Some(e),
                ElementOrRaw::Raw(_) => None,
            })
            .map(|e| match e {
                Element::Text(x) => &mut x.common,
                Element::Barcode(x) => &mut x.common,
                Element::Qr(x) => &mut x.common,
                Element::DataMatrix(x) => &mut x.common,
                Element::Image(x) => &mut x.common,
                Element::Rect(x) => &mut x.common,
                Element::Line(x) => &mut x.common,
            })
            .find(|c| c.id == id)
    }
}

impl Outcome {
    fn nothing() -> Outcome {
        Outcome {
            effect: Effect::Nothing,
            note: None,
        }
    }

    fn plain(effect: Effect) -> Outcome {
        Outcome { effect, note: None }
    }

    fn note(effect: Effect, note: String) -> Outcome {
        Outcome {
            effect,
            note: Some(note),
        }
    }
}

/// What to call an element in a message: its own name when it has one, because
/// that is what the operator typed, and its id when it does not.
fn describe(id: ElementId, name: &str) -> String {
    if name.is_empty() {
        format!("element {id}")
    } else {
        format!("\u{2018}{name}\u{2019}")
    }
}

/// The element kind, spelled for a person rather than for the file.
fn kind_name(e: &Element) -> &'static str {
    match e {
        Element::Text(_) => "Text",
        Element::Barcode(_) => "Barcode",
        Element::Qr(_) => "QR code",
        Element::DataMatrix(_) => "DataMatrix",
        Element::Image(_) => "Image",
        Element::Rect(_) => "Rectangle",
        Element::Line(_) => "Line",
    }
}

/// A frame in the document's own axes, for callers assembling a test label.
pub fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

/// Exposed so the canvas tests and the application build the same document.
pub fn rotation(deg: i32) -> Rot {
    Rot::from(deg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_render::document::{AuthoredFor, Common, Element, RectEl};

    fn rect(id: ElementId, f: RectDots, locked: bool) -> ElementOrRaw {
        let mut common = Common::new(id, f);
        common.locked = locked;
        common.name = format!("box {id}");
        ElementOrRaw::Known(Element::Rect(RectEl {
            common,
            stroke_dots: 0,
            radius_dots: 0,
        }))
    }

    fn label() -> Label {
        let mut l = Label::new(320, 96, Dpmm::DPI_203);
        l.elements = vec![
            rect(1, frame(8, 8, 40, 24), false),
            rect(2, frame(120, 8, 40, 24), false),
        ];
        l
    }

    fn doc() -> Doc {
        Doc::open(label(), PrintDirection::Left).expect("a printable label")
    }

    fn frame_of(doc: &Doc, id: ElementId) -> RectDots {
        doc.common(id).expect("the element is there").frame
    }

    /// The axis swap is not a preference: it comes from the printer, and the
    /// file records which printer it was drawn for.
    #[test]
    fn a_label_that_names_its_model_takes_the_print_direction_from_the_registry() {
        let mut l = label();
        l.meta.authored_for = Some(AuthoredFor {
            model: Some("B1".into()),
            head_px: 96,
            dpi: 203,
        });
        let doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        assert_eq!(
            doc.canvas_dots(),
            (96, 320),
            "B1's head runs across the canvas"
        );
        assert!(!doc.orientation_note().contains("assumed"));
    }

    #[test]
    fn a_label_that_names_no_model_says_its_orientation_was_assumed() {
        let doc = doc();
        assert_eq!(doc.canvas_dots(), (320, 96));
        assert!(
            doc.orientation_note().contains("assumed"),
            "{}",
            doc.orientation_note()
        );
    }

    #[test]
    fn a_label_no_printer_could_take_is_refused_with_its_own_numbers() {
        let mut l = label();
        l.canvas.feed = 70_000;
        assert!(matches!(
            Doc::open(l, PrintDirection::Left),
            Err(OpenError::Unprintable { feed: 70_000, .. })
        ));
    }

    #[test]
    fn a_move_to_the_dot_it_is_already_on_costs_nothing() {
        let mut doc = doc();
        doc.apply(Cmd::BeginMove(1));
        let out = doc.apply(Cmd::MoveTo { id: 1, x: 8, y: 8 });
        assert_eq!(out.effect, Effect::Nothing);
        let out = doc.apply(Cmd::MoveTo { id: 1, x: 9, y: 8 });
        assert_eq!(out.effect, Effect::Row(1));
    }

    /// A locked element says so once, at the press. Sixty refusals a second
    /// would be the same refusal, and it would be the only thing the status
    /// line ever had time to say.
    #[test]
    fn a_locked_element_refuses_at_the_press_and_is_silent_for_the_rest_of_the_drag() {
        let mut l = label();
        l.elements[0] = rect(1, frame(8, 8, 40, 24), true);
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        let out = doc.apply(Cmd::BeginMove(1));
        assert_eq!(out.note.as_deref(), Some("\u{2018}box 1\u{2019} is locked"));
        for x in 9..20 {
            let out = doc.apply(Cmd::MoveTo { id: 1, x, y: 8 });
            assert_eq!(out, Outcome::nothing());
        }
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 40, 24));
    }

    #[test]
    fn a_cancelled_gesture_puts_the_element_back_where_it_was() {
        let mut doc = doc();
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo {
            id: 1,
            x: 40,
            y: 20,
        });
        assert_eq!(frame_of(&doc, 1), frame(40, 20, 40, 24));
        let out = doc.apply(Cmd::EndMove { committed: false });
        assert_eq!(out.effect, Effect::Everything);
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 40, 24));
        // And there is nothing to undo, because nothing was committed.
        assert_eq!(
            doc.apply(Cmd::Undo).note.as_deref(),
            Some("nothing to undo")
        );
    }

    #[test]
    fn a_gesture_is_one_undo_step_whatever_it_sent() {
        let mut doc = doc();
        doc.apply(Cmd::BeginMove(1));
        for x in 9..40 {
            doc.apply(Cmd::MoveTo { id: 1, x, y: 8 });
        }
        doc.apply(Cmd::EndMove { committed: true });
        assert_eq!(frame_of(&doc, 1).feed, 39);
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).feed, 8);
        doc.apply(Cmd::Redo);
        assert_eq!(frame_of(&doc, 1).feed, 39);
    }

    #[test]
    fn an_edit_after_an_undo_drops_what_there_was_to_redo() {
        let mut doc = doc();
        doc.apply(Cmd::Nudge {
            id: 1,
            dx: 1,
            dy: 0,
            coarse: false,
        });
        doc.apply(Cmd::Undo);
        doc.apply(Cmd::Nudge {
            id: 1,
            dx: 0,
            dy: 1,
            coarse: false,
        });
        assert_eq!(
            doc.apply(Cmd::Redo).note.as_deref(),
            Some("nothing to redo")
        );
        assert_eq!(frame_of(&doc, 1), frame(8, 9, 40, 24));
    }

    #[test]
    fn the_undo_stack_is_bounded_rather_than_growing_with_the_session() {
        let mut doc = doc();
        for _ in 0..UNDO_DEPTH + 20 {
            doc.apply(Cmd::Nudge {
                id: 1,
                dx: 1,
                dy: 0,
                coarse: false,
            });
        }
        assert_eq!(doc.undo.len(), UNDO_DEPTH);
    }

    #[test]
    fn a_document_is_clean_until_something_actually_moves() {
        let mut doc = doc();
        assert!(!doc.dirty());
        doc.apply(Cmd::Select(Some(1)));
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 8, y: 8 });
        doc.apply(Cmd::EndMove { committed: true });
        assert!(!doc.dirty(), "a click is not an edit");
        doc.apply(Cmd::Nudge {
            id: 1,
            dx: 1,
            dy: 0,
            coarse: false,
        });
        assert!(doc.dirty());
    }

    #[test]
    fn selecting_what_is_already_selected_changes_nothing() {
        let mut doc = doc();
        assert_eq!(doc.apply(Cmd::Select(Some(1))).effect, Effect::Selection);
        assert_eq!(doc.apply(Cmd::Select(Some(1))).effect, Effect::Nothing);
        assert_eq!(doc.apply(Cmd::Select(None)).effect, Effect::Selection);
    }

    /// An element written by a later version is carried, not drawn, and not
    /// selectable: this build has no frame for it that it could honestly move.
    #[test]
    fn an_element_of_an_unknown_type_is_carried_and_not_shown() {
        let mut l = label();
        l.elements.push(ElementOrRaw::Raw(
            serde_json::from_str(r#"{"type":"hologram","id":9,"shimmer":3}"#).expect("json"),
        ));
        let doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        assert_eq!(doc.shown().len(), 2);
        assert_eq!(doc.undrawable(), 1);
        assert!(doc.shown_one(9).is_none());
        assert!(doc.label().to_json().expect("writable").contains("shimmer"));
    }
}
