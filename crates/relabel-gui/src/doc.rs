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
use crate::ins::{self, Field};
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
    /// Text an operator has typed into a field and not finished.
    ///
    /// Held, not written. The pane cannot know when an edit is over - the handler
    /// that would say so is a change handler, drained at the end of the input
    /// dispatch, so it arrives after the pointer press that moved the focus has
    /// already been acted on - and this side can, because every other command
    /// passes through [`Doc::apply`] first. No element id: a keystroke is
    /// synchronous, so the element the operator typed into is the one selected at
    /// that instant, and this records it.
    StageField {
        field: Field,
        text: String,
    },
    /// Write whatever is staged: Return, the focus leaving a field, or an
    /// assistive technology writing a whole value. Carries nothing on purpose - a
    /// late one finds the slot already flushed and does nothing, where a late one
    /// carrying text would write the field's *new* contents onto the element the
    /// operator had just left.
    CommitField,
    /// One increment of a field in its own unit: its own undo step, immediately,
    /// the way a nudge is.
    StepField {
        field: Field,
        by: i32,
    },
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
    /// The one field an operator is part way through typing into, the element
    /// they typed it into, and the text. At most one, because only one field can
    /// hold the keyboard, and it exists only between two commands.
    staged: Option<(ElementId, Field, String)>,
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
            staged: None,
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

    /// The one entry point. Every gesture, every key, every accessibility action
    /// and every field arrives here.
    pub fn apply(&mut self, cmd: Cmd) -> Outcome {
        // Staging is the one command that does not finish the edit it carries.
        if let Cmd::StageField { field, text } = cmd {
            return self.stage(field, text);
        }
        // A press, an undo and a redo drop whatever gesture is still open, as
        // `begin_move` and `step_history` always have. Dropped here, first, so the
        // flush below is not held back for a gesture that is about to end anyway.
        if matches!(cmd, Cmd::BeginMove(_) | Cmd::Undo | Cmd::Redo) {
            self.gesture = None;
        }
        // While the pointer is down on an element, the undo step being built is the
        // gesture's: one snapshot taken at the press and pushed at the release. A
        // write from anywhere else in between would push a snapshot of its own
        // inside that step, and the release would push the older one on top of it -
        // so undo walked through the drag without the edit, and a cancel restored a
        // snapshot from before the edit and took the edit away. Such a write
        // therefore waits for the release: the staged text stays staged and lands as
        // the step after the gesture's, and a step, a nudge or a spoken placement
        // is ignored.
        let held = self.gesture.is_some();
        // Everything else writes the staged text first, so a selection, a drag,
        // an undo or a save can never act on an element whose typed number has
        // not landed - and so the order in which the toolkit delivers a focus
        // change and a pointer press stops mattering.
        let first = if held {
            Outcome::nothing()
        } else {
            self.commit_staged()
        };
        let then = match cmd {
            // No early return here, unlike before the flush above existed: a
            // return would drop the outcome of the edit the flush just wrote.
            Cmd::Select(id) if self.selected == id => Outcome::nothing(),
            Cmd::Select(id) => {
                self.selected = id;
                Outcome::plain(Effect::Selection)
            }
            Cmd::BeginMove(id) => self.begin_move(id),
            Cmd::MoveTo { id, x, y } => {
                let (feed, head) = geom::to_document(&self.target, x, y);
                self.place(id, feed, head)
            }
            // The gesture's own step first and then the edit that waited for it, so
            // the edit is a step of its own that neither an undo nor a cancel folds
            // into the drag.
            Cmd::EndMove { committed } => {
                let out = self.end_move(committed);
                out.also(self.commit_staged())
            }
            Cmd::Nudge { .. } | Cmd::SetPlacement { .. } | Cmd::StepField { .. } if held => {
                Outcome::nothing()
            }
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
            Cmd::CommitField => Outcome::nothing(),
            Cmd::StepField { field, by } => self.step_field(field, by),
            Cmd::StageField { .. } => unreachable!("staging returned above"),
        };
        first.also(then)
    }

    /// What the inspector shows: the selected element as [`ins::SLOTS`] slots,
    /// with the staged text standing in for the value it is about to replace.
    ///
    /// The staged text is in here rather than left on screen untouched so that the
    /// push can be unconditional and idempotent: assigning a field the string it
    /// already holds marks no property dirty (`Property::set` compares first), so
    /// the caret does not move and typing through a push is indistinguishable from
    /// typing without one.
    pub fn inspected(&self) -> Option<Vec<ins::Slot>> {
        let e = self.element(self.selected?)?;
        let c = e.common();
        // From the frame as drawn, for `keep_reachable`'s reason: a rotated
        // element is kept reachable by the side that is actually on screen, and
        // the interval announced has to be the interval enforced.
        let drawn = geom::as_drawn(c.frame, c.rot);
        let reach = (
            geom::reach(drawn.feed_len, self.bounds.canvas.feed_len),
            geom::reach(drawn.head_len, self.bounds.canvas.head_len),
        );
        let mut slots = ins::common_slots(e, reach);
        slots.extend(ins::kind_slots(e));
        debug_assert!(
            slots.len() <= ins::SLOTS,
            "a {} needs {} slots and the pane has {}",
            kind_name(e),
            slots.len(),
            ins::SLOTS
        );
        if let Some((id, field, text)) = &self.staged
            && *id == c.id
        {
            for s in slots.iter_mut().filter(|s| s.field == *field) {
                s.value.clone_from(text);
            }
        }
        slots.resize(ins::SLOTS, ins::Slot::unused());
        Some(slots)
    }

    /// What the pane calls the selection. Composed here, because the UI never
    /// composes a message.
    pub fn inspected_heading(&self) -> String {
        self.selected
            .and_then(|id| self.element(id))
            .map(ins::heading)
            .unwrap_or_default()
    }

    /// The staged text, for the tests that pin the slot's lifetime.
    pub fn staged(&self) -> Option<(ElementId, Field, &str)> {
        self.staged
            .as_ref()
            .map(|(id, field, text)| (*id, *field, text.as_str()))
    }

    fn stage(&mut self, field: Field, text: String) -> Outcome {
        let Some(id) = self.selected else {
            return Outcome::nothing();
        };
        // Typing into a second field finishes the first, synchronously. The
        // keyboard arriving in the second field says so too, but later.
        let first = match &self.staged {
            Some((was, which, _)) if (*was, *which) != (id, field) => {
                // The one write that cannot wait for a pointer release, because the
                // slot holds a single edit and the new one needs it. The open
                // gesture becomes its own step first, so the edit still lands after
                // the drag rather than inside it; the price is that a cancel
                // arriving later has nothing left to roll back.
                self.close_gesture();
                self.commit_staged()
            }
            _ => Outcome::nothing(),
        };
        self.staged = Some((id, field, text));
        first
    }

    fn commit_staged(&mut self) -> Outcome {
        let Some((id, field, text)) = self.staged.take() else {
            return Outcome::nothing();
        };
        self.write_field(id, field, &text)
    }

    /// One field, one undo step - `one_step`'s shape, for a value rather than a
    /// placement.
    ///
    /// Returns its note whether or not anything changed: a number clamped onto the
    /// value the element already held changes nothing and still has to be
    /// explained, because the operator may not be looking at the canvas at all.
    ///
    /// `Bounds` is not rebuilt after a write. `Bounds::of` reads only
    /// `label.canvas` and `label.media`, and no field in this pane touches either
    /// - which a media pane would, and it will have to rebuild them.
    fn write_field(&mut self, id: ElementId, field: Field, text: &str) -> Outcome {
        let Some(current) = self.element(id) else {
            return Outcome::nothing();
        };
        let who = describe(id, &current.common().name);
        // Refused here rather than trusted to the pane's read-only flag, because
        // neither a LineEdit's nor a SpinBox's own set-value action checks it:
        // the pane announces the rule and this enforces it.
        if current.common().locked {
            return Outcome::note(Effect::Nothing, format!("{who} is locked"));
        }
        if matches!(field, Field::Feed | Field::Head) {
            return self.typed_placement(id, field, text);
        }
        let mut next = current.clone();
        let note = match ins::write(&mut next, field, text) {
            Err(why) => return Outcome::note(Effect::Nothing, why),
            Ok(ins::Wrote::Unchanged) => return Outcome::nothing(),
            Ok(ins::Wrote::Changed(note)) => note,
        };
        // A quarter turn swaps the drawn frame's sides, so the interval the
        // placement may land in turns with it. Re-clamped in the same undo step,
        // and said, because the operator asked for a rotation and not a move.
        let mut moved = None;
        if field == Field::Rot {
            let c = ins::common_of(&mut next);
            let drawn = geom::as_drawn(c.frame, c.rot);
            let (feed, head) =
                geom::keep_reachable(&self.bounds, drawn, c.frame.feed, c.frame.head);
            if (feed, head) != (c.frame.feed, c.frame.head) {
                moved = Some(format!(
                    "a turn to {} degrees would leave nothing of {who} on the canvas to grab, \
                     so it moved to feed {feed}, head {head}",
                    i32::from(c.rot)
                ));
                c.frame.feed = feed;
                c.frame.head = head;
            }
        }
        let before = self.label.clone();
        if let Some(e) = self.element_mut(id) {
            *e = next;
        }
        self.push_undo(before);
        self.dirty = true;
        Outcome {
            effect: Effect::Row(id),
            note: both(note, moved),
        }
    }

    /// A placement that was typed rather than dragged.
    ///
    /// Clamped by `geom::keep_reachable`, the same call the pointer obeys, because
    /// two answers to where an element may be is one too many - and said out loud,
    /// because the pointer needs no telling (the element visibly stops following
    /// it) and a typed number has no such feedback.
    fn typed_placement(&mut self, id: ElementId, field: Field, text: &str) -> Outcome {
        let typed = match ins::number(field, text) {
            Ok(n) => n,
            Err(why) => return Outcome::note(Effect::Nothing, why),
        };
        let Some(c) = self.common(id) else {
            return Outcome::nothing();
        };
        let who = describe(id, &c.name);
        let drawn = geom::as_drawn(c.frame, c.rot);
        let asked = match field {
            Field::Feed => (typed, c.frame.head),
            _ => (c.frame.feed, typed),
        };
        let kept = geom::keep_reachable(&self.bounds, drawn, asked.0, asked.1);
        let landed = if field == Field::Feed { kept.0 } else { kept.1 };
        let label = field.label();
        let note = (landed != typed).then(|| {
            format!(
                "{label} {typed} would leave nothing of {who} on the canvas to grab, \
                 so it is at {label} {landed}"
            )
        });
        let out = self.one_step(id, kept.0, kept.1);
        Outcome {
            effect: out.effect,
            note: both(note, out.note),
        }
    }

    fn step_field(&mut self, field: Field, by: i32) -> Outcome {
        let Some(id) = self.selected else {
            return Outcome::nothing();
        };
        let Some(e) = self.element(id) else {
            return Outcome::nothing();
        };
        if field.shape() != ins::Shape::Number {
            return Outcome::nothing();
        }
        let text = ins::stepped(e, field, by);
        self.write_field(id, field, &text)
    }

    fn element(&self, id: ElementId) -> Option<&Element> {
        self.label
            .elements
            .iter()
            .filter_map(ElementOrRaw::known)
            .find(|e| e.common().id == id)
    }

    /// The whole element, mutably. No match over the element type: the filter is
    /// over `ElementOrRaw::Known`, and the exhaustive part lives in
    /// `ins::common_of`.
    fn element_mut(&mut self, id: ElementId) -> Option<&mut Element> {
        self.label.elements.iter_mut().find_map(|e| match e {
            ElementOrRaw::Known(e) if e.common().id == id => Some(e),
            _ => None,
        })
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

    /// An open gesture's step, pushed now as its release would have pushed it.
    fn close_gesture(&mut self) {
        if let Some((before, true)) = self.gesture.take() {
            self.push_undo(before);
        }
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
    /// the same match to set one flag. The exhaustive match is
    /// [`ins::common_of`]'s, so it exists once for this and for the inspector's
    /// writer.
    fn common_mut(&mut self, id: ElementId) -> Option<&mut relabel_render::document::Common> {
        self.label
            .elements
            .iter_mut()
            .filter_map(|e| match e {
                ElementOrRaw::Known(e) => Some(e),
                ElementOrRaw::Raw(_) => None,
            })
            .map(ins::common_of)
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

    /// Both halves of one `apply`: the staged text that landed, and the command
    /// that asked for it. Joined here, because the UI never composes a message.
    fn also(self, then: Outcome) -> Outcome {
        Outcome {
            effect: self.effect.join(then.effect),
            note: both(self.note, then.note),
        }
    }
}

impl Effect {
    /// The wider of two effects. `Selection` joins away to the other, because the
    /// selection is now pushed unconditionally and the variant means only "nothing
    /// to redraw"; two *different* rows are everything, which keeps a move at
    /// pointer speed costing one row.
    fn join(self, other: Effect) -> Effect {
        use Effect::*;
        match (self, other) {
            (Everything, _) | (_, Everything) => Everything,
            (Row(a), Row(b)) if a != b => Everything,
            (Row(a), _) | (_, Row(a)) => Row(a),
            (Selection, _) | (_, Selection) => Selection,
            (Nothing, Nothing) => Nothing,
        }
    }
}

/// Two sentences as one, in the order they happened.
fn both(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}

/// What to call an element in a message: its own name when it has one, because
/// that is what the operator typed, and its id when it does not.
pub(crate) fn describe(id: ElementId, name: &str) -> String {
    if name.is_empty() {
        format!("element {id}")
    } else {
        format!("\u{2018}{name}\u{2019}")
    }
}

/// The element kind, spelled for a person rather than for the file.
pub(crate) fn kind_name(e: &Element) -> &'static str {
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

    fn stage(doc: &mut Doc, field: Field, text: &str) -> Outcome {
        doc.apply(Cmd::StageField {
            field,
            text: text.to_string(),
        })
    }

    fn name_of(doc: &Doc, id: ElementId) -> String {
        doc.common(id).expect("the element is there").name.clone()
    }

    fn shown_value(doc: &Doc, field: Field) -> String {
        doc.inspected()
            .expect("something is selected")
            .into_iter()
            .find(|s| s.field == field)
            .expect("the field is in the pane")
            .value
    }

    #[test]
    fn a_staged_edit_is_not_in_the_document_until_something_asks_for_it() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        assert_eq!(stage(&mut doc, Field::Feed, "37"), Outcome::nothing());
        assert_eq!(frame_of(&doc, 1).feed, 8);
        assert!(!doc.dirty());
        assert_eq!(doc.staged(), Some((1, Field::Feed, "37")));
        let out = doc.apply(Cmd::CommitField);
        assert_eq!(out.effect, Effect::Row(1));
        assert_eq!(frame_of(&doc, 1).feed, 37);
        assert_eq!(doc.staged(), None);
    }

    #[test]
    fn the_staged_slot_is_empty_after_every_command() {
        let commands = [
            Cmd::Select(Some(2)),
            Cmd::Select(Some(1)),
            Cmd::Select(None),
            Cmd::BeginMove(1),
            Cmd::MoveTo { id: 1, x: 9, y: 8 },
            Cmd::EndMove { committed: false },
            Cmd::Nudge {
                id: 1,
                dx: 1,
                dy: 0,
                coarse: false,
            },
            Cmd::SetPlacement {
                id: 1,
                value: "feed 3, head 4".into(),
            },
            Cmd::Undo,
            Cmd::Redo,
            Cmd::CommitField,
            Cmd::StepField {
                field: Field::Head,
                by: 1,
            },
        ];
        for cmd in commands {
            let mut doc = doc();
            doc.apply(Cmd::Select(Some(1)));
            stage(&mut doc, Field::Name, "typed");
            doc.apply(cmd.clone());
            assert_eq!(doc.staged(), None, "{cmd:?}");
            let expected = if cmd == Cmd::Undo { "box 1" } else { "typed" };
            assert_eq!(name_of(&doc, 1), expected, "{cmd:?}");
        }
    }

    #[test]
    fn selecting_another_element_writes_the_number_typed_into_the_last_one() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "37");
        let out = doc.apply(Cmd::Select(Some(2)));
        assert_eq!(frame_of(&doc, 1).feed, 37);
        assert_eq!(doc.selected(), Some(2));
        assert_eq!(
            out.effect,
            Effect::Row(1),
            "the row the edit landed on, not only the selection"
        );
    }

    /// The order the toolkit was measured to deliver: the press selects the
    /// second element and pushes its values, and only then does the first field's
    /// blur ask for a commit. A commit carrying the field's text would write the
    /// second element's own number back onto it; one carrying nothing finds the
    /// slot already flushed.
    #[test]
    fn a_staged_edit_lands_on_the_element_it_was_typed_into_and_not_the_one_now_selected() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "37");
        doc.apply(Cmd::Select(Some(2)));
        assert_eq!(doc.apply(Cmd::CommitField), Outcome::nothing());
        assert_eq!(frame_of(&doc, 1).feed, 37);
        assert_eq!(frame_of(&doc, 2).feed, 120);
        assert_eq!(doc.undo.len(), 1);
    }

    #[test]
    fn typing_into_a_second_field_finishes_the_first() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "37");
        let out = stage(&mut doc, Field::Head, "5");
        assert_eq!(out.effect, Effect::Row(1));
        assert_eq!(frame_of(&doc, 1), frame(37, 8, 40, 24));
        assert_eq!(doc.staged(), Some((1, Field::Head, "5")));
        // The same field again is still the same edit.
        assert_eq!(stage(&mut doc, Field::Head, "56"), Outcome::nothing());
        assert_eq!(doc.undo.len(), 1);
    }

    #[test]
    fn a_pointer_gesture_writes_the_staged_edit_before_it_takes_its_snapshot() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "37");
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 50, y: 8 });
        doc.apply(Cmd::EndMove { committed: true });
        assert_eq!(frame_of(&doc, 1).feed, 50);
        doc.apply(Cmd::Undo);
        assert_eq!(
            frame_of(&doc, 1).feed,
            37,
            "the drag's snapshot held the edit"
        );
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).feed, 8);
    }

    /// A field committed while a drag is open has its own undo step, so the
    /// drag's cancel must not take it away as well.
    #[test]
    fn a_cancelled_drag_cannot_roll_back_a_field_committed_while_it_was_open() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 50, y: 8 });
        stage(&mut doc, Field::Name, "typed");
        doc.apply(Cmd::CommitField);
        doc.apply(Cmd::EndMove { committed: false });
        assert_eq!(frame_of(&doc, 1).feed, 8, "the drag rolled back");
        assert_eq!(
            name_of(&doc, 1),
            "typed",
            "the field commit, which has its own undo step, did not"
        );
    }

    /// Undo after a field committed during an open drag has to walk back
    /// through states that actually existed, and end at the label as it opened.
    #[test]
    fn undoing_a_field_committed_during_a_drag_ends_at_the_label_as_it_opened() {
        let mut doc = doc();
        let opened = doc.label().clone();
        doc.apply(Cmd::Select(Some(1)));
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 50, y: 8 });
        stage(&mut doc, Field::Name, "typed");
        doc.apply(Cmd::CommitField);
        doc.apply(Cmd::EndMove { committed: true });
        let mut seen = Vec::new();
        while doc.apply(Cmd::Undo).note.is_none() {
            seen.push((frame_of(&doc, 1).feed, name_of(&doc, 1)));
        }
        assert_eq!(
            doc.label(),
            &opened,
            "the states undo walked through: {seen:?}"
        );
    }

    /// The staged slot holds one edit, so typing into a second field while the
    /// pointer is still down cannot wait for the release. Both edits still land
    /// after the drag's own step, never inside it.
    #[test]
    fn typing_into_a_second_field_during_a_drag_still_undoes_back_to_the_label_as_it_opened() {
        let mut doc = doc();
        let opened = doc.label().clone();
        doc.apply(Cmd::Select(Some(1)));
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 50, y: 8 });
        stage(&mut doc, Field::Name, "typed");
        stage(&mut doc, Field::StrokeDots, "3");
        doc.apply(Cmd::EndMove { committed: true });
        assert_eq!(name_of(&doc, 1), "typed");
        let mut seen = Vec::new();
        while doc.apply(Cmd::Undo).note.is_none() {
            seen.push((frame_of(&doc, 1).feed, name_of(&doc, 1)));
        }
        assert_eq!(
            doc.label(),
            &opened,
            "the states undo walked through: {seen:?}"
        );
    }

    /// An arrow key pressed while the pointer is still down would be a step inside
    /// the drag's step, the same shape as a field commit there.
    #[test]
    fn a_nudge_pressed_during_a_drag_does_not_split_the_drags_undo_step() {
        let mut doc = doc();
        let opened = doc.label().clone();
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 50, y: 8 });
        doc.apply(Cmd::Nudge {
            id: 1,
            dx: 1,
            dy: 0,
            coarse: false,
        });
        doc.apply(Cmd::EndMove { committed: true });
        let mut seen = Vec::new();
        while doc.apply(Cmd::Undo).note.is_none() {
            seen.push(frame_of(&doc, 1).feed);
        }
        assert_eq!(
            doc.label(),
            &opened,
            "the feeds undo walked through: {seen:?}"
        );
    }

    #[test]
    fn a_cancelled_drag_cannot_roll_back_an_edit_that_was_typed_before_it() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "37");
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 50, y: 8 });
        doc.apply(Cmd::EndMove { committed: false });
        assert_eq!(frame_of(&doc, 1).feed, 37);
    }

    #[test]
    fn an_undo_writes_the_staged_edit_and_then_undoes_it() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Name, "typed");
        let out = doc.apply(Cmd::Undo);
        assert_eq!(out.effect, Effect::Everything);
        assert_eq!(name_of(&doc, 1), "box 1");
        doc.apply(Cmd::Redo);
        assert_eq!(
            name_of(&doc, 1),
            "typed",
            "the edit was real, so it can be redone"
        );
    }

    #[test]
    fn a_commit_with_nothing_staged_changes_nothing_and_says_nothing() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        assert_eq!(doc.apply(Cmd::CommitField), Outcome::nothing());
        assert!(doc.undo.is_empty());
        assert!(!doc.dirty());
    }

    #[test]
    fn a_field_edit_is_one_undo_step_and_undoing_it_puts_the_old_value_back() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        for typed in ["a", "ab", "abc"] {
            stage(&mut doc, Field::Name, typed);
        }
        doc.apply(Cmd::CommitField);
        assert_eq!(name_of(&doc, 1), "abc");
        assert_eq!(doc.undo.len(), 1, "one field visit, one step");
        doc.apply(Cmd::Undo);
        assert_eq!(name_of(&doc, 1), "box 1");
    }

    #[test]
    fn a_step_is_its_own_undo_step_the_way_a_nudge_is() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        for _ in 0..2 {
            doc.apply(Cmd::StepField {
                field: Field::Head,
                by: 1,
            });
        }
        assert_eq!(frame_of(&doc, 1).head, 10);
        assert_eq!(doc.undo.len(), 2);
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).head, 9);
    }

    #[test]
    fn an_edit_that_wrote_nothing_leaves_the_undo_stack_alone() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        for (field, typed) in [
            (Field::Name, "box 1"),
            (Field::Feed, "8"),
            (Field::Feed, "twelve"),
            (Field::StrokeDots, "0"),
            (Field::StrokeDots, "-4"),
        ] {
            stage(&mut doc, field, typed);
            doc.apply(Cmd::CommitField);
        }
        assert!(doc.undo.is_empty());
        assert!(!doc.dirty());
    }

    #[test]
    fn a_typed_placement_is_kept_on_the_canvas_and_says_where_it_went() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "9000");
        let out = doc.apply(Cmd::CommitField);
        assert_eq!(frame_of(&doc, 1).feed, 319);
        assert_eq!(out.effect, Effect::Row(1));
        let note = out.note.expect("a clamp is said");
        assert!(
            note.contains("feed 9000") && note.contains("feed 319"),
            "{note}"
        );
        stage(&mut doc, Field::Head, "-9000");
        doc.apply(Cmd::CommitField);
        assert_eq!(frame_of(&doc, 1).head, -23);
        // A placement inside the interval is written as typed, silently.
        stage(&mut doc, Field::Feed, "-5");
        let out = doc.apply(Cmd::CommitField);
        assert_eq!(frame_of(&doc, 1).feed, -5);
        assert_eq!(out.note, None);
    }

    #[test]
    fn a_placement_that_clamps_to_where_the_element_already_is_still_says_so() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "9000");
        doc.apply(Cmd::CommitField);
        stage(&mut doc, Field::Feed, "9001");
        let out = doc.apply(Cmd::CommitField);
        assert_eq!(out.effect, Effect::Nothing);
        assert!(
            out.note.as_deref().is_some_and(|n| n.contains("feed 319")),
            "{out:?}"
        );
        assert_eq!(doc.undo.len(), 1);
    }

    #[test]
    fn a_quarter_turn_that_would_leave_nothing_to_grab_moves_the_element_and_says_so() {
        let mut l = label();
        l.elements[0] = rect(1, frame(-30, 8, 40, 24), false);
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Rot, "90");
        let out = doc.apply(Cmd::CommitField);
        assert_eq!(doc.common(1).expect("there").rot, Rot::D90);
        assert_eq!(frame_of(&doc, 1).feed, -23);
        assert!(
            out.note.as_deref().is_some_and(|n| n.contains("feed -23")),
            "{out:?}"
        );
        assert_eq!(doc.undo.len(), 1, "the turn and the move are one step");
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).feed, -30);
        assert_eq!(doc.common(1).expect("there").rot, Rot::D0);
    }

    #[test]
    fn a_locked_element_refuses_every_field_and_names_itself() {
        let mut l = label();
        l.elements[0] = rect(1, frame(8, 8, 40, 24), true);
        let mut doc = Doc::open(l.clone(), PrintDirection::Left).expect("a printable label");
        doc.apply(Cmd::Select(Some(1)));
        let slots = doc.inspected().expect("selected");
        assert!(slots.iter().all(|s| s.read_only));
        for s in slots
            .iter()
            .filter(|s| s.shape == ins::Shape::Number || s.shape == ins::Shape::Word)
        {
            stage(&mut doc, s.field, "1");
            let out = doc.apply(Cmd::CommitField);
            assert_eq!(
                out.note.as_deref(),
                Some("\u{2018}box 1\u{2019} is locked"),
                "{:?}",
                s.field
            );
            if s.shape == ins::Shape::Number {
                let out = doc.apply(Cmd::StepField {
                    field: s.field,
                    by: 1,
                });
                assert_eq!(out.note.as_deref(), Some("\u{2018}box 1\u{2019} is locked"));
            }
        }
        assert_eq!(doc.label(), &l);
    }

    #[test]
    fn a_name_edit_rewrites_the_sentence_the_canvas_reads() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Name, "SKU");
        let out = doc.apply(Cmd::CommitField);
        assert_eq!(out.effect, Effect::Row(1));
        assert_eq!(
            doc.shown_one(1).expect("shown").say,
            "Rectangle \u{2018}SKU\u{2019}, feed 8, head 8, 40 by 24 dots"
        );
        assert_eq!(doc.inspected_heading(), "Rectangle \u{2018}SKU\u{2019}");
    }

    #[test]
    fn nothing_selected_is_no_pane_at_all() {
        let mut doc = doc();
        assert_eq!(doc.inspected(), None);
        assert_eq!(doc.inspected_heading(), "");
        doc.apply(Cmd::Select(Some(1)));
        assert!(doc.inspected().is_some());
        doc.apply(Cmd::Select(None));
        assert_eq!(doc.inspected(), None);
    }

    #[test]
    fn a_field_edit_with_nothing_selected_changes_nothing() {
        let mut doc = doc();
        let before = doc.label().clone();
        assert_eq!(stage(&mut doc, Field::Feed, "37"), Outcome::nothing());
        assert_eq!(doc.staged(), None);
        assert_eq!(doc.apply(Cmd::CommitField), Outcome::nothing());
        assert_eq!(
            doc.apply(Cmd::StepField {
                field: Field::Feed,
                by: 1
            }),
            Outcome::nothing()
        );
        assert_eq!(doc.label(), &before);
    }

    #[test]
    fn the_pane_shows_the_staged_text_for_the_field_being_typed_into_and_the_document_for_every_other()
     {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "3");
        assert_eq!(shown_value(&doc, Field::Feed), "3");
        assert_eq!(shown_value(&doc, Field::Head), "8");
        assert_eq!(shown_value(&doc, Field::Name), "box 1");
        assert_eq!(frame_of(&doc, 1).feed, 8);
        let slots = doc.inspected().expect("selected");
        assert_eq!(slots.len(), ins::SLOTS);
        assert_eq!(
            slots
                .iter()
                .filter(|s| s.shape == ins::Shape::Unused)
                .count(),
            ins::SLOTS - 9,
            "a rectangle has nine rows"
        );
        // And the announced interval is `reach` on this element's drawn frame.
        let feed = slots.iter().find(|s| s.field == Field::Feed).expect("feed");
        assert_eq!((feed.min, feed.max), (-39.0, 319.0));
    }

    #[test]
    fn two_different_rows_in_one_outcome_ask_for_everything() {
        assert_eq!(Effect::Row(1).join(Effect::Row(2)), Effect::Everything);
        assert_eq!(Effect::Row(1).join(Effect::Row(1)), Effect::Row(1));
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "37");
        let out = doc.apply(Cmd::Nudge {
            id: 2,
            dx: 1,
            dy: 0,
            coarse: false,
        });
        assert_eq!(out.effect, Effect::Everything);
    }

    #[test]
    fn a_selection_joined_with_a_row_still_asks_for_the_row() {
        assert_eq!(Effect::Selection.join(Effect::Row(3)), Effect::Row(3));
        assert_eq!(Effect::Row(3).join(Effect::Selection), Effect::Row(3));
        assert_eq!(Effect::Nothing.join(Effect::Selection), Effect::Selection);
        assert_eq!(Effect::Nothing.join(Effect::Nothing), Effect::Nothing);
        assert_eq!(Effect::Everything.join(Effect::Nothing), Effect::Everything);
    }

    #[test]
    fn both_halves_of_one_command_are_said() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "9000");
        let out = doc.apply(Cmd::Redo);
        let note = out.note.expect("two sentences");
        assert!(note.starts_with("feed 9000 would leave"), "{note}");
        assert!(note.ends_with("; nothing to redo"), "{note}");
    }

    #[test]
    fn every_element_kind_can_be_inspected_without_panicking() {
        let f = r#""frame": {"feed": 8, "head": 4, "feed_len": 96, "head_len": 24}"#;
        let json = format!(
            r#"{{"format": "relabel-label", "version": 1,
                "media": {{"feed_tenths_mm": 400, "head_tenths_mm": 120}},
                "canvas": {{"feed": 320, "head": 96, "dpmm": {{"num": 8, "den": 1}}}},
                "elements": [
                  {{"type": "text", "id": 1, {f}, "text": "{{sku}}", "size": 20.0}},
                  {{"type": "barcode", "id": 2, {f}, "symbology": "code128", "data": "1", "module_dots": 2, "bar_dots": 0}},
                  {{"type": "qr", "id": 3, {f}, "data": "x", "module_dots": 2}},
                  {{"type": "data_matrix", "id": 4, {f}, "data": "x", "module_dots": 2}},
                  {{"type": "image", "id": 5, {f}}},
                  {{"type": "rect", "id": 6, {f}}},
                  {{"type": "line", "id": 7, {f}}}
                ]}}"#
        );
        let label = Label::from_json(&json).expect("a label of every kind");
        let mut doc = Doc::open(label, PrintDirection::Left).expect("a printable label");
        for id in 1..=7 {
            doc.apply(Cmd::Select(Some(id)));
            let slots = doc.inspected().expect("selected");
            assert_eq!(slots.len(), ins::SLOTS);
            let heading = doc.inspected_heading();
            assert!(heading.ends_with(&format!("element {id}")), "{heading}");
            for s in slots {
                for by in [1, -1] {
                    doc.apply(Cmd::StepField { field: s.field, by });
                }
                stage(&mut doc, s.field, &s.value);
                doc.apply(Cmd::CommitField);
            }
        }
    }
}
