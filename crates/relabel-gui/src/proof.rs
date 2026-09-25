//! What would print: the exact one-bit plane a job would encode, preflight's
//! issues, and every reason the editor could not print it here.
//!
//! No toolkit type in any signature, for `geom`'s reason: the print pane shows
//! what [`prove`] made and the sentences [`Proof`] says, and every rule is reached
//! by an ordinary unit test.
//!
//! # The same plane the command line encodes
//!
//! The label is bound to the clock alone and drawn for the document's own target,
//! then encoded for that target's head, exactly as `relabel-cli label --dry-run
//! --preview` does for a label with no rows (relabel-cli label.rs, `Prepared::page`
//! and `dry_run`). The clock is the epoch, which nothing shown reads: a label that
//! prints the date or time is stopped before it is drawn, because the editor has
//! no time zone to read it in. A label that takes rows from a file is stopped the
//! same way, since the editor prints no rows. Both stops name the command-line
//! route that does print them.
//!
//! # Never re-fitted
//!
//! Connected, the proof is checked against the printer's head, dpi and direction.
//! A label drawn for any other is stopped with Control+N as the remedy rather than
//! moved onto the printer's head: a label silently re-fitted is a label that does
//! not print as it was drawn. So when nothing is stopped, the connected printer's
//! target is the document's own, and the page kept here is the page that printer
//! is sent, never encoded again, with the density and label type the proof
//! records from the printer's facts.
//!
//! # When a proof is stale
//!
//! It records the document's revision and the printer link's generation it was
//! made at. The pane is a popup, so pointer and keys cannot edit behind it, and an
//! assistive technology's action still can (filing.rs); the wiring proves again
//! whenever either number has moved while the pane is open.

use crate::doc::Doc;
use crate::fresh;
use crate::printing::{counted, label_type_says};
use relabel_core::link::{Facts, NotPrintable};
use relabel_core::session::wire_len;
use relabel_proto::image::EncodedPage;
use relabel_proto::{BitPlane, HeadSpec, LabelType, PrintDirection, encode_page};
use relabel_render::bind::{Bindings, bind, uses_clock};
use relabel_render::datetime::CivilTime;
use relabel_render::document::{DataBinding, KnownBinding};
use relabel_render::preflight::{Issue, Preflight, Severity, preflight};
use relabel_render::preview::{Scale, plane_stamp};
use relabel_render::{Target, rasterise_plane};

/// How many rows the issue list shows. Past it, the last row counts the rest.
pub const MAX_ROWS: usize = 8;

/// The command line's time for one frame on the wire, for the time a page takes
/// to send: throughput is the write count times the pace, not the bytes
/// (relabel-cli main.rs, `dry_run`).
const MS_PER_FRAME: f64 = 12.0;

/// The clock a proof binds to. Never shown: a label that reads the clock is
/// stopped before it is drawn.
fn epoch() -> CivilTime {
    CivilTime::new(1970, 1, 1, 0, 0, 0, 0, 0).expect("the epoch is a real time")
}

/// One side of a head, dpi and direction difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side {
    pub head: u16,
    pub dpi: u16,
    pub direction: PrintDirection,
}

impl Side {
    fn of(t: &Target) -> Side {
        Side {
            head: t.head_axis(),
            dpi: t.dpi,
            direction: t.direction,
        }
    }

    /// "96-dot head at 203 dpi with the feed across the canvas".
    fn says(&self) -> String {
        let across = match self.direction {
            PrintDirection::Left => "with the feed across the canvas",
            PrintDirection::Top => "with the head across the top",
        };
        format!("{}-dot head at {} dpi {across}", self.head, self.dpi)
    }
}

/// A reason the editor will not print this label here, before preflight has a
/// say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Bound to rows of this file, which the editor does not print from.
    /// `beside` when the file's path names one in the label's own directory,
    /// the only place the command line reads a label's own data file from.
    TakesRows { file: String, beside: bool },
    /// Bound to data a later version set up.
    LaterData,
    /// Prints the date or time with `{dt}`.
    UsesClock,
    /// The connected printer's head width is not known.
    Unknown { model_id: Option<u16> },
    /// The connected printer's id names models whose heads differ.
    Ambiguous { names: Vec<&'static str> },
    /// The label is drawn for another head, dpi or direction than the connected
    /// printer's. `assumed` when the label names no model this build knows, so
    /// its direction was the editor's assumption; `unknown` is the model it
    /// names, when it names one this build does not know.
    OtherPrinter {
        label: Side,
        printer: Side,
        model: &'static str,
        assumed: bool,
        unknown: Option<String>,
    },
    /// Preflight or the render failed outright, with what it said.
    RenderFailed(String),
    /// The encoder refused the plane, with what it said.
    NotEncoded(String),
    /// A row of the page is longer than the link carries in one write.
    LinkTooNarrow { max: usize, frame: usize },
}

impl Stop {
    /// The whole sentence, which is the verdict when this stop comes first.
    pub fn says(&self) -> String {
        match self {
            Stop::TakesRows { file, beside: true } => format!(
                "This label prints one label for each row of {file}, and the editor does not \
                 print from rows: relabel-cli label prints it."
            ),
            Stop::TakesRows {
                file,
                beside: false,
            } => format!(
                "This label prints one label for each row of {file}, which is not beside the \
                 label, and the editor does not print from rows: relabel-cli label with --csv \
                 naming that file prints it."
            ),
            Stop::LaterData => "This label's data was set up by a later version of relabel, so \
                                the editor cannot show what it would print."
                .into(),
            Stop::UsesClock => "This label prints the date or time with {dt}, and the editor has \
                                no time zone to read it in: relabel-cli label --now prints it."
                .into(),
            Stop::Unknown { .. } => "relabel does not know this printer's print-head width, so \
                                     it cannot check or print a label for it."
                .into(),
            Stop::Ambiguous { names } => format!(
                "This printer's id is shared by {}, whose heads differ, and relabel will not \
                 guess which it is.",
                crate::printing::joined(names.iter().map(|n| format!("the {n}")), "and")
            ),
            Stop::OtherPrinter {
                label,
                printer,
                model,
                assumed,
                unknown,
            } => {
                let lead = match (assumed, unknown) {
                    (true, Some(made_for)) => format!(
                        "This label was made for a {made_for}, which this build does not know, \
                         and it"
                    ),
                    (true, None) => {
                        "This label does not say which printer it was made for, and it".into()
                    }
                    (false, _) => "This label".to_string(),
                };
                format!(
                    "{lead} is drawn for a {}; the {model} has a {}. Control+N makes a label for \
                     it.",
                    label.says(),
                    printer.says()
                )
            }
            Stop::RenderFailed(e) => format!("The label could not be drawn: {e}"),
            Stop::NotEncoded(e) => {
                format!("The label could not be encoded for the print head: {e}")
            }
            Stop::LinkTooNarrow { max, frame } => format!(
                "This Bluetooth link carries at most {} per write and a row of this label needs \
                 {frame}; relabel never splits a row, so it cannot print this label over this \
                 link.",
                counted(*max as u64, "byte", "bytes")
            ),
        }
    }
}

/// What would print, and why it would not.
#[derive(Debug, Clone)]
pub struct Proof {
    /// `Doc::revision` the proof was made at.
    pub rev: u64,
    /// The printer link's generation it was made at: `Printing::generation`.
    pub generation: u64,
    /// The document's own target, which is what was drawn and encoded.
    pub target: Target,
    /// The bound label, drawn. `None` when a stop or a failed bind came first.
    pub plane: Option<BitPlane>,
    /// `plane_stamp` of the plane at one pixel to the dot.
    pub stamp: Option<u64>,
    /// Preflight against the document's target. `None` when a stop came first.
    pub checked: Option<Preflight>,
    /// In the order the verdict takes them.
    pub stops: Vec<Stop>,
    /// The page a job would send: kept so that printing sends exactly this.
    pub page: Option<EncodedPage>,
    /// The connected printer's name and model, when one is connected.
    pub printer: Option<(String, &'static str)>,
    /// The model the label says it was made for.
    pub made_for: Option<String>,
    /// The connected printer's density and label type: what a print of this
    /// proof sends besides the page.
    pub settings: Option<(u8, LabelType)>,
}

/// Prove `doc`, against the connected printer's facts when there is one.
/// `generation` is the printer link's, recorded so the wiring can tell when the
/// proof is stale.
pub fn prove(doc: &Doc, printer: Option<&Facts>, generation: u64) -> Proof {
    let label = doc.label();
    let target = *doc.target();
    let mut proof = Proof {
        rev: doc.revision(),
        generation,
        target,
        plane: None,
        stamp: None,
        checked: None,
        stops: Vec::new(),
        page: None,
        printer: printer.map(|f| (f.name.clone(), f.model.map_or("printer", |m| m.name))),
        made_for: label
            .meta
            .authored_for
            .as_ref()
            .and_then(|a| a.model.clone()),
        settings: printer.map(|f| (f.print_density, f.print_label_type)),
    };

    // 1. What the editor does not print.
    match &label.data {
        Some(DataBinding::Known(KnownBinding::CsvFile(f))) => {
            proof.stops.push(Stop::TakesRows {
                file: f.path.clone(),
                // relabel-cli `label.rs` `beside`, which refuses anything else.
                beside: std::path::Path::new(&f.path)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            });
        }
        Some(DataBinding::Unknown(_)) => proof.stops.push(Stop::LaterData),
        None => {}
    }
    if uses_clock(label, &[]) {
        proof.stops.push(Stop::UsesClock);
    }
    let unprintable_data = !proof.stops.is_empty();

    // 2. The connected printer, which is never guessed at and never re-fitted to.
    if let Some(f) = printer {
        match &f.printable {
            Err(NotPrintable::UnknownModel { model_id }) => proof.stops.push(Stop::Unknown {
                model_id: *model_id,
            }),
            Err(NotPrintable::CandidatesDisagree { names }) => {
                proof.stops.push(Stop::Ambiguous {
                    names: names.clone(),
                });
            }
            Ok(()) => {
                let theirs = Target::for_label(f.head, target.feed_axis(), f.dpi, f.direction).ok();
                if theirs != Some(target) {
                    proof.stops.push(Stop::OtherPrinter {
                        label: Side::of(&target),
                        printer: Side {
                            head: f.head,
                            dpi: f.dpi,
                            direction: f.direction,
                        },
                        model: f.model.map_or("printer", |m| m.name),
                        assumed: doc.direction_assumed(),
                        // Assumed while naming a model means one the registry
                        // does not have (`Doc::open`).
                        unknown: proof.made_for.clone().filter(|_| doc.direction_assumed()),
                    });
                }
            }
        }
    }
    if unprintable_data {
        return proof;
    }

    // 3. Preflight, bound to the clock alone, as the command line checks a label
    // with no rows.
    let checked = match preflight(label, None, &target, epoch()) {
        Ok(p) => p,
        Err(e) => {
            proof.stops.push(Stop::RenderFailed(e.to_string()));
            return proof;
        }
    };
    proof.checked = Some(checked);

    // 4. The plane. A bind that fails is already one of preflight's refusals,
    // and draws nothing.
    let Ok(bound) = bind(label, &Bindings::clock_only(epoch())) else {
        return proof;
    };
    let plane = match rasterise_plane(&bound, &target) {
        Ok(p) => p,
        Err(e) => {
            proof.stops.push(Stop::RenderFailed(e.to_string()));
            return proof;
        }
    };
    proof.stamp = Some(plane_stamp(&plane, Scale::ONE));

    // 5. The page, for the target's own head.
    match encode_page(&plane, &HeadSpec::new(target.head_axis(), target.direction)) {
        Ok(page) => proof.page = Some(page),
        Err(e) => proof.stops.push(Stop::NotEncoded(e.to_string())),
    }
    proof.plane = Some(plane);

    // 6. A row is sent in one write, and never split.
    if let (Some(f), Some(page)) = (printer, &proof.page) {
        let frame = page.frames.iter().map(wire_len).max().unwrap_or(0);
        if frame > f.max_write {
            proof.stops.push(Stop::LinkTooNarrow {
                max: f.max_write,
                frame,
            });
        }
    }
    proof
}

impl Proof {
    /// Nothing stops it, preflight refuses nothing, and there is a page to send.
    pub fn printable(&self) -> bool {
        self.stops.is_empty()
            && self.checked.as_ref().is_some_and(|c| !c.refused())
            && self.page.is_some()
    }

    fn count(&self, severity: Severity) -> usize {
        self.checked.as_ref().map_or(0, |c| {
            c.issues.iter().filter(|i| i.severity == severity).count()
        })
    }

    /// The pane's first line, and what a screen reader hears when it opens: the
    /// first stop, else whether preflight refused anything, else what is shown.
    pub fn verdict(&self) -> String {
        if let Some(first) = self.stops.first() {
            let mut v = first.says();
            let more = self.stops.len() - 1;
            if more > 0 {
                v.push_str(&format!(" ({more} more below.)"));
            }
            return v;
        }
        let refused = self.count(Severity::Refuse);
        if refused > 0 {
            let (issues, are) = if refused == 1 {
                ("issue", "is")
            } else {
                ("issues", "are")
            };
            return format!("This would not print: {refused} {issues} below {are} refused.");
        }
        let mut v = match &self.printer {
            Some((name, _)) => format!("Nothing is refused. This is what would print on {name}."),
            None => format!(
                "Nothing is refused. This is the label as it would print on a {}-dot head at {} \
                 dpi; Connect checks it against your printer.",
                self.target.head_axis(),
                self.target.dpi
            ),
        };
        let some = |n: usize, one: &str, many: &str| (n > 0).then(|| counted(n as u64, one, many));
        let parts: Vec<String> = [
            some(self.count(Severity::Warn), "warning", "warnings"),
            some(self.count(Severity::Note), "note", "notes"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !parts.is_empty() {
            v.push_str(&format!(" {} below.", parts.join(" and ")));
        }
        v
    }

    /// The label's size in millimetres, then in dots, along the feed and across
    /// the head.
    fn sizes(&self) -> ((String, String), (u16, u16)) {
        let t = &self.target;
        let dpmm = t.dpmm();
        let mm = |dots: u16| fresh::mm(dpmm.tenths_mm_from_dots(i32::from(dots)));
        (
            (mm(t.feed_axis()), mm(t.head_axis())),
            (t.feed_axis(), t.head_axis()),
        )
    }

    /// The label's size, as Print's description says it: "40 × 12 mm".
    pub fn size_says(&self) -> String {
        let ((feed_mm, head_mm), _) = self.sizes();
        format!("{feed_mm} \u{d7} {head_mm} mm")
    }

    /// The line under the image: the size, what the page costs to send, and,
    /// when it would print, the settings it is sent with.
    pub fn facts_line(&self) -> String {
        let (_, (feed, head)) = self.sizes();
        let mut line = format!("{} \u{b7} {feed} \u{d7} {head} dots", self.size_says());
        if let Some(page) = &self.page {
            let frames = page.frames.len();
            let secs = frames as f64 * MS_PER_FRAME / 1000.0;
            line.push_str(&format!(
                " \u{b7} {}, {}, about {secs:.1} s to send",
                counted(frames as u64, "frame", "frames"),
                counted(page.wire_bytes() as u64, "byte", "bytes")
            ));
        }
        if self.printable()
            && let Some((density, label_type)) = self.settings
        {
            line.push_str(&format!(
                " \u{b7} density {density}, {}",
                label_type_says(label_type)
            ));
        }
        if self.printable()
            && let (Some((_, model)), Some(made)) = (&self.printer, &self.made_for)
            && made != model
        {
            line.push_str(&format!(
                " \u{b7} Made for a {made}; prints the same on this {model}."
            ));
        }
        line
    }

    /// The image's accessible description. `reduced` is the dots to a pixel when
    /// the pane shows the plane smaller than one pixel to the dot.
    pub fn plane_says(&self, reduced: Option<u32>) -> String {
        let Some(plane) = &self.plane else {
            return String::new();
        };
        let ((feed_mm, head_mm), _) = self.sizes();
        let mut says = format!(
            "{} by {} dots, {feed_mm} by {head_mm} millimetres",
            plane.width(),
            plane.height()
        );
        if let Some(k) = reduced.filter(|&k| k > 1) {
            says.push_str(&format!(", shown reduced: one pixel is {k} by {k} dots"));
        }
        let other = self.stops.iter().find_map(|s| match s {
            Stop::OtherPrinter { model, .. } => Some(*model),
            _ => None,
        });
        let refused = self.count(Severity::Refuse);
        match (other, refused) {
            (Some(model), _) => says.push_str(&format!("; not what the {model} would print")),
            (None, 0) => says.push_str("; nothing refused"),
            (None, n) => says.push_str(&format!("; {n} refused")),
        }
        says
    }

    /// The issue list: every stop, then preflight's issues, refusals first and
    /// otherwise in preflight's own order, at most [`MAX_ROWS`] of them.
    ///
    /// Preflight orders its issues by row and element, not by severity, so
    /// warnings on earlier elements would otherwise push the refusals the
    /// verdict points at out of the list. The last row, counting what is left
    /// out, says how many of those are refused when any are.
    pub fn issue_rows(&self) -> Vec<String> {
        let mut rows: Vec<(String, bool)> = self
            .stops
            .iter()
            .map(|s| (format!("Cannot print here: {}", s.says()), true))
            .collect();
        if let Some(c) = &self.checked {
            let mut issues: Vec<&Issue> = c.issues.iter().collect();
            // Stable, so preflight's order holds within each severity.
            issues.sort_by_key(|i| std::cmp::Reverse(i.severity));
            rows.extend(
                issues
                    .into_iter()
                    .map(|i| (issue_row(i), i.severity == Severity::Refuse)),
            );
        }
        if rows.len() > MAX_ROWS {
            let left_out = rows.split_off(MAX_ROWS - 1);
            let more = left_out.len();
            let refused = left_out.iter().filter(|(_, refused)| *refused).count();
            let last = match refused {
                0 => format!("and {more} more issues"),
                k => format!("and {more} more issues, {k} of them refused"),
            };
            rows.push((last, false));
        }
        rows.into_iter().map(|(row, _)| row).collect()
    }
}

/// One of preflight's issues, severity first, as a list item says it. The
/// editor binds no rows, so no issue has a row to name.
fn issue_row(i: &Issue) -> String {
    let severity = match i.severity {
        Severity::Refuse => "Refused",
        Severity::Warn => "Warning",
        Severity::Note => "Note",
    };
    let place = match (i.element, i.name.is_empty()) {
        (Some(e), false) => format!("element {e} \"{}\": ", i.name),
        (Some(e), true) => format!("element {e}: "),
        (None, _) => String::new(),
    };
    format!("{severity}: {place}{}", i.kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make::{self, Kind};
    use relabel_core::link::mock;
    use relabel_render::document::{
        AuthoredFor, Common, CsvFile, Element, ElementId, ElementOrRaw, Label, RectDots, RectEl,
    };
    use relabel_render::media::Bounds;
    use relabel_render::target::Dpmm;

    fn rect(id: ElementId, feed: i32, head: i32) -> ElementOrRaw {
        let mut common = Common::new(
            id,
            RectDots {
                feed,
                head,
                feed_len: 40,
                head_len: 24,
            },
        );
        common.name = format!("box {id}");
        ElementOrRaw::Known(Element::Rect(RectEl {
            common,
            stroke_dots: 0,
            radius_dots: 0,
        }))
    }

    /// The 40 by 12 mm label at 203 dpi this project measures everything
    /// against, as tests/support has it.
    fn reference_label() -> Label {
        let mut label = Label::new(320, 96, Dpmm::DPI_203);
        label.elements = vec![rect(1, 8, 8), rect(2, 120, 8), rect(3, 240, 40)];
        label
    }

    fn open(label: Label) -> Doc {
        Doc::open(label, PrintDirection::Left).expect("a printable label")
    }

    /// A new element of `kind` as the editor adds one, with its template set to
    /// `data`, on the reference label.
    fn with(kind: Kind, id: ElementId, data: &str) -> Label {
        let mut label = reference_label();
        let target = Target::for_label(96, 320, 203, PrintDirection::Left).expect("a target");
        let (mut el, _) = make::fresh(
            kind,
            id,
            &label,
            &target,
            PrintDirection::Left,
            &Bounds::of(&label),
            &[],
        );
        match &mut el {
            Element::Text(t) => t.text = data.into(),
            Element::Barcode(b) => {
                b.data = data.into();
                // Room for the longer symbol, so the key is the one refusal.
                b.module_dots = 2;
                b.common.frame.feed = 80;
                b.common.frame.feed_len = 236;
            }
            other => panic!("not a templated kind: {other:?}"),
        }
        label.elements.push(ElementOrRaw::Known(el));
        label
    }

    fn d110_m() -> Facts {
        mock::facts(2320, mock::D110_M_NAME)
    }

    #[test]
    fn a_plain_label_proves_to_its_canvas_plane() {
        let doc = open(reference_label());
        let proof = prove(&doc, None, 0);
        let canvas = rasterise_plane(doc.label(), doc.target()).expect("the canvas draws");
        assert_eq!(proof.plane.as_ref(), Some(&canvas));
        assert_eq!(proof.stamp, Some(plane_stamp(&canvas, Scale::ONE)));
        assert_eq!(proof.stops, []);
        assert!(proof.printable());
        let page = proof.page.as_ref().expect("a page");
        assert!(!page.frames.is_empty());
        assert_eq!(proof.rev, doc.revision());
        assert_eq!(
            proof.verdict(),
            "Nothing is refused. This is the label as it would print on a 96-dot head at 203 \
             dpi; Connect checks it against your printer."
        );
        assert_eq!(
            proof.plane_says(None),
            "320 by 96 dots, 40 by 12 millimetres; nothing refused"
        );
        assert_eq!(
            proof.plane_says(Some(2)),
            "320 by 96 dots, 40 by 12 millimetres, shown reduced: one pixel is 2 by 2 dots; \
             nothing refused"
        );
        assert!(proof.issue_rows().is_empty());
    }

    /// The numbers `relabel-cli label --dry-run` prints for the same label: one
    /// frame count and one byte count, from the same encoder.
    #[test]
    fn the_facts_line_counts_what_the_command_line_counts() {
        let doc = open(reference_label());
        let proof = prove(&doc, None, 0);
        let bound = bind(doc.label(), &Bindings::clock_only(epoch())).expect("binds");
        let plane = rasterise_plane(&bound, doc.target()).expect("draws");
        let page = encode_page(&plane, &HeadSpec::new(96, PrintDirection::Left)).expect("encodes");
        let secs = page.frames.len() as f64 * 0.012;
        assert_eq!(
            proof.facts_line(),
            format!(
                "40 \u{d7} 12 mm \u{b7} 320 \u{d7} 96 dots \u{b7} {} frames, {} bytes, about \
                 {secs:.1} s to send",
                page.frames.len(),
                page.wire_bytes()
            )
        );
        assert_eq!(proof.page, Some(page));
    }

    /// A blank 30 mm label, the one a launch opens on, is one frame, and the facts
    /// line says so in the singular, as it does every count it words.
    #[test]
    fn a_one_frame_page_is_one_frame_on_the_facts_line() {
        let proof = prove(&open(Label::new(240, 96, Dpmm::DPI_203)), None, 0);
        let page = proof.page.as_ref().expect("a page");
        assert_eq!(page.frames.len(), 1, "a blank 30 mm label is one frame");
        assert_eq!(
            proof.facts_line(),
            format!(
                "30 \u{d7} 12 mm \u{b7} 240 \u{d7} 96 dots \u{b7} 1 frame, {} bytes, about 0.0 s \
                 to send",
                page.wire_bytes()
            )
        );
        let narrow = Stop::LinkTooNarrow { max: 1, frame: 42 }.says();
        assert!(
            narrow.starts_with("This Bluetooth link carries at most 1 byte per write and"),
            "{narrow}"
        );
    }

    #[test]
    fn a_clock_label_is_stopped_with_the_cli_route() {
        let proof = prove(&open(with(Kind::Text, 4, "{dt}")), None, 0);
        assert_eq!(proof.stops, [Stop::UsesClock]);
        assert!(proof.plane.is_none() && proof.page.is_none() && proof.checked.is_none());
        assert!(!proof.printable());
        assert!(
            proof.verdict().contains("relabel-cli label --now"),
            "{}",
            proof.verdict()
        );
        assert_eq!(proof.plane_says(None), "");
        assert_eq!(
            proof.facts_line(),
            "40 \u{d7} 12 mm \u{b7} 320 \u{d7} 96 dots"
        );
    }

    #[test]
    fn a_label_with_rows_is_stopped_naming_its_file() {
        let mut label = reference_label();
        label.data = Some(DataBinding::Known(KnownBinding::CsvFile(CsvFile {
            path: "items.csv".into(),
            delimiter: ',',
            has_header: true,
            unknown: serde_json::Map::new(),
        })));
        let proof = prove(&open(label), None, 0);
        assert_eq!(
            proof.stops,
            [Stop::TakesRows {
                file: "items.csv".into(),
                beside: true
            }]
        );
        let verdict = proof.verdict();
        assert!(
            verdict.contains("items.csv") && verdict.contains("relabel-cli label"),
            "{verdict}"
        );
        assert_eq!(
            proof.issue_rows(),
            [format!("Cannot print here: {verdict}")]
        );
    }

    /// The command line reads a label's own data file only from beside the
    /// label, so a path that climbs out or is absolute needs `--csv`.
    #[test]
    fn a_data_file_not_beside_the_label_names_csv() {
        for (path, beside) in [
            ("items.csv", true),
            ("../items.csv", false),
            ("/srv/items.csv", false),
            ("sub/../items.csv", false),
        ] {
            let mut label = reference_label();
            label.data = Some(DataBinding::Known(KnownBinding::CsvFile(CsvFile {
                path: path.into(),
                delimiter: ',',
                has_header: true,
                unknown: serde_json::Map::new(),
            })));
            let verdict = prove(&open(label), None, 0).verdict();
            assert!(verdict.contains(path), "{verdict}");
            assert_eq!(verdict.contains("--csv"), !beside, "{verdict}");
        }
    }

    #[test]
    fn data_from_a_later_version_is_stopped() {
        let mut label = reference_label();
        label.data = Some(DataBinding::Unknown(serde_json::json!({"kind": "sheet"})));
        let proof = prove(&open(label), None, 0);
        assert_eq!(proof.stops, [Stop::LaterData]);
        assert!(proof.verdict().contains("later version"));
    }

    #[test]
    fn an_unresolved_symbol_key_is_refused() {
        let proof = prove(&open(with(Kind::Barcode, 4, "{sku}")), None, 0);
        let checked = proof.checked.as_ref().expect("preflight ran");
        assert!(checked.refused());
        assert!(!proof.printable());
        let rows = proof.issue_rows();
        assert!(
            rows.iter()
                .any(|r| r.starts_with("Refused: element 4") && r.contains("{sku}")),
            "{rows:?}"
        );
        assert_eq!(
            proof.verdict(),
            "This would not print: 1 issue below is refused.",
            "{rows:?}"
        );
        assert!(proof.plane_says(None).ends_with("; 1 refused"));
        // What would print is still drawn: the key, as written.
        assert!(proof.plane.is_some());
    }

    #[test]
    fn a_text_placeholder_only_warns() {
        let proof = prove(&open(with(Kind::Text, 4, "{sku}")), None, 0);
        let rows = proof.issue_rows();
        assert!(rows.iter().any(|r| r.starts_with("Warning: ")), "{rows:?}");
        assert!(proof.printable());
        let verdict = proof.verdict();
        assert!(verdict.starts_with("Nothing is refused."), "{verdict}");
        assert!(verdict.ends_with(" 1 warning below."), "{verdict}");
    }

    #[test]
    fn a_d110_label_proves_the_same_on_a_d110_m() {
        let mut label = reference_label();
        label.meta.authored_for = Some(AuthoredFor {
            model: Some("D110".into()),
            head_px: 96,
            dpi: 203,
        });
        let doc = open(label);
        let alone = prove(&doc, None, 0);
        let connected = prove(&doc, Some(&d110_m()), 1);
        assert_eq!(connected.stops, []);
        assert_eq!(connected.generation, 1);
        assert_eq!(connected.stamp, alone.stamp);
        assert_eq!(connected.page, alone.page);
        assert!(
            connected
                .facts_line()
                .ends_with(" \u{b7} Made for a D110; prints the same on this D110_M."),
            "{}",
            connected.facts_line()
        );
        assert_eq!(
            connected.verdict(),
            "Nothing is refused. This is what would print on D110_M-GB28010920."
        );
        // Unconnected, there is no printer to compare with.
        assert!(!alone.facts_line().contains("Made for"));
    }

    /// Connected and printable, the facts line says what the job will send
    /// besides the page: the density and the label type, after the send time
    /// and before the clause about the model the label was made for.
    #[test]
    fn the_facts_line_names_the_settings_a_print_sends() {
        let doc = open(reference_label());
        let alone = prove(&doc, None, 0);
        assert_eq!(alone.settings, None);
        assert!(!alone.facts_line().contains("density"));

        let connected = prove(&doc, Some(&d110_m()), 1);
        assert_eq!(connected.settings, Some((3, LabelType::Gap)));
        let line = connected.facts_line();
        assert_eq!(
            line,
            format!("{} \u{b7} density 3, labels with gaps", alone.facts_line())
        );
        assert_eq!(connected.size_says(), "40 \u{d7} 12 mm");

        // Made for another model it matches: the settings, then the model.
        let mut label = reference_label();
        label.meta.authored_for = Some(AuthoredFor {
            model: Some("D110".into()),
            head_px: 96,
            dpi: 203,
        });
        let line = prove(&open(label), Some(&d110_m()), 1).facts_line();
        assert!(
            line.ends_with(
                " \u{b7} density 3, labels with gaps \u{b7} Made for a D110; prints the same on \
                 this D110_M."
            ),
            "{line}"
        );

        // Not printable, it says no settings: nothing would be sent.
        let proof = prove(&open(with(Kind::Text, 4, "{dt}")), Some(&d110_m()), 1);
        assert!(!proof.facts_line().contains("density"));
        let proof = prove(&open(with(Kind::Barcode, 4, "{sku}")), Some(&d110_m()), 1);
        assert!(!proof.printable());
        assert!(!proof.facts_line().contains("density"));
    }

    #[test]
    fn another_head_is_stopped_with_control_n() {
        let mut label = reference_label();
        label.meta.authored_for = Some(AuthoredFor {
            model: Some("D110_M".into()),
            head_px: 96,
            dpi: 203,
        });
        let facts = Facts {
            head: 120,
            ..d110_m()
        };
        let proof = prove(&open(label), Some(&facts), 1);
        assert!(
            proof
                .stops
                .iter()
                .any(|s| matches!(s, Stop::OtherPrinter { assumed: false, .. })),
            "{:?}",
            proof.stops
        );
        let verdict = proof.verdict();
        assert!(
            verdict.contains("Control+N") && !verdict.contains("re-fitted"),
            "{verdict}"
        );
        assert_eq!(
            verdict,
            "This label is drawn for a 96-dot head at 203 dpi with the feed across the canvas; \
             the D110_M has a 120-dot head at 203 dpi with the feed across the canvas. \
             Control+N makes a label for it."
        );
        assert!(!proof.printable());
        // Drawn as the label is, and said not to be what the printer would print.
        assert!(
            proof
                .plane_says(None)
                .ends_with("; not what the D110_M would print")
        );
    }

    #[test]
    fn an_assumed_direction_that_differs_is_stopped() {
        let facts = Facts {
            direction: PrintDirection::Top,
            ..d110_m()
        };
        let proof = prove(&open(reference_label()), Some(&facts), 1);
        assert!(
            matches!(
                proof.stops.as_slice(),
                [Stop::OtherPrinter { assumed: true, .. }]
            ),
            "{:?}",
            proof.stops
        );
        assert!(
            proof
                .verdict()
                .starts_with("This label does not say which printer it was made for"),
            "{}",
            proof.verdict()
        );
    }

    /// A label that names a model this build does not know has its direction
    /// assumed too, but it does say which printer it was made for.
    #[test]
    fn a_label_for_a_model_this_build_does_not_know_says_so() {
        let facts = Facts {
            direction: PrintDirection::Top,
            ..d110_m()
        };
        let mut label = reference_label();
        label.meta.authored_for = Some(AuthoredFor {
            model: Some("FUTURE_X".into()),
            head_px: 96,
            dpi: 203,
        });
        let doc = open(label);
        assert!(doc.direction_assumed());
        let proof = prove(&doc, Some(&facts), 1);
        let verdict = proof.verdict();
        assert!(
            verdict
                .starts_with("This label was made for a FUTURE_X, which this build does not know,"),
            "{verdict}"
        );
        assert!(!verdict.contains("does not say which printer"), "{verdict}");
    }

    #[test]
    fn an_unknown_or_ambiguous_printer_is_stopped() {
        let unknown = mock::facts(9999, "X");
        let proof = prove(&open(reference_label()), Some(&unknown), 1);
        assert_eq!(
            proof.stops,
            [Stop::Unknown {
                model_id: Some(9999)
            }]
        );
        assert!(proof.verdict().starts_with("relabel does not know"));

        let ambiguous = mock::facts(2305, "X");
        let proof = prove(&open(reference_label()), Some(&ambiguous), 1);
        assert!(
            matches!(proof.stops.as_slice(), [Stop::Ambiguous { .. }]),
            "{:?}",
            proof.stops
        );
        let verdict = proof.verdict();
        assert!(
            verdict.contains("the D110") && verdict.contains("the Hi-D110"),
            "{verdict}"
        );
    }

    #[test]
    fn a_link_too_narrow_is_stopped_with_both_numbers() {
        let facts = Facts {
            max_write: 20,
            ..d110_m()
        };
        let proof = prove(&open(reference_label()), Some(&facts), 1);
        let frame = proof
            .stops
            .iter()
            .find_map(|s| match s {
                Stop::LinkTooNarrow { max: 20, frame } => Some(*frame),
                _ => None,
            })
            .expect("stopped for the link");
        assert!(frame > 20);
        assert!(proof.verdict().contains("20 bytes"), "{}", proof.verdict());
        assert!(proof.verdict().contains(&format!("needs {frame}")));
    }

    #[test]
    fn several_stops_are_counted_in_the_verdict_and_listed() {
        let facts = mock::facts(9999, "X");
        let proof = prove(&open(with(Kind::Text, 4, "{dt}")), Some(&facts), 1);
        assert_eq!(
            proof.stops,
            [
                Stop::UsesClock,
                Stop::Unknown {
                    model_id: Some(9999)
                }
            ]
        );
        assert!(proof.verdict().ends_with(" (1 more below.)"));
        let rows = proof.issue_rows();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.starts_with("Cannot print here: ")));
    }

    #[test]
    fn the_issue_list_stops_at_eight_rows() {
        let mut label = reference_label();
        for id in 10..20 {
            let (mut el, _) = make::fresh(
                Kind::Text,
                id,
                &label,
                &Target::for_label(96, 320, 203, PrintDirection::Left).expect("a target"),
                PrintDirection::Left,
                &Bounds::of(&label),
                &[],
            );
            if let Element::Text(t) = &mut el {
                t.text = "{sku}".into();
            }
            label.elements.push(ElementOrRaw::Known(el));
        }
        let proof = prove(&open(label), None, 0);
        let issues = proof.checked.as_ref().expect("preflight ran").issues.len();
        assert!(issues > MAX_ROWS, "{issues}");
        let rows = proof.issue_rows();
        assert_eq!(rows.len(), MAX_ROWS);
        assert_eq!(
            rows[MAX_ROWS - 1],
            format!("and {} more issues", issues - (MAX_ROWS - 1))
        );
    }

    #[test]
    fn a_refusal_after_many_warnings_is_still_listed() {
        let mut label = reference_label();
        for id in 10..20 {
            let (mut el, _) = make::fresh(
                Kind::Text,
                id,
                &label,
                &Target::for_label(96, 320, 203, PrintDirection::Left).expect("a target"),
                PrintDirection::Left,
                &Bounds::of(&label),
                &[],
            );
            if let Element::Text(t) = &mut el {
                t.text = "{sku}".into();
            }
            label.elements.push(ElementOrRaw::Known(el));
        }
        // Last in the document, so last in preflight's own order.
        let barcode = with(Kind::Barcode, 30, "{sku}")
            .elements
            .pop()
            .expect("the barcode");
        label.elements.push(barcode);
        let proof = prove(&open(label), None, 0);
        let checked = proof.checked.as_ref().expect("preflight ran");
        assert!(checked.issues.len() > MAX_ROWS);
        assert!(checked.refused());
        let rows = proof.issue_rows();
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(rows[0].starts_with("Refused: element 30"), "{rows:?}");
        let refused = rows.iter().filter(|r| r.starts_with("Refused: ")).count();
        assert_eq!(refused, proof.count(Severity::Refuse), "{rows:?}");
        assert!(rows[MAX_ROWS - 1].starts_with("and "), "{rows:?}");
    }
}
