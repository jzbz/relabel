//! Preflight: every row rendered and measured before a single label is sent.
//!
//! On a 96-dot head overflow is the common case, not the edge case: twenty
//! letters of Code 128 need 550 dots at the two-dot floor, quiet zones
//! included, and a whole 40 mm label is 320. A run of 500 labels that fails at
//! the 300th has burned 300 labels of stock. So before anything prints, the
//! label is bound to every row and drawn, and each problem is reported with
//! the row and the element it belongs to, and the numbers that explain it.
//!
//! # What it costs
//!
//! One render of the whole label, bound only to the clock, gives everything
//! that cannot change from row to row: every frame warning - off the canvas, a
//! rotation approximated, artwork resampled - and every failure of an element
//! with no data in it. Then only the elements that take a column are drawn
//! again, once for each row that prints. The copies `$times` asks for are
//! identical - one clock per job, and no page number - so a row is drawn once
//! however many times it prints. The rows of a 500-row table cost about a
//! tenth of a second.

use crate::bind::{BindError, Bindings, Table, TimesError, bind, parse_times};
use crate::datetime::CivilTime;
use crate::document::{DmSize, Element, ElementId, ElementOrRaw, Label};
use crate::error::RenderError;
use crate::media::{self, GeometryFault, Keepout};
use crate::rasterise::{Failure, rasterise};
use crate::target::Target;
use crate::template::{is_js_space, placeholders};
use crate::text::TextClip;
use std::collections::{BTreeSet, HashMap};
use std::fmt;

/// What a run would do: how many times each row prints, and what is wrong.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preflight {
    /// Labels per row of the table, in order, before any job-wide copies;
    /// `[1]` with no table. Rows print in order, each repeated back to back.
    pub times: Vec<u32>,
    /// Row-wide problems first, then by row, each in document order.
    pub issues: Vec<Issue>,
}

impl Preflight {
    /// Labels the run prints, before job-wide copies.
    pub fn pages(&self) -> u64 {
        self.times.iter().map(|&t| u64::from(t)).sum()
    }

    /// Whether anything should stop the run before it starts.
    pub fn refused(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Refuse)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The row of the table, counted from 0 below the header and before
    /// `$times` repeats it; `None` for a problem on every row.
    pub row: Option<usize>,
    pub element: Option<ElementId>,
    /// The element's own name, for a person to find it by; empty when it has
    /// none.
    pub name: String,
    pub kind: IssueKind,
    pub severity: Severity,
}

/// Ordered, so the worst of a set is its maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth knowing; nothing prints wrong.
    Note,
    /// Prints, but not quite as drawn.
    Warn,
    /// Would print a wrong or missing mark: the run should not start.
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    /// The label is drawn for a canvas of a different size, in dots along the
    /// feed and across the head.
    TargetMismatch {
        canvas: (i32, i32),
        target: (u16, u16),
    },
    DuplicateId,
    NoRows,
    /// A column no placeholder can ever name: a key is ASCII letters, digits
    /// and underscores, with an optional leading `$`.
    UnreachableColumn {
        column: String,
    },
    /// A placeholder no column answers, printed as written; `near` is a column
    /// that differs only in case, spacing or a byte-order mark.
    UnresolvedKey {
        key: String,
        near: Option<String>,
    },
    /// A column's value is used as it is; a filter after it does nothing.
    FilterIgnored {
        key: String,
        filter: String,
    },
    /// A `dt` column replaces the time in `{dt}`.
    ClockShadowed,
    /// A QR code without a pinned version, or a DataMatrix without a fixed
    /// size, whose data changes by row: its size can change with it.
    SizeUnpinned,
    Bind(BindError),
    BadTimes(TimesError),
    /// `$times` is 0: the row prints nothing.
    Skipped,
    Failed(Failure),
    TextClipped(TextClip),
    OffCanvas,
    RotationApproximated,
    ImageResampled {
        stored: (u32, u32),
        drawn: (u32, u32),
    },
    ImageClipped,
    ZoneShared,
    Unimplemented(&'static str),
    Unrecognised(String),
    /// Media geometry this build read and cannot act on: a shape, tail, fold or
    /// repeat from a later version, or one whose numbers do not work. Wraps its
    /// own error, the way `Bind` and `BadTimes` already do, so the numbers reach
    /// the operator intact.
    MediaGeometry(GeometryFault),
    /// The element is on the canvas and still cannot put a readable mark there.
    Keepout(Keepout),
    /// The label was drawn for another printer's head. Advisory only: the canvas
    /// has been re-fitted, and `TargetMismatch` is what refuses one that has not.
    AuthoredElsewhere {
        head_px: u16,
        dpi: u16,
    },
}

impl fmt::Display for IssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueKind::TargetMismatch { canvas, target } => write!(
                f,
                "the label is {} by {} dots along the feed and across the head, but the printer takes {} by {}",
                canvas.0, canvas.1, target.0, target.1
            ),
            IssueKind::DuplicateId => f.write_str("another element has the same id"),
            IssueKind::NoRows => f.write_str("the data has no rows, so nothing would print"),
            IssueKind::UnreachableColumn { column } => write!(
                f,
                "no placeholder can name the column {column:?}: a name is letters, digits and underscores, with an optional leading $"
            ),
            IssueKind::UnresolvedKey { key, near } => {
                write!(f, "{{{key}}} is not a column in the data, so it prints as written")?;
                match near {
                    Some(n) => write!(f, "; the column {n:?} is close"),
                    None => Ok(()),
                }
            }
            IssueKind::FilterIgnored { key, filter } => write!(
                f,
                "the filter {filter:?} after {key} does nothing: a column's value is used as it is"
            ),
            IssueKind::ClockShadowed => {
                f.write_str("the data has a dt column, which replaces the time in {dt}")
            }
            IssueKind::SizeUnpinned => f.write_str(
                "the symbol's size is not pinned, so it can change from row to row and move the layout",
            ),
            IssueKind::Bind(e) => write!(f, "{e}"),
            IssueKind::BadTimes(e) => write!(f, "{e}"),
            IssueKind::Skipped => f.write_str("$times is 0, so this row prints nothing"),
            IssueKind::Failed(e) => write!(f, "{e}"),
            IssueKind::TextClipped(c) => write!(
                f,
                "the text does not fit its frame: {} of {} lines whole, {} dots too wide, {} dots of ink cut",
                c.lines_whole, c.lines, c.overhang_dots, c.ink_clipped_dots
            ),
            IssueKind::OffCanvas => f.write_str("the element runs past the edge of the label"),
            IssueKind::RotationApproximated => {
                f.write_str("the rotation is not a quarter turn, so it is drawn at the nearest one")
            }
            IssueKind::ImageResampled { stored, drawn } => write!(
                f,
                "the artwork was imported at {}x{} and is drawn at {}x{}; import it again for this frame",
                stored.0, stored.1, drawn.0, drawn.1
            ),
            IssueKind::ImageClipped => f.write_str("the artwork is larger than its frame, and cropped"),
            IssueKind::ZoneShared => f.write_str(
                "the label has more elements than halftone zones, so this image's own halftone is not used",
            ),
            IssueKind::Unimplemented(kind) => write!(f, "this build cannot draw {kind} elements"),
            IssueKind::Unrecognised(kind) => write!(
                f,
                "a {kind} element was written by a later version, and cannot be drawn"
            ),
            IssueKind::MediaGeometry(e) => write!(f, "{e}"),
            IssueKind::Keepout(Keepout::Repeated) => f.write_str(
                "the element is in a panel the fold repeats, so a copy of the first panel prints over it",
            ),
            IssueKind::Keepout(Keepout::Tail) => f.write_str(
                "part of the element is on the tail, which is paper the fold hides: it prints, but not on the face of the label",
            ),
            IssueKind::Keepout(Keepout::Inset) => f.write_str(
                "part of the element is in the margin the print head cannot reach",
            ),
            IssueKind::Keepout(Keepout::Shape) => {
                f.write_str("part of the element is outside the die-cut, where there is no paper")
            }
            IssueKind::AuthoredElsewhere { head_px, dpi } => write!(
                f,
                "the label was drawn for a {head_px}-dot head at {dpi} dpi, and has been re-fitted to this one"
            ),
        }
    }
}

/// The template an element carries, and whether it is a symbol's data - where
/// a placeholder printed as written is a scannable wrong value, not a typo.
fn template_of(el: &Element) -> Option<(&str, bool)> {
    match el {
        Element::Text(e) => Some((&e.text, false)),
        Element::Barcode(e) => Some((&e.data, true)),
        Element::Qr(e) => Some((&e.data, true)),
        Element::DataMatrix(e) => Some((&e.data, true)),
        Element::Image(_) | Element::Rect(_) | Element::Line(_) => None,
    }
}

fn is_symbol(el: &Element) -> bool {
    matches!(
        el,
        Element::Barcode(_) | Element::Qr(_) | Element::DataMatrix(_)
    )
}

/// A column name a placeholder can reach: `\$?\w+`.
fn is_key(s: &str) -> bool {
    let t = s.strip_prefix('$').unwrap_or(s);
    !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A name with the differences a spreadsheet introduces taken out: a
/// byte-order mark, surrounding spaces, case, and spaces or hyphens for
/// underscores.
fn loosely(s: &str) -> String {
    let s: String = s.chars().filter(|&c| c != '\u{feff}').collect();
    s.trim_matches(is_js_space)
        .chars()
        .map(|c| match c {
            '-' | ' ' => '_',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// Everything a run of `label` over `data` would print wrong, before it runs.
///
/// `data` is the table the editor or command line read; `None` is a label
/// printed once, bound only to the clock. `now` is the job's one reading of
/// the clock, the same one printing will use.
pub fn preflight(
    label: &Label,
    data: Option<&Table>,
    target: &Target,
    now: CivilTime,
) -> Result<Preflight, RenderError> {
    let columns: &[String] = data.map_or(&[], |t| &t.columns);
    let mut column_count: HashMap<&str, usize> = HashMap::new();
    for c in columns {
        *column_count.entry(c.as_str()).or_default() += 1;
    }
    let names: HashMap<ElementId, &str> = label
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .map(|e| (e.common().id, e.common().name.as_str()))
        .collect();
    let mut issues: Vec<Issue> = Vec::new();
    let mut raise = |row: Option<usize>, element: Option<ElementId>, kind, severity| {
        issues.push(Issue {
            row,
            element,
            name: element
                .and_then(|id| names.get(&id))
                .map(|n| n.to_string())
                .unwrap_or_default(),
            kind,
            severity,
        })
    };
    let mut stop = false;

    // 1. The label and the data as a whole.
    let canvas = (label.canvas.feed, label.canvas.head);
    let axes = (target.feed_axis(), target.head_axis());
    if canvas != (i32::from(axes.0), i32::from(axes.1)) {
        raise(
            None,
            None,
            IssueKind::TargetMismatch {
                canvas,
                target: axes,
            },
            Severity::Refuse,
        );
    }
    // The media's own geometry: read straight off the document, so none of it
    // needs a render and none of it is said twice however many rows print.
    for fault in media::faults(label) {
        let severity = fault.severity();
        raise(None, None, IssueKind::MediaGeometry(fault), severity);
    }
    if let Some(a) = &label.meta.authored_for
        && (a.head_px != target.head_axis() || a.dpi != target.dpi)
    {
        raise(
            None,
            None,
            IssueKind::AuthoredElsewhere {
                head_px: a.head_px,
                dpi: a.dpi,
            },
            Severity::Note,
        );
    }
    let mut seen = BTreeSet::new();
    for el in label.elements.iter().filter_map(ElementOrRaw::known) {
        if !seen.insert(el.common().id) {
            raise(
                None,
                Some(el.common().id),
                IssueKind::DuplicateId,
                Severity::Refuse,
            );
        }
    }
    if let Some(t) = data {
        if t.rows.is_empty() {
            raise(None, None, IssueKind::NoRows, Severity::Refuse);
            stop = true;
        }
        for c in &t.columns {
            if !is_key(c) {
                raise(
                    None,
                    None,
                    IssueKind::UnreachableColumn { column: c.clone() },
                    Severity::Note,
                );
            }
        }
    }
    let mut ambiguous = BTreeSet::new();
    let mut varying: BTreeSet<ElementId> = BTreeSet::new();
    let drawable: BTreeSet<ElementId> = label.drawable().map(|e| e.common().id).collect();
    let symbols: BTreeSet<ElementId> = label
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .filter(|e| is_symbol(e))
        .map(|e| e.common().id)
        .collect();
    // Reversed, so the first column of a set that loosens to the same name is
    // the one suggested.
    let loose: HashMap<String, &str> = columns
        .iter()
        .rev()
        .map(|c| (loosely(c), c.as_str()))
        .collect();
    for el in label.elements.iter().filter_map(ElementOrRaw::known) {
        let Some((template, symbol)) = template_of(el) else {
            continue;
        };
        let id = el.common().id;
        let shown = drawable.contains(&id);
        let mut unresolved = BTreeSet::new();
        let mut filtered = BTreeSet::new();
        let mut shadowed = false;
        for p in placeholders(template) {
            match column_count.get(p.key) {
                Some(&n) if n > 1 => {
                    if ambiguous.insert(p.key.to_string()) {
                        raise(
                            None,
                            None,
                            IssueKind::Bind(BindError::AmbiguousColumn(p.key.to_string())),
                            Severity::Refuse,
                        );
                    }
                    stop = true;
                }
                Some(_) => {
                    // Bound on every row even when hidden: binding is where a
                    // field too long or an ambiguous column shows up, and that
                    // stops the run whether or not the element prints.
                    varying.insert(id);
                    if !shown {
                        continue;
                    }
                    if p.key == "dt" {
                        shadowed = true;
                    } else if let Some(f) = p.filter.filter(|f| !f.is_empty())
                        && filtered.insert((p.key.to_string(), f.to_string()))
                    {
                        raise(
                            None,
                            Some(id),
                            IssueKind::FilterIgnored {
                                key: p.key.to_string(),
                                filter: f.to_string(),
                            },
                            Severity::Note,
                        );
                    }
                }
                None if p.key == "dt" => {}
                None => {
                    if shown && unresolved.insert(p.key.to_string()) {
                        let near = loose.get(&loosely(p.key)).map(|c| c.to_string());
                        raise(
                            None,
                            Some(id),
                            IssueKind::UnresolvedKey {
                                key: p.key.to_string(),
                                near,
                            },
                            if symbol {
                                Severity::Refuse
                            } else {
                                Severity::Warn
                            },
                        );
                    }
                }
            }
        }
        if shadowed {
            raise(None, Some(id), IssueKind::ClockShadowed, Severity::Note);
        }
    }

    // 2. How many times each row prints, and the rows that cannot be bound.
    let times_column = match column_count.get("$times") {
        Some(&n) if n > 1 => {
            if ambiguous.insert("$times".to_string()) {
                raise(
                    None,
                    None,
                    IssueKind::Bind(BindError::AmbiguousColumn("$times".into())),
                    Severity::Refuse,
                );
            }
            stop = true;
            None
        }
        Some(_) => columns.iter().position(|c| c == "$times"),
        None => None,
    };
    let times: Vec<u32> = match data {
        None => vec![1],
        Some(t) => t
            .rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                if row.len() != columns.len() {
                    raise(
                        Some(r),
                        None,
                        IssueKind::Bind(BindError::RowLength {
                            cells: row.len(),
                            columns: columns.len(),
                        }),
                        Severity::Refuse,
                    );
                    return 0;
                }
                let n = match times_column.map(|i| parse_times(&row[i])) {
                    None => 1,
                    Some(Ok(n)) => n,
                    Some(Err(e)) => {
                        raise(Some(r), None, IssueKind::BadTimes(e), Severity::Refuse);
                        0
                    }
                };
                if n == 0 && times_column.is_some_and(|i| parse_times(&row[i]).is_ok()) {
                    raise(Some(r), None, IssueKind::Skipped, Severity::Note);
                }
                n
            })
            .collect(),
    };
    if times.iter().filter(|&&n| n > 0).count() > 1 {
        for el in label.drawable() {
            let unpinned = match el {
                Element::Qr(q) => q.version.is_none(),
                Element::DataMatrix(d) => !matches!(d.size, DmSize::Fixed { .. }),
                _ => false,
            };
            if unpinned && varying.contains(&el.common().id) {
                raise(
                    None,
                    Some(el.common().id),
                    IssueKind::SizeUnpinned,
                    Severity::Warn,
                );
            }
        }
    }

    // 3. One render bound only to the clock, for everything no row can change.
    match bind(label, &Bindings::clock_only(now)) {
        Err(e) => {
            raise(None, e.element(), IssueKind::Bind(e), Severity::Refuse);
            stop = true;
        }
        Ok(bound) => {
            let (_, r) = rasterise(&bound, target)?;
            for id in r.off_canvas {
                // The canvas would clip a symbol's bars, so it is refused;
                // anything else is drawn as far as the edge.
                let severity = if symbols.contains(&id) {
                    Severity::Refuse
                } else {
                    Severity::Warn
                };
                raise(None, Some(id), IssueKind::OffCanvas, severity);
            }
            for (id, k) in r.keepout {
                // The same rule `OffCanvas` uses, and the table is `Keepout`'s
                // own: a symbol whose dots the label will not carry refuses the
                // run, and the tail and the inset only ever warn.
                let severity = k.severity(symbols.contains(&id));
                raise(None, Some(id), IssueKind::Keepout(k), severity);
            }
            for id in r.rotation_approximated {
                raise(
                    None,
                    Some(id),
                    IssueKind::RotationApproximated,
                    Severity::Warn,
                );
            }
            for (id, stored, drawn) in r.image_resampled {
                raise(
                    None,
                    Some(id),
                    IssueKind::ImageResampled { stored, drawn },
                    Severity::Note,
                );
            }
            for id in r.image_clipped {
                raise(None, Some(id), IssueKind::ImageClipped, Severity::Warn);
            }
            for id in r.zone_shared {
                raise(None, Some(id), IssueKind::ZoneShared, Severity::Note);
            }
            for kind in r.unimplemented {
                raise(None, None, IssueKind::Unimplemented(kind), Severity::Refuse);
            }
            for kind in r.unrecognised {
                raise(None, None, IssueKind::Unrecognised(kind), Severity::Refuse);
            }
            for (id, f) in r.failed {
                if !varying.contains(&id) {
                    raise(None, Some(id), IssueKind::Failed(f), Severity::Refuse);
                }
            }
            for (id, c) in r.text_clipped {
                if !varying.contains(&id) {
                    raise(None, Some(id), IssueKind::TextClipped(c), Severity::Warn);
                }
            }
        }
    }

    // 4 and 5. The elements that take a column, drawn for every row that
    // prints. Hiding the rest keeps their ids and changes none of these
    // results: each element is drawn from its own frame and data alone.
    if let Some(t) = data
        && !stop
        && !varying.is_empty()
    {
        let mut only = label.clone();
        for el in &mut only.elements {
            if let ElementOrRaw::Known(el) = el
                && !varying.contains(&el.common().id)
            {
                set_hidden(el);
            }
        }
        for (r, &n) in times.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let Some(b) = t.bindings(r, now) else {
                continue;
            };
            let bound = match bind(&only, &b) {
                Ok(bound) => bound,
                Err(e) => {
                    raise(Some(r), e.element(), IssueKind::Bind(e), Severity::Refuse);
                    continue;
                }
            };
            let (_, report) = rasterise(&bound, target)?;
            for (id, f) in report.failed {
                raise(Some(r), Some(id), IssueKind::Failed(f), Severity::Refuse);
            }
            for (id, c) in report.text_clipped {
                raise(Some(r), Some(id), IssueKind::TextClipped(c), Severity::Warn);
            }
        }
    }

    // 6. Row-wide first, then by row; within one, in document order.
    let order: HashMap<ElementId, usize> = label
        .elements
        .iter()
        .filter_map(ElementOrRaw::known)
        .enumerate()
        .map(|(i, e)| (e.common().id, i))
        .collect();
    issues.sort_by_key(|i| (i.row, i.element.map(|id| order.get(&id).copied())));
    Ok(Preflight { times, issues })
}

fn set_hidden(el: &mut Element) {
    let c = match el {
        Element::Text(e) => &mut e.common,
        Element::Barcode(e) => &mut e.common,
        Element::Qr(e) => &mut e.common,
        Element::DataMatrix(e) => &mut e.common,
        Element::Image(e) => &mut e.common,
        Element::Rect(e) => &mut e.common,
        Element::Line(e) => &mut e.common,
    };
    c.hidden = true;
}
