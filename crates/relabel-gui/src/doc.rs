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
//! written against. A grip drag is the same transaction with
//! [`Cmd::BeginResize`] and [`Cmd::ResizeTo`], closed by the same `EndMove`.
//!
//! # What a held pointer forbids
//!
//! While the pointer is down, the row under it holds the grab. A command that
//! removes that row strands the press: the release never reaches the grabber, and
//! the gesture never ends. So adding, deleting, and an undo or redo that would add
//! or take away an element all wait for the release, and say so.

use crate::a11y;
use crate::geom;
use crate::ins::{self, Field};
use crate::make;
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
    /// A new element of this kind, on top of the others, selected.
    Create(make::Kind),
    /// Take away an element: the selection, or `None` when there is none, which is
    /// said rather than ignored because a button or a key asked.
    Delete {
        id: Option<ElementId>,
    },
    /// Pointer down on a grip: open one undo step for the resize that follows.
    BeginResize {
        id: ElementId,
        grip: geom::Grip,
    },
    /// The gripped edges dragged to canvas dots `right` and `bottom`, the exclusive
    /// far edges, as the pointer reports them.
    ResizeTo {
        id: ElementId,
        right: i32,
        bottom: i32,
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
    /// Each earlier and later state with the revision it had.
    undo: Vec<(Label, u64)>,
    redo: Vec<(Label, u64)>,
    /// The pointer gesture being built into one undo step, if any.
    gesture: Option<Gesture>,
    /// Whether the pointer is down on an element, from the press to the release.
    ///
    /// Not the same as a gesture being open: an undo drops the gesture and leaves
    /// the pointer down, and a press on a locked element opens none. What a held
    /// pointer forbids depends on the pointer, not on the step.
    pressed: bool,
    /// The id the next element is given. Only ever rises, so an id undone away is
    /// not handed to a different element while its old snapshot is still on the
    /// stack; wide, so the rise past the row id range is a fact and not a wrap.
    next_id: u64,
    /// Which state the document is in. Every edit gives it a revision no state
    /// has had, from `last_rev`, and an undo or a redo brings back the one the
    /// state it restores had; so the document is dirty exactly when this is not
    /// `saved_rev`, and a cancelled drag or an undo back to the save reads clean.
    rev: u64,
    /// The last revision handed out. Only ever rises.
    last_rev: u64,
    /// The revision that was last written to disk, or the one the label opened at.
    saved_rev: u64,
    /// The one field an operator is part way through typing into, the element
    /// they typed it into, and the text. At most one, because only one field can
    /// hold the keyboard, and it exists only between two commands.
    staged: Option<(ElementId, Field, String)>,
}

/// One pointer gesture: the document as it was when the pointer went down, whether
/// anything has changed since, and what the pointer is doing to its element.
struct Gesture {
    before: Label,
    /// The revision `before` had, which a cancel restores with it.
    before_rev: u64,
    moved: bool,
    kind: Held,
}

enum Held {
    /// Dragging an element by its frame.
    Move,
    /// Dragging one of an element's far edges, from the frame it had at the press.
    /// `clamp` is what last held the drag, said once, at the release, rather than
    /// sixty times a second.
    Resize {
        id: ElementId,
        grip: geom::Grip,
        from: RectDots,
        clamp: Option<String>,
    },
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
            next_id: held_ids(&label).max().map_or(1, |m| u64::from(m) + 1),
            label,
            target,
            direction,
            direction_assumed: from_file.is_none(),
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            gesture: None,
            pressed: false,
            rev: 0,
            last_rev: 0,
            saved_rev: 0,
            staged: None,
        })
    }

    /// Whether the pointer is down on an element. The canvas asks, so that what it
    /// offers under the pointer does not change mid-press.
    pub fn pressed(&self) -> bool {
        self.pressed
    }

    /// What the canvas needs for the selected element's grips: its drawn canvas
    /// box, whether it is locked, and the room beyond its far edges that no other
    /// element's ink is in. `None` when nothing drawn is selected.
    pub fn grips(&self) -> Option<geom::Grips> {
        let id = self.selected?;
        let drawn: Vec<(ElementId, bool)> = self
            .label
            .drawable()
            .map(|e| (e.common().id, e.common().locked))
            .collect();
        let i = drawn.iter().position(|d| d.0 == id)?;
        let solids = self.solids();
        let (x, y, w, h) = solids[i].frame;
        Some(geom::Grips {
            id,
            x,
            y,
            w,
            h,
            locked: drawn[i].1,
            room: geom::grip_room(&solids, i),
        })
    }

    /// Where every drawn element puts ink, in z-order, which a new element and the
    /// grips keep clear of. Hidden elements and elements this build cannot read draw
    /// nothing, so they are not among them.
    fn solids(&self) -> Vec<geom::Solid> {
        self.label
            .drawable()
            .map(|e| geom::solid(&self.target, e))
            .collect()
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

    /// Whether the document differs from what was last saved, or from what opened
    /// if it has not been saved. Staged text is not in the document yet, so it does
    /// not count here; [`Doc::unsaved`] counts it.
    pub fn dirty(&self) -> bool {
        self.rev != self.saved_rev
    }

    /// Whether closing now would lose work: an edit since the save, or text typed
    /// into a field and not yet finished.
    pub fn unsaved(&self) -> bool {
        self.dirty() || self.staged.is_some()
    }

    /// Which state the document is in, so a caller that let go of the keyboard -
    /// to show a dialog, say - can tell afterwards whether anything changed.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// The document as it stands has been written to disk.
    ///
    /// Only between commands, with nothing staged and the pointer up: the caller
    /// commits staged text first, and a save waits for the release, so what was
    /// written is the document and not a state part way through an edit.
    pub fn mark_saved(&mut self) {
        debug_assert!(self.staged.is_none(), "saved with text still staged");
        debug_assert!(!self.pressed, "saved with the pointer down");
        self.saved_rev = self.rev;
    }

    /// Give the document a revision no state has had: it has just been edited.
    fn touch(&mut self) {
        self.last_rev += 1;
        self.rev = self.last_rev;
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

    /// What the status line says when this document opens: which way round the
    /// canvas is, what this build cannot draw, and whether the label can be saved
    /// as it stands - a label a later version or a hand edit left unsaveable still
    /// opens, and is refused only at the save, so the operator hears it first.
    pub fn opening_note(&self) -> String {
        let mut note = self.orientation_note();
        match self.undrawable() {
            0 => {}
            1 => note.push_str("; 1 element this build cannot draw is kept as it is"),
            n => note.push_str(&format!(
                "; {n} elements this build cannot draw are kept as they are"
            )),
        }
        if let Err(why) = self.label.writable() {
            note.push_str(&format!("; it cannot be saved as it stands: {why}"));
        }
        note
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
        // Refused before anything else happens, the gesture included: the operator
        // is still holding the element, and after the release this same undo is an
        // ordinary one.
        if matches!(cmd, Cmd::Undo | Cmd::Redo) && self.pressed {
            let undo = cmd == Cmd::Undo;
            let top = if undo {
                self.undo.last()
            } else {
                self.redo.last()
            };
            if top.is_some_and(|(l, _)| drawable_ids(l) != drawable_ids(&self.label)) {
                let word = if undo { "undo" } else { "redo" };
                return Outcome::note(
                    Effect::Nothing,
                    format!(
                        "{word} waits until the element is let go: it would add or take away an \
                         element"
                    ),
                );
            }
        }
        // An undo and a redo drop whatever gesture is still open, as `step_history`
        // always has: the snapshot it holds is older than the one about to be
        // restored. A press closes it instead, as its release would have. A second
        // press arrives only while a first is open - a second finger, or a button the
        // canvas does not filter - and dropping the gesture there left the drag so far
        // in no undo step at all, so one undo took back the edit before it too. Here,
        // first, so the flush below is not held back for a gesture that has ended.
        if matches!(cmd, Cmd::Undo | Cmd::Redo) {
            self.gesture = None;
        }
        if matches!(cmd, Cmd::BeginMove(_) | Cmd::BeginResize { .. }) {
            self.close_gesture();
            self.pressed = true;
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
                self.pressed = false;
                out.also(self.commit_staged())
            }
            Cmd::Create(_) | Cmd::Delete { .. } if self.pressed => {
                let doing = if matches!(cmd, Cmd::Create(_)) {
                    "adding"
                } else {
                    "deleting"
                };
                Outcome::note(
                    Effect::Nothing,
                    format!("{doing} waits until the element is let go"),
                )
            }
            Cmd::Create(kind) => self.create(kind),
            Cmd::Delete { id } => self.delete(id),
            Cmd::BeginResize { id, grip } => self.begin_resize(id, grip),
            Cmd::ResizeTo { id, right, bottom } => self.resize_to(id, right, bottom),
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
        let lens = (
            length_reach(c, Field::FeedLen),
            length_reach(c, Field::HeadLen),
        );
        let mut slots = ins::common_slots(e, reach, lens);
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
        if matches!(field, Field::FeedLen | Field::HeadLen) {
            return self.typed_length(id, field, text);
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
        let before = (self.label.clone(), self.rev);
        if let Some(e) = self.element_mut(id) {
            *e = next;
        }
        self.push_undo(before);
        self.touch();
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

    /// A length that was typed, stepped or set through the accessibility tree: the
    /// resize the keyboard and an assistive technology have.
    ///
    /// Held to [`length_reach`], the interval the field announces and the grips obey.
    /// A number past its top is refused, because no canvas is that long and the
    /// operator meant something else; one below its floor is clamped and said, the
    /// way a typed placement is, because a length of nothing or a frame with nothing
    /// left on the canvas is the one mistake the number itself can make. The origin
    /// never moves: a resize that moved the placement would be two edits in one step.
    fn typed_length(&mut self, id: ElementId, field: Field, text: &str) -> Outcome {
        // Past `i32` a length is still a number, and too long rather than unreadable.
        let typed = match ins::number(field, text) {
            Ok(n) => i128::from(n),
            Err(why) => match text.trim().parse::<i128>() {
                Ok(n) => n,
                Err(_) => return Outcome::note(Effect::Nothing, why),
            },
        };
        let Some(c) = self.common(id) else {
            return Outcome::nothing();
        };
        let who = describe(id, &c.name);
        let (lo, hi) = length_reach(c, field);
        let label = field.label();
        if typed > i128::from(hi) {
            return Outcome::note(
                Effect::Nothing,
                format!(
                    "{label} takes a whole number of dots up to {hi}, and {typed} is more than that"
                ),
            );
        }
        let landed = typed.max(i128::from(lo)) as i32;
        let note = (i128::from(landed) != typed).then(|| {
            if lo == 1 {
                format!("{label} {typed} would leave {who} with nothing to draw, so it is 1")
            } else {
                format!(
                    "{label} {typed} would leave nothing of {who} on the canvas to grab, so it \
                     is {lo}"
                )
            }
        });
        let mut rect = c.frame;
        if field == Field::FeedLen {
            rect.feed_len = landed;
        } else {
            rect.head_len = landed;
        }
        let before = (self.label.clone(), self.rev);
        match self.write_rect(id, rect) {
            Some(effect) => {
                let drawn = self.render_note(&before.0, id);
                self.push_undo(before);
                Outcome {
                    effect,
                    note: both(note, drawn),
                }
            }
            None => Outcome {
                effect: Effect::Nothing,
                note,
            },
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
        self.gesture = Some(Gesture {
            before: self.label.clone(),
            before_rev: self.rev,
            moved: false,
            kind: Held::Move,
        });
        Outcome::nothing()
    }

    /// A placement during an open move. Silent when nothing is open, which is how
    /// a refused press stays refused for the rest of the drag - and when a resize
    /// is, because a grip's pointer is not dragging the frame.
    fn place(&mut self, id: ElementId, feed: i32, head: i32) -> Outcome {
        if !matches!(
            self.gesture,
            Some(Gesture {
                kind: Held::Move,
                ..
            })
        ) {
            return Outcome::nothing();
        }
        match self.write_frame(id, feed, head) {
            Some(effect) => {
                if let Some(g) = &mut self.gesture {
                    g.moved = true;
                }
                Outcome::plain(effect)
            }
            None => Outcome::nothing(),
        }
    }

    /// Pointer down on a grip. Refuses a locked element once, here, for
    /// `begin_move`'s reason.
    fn begin_resize(&mut self, id: ElementId, grip: geom::Grip) -> Outcome {
        let Some(c) = self
            .label
            .drawable()
            .map(Element::common)
            .find(|c| c.id == id)
        else {
            return Outcome::nothing();
        };
        if c.locked {
            let name = describe(c.id, &c.name);
            return Outcome::note(Effect::Nothing, format!("{name} is locked"));
        }
        let from = c.frame;
        self.gesture = Some(Gesture {
            before: self.label.clone(),
            before_rev: self.rev,
            moved: false,
            kind: Held::Resize {
                id,
                grip,
                from,
                clamp: None,
            },
        });
        Outcome::nothing()
    }

    /// The gripped edges at a new place. Resized from the frame at the press rather
    /// than the last event's, so a drag that was clamped and came back is where the
    /// pointer is and not where the clamp left it.
    fn resize_to(&mut self, id: ElementId, right: i32, bottom: i32) -> Outcome {
        let Some(Gesture {
            kind:
                Held::Resize {
                    id: held,
                    grip,
                    from,
                    ..
                },
            ..
        }) = &self.gesture
        else {
            return Outcome::nothing();
        };
        if *held != id {
            return Outcome::nothing();
        }
        let (grip, from) = (*grip, *from);
        let Some(c) = self.common(id) else {
            return Outcome::nothing();
        };
        let who = describe(id, &c.name);
        let (rect, said) = geom::resized(&self.target, from, c.rot, grip, right, bottom, &who);
        let written = self.write_rect(id, rect);
        if let Some(g) = &mut self.gesture {
            if let Held::Resize { clamp, .. } = &mut g.kind {
                *clamp = said;
            }
            g.moved |= written.is_some();
        }
        written.map_or_else(Outcome::nothing, Outcome::plain)
    }

    fn end_move(&mut self, committed: bool) -> Outcome {
        let Some(Gesture {
            before,
            before_rev,
            moved,
            kind,
        }) = self.gesture.take()
        else {
            return Outcome::nothing();
        };
        if !moved {
            // A click that selected and did not drag is not an undo step.
            return Outcome::nothing();
        }
        if !committed {
            self.label = before;
            self.rev = before_rev;
            self.bounds = Bounds::of(&self.label);
            return Outcome::plain(Effect::Everything);
        }
        // A resize is said at the release: what held it, then what the renderer
        // makes of the frame it ended with.
        let note = match kind {
            Held::Move => None,
            Held::Resize { id, clamp, .. } => both(clamp, self.render_note(&before, id)),
        };
        self.push_undo((before, before_rev));
        Outcome {
            effect: Effect::Nothing,
            note,
        }
    }

    /// An open gesture's step, pushed now as its release would have pushed it.
    fn close_gesture(&mut self) {
        if let Some(Gesture {
            before,
            before_rev,
            moved: true,
            ..
        }) = self.gesture.take()
        {
            self.push_undo((before, before_rev));
        }
    }

    /// What the renderer says about element `id` now that was not so in `before`.
    fn render_note(&self, before: &Label, id: ElementId) -> Option<String> {
        let was = before
            .elements
            .iter()
            .filter_map(ElementOrRaw::known)
            .find(|e| e.common().id == id)?;
        let now = self.element(id)?;
        make::render_note(&self.label, &self.target, was, now)
    }

    /// The next id no element has, inside the row id range.
    ///
    /// The canvas carries an id as an `i32`, so an element above `i32::MAX` could
    /// never be selected. Past that range, which only a file that already holds
    /// such an id reaches, the lowest id no element of the label holds is used
    /// instead: the one fallback that can repeat an id from an undone snapshot.
    fn allocate_id(&mut self) -> Option<ElementId> {
        let top = i32::MAX as u32;
        if let Ok(id) = u32::try_from(self.next_id)
            && id <= top
        {
            self.next_id += 1;
            return Some(id);
        }
        let mut held: Vec<u32> = held_ids(&self.label).collect();
        held.sort_unstable();
        held.dedup();
        let mut free = 1u32;
        for id in held {
            if id == free {
                free += 1;
            } else if id > free {
                break;
            }
        }
        (free <= top).then_some(free)
    }

    /// A new element on top of the others, selected. One undo step, and said, with
    /// where it went: the operator may not be looking at the canvas.
    fn create(&mut self, kind: make::Kind) -> Outcome {
        let Some(id) = self.allocate_id() else {
            return Outcome::note(Effect::Nothing, "no element id is free".into());
        };
        let others = self.solids();
        let (element, extra) = make::fresh(
            kind,
            id,
            &self.label,
            &self.target,
            self.direction,
            &self.bounds,
            &others,
        );
        let f = element.common().frame;
        let added = format!(
            "added {} at feed {}, head {}, {} by {} dots",
            ins::heading(&element),
            f.feed,
            f.head,
            f.feed_len,
            f.head_len
        );
        let before = (self.label.clone(), self.rev);
        self.label.elements.push(ElementOrRaw::Known(element));
        debug_assert!(
            {
                let mut ids: Vec<ElementId> = self
                    .label
                    .elements
                    .iter()
                    .filter_map(ElementOrRaw::known)
                    .map(|e| e.common().id)
                    .collect();
                let n = ids.len();
                ids.sort_unstable();
                ids.dedup();
                ids.len() == n
            },
            "element id {id} was given twice"
        );
        self.push_undo(before);
        self.touch();
        self.selected = Some(id);
        Outcome {
            effect: Effect::Everything,
            note: both(Some(added), extra),
        }
    }

    /// Take away a drawn element. Only that entry leaves the list, so an element
    /// this build cannot read keeps its place in the z-order.
    fn delete(&mut self, id: Option<ElementId>) -> Outcome {
        let Some(id) = id else {
            return Outcome::note(Effect::Nothing, "nothing is selected to delete".into());
        };
        let Some((i, e)) = self.label.elements.iter().enumerate().find_map(|(i, e)| {
            e.known()
                .filter(|e| e.common().id == id && !e.common().hidden)
                .map(|e| (i, e))
        }) else {
            return Outcome::nothing();
        };
        if e.common().locked {
            let name = describe(id, &e.common().name);
            return Outcome::note(Effect::Nothing, format!("{name} is locked"));
        }
        let said = format!("deleted {}; undo brings it back", ins::heading(e));
        let before = (self.label.clone(), self.rev);
        self.label.elements.remove(i);
        self.push_undo(before);
        self.touch();
        if self.selected == Some(id) {
            self.selected = None;
        }
        Outcome::note(Effect::Everything, said)
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
        let before = (self.label.clone(), self.rev);
        match self.write_frame(id, feed, head) {
            Some(effect) => {
                self.push_undo(before);
                Outcome::plain(effect)
            }
            None => Outcome::nothing(),
        }
    }

    /// The only place a frame is written: its placement and its lengths together.
    ///
    /// `None` when nothing changed, so a move event that lands on the dot the
    /// element is already on costs one comparison and no redraw - which is most
    /// of them, at pointer speed. The placement is held by `geom::keep_reachable`
    /// against the frame as it will be drawn with the lengths it is given, so a new
    /// length and the origin it grows from are judged as one frame.
    fn write_rect(&mut self, id: ElementId, rect: RectDots) -> Option<Effect> {
        let drawn = {
            let c = self.common(id)?;
            if c.locked {
                return None;
            }
            geom::as_drawn(rect, c.rot)
        };
        let (feed, head) = geom::keep_reachable(&self.bounds, drawn, rect.feed, rect.head);
        let rect = RectDots { feed, head, ..rect };
        let c = self.common_mut(id)?;
        if c.frame == rect {
            return None;
        }
        c.frame = rect;
        self.touch();
        Some(Effect::Row(id))
    }

    /// A placement: [`Doc::write_rect`] with the lengths the element already has.
    fn write_frame(&mut self, id: ElementId, feed: i32, head: i32) -> Option<Effect> {
        let now = self.common(id)?.frame;
        self.write_rect(id, RectDots { feed, head, ..now })
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
        let Some((other, other_rev)) = from.pop() else {
            return Outcome::note(
                Effect::Nothing,
                if undo {
                    "nothing to undo".into()
                } else {
                    "nothing to redo".into()
                },
            );
        };
        to.push((std::mem::replace(&mut self.label, other), self.rev));
        self.rev = other_rev;
        self.bounds = Bounds::of(&self.label);
        // The selected element may be one that does not exist in the restored
        // document: an add undone, or a delete redone.
        if self.selected.is_some_and(|id| self.shown_one(id).is_none()) {
            self.selected = None;
        }
        // An element that came or went is said, because the operator may be looking
        // at the pane or listening rather than watching the canvas. The label left
        // behind is the one just pushed.
        let word = if undo { "undo" } else { "redo" };
        let left = if undo { &self.redo } else { &self.undo }
            .last()
            .map(|(l, _)| l);
        let was = left.map(drawable_ids).unwrap_or_default();
        let now = drawable_ids(&self.label);
        let said = |verb: &str, ids: Vec<&ElementId>, label: &Label| match ids.as_slice() {
            [] => None,
            [id] => label
                .drawable()
                .find(|e| e.common().id == **id)
                .map(|e| format!("{word} {verb} {}", ins::heading(e))),
            many => Some(format!("{word} {verb} {} elements", many.len())),
        };
        let back = said("brought back", now.difference(&was).collect(), &self.label);
        let gone = left.and_then(|l| said("took away", was.difference(&now).collect(), l));
        Outcome {
            effect: Effect::Everything,
            note: both(back, gone),
        }
    }

    /// Push the state an edit started from, with its revision. A snapshot trimmed
    /// off the bottom takes its revision with it, so the state it was can never be
    /// reached again - and if it was the saved one, the document can never read
    /// clean again by accident.
    fn push_undo(&mut self, before: (Label, u64)) {
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

/// The ids of the elements a label draws, which are the rows the canvas holds.
fn drawable_ids(label: &Label) -> std::collections::BTreeSet<ElementId> {
    label.drawable().map(|e| e.common().id).collect()
}

/// Every id an element of this label holds: a known element's, and an unreadable
/// one's when its `id` is a whole number an id could be - a later version's element
/// is still that version's element, and a new one must not take its id.
fn held_ids(label: &Label) -> impl Iterator<Item = ElementId> + '_ {
    label.elements.iter().filter_map(|e| match e {
        ElementOrRaw::Known(e) => Some(e.common().id),
        ElementOrRaw::Raw(v) => {
            let id = v.get("id")?;
            let whole = id.as_u64().or_else(|| {
                id.as_f64()
                    .filter(|f| f.is_finite() && f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })?;
            ElementId::try_from(whole).ok()
        }
    })
}

/// The interval a length field may be set to: [`geom::len_reach`] of the origin on
/// the axis that length is drawn along. A quarter turn draws the stored feed length
/// down the head axis, from the head origin, and the head length the other way.
fn length_reach(c: &relabel_render::document::Common, field: Field) -> (i32, i32) {
    let quarter = matches!(c.rot.quantised(), Rot::D90 | Rot::D270);
    let along_feed = (field == Field::FeedLen) != quarter;
    geom::len_reach(if along_feed {
        c.frame.feed
    } else {
        c.frame.head
    })
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
            Cmd::Create(make::Kind::Rect),
            Cmd::Delete { id: Some(2) },
            Cmd::Delete { id: None },
            Cmd::BeginResize {
                id: 1,
                grip: geom::Grip::East,
            },
            Cmd::ResizeTo {
                id: 1,
                right: 60,
                bottom: 0,
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
            ins::SLOTS - 10,
            "a rectangle has ten rows"
        );
        // And the announced interval is `reach` on this element's drawn frame.
        let feed = slots.iter().find(|s| s.field == Field::Feed).expect("feed");
        assert_eq!((feed.min, feed.max), (-39.0, 319.0));
    }

    fn slot_of(doc: &Doc, field: Field) -> ins::Slot {
        doc.inspected()
            .expect("something is selected")
            .into_iter()
            .find(|s| s.field == field)
            .expect("the field is in the pane")
    }

    fn commit(doc: &mut Doc, field: Field, text: &str) -> Outcome {
        stage(doc, field, text);
        doc.apply(Cmd::CommitField)
    }

    /// Rectangle 1 of `label()` moved to `f` and turned to `rot`.
    fn doc_with_first(f: RectDots, rot: Rot) -> Doc {
        let mut l = label();
        let mut first = rect(1, f, false);
        if let ElementOrRaw::Known(e) = &mut first {
            ins::common_of(e).rot = rot;
        }
        l.elements[0] = first;
        Doc::open(l, PrintDirection::Left).expect("a printable label")
    }

    #[test]
    fn a_typed_length_is_one_step_and_clamps_out_loud() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        let out = commit(&mut doc, Field::FeedLen, "60");
        assert_eq!(out, Outcome::plain(Effect::Row(1)));
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 60, 24));
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).feed_len, 40, "one undo step");

        let out = commit(&mut doc, Field::FeedLen, "0");
        assert_eq!(frame_of(&doc, 1).feed_len, 1);
        let note = out.note.expect("the clamp is said");
        assert!(note.ends_with("so it is 1"), "{note}");
        assert!(note.contains("nothing to draw"), "{note}");

        for typed in ["70000", "99999999999"] {
            let out = commit(&mut doc, Field::FeedLen, typed);
            assert_eq!(
                out.note.as_deref(),
                Some(
                    format!(
                        "feed length takes a whole number of dots up to 65535, and {typed} is \
                         more than that"
                    )
                    .as_str()
                )
            );
            assert_eq!(out.effect, Effect::Nothing);
            assert_eq!(frame_of(&doc, 1).feed_len, 1, "{typed}");
        }
        let out = commit(&mut doc, Field::HeadLen, "wide");
        assert!(
            out.note
                .as_deref()
                .is_some_and(|n| n.contains("whole number")),
            "{out:?}"
        );

        let mut doc = doc_with_first(frame(-30, 8, 40, 24), Rot::D0);
        doc.apply(Cmd::Select(Some(1)));
        let out = commit(&mut doc, Field::FeedLen, "5");
        assert_eq!(frame_of(&doc, 1), frame(-30, 8, 31, 24), "the origin stays");
        let note = out.note.expect("the clamp is said");
        assert!(note.ends_with("so it is 31"), "{note}");
        assert!(note.contains("on the canvas to grab"), "{note}");
    }

    #[test]
    fn a_length_that_clamps_onto_the_length_it_has_still_says_so_and_is_no_step() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        commit(&mut doc, Field::HeadLen, "1");
        let out = commit(&mut doc, Field::HeadLen, "-4");
        assert_eq!(out.effect, Effect::Nothing);
        assert!(
            out.note
                .as_deref()
                .is_some_and(|n| n.ends_with("so it is 1")),
            "{out:?}"
        );
        assert_eq!(doc.undo.len(), 1);
    }

    #[test]
    fn a_length_step_holds_at_one_dot() {
        let mut doc = doc_with_first(frame(8, 8, 40, 1), Rot::D0);
        doc.apply(Cmd::Select(Some(1)));
        let out = doc.apply(Cmd::StepField {
            field: Field::HeadLen,
            by: -1,
        });
        assert_eq!(out, Outcome::nothing());
        assert_eq!(frame_of(&doc, 1).head_len, 1);
        assert_eq!(
            doc.apply(Cmd::Undo).note.as_deref(),
            Some("nothing to undo")
        );
    }

    /// A reader is told the interval the typed length is held to, on every axis a
    /// quarter turn can put the length along.
    #[test]
    fn a_length_field_announces_the_interval_it_enforces() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        let s = slot_of(&doc, Field::FeedLen);
        assert_eq!((s.min, s.max, s.step), (1.0, 65535.0, 1.0));
        assert_eq!((s.shape, s.value.as_str()), (ins::Shape::Number, "40"));

        let mut doc = doc_with_first(frame(-30, 8, 40, 24), Rot::D0);
        doc.apply(Cmd::Select(Some(1)));
        assert_eq!(slot_of(&doc, Field::FeedLen).min, 31.0);
        assert_eq!(slot_of(&doc, Field::HeadLen).min, 1.0);

        let mut doc = doc_with_first(frame(8, -10, 40, 24), Rot::D90);
        doc.apply(Cmd::Select(Some(1)));
        for (field, min) in [(Field::FeedLen, 11), (Field::HeadLen, 1)] {
            assert_eq!(slot_of(&doc, field).min, min as f32, "{field:?}");
            // And one under the announced floor lands on it.
            commit(&mut doc, field, &(min - 1).to_string());
            let f = frame_of(&doc, 1);
            let got = if field == Field::FeedLen {
                f.feed_len
            } else {
                f.head_len
            };
            assert_eq!(got, min, "{field:?}");
            assert_eq!((f.feed, f.head), (8, -10), "the origin stays");
        }
    }

    #[test]
    fn a_locked_element_refuses_a_typed_length() {
        let mut l = label();
        l.elements[0] = rect(1, frame(8, 8, 40, 24), true);
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        doc.apply(Cmd::Select(Some(1)));
        let out = commit(&mut doc, Field::FeedLen, "60");
        assert_eq!(out.note.as_deref(), Some("\u{2018}box 1\u{2019} is locked"));
        assert_eq!(frame_of(&doc, 1).feed_len, 40);
    }

    fn ids_of(doc: &Doc) -> Vec<ElementId> {
        doc.label()
            .elements
            .iter()
            .filter_map(ElementOrRaw::known)
            .map(|e| e.common().id)
            .collect()
    }

    fn note_of(out: &Outcome) -> &str {
        out.note.as_deref().unwrap_or("")
    }

    fn undo_note(doc: &mut Doc) -> Option<String> {
        doc.apply(Cmd::Undo).note
    }

    fn raw(json: &str) -> ElementOrRaw {
        ElementOrRaw::Raw(serde_json::from_str(json).expect("json"))
    }

    fn create(doc: &mut Doc) -> Outcome {
        doc.apply(Cmd::Create(make::Kind::Rect))
    }

    #[test]
    fn create_appends_on_top_selects_and_is_one_undo_step() {
        let mut doc = doc();
        let out = create(&mut doc);
        assert_eq!(doc.label().elements.len(), 3);
        let Some(Element::Rect(r)) = doc.label().elements.last().and_then(ElementOrRaw::known)
        else {
            panic!("a rectangle on top");
        };
        assert_eq!((r.common.id, r.stroke_dots), (3, 2));
        assert_eq!(doc.selected(), Some(3));
        assert_eq!(out.effect, Effect::Everything);
        assert!(
            note_of(&out)
                .starts_with("added Rectangle element 3 at feed 56, head 8, 40 by 24 dots"),
            "{out:?}"
        );
        assert!(doc.dirty());

        let out = doc.apply(Cmd::Undo);
        assert_eq!(doc.label().elements.len(), 2);
        assert_eq!(doc.selected(), None);
        assert_eq!(note_of(&out), "undo took away Rectangle element 3");
        assert_eq!(undo_note(&mut doc).as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn a_second_rectangle_lands_clear_of_the_first() {
        let mut l = label();
        l.elements.push(rect(3, frame(240, 40, 40, 24), false));
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        create(&mut doc);
        assert_eq!(frame_of(&doc, 4), frame(56, 8, 40, 24));
        create(&mut doc);
        assert_eq!(frame_of(&doc, 5), frame(168, 8, 40, 24));
    }

    #[test]
    fn ids_are_never_reused_in_a_session() {
        let mut doc = doc();
        create(&mut doc);
        assert_eq!(doc.selected(), Some(3));
        doc.apply(Cmd::Undo);
        create(&mut doc);
        assert_eq!(doc.selected(), Some(4));
        assert_eq!(ids_of(&doc), [1, 2, 4]);
    }

    #[test]
    fn new_ids_step_past_elements_this_build_cannot_read() {
        for (json, want) in [
            (r#"{"type":"hologram","id":9}"#, 10),
            (r#"{"type":"hologram","id":3.0}"#, 4),
            (r#"{"type":"hologram","id":"99"}"#, 2),
            (r#"{"type":"hologram","id":4294967296}"#, 2),
        ] {
            let mut l = label();
            l.elements.truncate(1);
            l.elements.insert(0, raw(json));
            let unread = l.elements[0].clone();
            let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
            create(&mut doc);
            assert_eq!(doc.selected(), Some(want), "{json}");
            assert_eq!(doc.label().elements[0], unread, "{json}");
            assert_eq!(doc.label().elements.len(), 3, "{json}");
        }
    }

    #[test]
    fn new_ids_stay_inside_the_signed_row_range() {
        let top = i32::MAX as u32;
        let mut l = label();
        l.elements[1] = rect(top, frame(120, 8, 40, 24), false);
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        create(&mut doc);
        assert_eq!(doc.selected(), Some(2));
        create(&mut doc);
        assert_eq!(doc.selected(), Some(3));

        let mut l = label();
        l.elements.truncate(1);
        l.elements[0] = rect(u32::MAX, frame(8, 8, 40, 24), false);
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        create(&mut doc);
        assert_eq!(doc.selected(), Some(1));
        create(&mut doc);
        assert_eq!(doc.selected(), Some(2));
        assert!(
            ids_of(&doc)
                .iter()
                .filter(|&&id| id != u32::MAX)
                .all(|&id| id <= top)
        );
    }

    #[test]
    fn a_created_label_saves_and_loads_back() {
        let mut doc = doc();
        for kind in make::Kind::ALL {
            doc.apply(Cmd::Create(kind));
        }
        assert_eq!(doc.label().elements.len(), 8);
        let saved = doc.label().to_json().expect("writable");
        assert_eq!(&Label::from_json(&saved).expect("readable"), doc.label());
    }

    #[test]
    fn create_and_delete_wait_while_a_pointer_is_down() {
        let mut doc = doc();
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 20, y: 8 });
        let out = create(&mut doc);
        assert_eq!(doc.label().elements.len(), 2);
        assert!(
            note_of(&out).contains("waits until the element is let go"),
            "{out:?}"
        );
        let out = doc.apply(Cmd::Delete { id: Some(2) });
        assert_eq!(ids_of(&doc), [1, 2]);
        assert!(
            note_of(&out).contains("waits until the element is let go"),
            "{out:?}"
        );
        doc.apply(Cmd::EndMove { committed: false });
        assert_eq!(doc.label(), &label());
        assert_eq!(undo_note(&mut doc).as_deref(), Some("nothing to undo"));
        assert!(!doc.pressed());
    }

    /// A press on a locked element opens no gesture, and the pointer is still down.
    #[test]
    fn a_press_that_opened_no_gesture_still_holds_back_an_add() {
        let mut l = label();
        l.elements[0] = rect(1, frame(8, 8, 40, 24), true);
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        doc.apply(Cmd::BeginMove(1));
        assert!(doc.pressed());
        create(&mut doc);
        assert_eq!(doc.label().elements.len(), 2);
        doc.apply(Cmd::EndMove { committed: true });
        create(&mut doc);
        assert_eq!(doc.label().elements.len(), 3);
    }

    #[test]
    fn create_lands_staged_text_on_the_element_it_was_typed_into() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Name, "kept");
        create(&mut doc);
        assert_eq!(name_of(&doc, 1), "kept");
        assert_eq!(name_of(&doc, 3), "");
        assert_eq!(doc.staged(), None);
        doc.apply(Cmd::Undo);
        assert!(doc.common(3).is_none());
        assert_eq!(name_of(&doc, 1), "kept");
        doc.apply(Cmd::Undo);
        assert_eq!(name_of(&doc, 1), "box 1");
    }

    #[test]
    fn delete_removes_the_element_and_undo_puts_it_back() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        let out = doc.apply(Cmd::Delete { id: Some(1) });
        assert_eq!(ids_of(&doc), [2]);
        assert_eq!(doc.selected(), None);
        assert_eq!(out.effect, Effect::Everything);
        assert_eq!(
            note_of(&out),
            "deleted Rectangle \u{2018}box 1\u{2019}; undo brings it back"
        );
        let out = doc.apply(Cmd::Undo);
        assert_eq!(doc.label(), &label());
        assert_eq!(
            note_of(&out),
            "undo brought back Rectangle \u{2018}box 1\u{2019}"
        );
    }

    #[test]
    fn delete_leaves_unknown_elements_where_they_were() {
        let mut l = label();
        let (a, b) = (
            raw(r#"{"type":"hologram","id":7,"shimmer":1}"#),
            raw(r#"{"type":"hologram","id":8,"shimmer":2}"#),
        );
        l.elements = vec![
            rect(1, frame(8, 8, 40, 24), false),
            a.clone(),
            rect(2, frame(120, 8, 40, 24), false),
            b.clone(),
        ];
        let mut doc = Doc::open(l.clone(), PrintDirection::Left).expect("a printable label");
        doc.apply(Cmd::Delete { id: Some(1) });
        assert_eq!(
            doc.label().elements,
            vec![a, rect(2, frame(120, 8, 40, 24), false), b]
        );
        doc.apply(Cmd::Undo);
        assert_eq!(doc.label(), &l);
    }

    #[test]
    fn delete_refuses_a_locked_element_out_loud() {
        let mut l = label();
        l.elements[0] = rect(1, frame(8, 8, 40, 24), true);
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        let out = doc.apply(Cmd::Delete { id: Some(1) });
        assert_eq!(ids_of(&doc), [1, 2]);
        assert_eq!(note_of(&out), "\u{2018}box 1\u{2019} is locked");
        assert_eq!(undo_note(&mut doc).as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn deleting_another_element_keeps_the_selection() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        doc.apply(Cmd::Delete { id: Some(2) });
        assert_eq!(doc.selected(), Some(1));
        // And an id that is not drawn is nothing to delete.
        assert_eq!(doc.apply(Cmd::Delete { id: Some(9) }), Outcome::nothing());
    }

    #[test]
    fn delete_commits_staged_text_first() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Name, "x");
        doc.apply(Cmd::Delete { id: Some(1) });
        doc.apply(Cmd::Undo);
        assert_eq!(name_of(&doc, 1), "x");
        doc.apply(Cmd::Undo);
        assert_eq!(name_of(&doc, 1), "box 1");
    }

    #[test]
    fn delete_of_nothing_says_so() {
        let mut doc = doc();
        let out = doc.apply(Cmd::Delete { id: None });
        assert_eq!(note_of(&out), "nothing is selected to delete");
        assert_eq!(out.effect, Effect::Nothing);
    }

    #[test]
    fn an_undo_that_would_take_away_a_held_element_waits() {
        let mut doc = doc();
        create(&mut doc);
        doc.apply(Cmd::BeginMove(3));
        doc.apply(Cmd::MoveTo { id: 3, x: 60, y: 8 });
        let out = doc.apply(Cmd::Undo);
        assert_eq!(ids_of(&doc), [1, 2, 3]);
        assert!(note_of(&out).contains("let go"), "{out:?}");
        assert_eq!(frame_of(&doc, 3).feed, 60);
        assert!(doc.pressed());
        doc.apply(Cmd::EndMove { committed: true });
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 3).feed, 56);
        doc.apply(Cmd::Undo);
        assert_eq!(ids_of(&doc), [1, 2]);
    }

    #[test]
    fn a_redo_that_would_add_an_element_waits_and_a_plain_undo_does_not() {
        let mut doc = doc();
        create(&mut doc);
        doc.apply(Cmd::Undo);
        doc.apply(Cmd::BeginMove(1));
        let out = doc.apply(Cmd::Redo);
        assert_eq!(ids_of(&doc), [1, 2]);
        assert!(note_of(&out).contains("let go"), "{out:?}");
        doc.apply(Cmd::EndMove { committed: false });
        doc.apply(Cmd::Redo);
        assert_eq!(ids_of(&doc), [1, 2, 3]);

        let mut doc = self::doc();
        doc.apply(Cmd::Nudge {
            id: 1,
            dx: 1,
            dy: 0,
            coarse: false,
        });
        doc.apply(Cmd::BeginMove(2));
        doc.apply(Cmd::MoveTo {
            id: 2,
            x: 130,
            y: 8,
        });
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).feed, 8);
        assert_eq!(frame_of(&doc, 2).feed, 120);
    }

    #[test]
    fn a_structural_undo_and_redo_say_what_came_and_went() {
        let mut doc = doc();
        create(&mut doc);
        assert_eq!(
            undo_note(&mut doc).as_deref(),
            Some("undo took away Rectangle element 3")
        );
        assert_eq!(
            doc.apply(Cmd::Redo).note.as_deref(),
            Some("redo brought back Rectangle element 3")
        );
        doc.apply(Cmd::Select(Some(3)));
        doc.apply(Cmd::Delete { id: Some(3) });
        assert_eq!(
            undo_note(&mut doc).as_deref(),
            Some("undo brought back Rectangle element 3")
        );
        // A plain step says nothing more than it did.
        doc.apply(Cmd::Nudge {
            id: 1,
            dx: 1,
            dy: 0,
            coarse: false,
        });
        assert_eq!(undo_note(&mut doc), None);
    }

    #[test]
    fn an_undo_that_brings_back_several_elements_counts_them() {
        let mut l = label();
        l.elements.clear();
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        let mut restored = label();
        restored.elements.truncate(2);
        // A state the document was never in, so a revision no state has had.
        doc.undo.push((restored, u64::MAX));
        assert_eq!(
            undo_note(&mut doc).as_deref(),
            Some("undo brought back 2 elements")
        );
        assert_eq!(
            doc.apply(Cmd::Redo).note.as_deref(),
            Some("redo took away 2 elements")
        );
    }

    fn resize(doc: &mut Doc, grip: geom::Grip, ends: &[(i32, i32)], committed: bool) -> Outcome {
        doc.apply(Cmd::BeginResize { id: 1, grip });
        for &(right, bottom) in ends {
            doc.apply(Cmd::ResizeTo {
                id: 1,
                right,
                bottom,
            });
        }
        doc.apply(Cmd::EndMove { committed })
    }

    #[test]
    fn a_resize_drag_is_one_undo_step() {
        let mut doc = doc();
        let out = resize(&mut doc, geom::Grip::East, &[(58, 0), (60, 0)], true);
        assert_eq!(out, Outcome::nothing());
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 52, 24));
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).feed_len, 40);
        assert_eq!(undo_note(&mut doc).as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn a_second_press_keeps_the_drag_before_it_as_its_own_step() {
        let mut doc = doc();
        doc.apply(Cmd::Nudge {
            id: 2,
            dx: 1,
            dy: 0,
            coarse: false,
        });
        doc.apply(Cmd::BeginResize {
            id: 1,
            grip: geom::Grip::East,
        });
        doc.apply(Cmd::ResizeTo {
            id: 1,
            right: 53,
            bottom: 0,
        });
        // A press that arrives while one is open, from a second finger or a button
        // the canvas did not filter: the drag so far is a step, not lost in the next.
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::EndMove { committed: true });
        assert_eq!(frame_of(&doc, 1).feed_len, 45);
        doc.apply(Cmd::Undo);
        assert_eq!(frame_of(&doc, 1).feed_len, 40);
        assert_eq!(frame_of(&doc, 2).feed, 121, "the nudge is still there");
    }

    #[test]
    fn a_cancelled_resize_rolls_back() {
        let mut doc = doc();
        let out = resize(&mut doc, geom::Grip::SouthEast, &[(70, 50)], false);
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 40, 24));
        assert_eq!(out.effect, Effect::Everything);
        assert_eq!(undo_note(&mut doc).as_deref(), Some("nothing to undo"));
        assert!(!doc.pressed());
    }

    #[test]
    fn a_resize_past_the_opposite_edge_stops_at_one_dot_and_says_so() {
        let mut doc = doc();
        let out = resize(&mut doc, geom::Grip::East, &[(-100, 0)], true);
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 1, 24));
        assert!(
            note_of(&out).contains("stops at a feed length of 1 dot"),
            "{out:?}"
        );
        // Said once, for the last event, and not for a clamp the drag came back from.
        let mut doc = self::doc();
        let out = resize(&mut doc, geom::Grip::East, &[(-100, 0), (30, 0)], true);
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 22, 24));
        assert_eq!(out.note, None);
    }

    #[test]
    fn a_resize_keeps_a_dot_on_the_canvas_without_moving_the_element() {
        let mut doc = doc_with_first(frame(-30, 8, 40, 24), Rot::D0);
        let out = resize(&mut doc, geom::Grip::East, &[(-25, 0)], true);
        assert_eq!(frame_of(&doc, 1), frame(-30, 8, 31, 24));
        assert!(note_of(&out).contains("stays on the canvas"), "{out:?}");
    }

    #[test]
    fn a_resize_under_a_quarter_turn_writes_the_stored_length() {
        let mut doc = doc_with_first(frame(8, 8, 40, 24), Rot::D90);
        resize(&mut doc, geom::Grip::East, &[(42, 0)], true);
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 40, 34));
    }

    #[test]
    fn resize_refuses_a_locked_element_and_ignores_the_other_gesture() {
        let mut l = label();
        l.elements[0] = rect(1, frame(8, 8, 40, 24), true);
        let mut doc = Doc::open(l.clone(), PrintDirection::Left).expect("a printable label");
        let out = doc.apply(Cmd::BeginResize {
            id: 1,
            grip: geom::Grip::East,
        });
        assert_eq!(note_of(&out), "\u{2018}box 1\u{2019} is locked");
        let out = doc.apply(Cmd::ResizeTo {
            id: 1,
            right: 90,
            bottom: 0,
        });
        assert_eq!(out, Outcome::nothing());
        assert_eq!(doc.label(), &l);

        let mut doc = self::doc();
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::ResizeTo {
            id: 1,
            right: 90,
            bottom: 0,
        });
        assert_eq!(frame_of(&doc, 1).feed_len, 40);
        doc.apply(Cmd::EndMove { committed: true });

        doc.apply(Cmd::BeginResize {
            id: 1,
            grip: geom::Grip::East,
        });
        doc.apply(Cmd::MoveTo {
            id: 1,
            x: 50,
            y: 50,
        });
        assert_eq!(frame_of(&doc, 1).feed, 8);
        // Nor does a resize of one element write another.
        doc.apply(Cmd::ResizeTo {
            id: 2,
            right: 200,
            bottom: 0,
        });
        assert_eq!(frame_of(&doc, 2).feed_len, 40);
    }

    #[test]
    fn a_committed_resize_says_what_the_renderer_makes_of_the_frame() {
        let json = r#"{"format": "relabel-label", "version": 1,
            "media": {"feed_tenths_mm": 400, "head_tenths_mm": 120},
            "canvas": {"feed": 320, "head": 96, "dpmm": {"num": 8, "den": 1}},
            "elements": [{"type": "qr", "id": 1, "data": "relabel", "module_dots": 3,
                "frame": {"feed": 8, "head": 4, "feed_len": 87, "head_len": 87}}]}"#;
        let l = Label::from_json(json).expect("a label");
        let mut doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        let out = resize(&mut doc, geom::Grip::East, &[(90, 0)], true);
        let note = out.note.expect("said");
        assert!(
            note.starts_with("QR code element 1 will not be drawn"),
            "{note}"
        );
    }

    #[test]
    fn grips_are_the_selected_drawn_box_and_the_room_beyond_it() {
        let mut doc = doc_with_first(frame(8, 8, 40, 24), Rot::D90);
        assert_eq!(doc.grips(), None);
        doc.apply(Cmd::Select(Some(1)));
        assert_eq!(
            doc.grips(),
            Some(geom::Grips {
                id: 1,
                x: 8,
                y: 8,
                w: 24,
                h: 40,
                locked: false,
                room: [64, 64, 64],
            })
        );
        doc.apply(Cmd::Select(Some(9)));
        assert_eq!(doc.grips(), None);
    }

    fn kitchen_sink() -> Doc {
        let l = Label::from_json(include_str!(
            "../../relabel-render/tests/corpus/v1/kitchen-sink.relabel"
        ))
        .expect("a corpus label");
        Doc::open(l, PrintDirection::Left).expect("a printable label")
    }

    /// A 320 by 96 label holding one element, written as JSON.
    fn label_holding(element: &str) -> Doc {
        let json = format!(
            r#"{{"format": "relabel-label", "version": 1,
                "media": {{"feed_tenths_mm": 400, "head_tenths_mm": 120}},
                "canvas": {{"feed": 320, "head": 96, "dpmm": {{"num": 8, "den": 1}}}},
                "elements": [{element}]}}"#
        );
        let label = Label::from_json(&json).expect("a label");
        Doc::open(label, PrintDirection::Left).expect("a printable label")
    }

    const WHOLE: &str = r#""frame": {"feed": 0, "head": 0, "feed_len": 320, "head_len": 96}"#;

    #[test]
    fn grips_inside_a_border_have_room_up_to_its_stroke() {
        let mut doc = kitchen_sink();
        for (id, room) in [
            (3, [24, 8, 8]),
            (2, [6, 7, 6]),
            (4, [24, 6, 6]),
            (7, [0, 0, 0]),
            (1, [64, 64, 64]),
        ] {
            doc.apply(Cmd::Select(Some(id)));
            assert_eq!(doc.grips().expect("selected").room, room, "element {id}");
        }
    }

    #[test]
    fn a_text_a_qr_and_a_line_added_inside_a_border_land_clear_of_it_and_of_each_other() {
        let mut doc = label_holding(&format!(
            r#"{{"type": "rect", "id": 1, {WHOLE}, "stroke_dots": 2}}"#
        ));
        for (kind, id, f, said) in [
            (
                make::Kind::Text,
                2,
                frame(16, 16, 96, 24),
                "added Text element 2 at feed 16, head 16, 96 by 24 dots",
            ),
            (
                make::Kind::Qr,
                3,
                frame(120, 4, 87, 87),
                "added QR code element 3 at feed 120, head 4, 87 by 87 dots",
            ),
            (
                make::Kind::Line,
                4,
                frame(16, 48, 96, 2),
                "added Line element 4 at feed 16, head 48, 96 by 2 dots",
            ),
        ] {
            let out = doc.apply(Cmd::Create(kind));
            assert_eq!(frame_of(&doc, id), f, "{kind:?}");
            assert_eq!(out.note.as_deref(), Some(said), "{kind:?}");
        }
    }

    #[test]
    fn a_new_element_on_a_filled_rectangle_or_artwork_covering_the_label_still_says_it_overlaps() {
        for (element, kind, f) in [
            (
                format!(r#"{{"type": "rect", "id": 1, {WHOLE}}}"#),
                make::Kind::Text,
                frame(8, 8, 96, 24),
            ),
            (
                format!(r#"{{"type": "image", "id": 1, {WHOLE}, "path": "logo.png"}}"#),
                make::Kind::Rect,
                frame(8, 8, 40, 24),
            ),
        ] {
            let mut doc = label_holding(&element);
            let out = doc.apply(Cmd::Create(kind));
            assert_eq!(frame_of(&doc, 2), f, "{element}");
            assert!(
                note_of(&out).contains("it overlaps another element"),
                "{element}: {out:?}"
            );
        }
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

    fn nudge(doc: &mut Doc) {
        doc.apply(Cmd::Nudge {
            id: 1,
            dx: 1,
            dy: 0,
            coarse: false,
        });
    }

    /// A drag that was cancelled put everything back, so nothing is left to save.
    #[test]
    fn a_cancelled_drag_leaves_the_document_clean() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        doc.apply(Cmd::BeginMove(1));
        doc.apply(Cmd::MoveTo { id: 1, x: 40, y: 8 });
        assert!(doc.dirty(), "mid-drag the label has moved");
        doc.apply(Cmd::EndMove { committed: false });
        assert!(!doc.dirty());
        assert!(!doc.unsaved());
        assert_eq!(doc.revision(), 0);
        assert_eq!(frame_of(&doc, 1), frame(8, 8, 40, 24));
    }

    /// Dirty means "not what was saved": an undo back to the saved state is clean,
    /// and a redo away from it is not. The state before the save was never saved.
    #[test]
    fn undo_back_to_the_saved_revision_is_clean_and_redo_is_not() {
        let mut doc = doc();
        nudge(&mut doc);
        doc.mark_saved();
        assert!(!doc.dirty());
        nudge(&mut doc);
        assert!(doc.dirty());
        doc.apply(Cmd::Undo);
        assert!(!doc.dirty());
        doc.apply(Cmd::Redo);
        assert!(doc.dirty());
        doc.apply(Cmd::Undo);
        doc.apply(Cmd::Undo);
        assert!(doc.dirty());
    }

    /// Text typed into a field and not finished is not in the document yet, so the
    /// document is not dirty; it is still work a close would lose.
    #[test]
    fn staged_text_is_unsaved_but_not_yet_dirty() {
        let mut doc = doc();
        doc.apply(Cmd::Select(Some(1)));
        stage(&mut doc, Field::Feed, "37");
        assert!(!doc.dirty() && doc.unsaved());
        doc.apply(Cmd::CommitField);
        assert!(doc.dirty() && doc.unsaved());
        doc.mark_saved();
        assert!(!doc.unsaved());
    }

    /// Each state the document has been in keeps its own revision, through undo and
    /// redo, and a new edit is never given one an older state had.
    #[test]
    fn revisions_travel_with_undo_and_redo() {
        let mut doc = doc();
        let r0 = doc.revision();
        nudge(&mut doc);
        let r1 = doc.revision();
        assert_ne!(r1, r0);
        doc.apply(Cmd::Undo);
        assert_eq!(doc.revision(), r0);
        doc.apply(Cmd::Redo);
        assert_eq!(doc.revision(), r1);
        doc.apply(Cmd::Undo);
        nudge(&mut doc);
        assert!(![r0, r1].contains(&doc.revision()), "{}", doc.revision());
    }

    /// The snapshot trimmed off the bottom of the stack takes its revision with it,
    /// so no state reached afterwards can be mistaken for the saved one.
    #[test]
    fn a_saved_state_trimmed_off_the_undo_stack_never_reads_clean_again() {
        let mut doc = doc();
        doc.mark_saved();
        for _ in 0..=UNDO_DEPTH {
            nudge(&mut doc);
        }
        for _ in 0..UNDO_DEPTH {
            doc.apply(Cmd::Undo);
        }
        assert!(doc.dirty());
    }

    /// What the status line says when a label opens: which way round it is, what
    /// this build cannot draw, and whether it can be saved as it stands.
    #[test]
    fn the_opening_note_counts_what_cannot_be_drawn_or_saved() {
        let mut l = label();
        l.elements.push(ElementOrRaw::Raw(
            serde_json::from_str(r#"{"type":"hologram","id":9}"#).expect("json"),
        ));
        let doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        assert_eq!(
            doc.opening_note(),
            doc.orientation_note() + "; 1 element this build cannot draw is kept as it is"
        );

        let mut l = label();
        for id in [8, 9] {
            l.elements.push(ElementOrRaw::Raw(
                serde_json::json!({ "type": "hologram", "id": id }),
            ));
        }
        let doc = Doc::open(l, PrintDirection::Left).expect("a printable label");
        assert!(
            doc.opening_note()
                .ends_with("; 2 elements this build cannot draw are kept as they are"),
            "{}",
            doc.opening_note()
        );

        let zero = Label::from_json(
            r#"{"format": "relabel-label", "version": 1,
                "media": {"feed_tenths_mm": 400, "head_tenths_mm": 120},
                "canvas": {"feed": 320, "head": 96, "dpmm": {"num": 8, "den": 1}},
                "elements": [{"type": "text", "id": 3, "text": "x", "size": 0.0,
                  "frame": {"feed": 8, "head": 8, "feed_len": 40, "head_len": 24}}]}"#,
        )
        .expect("a label a save would refuse still loads");
        let doc = Doc::open(zero, PrintDirection::Left).expect("a printable label");
        let note = doc.opening_note();
        assert!(
            note.contains("; it cannot be saved as it stands: element") && note.contains("size"),
            "{note}"
        );

        let doc = self::doc();
        assert_eq!(doc.opening_note(), doc.orientation_note());
    }
}
