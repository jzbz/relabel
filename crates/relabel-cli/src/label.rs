//! `relabel-cli label`: a `.relabel` file bound to each row of its data,
//! checked in full, then printed one label at a time.
//!
//! Every row is bound and drawn before anything connects, and a row that would
//! print a wrong or missing mark stops the run there, with its row, its line in
//! the CSV and its element named. Printing is one job per label: the job
//! engine's deadline covers a whole job, so one job of five hundred labels
//! would time out where five hundred jobs of one do not.

use anyhow::{Context, Result, anyhow, bail};
use relabel_core::Session;
use relabel_core::job::{PrintOptions, print};
use relabel_core::printer::Identity;
use relabel_core::table::read_csv;
use relabel_proto::image::EncodedPage;
use relabel_proto::{HeadSpec, LabelType, PrintDirection, encode_page};
use relabel_render::bind::{Bindings, Table, bind, uses_clock};
use relabel_render::datetime::CivilTime;
use relabel_render::document::{DataBinding, KnownBinding, Label};
use relabel_render::preflight::{Issue, Preflight, Severity, preflight};
use relabel_render::{Dpmm, Target, rasterise_plane};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::bitmap;

/// The largest CSV file read: far more rows than any run prints, and a bound
/// on what a mistaken path can load.
const MAX_CSV_BYTES: u64 = 64 << 20;

/// What the command line asked for.
pub struct Opts<'a> {
    pub file: &'a Path,
    pub csv: Option<&'a Path>,
    pub now: Option<&'a str>,
    pub copies: u16,
    pub check: bool,
    pub dry_run: bool,
    pub head: Option<u16>,
    pub direction: PrintDirection,
    pub density: Option<u8>,
    pub preview: bool,
}

/// A label, its rows and its clock, checked and ready to print.
pub struct Prepared {
    label: Label,
    rows: Option<Rows>,
    now: CivilTime,
    target: Target,
    checked: Preflight,
    copies: u16,
    density: Option<u8>,
    preview: bool,
}

/// The data, and the line of the CSV each row starts on.
struct Rows {
    table: Table,
    lines: Vec<u32>,
}

/// Loads and checks the label. `None` when `--check` or `--dry-run` has
/// already done everything asked.
pub fn prepare(o: &Opts<'_>) -> Result<Option<Prepared>> {
    if o.copies == 0 {
        bail!("--copies 0 would print nothing");
    }
    let json =
        std::fs::read_to_string(o.file).with_context(|| format!("reading {}", o.file.display()))?;
    let label = Label::from_json(&json).with_context(|| format!("loading {}", o.file.display()))?;
    let rows = read_rows(&label, o.file, o.csv)?;
    let columns = rows.as_ref().map_or(&[][..], |r| &r.table.columns[..]);
    let now = match o.now {
        Some(s) => parse_now(s)?,
        None if uses_clock(&label, columns) => bail!(
            "the label prints the time with {{dt}}, and this command has no time zone to \
             read it in: pass it, as --now \"$(date -Iseconds)\""
        ),
        // Never printed: nothing shown reads it.
        None => CivilTime::new(1970, 1, 1, 0, 0, 0, 0, 0).expect("the epoch is a real time"),
    };
    let (feed, head) = (label.canvas.feed, label.canvas.head);
    let (Ok(feed), Ok(label_head)) = (u16::try_from(feed), u16::try_from(head)) else {
        bail!("the label's canvas is {feed} by {head} dots, which no printer takes");
    };
    let dpi = [203, 300]
        .into_iter()
        .find(|&d| Dpmm::for_dpi(d) == label.canvas.dpmm)
        .with_context(|| {
            format!(
                "the label is drawn at {}/{} dots per millimetre, which no printer uses",
                label.canvas.dpmm.num, label.canvas.dpmm.den
            )
        })?;
    // Checked against its own canvas until a printer says otherwise; a head
    // given on the command line is checked against as it is.
    let target = Target::for_label(o.head.unwrap_or(label_head), feed, dpi, o.direction)?;
    let mut p = Prepared {
        label,
        rows,
        now,
        target,
        checked: Preflight::default(),
        copies: o.copies,
        density: o.density,
        preview: o.preview,
    };
    p.checked = p.check()?;
    if o.check {
        println!("{}; nothing refused.", p.counted());
        return Ok(None);
    }
    if o.dry_run {
        p.dry_run()?;
        return Ok(None);
    }
    // Nothing to print is not a reason to interrupt whatever the printer is
    // doing, so it is said here rather than after connecting.
    if p.labels() == 0 {
        bail!("nothing would print: every row asks for no copies");
    }
    Ok(Some(p))
}

/// The CSV named by `--csv`, or else by the label's own binding, beside the
/// label. `--csv` replaces only the path: the binding's delimiter and header
/// still apply.
fn read_rows(label: &Label, file: &Path, csv: Option<&Path>) -> Result<Option<Rows>> {
    let (delimiter, has_header, named) = match &label.data {
        None => (',', true, None),
        Some(DataBinding::Known(KnownBinding::CsvFile(f))) => {
            (f.delimiter, f.has_header, Some(beside(file, &f.path)?))
        }
        Some(DataBinding::Unknown(_)) if csv.is_none() => bail!(
            "the label's data was set up by a later version of relabel, which is needed to \
             print it; --csv gives it rows instead"
        ),
        Some(DataBinding::Unknown(_)) => (',', true, None),
    };
    let Some(path) = csv.map(Path::to_path_buf).or(named) else {
        return Ok(None);
    };
    let bytes = read_capped(&path).with_context(|| format!("reading {}", path.display()))?;
    let t = read_csv(&bytes, delimiter, has_header)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(Rows {
        table: Table {
            columns: t.columns,
            rows: t.rows,
        },
        lines: t.lines,
    }))
}

/// The label's own data file, resolved beside it. A binding travels with the
/// label, so it may only name a file in the label's own directory: a path that
/// is absolute, or that climbs out with `..`, would make opening a label
/// someone sent read a file of their choosing. `--csv` is the way to name a
/// file anywhere.
fn beside(label: &Path, path: &str) -> Result<PathBuf> {
    if !Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        bail!("the label's data file {path:?} is not beside the label; --csv reads one elsewhere");
    }
    Ok(label.parent().unwrap_or(Path::new("")).join(path))
}

/// A file, refused past [`MAX_CSV_BYTES`]. The length is counted as the bytes
/// arrive rather than asked for first, because a pipe or a character device
/// reports no length at all and would otherwise be read until memory ran out.
fn read_capped(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_CSV_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CSV_BYTES {
        bail!("it is longer than {MAX_CSV_BYTES} bytes, so it is not a label run");
    }
    Ok(bytes)
}

impl Prepared {
    /// Runs preflight against the current target and prints what it found.
    fn check(&self) -> Result<Preflight> {
        let p = preflight(
            &self.label,
            self.rows.as_ref().map(|r| &r.table),
            &self.target,
            self.now,
        )?;
        for i in &p.issues {
            println!("{}", describe(i, self.rows.as_ref().map(|r| &r.lines[..])));
        }
        if p.refused() {
            bail!("nothing was printed: fix what is refused above first");
        }
        Ok(p)
    }

    fn labels(&self) -> u64 {
        self.checked.pages() * u64::from(self.copies)
    }

    /// "6 labels from 3 rows", or "1 label" for a label with no data.
    fn counted(&self) -> String {
        let n = self.labels();
        let labels = format!("{n} {}", if n == 1 { "label" } else { "labels" });
        match &self.rows {
            Some(r) => {
                let rows = r.table.rows.len();
                let s = if rows == 1 { "row" } else { "rows" };
                format!("{labels} from {rows} {s}")
            }
            None => labels,
        }
    }

    /// Row `row` bound, drawn and encoded for `spec`, with a preview of it
    /// printed when one was asked for.
    fn page(&self, row: usize, spec: &HeadSpec, preview: bool) -> Result<EncodedPage> {
        let bound = match &self.rows {
            Some(r) => bind(
                &self.label,
                &r.table
                    .bindings(row, self.now)
                    .context("a row preflight counted is missing")?,
            )?,
            None => bind(&self.label, &Bindings::clock_only(self.now))?,
        };
        let plane = rasterise_plane(&bound, &self.target)?;
        if preview {
            println!("{}", bitmap::preview(&plane, 120));
        }
        Ok(encode_page(&plane, spec)?)
    }

    /// The rows that print, with how many labels each prints.
    fn printing(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        let copies = u64::from(self.copies);
        self.checked
            .times
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(move |(r, &n)| (r, u64::from(n) * copies))
    }

    fn dry_run(&self) -> Result<()> {
        let spec = HeadSpec::new(self.target.head_axis(), self.target.direction);
        let (mut frames, mut bytes) = (0u64, 0u64);
        let mut preview = self.preview;
        for (r, n) in self.printing() {
            let page = self.page(r, &spec, std::mem::take(&mut preview))?;
            frames += page.frames.len() as u64 * n;
            bytes += page.wire_bytes() as u64 * n;
        }
        println!("{}; nothing refused.", self.counted());
        println!("{frames} frames and {bytes} bytes on the wire in all.");
        println!(
            "(dry run: assumed a {} dot head, direction {:?})",
            self.target.head_axis(),
            self.target.direction
        );
        Ok(())
    }

    /// Prints every label on a connected printer, one job each.
    pub async fn print(mut self, session: &Session, id: &Identity) -> Result<()> {
        let model = id.model;
        let head = id.printhead_px();
        if head == 0 {
            bail!(
                "this printer's head width is unknown, so the label cannot be checked against it"
            );
        }
        let direction = model.map(|m| m.direction).unwrap_or(PrintDirection::Top);
        let dpi = model.map(|m| m.dpi).unwrap_or(203);
        let target = Target::for_label(head, self.target.feed_axis(), dpi, direction)?;
        if target != self.target {
            println!("Checking again for this printer's {head} dot head at {dpi} dpi.");
            self.target = target;
            self.checked = self.check()?;
        }
        let spec = HeadSpec::new(head, direction);
        let opts = PrintOptions {
            density: self.density.or(model.map(|m| m.density.2)).unwrap_or(2),
            label_type: model
                .and_then(|m| m.label_types.first().copied())
                .unwrap_or(LabelType::Gap),
            copies: 1,
            ..PrintOptions::default()
        }
        .for_model(id.model_id);
        println!(
            "Printing {} at density {} on {}, flow {:?}.",
            self.counted(),
            opts.density,
            opts.label_type.label(),
            id.flow()
        );
        let total = self.labels();
        let mut done = 0u64;
        let mut preview = self.preview;
        for (r, n) in self.printing() {
            let page = self.page(r, &spec, std::mem::take(&mut preview))?;
            for _ in 0..n {
                done += 1;
                match &self.rows {
                    Some(rows) => println!("  label {done} of {total}: row {}", rows.at(r)),
                    None => println!("  label {done} of {total}"),
                }
                print(
                    session,
                    id.flow(),
                    std::slice::from_ref(&page),
                    &opts,
                    |_| {},
                )
                .await
                .with_context(|| format!("label {done} of {total}"))?;
            }
        }
        println!("Done.");
        Ok(())
    }
}

impl Rows {
    /// A row as a person finds it: counted from 1, with its line in the file.
    fn at(&self, row: usize) -> String {
        at(row, Some(&self.lines))
    }
}

fn at(row: usize, lines: Option<&[u32]>) -> String {
    match lines.and_then(|l| l.get(row)) {
        Some(line) => format!("{} (line {line})", row + 1),
        None => format!("{}", row + 1),
    }
}

/// `row 3 (line 4), element 2 "SKU barcode": the message [refused]`.
fn describe(i: &Issue, lines: Option<&[u32]>) -> String {
    let mut place = Vec::new();
    if let Some(r) = i.row {
        place.push(format!("row {}", at(r, lines)));
    }
    if let Some(e) = i.element {
        place.push(if i.name.is_empty() {
            format!("element {e}")
        } else {
            format!("element {e} {:?}", i.name)
        });
    }
    let severity = match i.severity {
        Severity::Note => "note",
        Severity::Warn => "warning",
        Severity::Refuse => "refused",
    };
    if place.is_empty() {
        format!("{} [{severity}]", i.kind)
    } else {
        format!("{}: {} [{severity}]", place.join(", "), i.kind)
    }
}

/// RFC 3339, as `date -Iseconds` writes it - `2026-09-11T16:05:09+02:00` -
/// with optional fractional seconds, and `Z` for UTC. The offset is required:
/// it decides which date the label prints.
pub fn parse_now(s: &str) -> Result<CivilTime> {
    let bad = || anyhow!("--now {s:?} is not an RFC 3339 time such as 2026-09-11T16:05:09+02:00");
    let b = s.as_bytes();
    let num = |from: usize, to: usize| -> Result<u16> {
        match b.get(from..to) {
            Some(d) if d.iter().all(u8::is_ascii_digit) => {
                Ok(d.iter().fold(0, |n, c| n * 10 + u16::from(c - b'0')))
            }
            _ => Err(bad()),
        }
    };
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return Err(bad());
    }
    let year = num(0, 4)?;
    let [month, day, hour, minute, second] = [
        num(5, 7)?,
        num(8, 10)?,
        num(11, 13)?,
        num(14, 16)?,
        num(17, 19)?,
    ]
    .map(|n| n as u8);
    let mut i = 19;
    let mut milli = 0;
    if b[i] == b'.' {
        let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return Err(bad());
        }
        // Truncated to milliseconds, the finest a label can show.
        milli = b[i + 1..i + 1 + digits]
            .iter()
            .chain(std::iter::repeat(&b'0'))
            .take(3)
            .fold(0, |n, c| n * 10 + u16::from(c - b'0'));
        i += 1 + digits;
    }
    let offset: i16 = match &b[i..] {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let (h, m) = (num(i + 1, i + 3)?, num(i + 4, i + 6)?);
            if m >= 60 {
                return Err(bad());
            }
            let minutes = (h * 60 + m) as i16;
            if *sign == b'-' { -minutes } else { minutes }
        }
        _ => return Err(bad()),
    };
    CivilTime::new(year, month, day, hour, minute, second, milli, offset)
        .map_err(|e| anyhow!("--now {s:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::parse_now;
    use relabel_render::datetime::CivilTime;

    #[test]
    fn now_reads_what_date_writes() {
        assert_eq!(
            parse_now("2026-09-11T16:05:09+02:00").unwrap(),
            CivilTime::new(2026, 9, 11, 16, 5, 9, 0, 120).unwrap()
        );
        assert_eq!(
            parse_now("2025-01-05 00:07:03.0071-01:30").unwrap(),
            CivilTime::new(2025, 1, 5, 0, 7, 3, 7, -90).unwrap()
        );
        assert_eq!(
            parse_now("2026-09-11t16:05:09.5z").unwrap(),
            CivilTime::new(2026, 9, 11, 16, 5, 9, 500, 0).unwrap()
        );
    }

    #[test]
    fn now_needs_its_offset_and_a_real_time() {
        for bad in [
            "2026-09-11T16:05:09",
            "2026-09-11T16:05:09+0200",
            "2026-09-11T16:05:09.+02:00",
            "2026-09-11T16:05+02:00",
            "2026-9-11T16:05:09+02:00",
            "2026-02-30T16:05:09+02:00",
            "2026-09-11T24:00:00Z",
            "2026-09-11T16:05:09+24:00",
            "2026-09-11T16:05:09+02:60",
            "2026-09-11T16:05:09+02:00 ",
            "2026-09-11\u{00e9}16:05:09+02:00",
            "",
        ] {
            assert!(parse_now(bad).is_err(), "{bad:?}");
        }
    }
}
