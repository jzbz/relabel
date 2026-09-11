//! Binding a label to one row of data.
//!
//! Every `{column}` and `{dt}` in a label's text, barcode, QR and DataMatrix
//! data is resolved against a row and a clock, and the result is a new label:
//! the one being edited is never touched, so "preview row 37" is one call,
//!
//! ```ignore
//! let bound = bind(&label, &table.bindings(36, now).unwrap())?;
//! ```
//!
//! and printing is the same call for each row. A barcode whose whole `data` is
//! `{sku}` is a bound field; there is no second mechanism.
//!
//! # niimblue's rules, and the three it breaks
//!
//! The grammar is niimblue's ([`crate::template`]), and so is the order a
//! placeholder resolves in: a column of that name first - its cell as it is,
//! any `|filter` ignored, an empty cell blanking the placeholder - then `dt`,
//! formatted by [`format_dt`], and otherwise left exactly as written. A column
//! called `dt` therefore wins over the clock, even when its cell is empty.
//!
//! Three things niimblue does are not copied, because each prints the wrong
//! label without a word:
//!
//! - Only the table's own column names resolve. niimblue's lookup walks the
//!   JavaScript prototype chain, so `{constructor}` printed a function's
//!   source code.
//! - A row must have exactly as many fields as the header. niimblue pads a
//!   short row and drops extra fields, and an extra field usually means a
//!   comma that should have been quoted.
//! - `$times` is a whole number from 0 to [`MAX_TIMES`], or empty for 1
//!   ([`parse_times`]). niimblue's `parseInt` printed three labels for `3abc`,
//!   sixteen for `0x10`, none at all for `abc`, and hung on a twenty-digit
//!   number.
//!
//! Each bound field is capped at [`MAX_FIELD_BYTES`], checked before anything
//! is added, so a template that references a large cell many times cannot
//! allocate without bound.

use crate::datetime::{CivilTime, format_dt};
use crate::document::{Element, ElementId, ElementOrRaw, Label};
use crate::template::{self, ExpandError, Placeholder, is_js_space};
use std::borrow::Cow;
use std::collections::HashMap;

/// The longest a bound field may be, in bytes: more than text's 10 000
/// characters at four bytes each, and more than any QR code holds.
pub const MAX_FIELD_BYTES: usize = 65_536;

/// The most copies one row may ask for with `$times`.
pub const MAX_TIMES: u32 = 10_000;

/// Rows under a header, as the caller read them. relabel-render reads no
/// files and parses no CSV: the editor and the command line do, and hand
/// over this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// Row `row`, counted from 0 below the header, with the job's clock.
    pub fn bindings(&self, row: usize, now: CivilTime) -> Option<Bindings<'_>> {
        self.rows.get(row).map(|r| Bindings {
            columns: &self.columns,
            row: r,
            now,
        })
    }
}

/// What a label is bound against: one row, its header, and the time.
#[derive(Debug, Clone, Copy)]
pub struct Bindings<'a> {
    pub columns: &'a [String],
    pub row: &'a [String],
    pub now: CivilTime,
}

impl Bindings<'static> {
    /// No data, only the clock: what niimblue does with its CSV switched off.
    pub fn clock_only(now: CivilTime) -> Bindings<'static> {
        Bindings {
            columns: &[],
            row: &[],
            now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindError {
    #[error("the row has {cells} fields but the header has {columns}")]
    RowLength { cells: usize, columns: usize },
    #[error("{0:?} names more than one column")]
    AmbiguousColumn(String),
    #[error("element {element} would be longer than {limit} bytes once bound")]
    FieldTooLong { element: ElementId, limit: usize },
}

impl BindError {
    /// The element the error is about, where there is one.
    pub fn element(&self) -> Option<ElementId> {
        match self {
            BindError::FieldTooLong { element, .. } => Some(*element),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("$times is {value:?}; it must be a whole number from 0 to 10000, or empty for 1")]
pub struct TimesError {
    pub value: String,
}

/// How many copies a row's `$times` cell asks for: empty - after trimming the
/// same spaces the placeholder grammar skips - means 1, and otherwise ASCII
/// digits from 0 to [`MAX_TIMES`]. 0 is a deliberate skip.
pub fn parse_times(cell: &str) -> Result<u32, TimesError> {
    let t = cell.trim_matches(is_js_space);
    if t.is_empty() {
        return Ok(1);
    }
    let refuse = || TimesError {
        value: cell.to_string(),
    };
    let mut n: u32 = 0;
    for b in t.bytes() {
        if !b.is_ascii_digit() {
            return Err(refuse());
        }
        n = n * 10 + u32::from(b - b'0');
        // Checked as each digit arrives, so a long run of digits stops early
        // and the arithmetic cannot overflow.
        if n > MAX_TIMES {
            return Err(refuse());
        }
    }
    Ok(n)
}

/// Whether the label as drawn prints the clock: a shown element has a `{dt}`
/// that no `dt` column answers. Only then is the time worth asking for.
pub fn uses_clock(label: &Label, columns: &[String]) -> bool {
    if columns.iter().any(|c| c == "dt") {
        return false;
    }
    label.drawable().any(|el| {
        let field = match el {
            Element::Text(e) => &e.text,
            Element::Barcode(e) => &e.data,
            Element::Qr(e) => &e.data,
            Element::DataMatrix(e) => &e.data,
            Element::Image(_) | Element::Rect(_) | Element::Line(_) => return false,
        };
        template::placeholders(field).any(|p| p.key == "dt")
    })
}

/// Each column name, with its index - or `None` where the name is used twice,
/// which is an error only if a placeholder asks for it.
fn column_index(columns: &[String]) -> HashMap<&str, Option<usize>> {
    let mut index = HashMap::with_capacity(columns.len());
    for (i, c) in columns.iter().enumerate() {
        index
            .entry(c.as_str())
            .and_modify(|v: &mut Option<usize>| *v = None)
            .or_insert(Some(i));
    }
    index
}

/// The label with every template in `b`'s row and clock resolved.
///
/// Pure: the input is cloned, not changed. Text, barcode, QR and DataMatrix
/// data are the only fields touched - never names, titles, image paths or
/// anything a later version wrote - and hidden elements are bound too, so the
/// result is the same whatever is shown.
pub fn bind(label: &Label, b: &Bindings<'_>) -> Result<Label, BindError> {
    if b.row.len() != b.columns.len() {
        return Err(BindError::RowLength {
            cells: b.row.len(),
            columns: b.columns.len(),
        });
    }
    let index = column_index(b.columns);
    let resolve = |p: &Placeholder<'_>| -> Result<Option<Cow<'_, str>>, BindError> {
        match index.get(p.key) {
            Some(Some(i)) => Ok(Some(Cow::Borrowed(b.row[*i].as_str()))),
            Some(None) => Err(BindError::AmbiguousColumn(p.key.to_string())),
            None if p.key == "dt" => Ok(Some(Cow::Owned(format_dt(&b.now, p.filter)))),
            None => Ok(None),
        }
    };
    let mut out = label.clone();
    for el in &mut out.elements {
        let ElementOrRaw::Known(el) = el else {
            continue;
        };
        let id = el.common().id;
        let field = match el {
            Element::Text(e) => &mut e.text,
            Element::Barcode(e) => &mut e.data,
            Element::Qr(e) => &mut e.data,
            Element::DataMatrix(e) => &mut e.data,
            Element::Image(_) | Element::Rect(_) | Element::Line(_) => continue,
        };
        match template::expand(field, MAX_FIELD_BYTES, resolve) {
            Ok(Some(s)) => *field = s,
            Ok(None) => {}
            Err(ExpandError::TooLong) => {
                return Err(BindError::FieldTooLong {
                    element: id,
                    limit: MAX_FIELD_BYTES,
                });
            }
            Err(ExpandError::Resolve(e)) => return Err(e),
        }
    }
    Ok(out)
}
