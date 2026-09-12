//! Media geometry: the die-cut, the tail, the fold, and the repeat.
//!
//! Three of the four things a label says about its stock draw nothing. The
//! shape is a die-cut outline the content has to stay inside. The tail is real
//! paper at one end that carries no content - the part of a cable flag that
//! wraps the cable - so it constrains layout without leaving a mark. The fold
//! is a crease. Only [`Repeat`] moves a dot.
//!
//! # The canvas is not shortened
//!
//! The incumbent tool's `getLabelBounds()` subtracts the tail length from its
//! canvas. This does not, and the reason is the feed: the page handed to the
//! printer has to cover the whole label, tail included, because the paper
//! travels that far - and on a head that prints sideways a canvas narrower than
//! the head does not print inset, it prints shifted, which is why
//! [`Target::require_head`](crate::target::Target::require_head) demands
//! equality. So the tail comes off the *usable area* and the page stays the
//! size of the stock. [`Bounds`] carries all three rectangles.
//!
//! # Why the repeat happens after binarisation
//!
//! A fold-over flag carries the same content twice so it reads from both sides,
//! and the second panel is the first turned through half a turn. Duplicating
//! the rendered *plane* rather than cloning each element is undo-safe and gets
//! upside-down text right with no second code path - but *where* in the
//! pipeline decides whether the promise can be kept at all.
//!
//! Error diffusion runs in the canvas frame. Duplicate a grey region and
//! binarise the result and the turned panel is dithered along its own scan
//! rather than carrying the first panel's dither round with it: measured on a
//! photographic grey filling half a 320 by 96 label, the turned panel disagreed
//! with the half turn of the first in 6,209 of 15,360 dots under
//! Floyd-Steinberg, 5,070 under Atkinson, 5,032 under Sierra3 and 4,443 under
//! Bayer-8. So no pre-binarisation reading can satisfy the one thing the plan
//! asks of `Flip`. Worse, a grey copy that shares its source's zone id lets
//! error cross the seam in both directions, so turning the repeat on moved
//! 3,794 dots of the panel that was already right - a proof sheet would stop
//! matching the run. Duplicating finished dots moves none of them, and the
//! turned panel is the half turn exactly, by construction.
//!
//! It also settles the halftoning zones by not having any: by the time the dots
//! exist the zones have done their work, so a copied dot needs no id. Copying
//! the ids instead leaks error across the seam (measured above), per-element
//! renumbering has no range left - elements already run to
//! [`LAST_ZONE`](crate::rasterise) - and collapsing a whole copy into one
//! fenced id would destroy the per-element fence inside the copy and discard
//! each copied image's own halftone, which is the hazard the zone machinery
//! exists to prevent.

use crate::document::{
    Axis, Canvas, Fold, KNOWN_SHAPE_KINDS, KnownShape, Label, MediaShape, RectDots, Repeat,
};
use crate::greymap::GreyMap;
use crate::preflight::Severity;
use crate::target::Target;
use relabel_proto::BitPlane;
use serde_json::Value;

/// Paper an element cannot put a readable mark on, and which paper it is: the
/// fix differs for each, so the variant carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keepout {
    /// A panel a repeat overwrites with a copy of the first. Tested first,
    /// because it is the only one of the four that replaces the element's dots
    /// with other dots rather than losing them.
    Repeated,
    /// The length at one end that wraps around a cable: it prints, and then it
    /// is on the inside.
    Tail,
    /// The margin the print head cannot reach.
    Inset,
    /// Outside the die-cut outline: paper that is not there.
    Shape,
}

impl Keepout {
    /// Refuse for a symbol whose dots the label will not carry - the rule
    /// `OffCanvas` already uses, for the reason that already holds: a cut
    /// Code 128 scans as a different value, where anything else prints as far
    /// as the edge.
    ///
    /// The tail and the inset are only ever a Warn, because those dots do
    /// print: the tail is paper the fold hides, and the inset is a margin whose
    /// four edges are still unconfirmed, so a wrong guess there must not be
    /// able to stop a run. The inset's cap comes off when print test P8
    /// settles which edge is which.
    pub fn severity(self, symbol: bool) -> Severity {
        match self {
            Keepout::Repeated | Keepout::Shape if symbol => Severity::Refuse,
            _ => Severity::Warn,
        }
    }
}

/// A die-cut outline resolved against the canvas it is cut from.
///
/// Resolved rather than stored. A circle needs a centre and a diameter to be
/// geometry rather than a name, and both follow from the canvas - so storing
/// them would put a second authority beside [`Canvas`], and a file whose stored
/// centre disagreed with its canvas would mean nothing at all. A corner radius
/// is the one number the canvas does *not* give, because it belongs to the die,
/// so that one is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outline {
    Rect(RectDots),
    /// `radius` already clamped to `0 ..= min(feed_len, head_len) / 2`.
    RoundedRect {
        rect: RectDots,
        radius: i32,
    },
    /// The largest circle that fits `within`, centred in it. Held as its
    /// bounding rectangle, so a canvas with an odd side never materialises a
    /// half-dot centre.
    Circle {
        within: RectDots,
    },
}

impl Outline {
    /// A shape this build has never heard of resolves to the rectangle it is
    /// drawn as: it is reported once, by [`faults`], and it silently shrinks
    /// nothing.
    pub fn of(shape: &MediaShape, canvas: RectDots) -> Outline {
        // `max(0)` before the halving: a canvas side out of a file can be
        // negative, and `clamp` panics when its low bound exceeds its high one.
        let short = canvas.feed_len.min(canvas.head_len).max(0);
        match shape {
            MediaShape::Known(KnownShape::RoundedRect { radius }) => Outline::RoundedRect {
                rect: canvas,
                radius: (*radius).clamp(0, short / 2),
            },
            MediaShape::Known(KnownShape::Circle) => Outline::Circle { within: canvas },
            _ => Outline::Rect(canvas),
        }
    }

    pub fn bounds(&self) -> RectDots {
        match *self {
            Outline::Rect(r)
            | Outline::RoundedRect { rect: r, .. }
            | Outline::Circle { within: r } => r,
        }
    }

    /// Whether a frame lies entirely inside the outline.
    ///
    /// Exact at four corner tests rather than one per dot, and that is a
    /// property of the shapes rather than an approximation: all three outlines
    /// are convex, and a rectangle lies inside a convex region exactly when its
    /// corners do. The frame is the continuous rectangle its dots cover, which
    /// is how `fits` reads one, so a frame that touches the outline is inside.
    ///
    /// The arithmetic is `i128` on doubled coordinates: doubling keeps the
    /// centre of an odd-sided canvas exact instead of rounding the outline onto
    /// one side, and the square of the difference of two doubled `i32`s does not
    /// fit an `i64`. These numbers come out of a file.
    pub fn contains(&self, f: &RectDots) -> bool {
        if !within(f, &self.bounds()) {
            return false;
        }
        match self {
            Outline::Rect(_) => true,
            Outline::RoundedRect { rect, radius } => {
                corners(f).iter().all(|&p| in_rounded(p, rect, *radius))
            }
            Outline::Circle { within: w } => corners(f).iter().all(|&p| in_circle(p, w)),
        }
    }
}

/// Everything a label's own geometry says about where content may go, resolved
/// once against the canvas, in document dots.
///
/// One answer, read by the rasteriser's report, by preflight and by an editor's
/// guides, so that a label someone was allowed to draw is a label preflight
/// agrees with. Two subtractions at two call sites is how those drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// The whole page that prints, tail included.
    pub canvas: RectDots,
    /// The stock that carries content: the canvas less the tail. The fold's
    /// panels divide this.
    pub content: RectDots,
    /// Where an element may actually go: `content` less the margin the head
    /// cannot reach.
    pub usable: RectDots,
    /// The die-cut, inscribed in `canvas` - not in `usable`, because the die
    /// cuts the stock and the inset is the printer's reach.
    pub outline: Outline,
    /// The panels a repeat overwrites, as one rectangle; `None` when nothing
    /// repeats.
    pub overwritten: Option<RectDots>,
}

impl Bounds {
    pub fn of(label: &Label) -> Bounds {
        let canvas = canvas_rect(&label.canvas);
        let content = content_rect(label);
        Bounds {
            canvas,
            content,
            usable: inset_rect(content, &label.canvas),
            outline: Outline::of(&label.media.shape, canvas),
            overwritten: plan(label).ok().flatten().and_then(|p| p.overwritten()),
        }
    }

    /// Which keep-out a frame reaches, or `None` when it is clear.
    ///
    /// For a frame already known to be on the canvas: one that runs off the
    /// label is reported as that, once, and moving it is the same fix.
    ///
    /// In the order the operator loses most by: a copy replaces the element's
    /// dots, the tail hides them, the inset never fires them, and the die cuts
    /// them away.
    pub fn keepout(&self, f: &RectDots) -> Option<Keepout> {
        if self.overwritten.is_some_and(|r| overlaps(f, &r)) {
            return Some(Keepout::Repeated);
        }
        if !within(f, &self.content) {
            return Some(Keepout::Tail);
        }
        if !within(f, &self.usable) {
            return Some(Keepout::Inset);
        }
        if !self.outline.contains(f) {
            return Some(Keepout::Shape);
        }
        None
    }
}

/// Media geometry this build can read and cannot act on.
///
/// Reported, never refused at load, for the reason the whole document follows:
/// a file that will not open cannot be repaired in the editor that wrote it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GeometryFault {
    #[error(
        "the label repeats in a way this build does not know ({value}), so it would print one panel where the file asks for more"
    )]
    RepeatUnknown { value: String },
    #[error(
        "the label repeats across a fold and the media has no fold, so only one panel would print"
    )]
    RepeatWithoutFold,
    #[error(
        "the fold runs along {value}, which a later version wrote, so which way the label creases is not known"
    )]
    FoldAxisUnknown { value: String, needed: bool },
    #[error(
        "a fold at {at_dots} dots into {parts} panels does not divide content that runs from {} to {} dots",
        content.0,
        content.1
    )]
    FoldUnusable {
        at_dots: i32,
        parts: u8,
        content: (i32, i32),
        needed: bool,
    },
    #[error(
        "the label is too short for the fold: {printed} panels are copied, {short} dots of the last one are cut, and {dropped} have no room at all"
    )]
    FoldDoesNotFit {
        printed: u8,
        short: i32,
        dropped: u8,
    },
    #[error(
        "the tail runs along {value}, which a later version wrote, so no paper is kept clear for it"
    )]
    TailAxisUnknown { value: String },
    #[error(
        "the tail is {len_dots} dots on a label {along} dots along that axis, so it is read as {clamped}"
    )]
    TailClamped {
        len_dots: i32,
        along: i32,
        clamped: i32,
    },
    #[error(
        "the label is cut to a shape this build does not know ({value}), so the guide is a rectangle"
    )]
    ShapeUnknown { value: String },
    #[error(
        "the label is cut to a {kind} this build does know, and it is malformed ({reason}), so the guide is a rectangle"
    )]
    ShapeMalformed { kind: String, reason: String },
    #[error(
        "the label is cut round on a canvas {feed} by {head} dots, so the die-cut is the largest circle that fits and the ends of the label are outside it"
    )]
    ShapeNotSquare { feed: i32, head: i32 },
}

impl GeometryFault {
    /// Refuse when it would print a wrong or missing mark, Warn when it prints
    /// but not as drawn, Note when only a guide is affected.
    ///
    /// A fold fault is a Refuse only when a repeat depends on it: half a
    /// fold-over label is a missing mark, where a crease guide drawn in the
    /// wrong place prints nothing at all.
    pub fn severity(&self) -> Severity {
        match self {
            GeometryFault::RepeatUnknown { .. } | GeometryFault::RepeatWithoutFold => {
                Severity::Refuse
            }
            GeometryFault::FoldAxisUnknown { needed, .. }
            | GeometryFault::FoldUnusable { needed, .. } => {
                if *needed {
                    Severity::Refuse
                } else {
                    Severity::Note
                }
            }
            GeometryFault::FoldDoesNotFit { .. }
            | GeometryFault::TailAxisUnknown { .. }
            | GeometryFault::TailClamped { .. }
            | GeometryFault::ShapeMalformed { .. } => Severity::Warn,
            GeometryFault::ShapeUnknown { .. } | GeometryFault::ShapeNotSquare { .. } => {
                Severity::Note
            }
        }
    }
}

/// Where a repeat writes, in document dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatPlan {
    /// Panel 0: the content up to the first crease, and the region that is
    /// copied.
    pub source: RectDots,
    /// Whether the panels run along the feed. The axis is resolved here so no
    /// later step has to ask again; [`step`](Self::step) gives it as an offset.
    pub along_feed: bool,
    /// One panel to the next, in dots: the length of panel 0 along the fold's
    /// axis.
    pub pitch: i32,
    /// Panels written after the first. Only the panels that have room are
    /// counted, which is what bounds the copy: it can never write more dots
    /// than the page holds.
    pub copies: u8,
    /// Whether every other panel is turned through half a turn.
    pub flip: bool,
    /// Dots of panel 0 the last written panel has no room for.
    pub short: i32,
    /// Panels with no room at all.
    pub dropped: u8,
}

impl RepeatPlan {
    /// One panel to the next, as a document offset.
    pub fn step(&self) -> (i32, i32) {
        if self.along_feed {
            (self.pitch, 0)
        } else {
            (0, self.pitch)
        }
    }

    /// Where panel `n` goes; panel 0 is [`source`](Self::source).
    pub fn part(&self, n: u8) -> RectDots {
        let (sf, sh) = self.step();
        let n = i64::from(n);
        RectDots {
            feed: clip(i64::from(self.source.feed) + n * i64::from(sf)),
            head: clip(i64::from(self.source.head) + n * i64::from(sh)),
            ..self.source
        }
    }

    /// Every panel after the first, as one rectangle: they are contiguous, and
    /// an element that reaches any of them has its dots replaced.
    pub fn overwritten(&self) -> Option<RectDots> {
        if self.copies == 0 {
            return None;
        }
        let first = self.part(1);
        let covered = i64::from(self.pitch) * i64::from(self.copies) - i64::from(self.short).max(0);
        Some(if self.along_feed {
            RectDots {
                feed_len: clip(covered),
                ..first
            }
        } else {
            RectDots {
                head_len: clip(covered),
                ..first
            }
        })
    }
}

/// What a label's `repeat` and `fold` amount to, in document dots.
///
/// `Ok(None)` is a label that does not repeat. One pure function, so preflight
/// and [`crate::rasterise::rasterise_plane`] cannot disagree about what a file
/// meant.
pub fn plan(label: &Label) -> Result<Option<RepeatPlan>, GeometryFault> {
    let flip = match &label.repeat {
        Repeat::None => return Ok(None),
        Repeat::Copy => false,
        Repeat::Flip => true,
        Repeat::Unknown(v) => return Err(GeometryFault::RepeatUnknown { value: shown(v) }),
    };
    // Not invented when there is no fold: a duplicate with nowhere to go is
    // how half a fold-over label gets printed, which is the failure this whole
    // module is about.
    let Some(fold) = &label.media.fold else {
        return Err(GeometryFault::RepeatWithoutFold);
    };
    divide(fold, content_rect(label), flip, true).map(Some)
}

/// Everything about a label's media geometry that this build cannot act on.
///
/// Read straight off the document, so it costs no render and can be asked
/// before one. Each fact is reported once: an unknown fold axis is a Refuse
/// when a repeat needs it and a Note when it only misplaces a guide, and it is
/// still reported when the repeat itself is a value from the future.
pub fn faults(label: &Label) -> Vec<GeometryFault> {
    let mut out = Vec::new();
    let canvas = canvas_rect(&label.canvas);
    match &label.media.shape {
        MediaShape::Unknown(v) => out.push(match damaged_shape(v) {
            Some((kind, reason)) => GeometryFault::ShapeMalformed { kind, reason },
            None => GeometryFault::ShapeUnknown { value: shown(v) },
        }),
        MediaShape::Known(KnownShape::Circle) if canvas.feed_len != canvas.head_len => {
            out.push(GeometryFault::ShapeNotSquare {
                feed: canvas.feed_len,
                head: canvas.head_len,
            });
        }
        MediaShape::Known(_) => {}
    }
    if let Some(t) = &label.media.tail {
        match &t.axis {
            Axis::Unknown(v) => out.push(GeometryFault::TailAxisUnknown { value: shown(v) }),
            _ => {
                let along = if matches!(t.axis, Axis::Feed) {
                    canvas.feed_len
                } else {
                    canvas.head_len
                };
                let clamped = t.len_dots.clamp(0, along);
                if clamped != t.len_dots {
                    out.push(GeometryFault::TailClamped {
                        len_dots: t.len_dots,
                        along,
                        clamped,
                    });
                }
            }
        }
    }
    let repeats = matches!(label.repeat, Repeat::Copy | Repeat::Flip);
    match &label.repeat {
        Repeat::Unknown(v) => out.push(GeometryFault::RepeatUnknown { value: shown(v) }),
        _ if repeats && label.media.fold.is_none() => out.push(GeometryFault::RepeatWithoutFold),
        _ => {}
    }
    if let Some(fold) = &label.media.fold {
        match divide(fold, content_rect(label), false, repeats) {
            Ok(p) if repeats && (p.short > 0 || p.dropped > 0) => {
                out.push(GeometryFault::FoldDoesNotFit {
                    printed: p.copies,
                    short: p.short,
                    dropped: p.dropped,
                });
            }
            Ok(_) => {}
            Err(e) => out.push(e),
        }
    }
    out
}

/// Duplicate panel 0 into every other panel of the fold.
///
/// Reads `plane` and writes into a clone of it, so a panel can never pick up a
/// dot another panel just wrote: the non-overlap is a property of the code
/// rather than an argument about the arithmetic.
///
/// Dot by dot through [`BitPlane::get`] and [`BitPlane::set`] rather than
/// shifting packed rows. The packed route is ten to twenty times faster and
/// saves 9 ms on the largest page in the registry, against a print measured in
/// seconds; the sub-byte shift a crease that is not a multiple of eight needs is
/// exactly where a one-dot offset would hide, and this is the one transform in
/// the crate that has to be predictable to the dot.
pub fn repeat_plane(plane: &BitPlane, t: &Target, plan: &RepeatPlan) -> BitPlane {
    let mut out = plane.clone();
    let (w, h) = (i64::from(plane.width()), i64::from(plane.height()));
    walk(plan, t, w, h, |sx, sy, dx, dy| {
        let black = match (u16::try_from(sx), u16::try_from(sy)) {
            (Ok(x), Ok(y)) => plane.get(x, y),
            // A source dot outside the page was never drawn, so it copies as
            // paper rather than as whatever a wrapped coordinate would find.
            _ => false,
        };
        out.set(dx as u16, dy as u16, black);
    });
    out
}

/// The same duplication on the grey map, **for a preview pane and for
/// [`GreyMap::coverage`] only**.
///
/// Binarising what this returns is the reading the module docs reject: the
/// copied dots would be dithered on their own, and a copy that carried its
/// source's zone ids would leak error across the seam. The zone ids are
/// deliberately not copied, because this map is drawn and never binarised, and a
/// copied id would claim a fence the plane no longer has.
pub fn repeat_grey(grey: &GreyMap, t: &Target, plan: &RepeatPlan) -> GreyMap {
    let mut out = grey.clone();
    let (w, h) = (i64::from(grey.w), i64::from(grey.h));
    walk(plan, t, w, h, |sx, sy, dx, dy| {
        let v = match (u16::try_from(sx), u16::try_from(sy)) {
            (Ok(x), Ok(y)) => grey.at(x, y),
            _ => 255,
        };
        out.px[(dy * w + dx) as usize] = v;
    });
    out
}

/// Every dot a repeat copies, as source and destination in canvas coordinates.
///
/// The one piece of geometry the plane and the grey map share, so a preview pane
/// cannot disagree with the dots about where a panel is.
///
/// Destinations are always inside `w` by `h`; sources may be outside it, and
/// the caller reads those as paper. Every coordinate is computed in `i64` and
/// bounded before it is narrowed - the panel positions come from a file, and a
/// `u16` that wrapped would copy a panel onto a real row somewhere else.
///
/// Panel 0 goes through [`Target::place`] and [`Target::extent`] like every
/// other placement in the crate, which is what keeps a `Top`-direction printer
/// from coming out transposed. The half turn is taken inside the panel, and a
/// half turn commutes with that axis swap - unlike the reflection a 2D symbol's
/// grid needs - so a flipped fold-over label does not inherit that hazard.
fn walk(plan: &RepeatPlan, t: &Target, w: i64, h: i64, mut mv: impl FnMut(i64, i64, i64, i64)) {
    if plan.copies == 0 {
        return;
    }
    let (sx, sy) = t.place(plan.source.feed, plan.source.head);
    let (sw, sh) = t.extent(plan.source.feed_len, plan.source.head_len);
    let (sw, sh) = (i64::from(sw).max(0), i64::from(sh).max(0));
    let (x0, y0) = (i64::from(sx), i64::from(sy));
    for n in 1..=plan.copies {
        let p = plan.part(n);
        let (dx, dy) = t.place(p.feed, p.head);
        let (dx, dy) = (i64::from(dx), i64::from(dy));
        // Only the part of this panel that is on the page. A panel wholly off
        // it is skipped, which with the count of panels that have room is what
        // keeps the work bounded by the page however hostile the numbers.
        let (u0, v0) = ((-dx).max(0), (-dy).max(0));
        // The last panel may have less room than a whole one, because the
        // content ends before the page does whenever a tail trails it - and
        // the dots past the content belong to no panel. Clipping by the same
        // `short` the plan reports is what makes that number true of the plane
        // as well: without it a repeat writes its full pitch over a trailing
        // tail, replacing marks the report says are only hidden by it.
        let cut = if n == plan.copies {
            plan.short.max(0)
        } else {
            0
        };
        let (cu, cv) = if plan.along_feed {
            t.extent(cut, 0)
        } else {
            t.extent(0, cut)
        };
        let (u1, v1) = (
            (w - dx).min(sw - i64::from(cu)),
            (h - dy).min(sh - i64::from(cv)),
        );
        if u1 <= u0 || v1 <= v0 {
            continue;
        }
        // Half a turn, because a fold reverses which way the paper faces and
        // reversing twice faces it the way it started. The centre is the
        // *unclipped* panel's, so a panel the label cuts short still turns
        // about the point the paper folds on.
        let turned = plan.flip && n % 2 == 1;
        for v in v0..v1 {
            for u in u0..u1 {
                let (su, sv) = if turned {
                    (sw - 1 - u, sh - 1 - v)
                } else {
                    (u, v)
                };
                mv(x0 + su, y0 + sv, dx + u, dy + v);
            }
        }
    }
}

/// The fold's panels, with no reference to `repeat`, so the same arithmetic
/// answers both "where do the copies go" and "where are the creases".
///
/// `needed` says whether a repeat depends on it, which is the only thing that
/// changes how loudly a fault is reported.
fn divide(
    fold: &Fold,
    content: RectDots,
    flip: bool,
    needed: bool,
) -> Result<RepeatPlan, GeometryFault> {
    let along_feed = match &fold.axis {
        Axis::Feed => true,
        Axis::Head => false,
        // Guessing the feed would crease the label the wrong way, which is the
        // one failure an open axis exists to prevent.
        Axis::Unknown(v) => {
            return Err(GeometryFault::FoldAxisUnknown {
                value: shown(v),
                needed,
            });
        }
    };
    let (start, len) = if along_feed {
        (content.feed, content.feed_len)
    } else {
        (content.head, content.head_len)
    };
    let (first, last) = (i64::from(start), i64::from(start) + i64::from(len));
    let at = i64::from(fold.at_dots);
    if fold.parts < 2 || at <= first || at >= last {
        return Err(GeometryFault::FoldUnusable {
            at_dots: fold.at_dots,
            parts: fold.parts,
            content: (clip(first), clip(last)),
            needed,
        });
    }
    let pitch = at - first;
    let (mut copies, mut short, mut dropped) = (0u8, 0i64, 0u8);
    for n in 1..i64::from(fold.parts) {
        let room = last - (at + (n - 1) * pitch);
        if room <= 0 {
            dropped = dropped.saturating_add(1);
            continue;
        }
        copies = n as u8;
        short = (pitch - room).max(0);
    }
    let source = if along_feed {
        RectDots {
            feed_len: clip(pitch),
            ..content
        }
    } else {
        RectDots {
            head_len: clip(pitch),
            ..content
        }
    };
    Ok(RepeatPlan {
        source,
        along_feed,
        pitch: clip(pitch),
        copies,
        flip,
        short: clip(short),
        dropped,
    })
}

/// The canvas as a rectangle, with a side a file wrote backwards read as empty
/// rather than as a rectangle that contains nothing and crashes on the way.
fn canvas_rect(c: &Canvas) -> RectDots {
    RectDots {
        feed: 0,
        head: 0,
        feed_len: c.feed.max(0),
        head_len: c.head.max(0),
    }
}

/// The canvas less the tail.
///
/// A tail whose axis this build cannot place keeps nothing clear: guessing
/// which end of which axis would report the wrong elements, where [`faults`]
/// says plainly once that it could not be read.
fn content_rect(label: &Label) -> RectDots {
    let c = canvas_rect(&label.canvas);
    let Some(t) = &label.media.tail else {
        return c;
    };
    let along_feed = match t.axis {
        Axis::Feed => true,
        Axis::Head => false,
        Axis::Unknown(_) => return c,
    };
    let along = if along_feed { c.feed_len } else { c.head_len };
    let len = t.len_dots.clamp(0, along);
    let start = if t.at_start { len } else { 0 };
    if along_feed {
        RectDots {
            feed: start,
            feed_len: along - len,
            ..c
        }
    } else {
        RectDots {
            head: start,
            head_len: along - len,
            ..c
        }
    }
}

/// The content less the printer's unprintable margin, never inside out: an
/// inset wider than the stock leaves an empty rectangle, which is one fault
/// about the insets rather than one about every element.
fn inset_rect(content: RectDots, c: &Canvas) -> RectDots {
    let i = c.inset;
    let (fs, fe) = (i64::from(i.feed_start).max(0), i64::from(i.feed_end).max(0));
    let (hs, he) = (i64::from(i.head_start).max(0), i64::from(i.head_end).max(0));
    RectDots {
        feed: clip(i64::from(content.feed) + fs),
        head: clip(i64::from(content.head) + hs),
        feed_len: clip((i64::from(content.feed_len) - fs - fe).max(0)),
        head_len: clip((i64::from(content.head_len) - hs - he).max(0)),
    }
}

/// Whether a frame lies entirely inside a bound, on both axes and at both ends.
///
/// `i64`, like `fits`, because these are signed values straight out of a file
/// and `feed + feed_len` overflows for a frame near `i32::MAX`.
fn within(f: &RectDots, b: &RectDots) -> bool {
    let (fx, fy) = (i64::from(f.feed), i64::from(f.head));
    let (bx, by) = (i64::from(b.feed), i64::from(b.head));
    fx >= bx
        && fy >= by
        && fx + i64::from(f.feed_len) <= bx + i64::from(b.feed_len)
        && fy + i64::from(f.head_len) <= by + i64::from(b.head_len)
}

/// Whether a frame shares a dot with a rectangle. Zero-sized frames overlap
/// nothing, which is what keeps a hidden-width element out of the report.
fn overlaps(f: &RectDots, b: &RectDots) -> bool {
    let (fx, fy) = (i64::from(f.feed), i64::from(f.head));
    let (bx, by) = (i64::from(b.feed), i64::from(b.head));
    fx < bx + i64::from(b.feed_len)
        && bx < fx + i64::from(f.feed_len)
        && fy < by + i64::from(b.head_len)
        && by < fy + i64::from(f.head_len)
}

/// The frame's four corners, in the continuous coordinates `fits` uses.
fn corners(f: &RectDots) -> [(i64, i64); 4] {
    let (x0, y0) = (i64::from(f.feed), i64::from(f.head));
    let (x1, y1) = (x0 + i64::from(f.feed_len), y0 + i64::from(f.head_len));
    [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
}

fn in_circle(p: (i64, i64), w: &RectDots) -> bool {
    let d = i128::from(w.feed_len.min(w.head_len).max(0));
    let cx = 2 * i128::from(w.feed) + i128::from(w.feed_len);
    let cy = 2 * i128::from(w.head) + i128::from(w.head_len);
    let (dx, dy) = (2 * i128::from(p.0) - cx, 2 * i128::from(p.1) - cy);
    dx * dx + dy * dy <= d * d
}

fn in_rounded(p: (i64, i64), rect: &RectDots, radius: i32) -> bool {
    let r = 2 * i128::from(radius.max(0));
    let (x, y) = (2 * i128::from(p.0), 2 * i128::from(p.1));
    let (x0, y0) = (2 * i128::from(rect.feed), 2 * i128::from(rect.head));
    let (x1, y1) = (
        x0 + 2 * i128::from(rect.feed_len),
        y0 + 2 * i128::from(rect.head_len),
    );
    // The distance from the nearest corner circle's centre, zero along the
    // straight sides: one test covers all four corners and the middle.
    let dx = (x0 + r - x).max(x - (x1 - r)).max(0);
    let dy = (y0 + r - y).max(y - (y1 - r)).max(0);
    dx * dx + dy * dy <= r * r
}

/// A value from a later version, short enough to read in a report: the string
/// it is, or the JSON it is, cut to something an operator can take in.
fn shown(v: &Value) -> String {
    let s = match v.as_str() {
        Some(s) => s.to_string(),
        None => v.to_string(),
    };
    match s.char_indices().nth(60) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s,
    }
}

/// Why a shape that reached [`MediaShape::Unknown`] is damaged rather than a
/// stranger's, if it is: the kind, and what serde made of it.
///
/// Narrower than `damaged_binding`, which also refuses a non-object. A shape is
/// a small value a later version might plausibly write as a bare string, and an
/// unrecognised one only costs a guide - so this is a Warn and never a load
/// error, where a damaged element vanishes and a damaged binding prints the
/// wrong data.
fn damaged_shape(v: &Value) -> Option<(String, String)> {
    let kind = v.get("kind")?.as_str()?;
    if !KNOWN_SHAPE_KINDS.contains(&kind) {
        return None;
    }
    match serde_json::from_value::<KnownShape>(v.clone()) {
        Ok(_) => None,
        Err(e) => Some((kind.to_string(), e.to_string())),
    }
}

/// A dot position narrowed back to the document's own `i32`, saturating.
///
/// Nothing downstream can place a dot past `i32::MAX` anyway, and a silent wrap
/// would move a panel somewhere real.
fn clip(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
