//! Artwork from a file: what the document is told to size it for, the read and
//! the decode, what comes back ready to become an image element's, and the seam
//! a test stands in for.
//!
//! No toolkit type in any signature, for `geom`'s reason. [`crate::doc::Doc`]
//! decides what a new or re-imported image is sized for ([`Sizing`]) before any
//! file is read, and takes a [`Ready`] when the decode is done; nothing between
//! the two borrows the document, so the read and the decode can run anywhere.
//!
//! # Off the event loop
//!
//! A 40-megapixel photograph takes about a third of a second to decode in a
//! release build and many seconds in a debug one, and a file on a slow mount can
//! take any time at all to read. So [`ThreadImporter`], the one [`Importer`] the
//! application uses, runs [`run`] on a thread of its own, and hands the answer
//! back through a one-shot polled by `slint::spawn_local` on the event loop,
//! which is `plug`'s pattern: Slint posts the worker's wake back to the loop
//! (i-slint-core `future.rs:60-86`), so no Send closure touches the editor's
//! state and no Slint handle is upgraded off the UI thread. Tests stand in their
//! own importers, which answer at once or when told to.
//!
//! One worker at a time, whatever the caller does: a decode peaks at about 266 MiB
//! for a 40-megapixel JPEG, plus up to 64 MiB of file, and two at once would be
//! twice that. `filing` refuses a second import while an answer is owed, even for
//! one it has let go; [`ThreadImporter`] refuses to start a second worker too.
//!
//! # What is read
//!
//! [`read_capped`] asks the file's metadata before it opens anything, so a folder,
//! a FIFO or a device is refused unopened and a file over
//! [`relabel_render::artwork::MAX_INPUT`] is refused by its length alone; what it
//! does read is capped as it is read, for a file that grows in between. The
//! decode is `relabel_render::artwork`'s, which sniffs the bytes and never trusts
//! the file's name, and carries its own limits on sides, pixels and memory. A
//! panic inside it comes back as a sentence ([`guarded`]).

use relabel_render::artwork::{
    self, ArtworkError, Imported, LINE_ART_THRESHOLD, MAX_INPUT, SourceFormat, SvgRefusal,
};
use relabel_render::document::{ImageFit, ImageHalftone};
use relabel_render::target::Dpmm;
use std::any::Any;
use std::cell::Cell;
use std::fs::File;
use std::future::Future;
use std::io::{self, Read};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

/// What the artwork is to be sized for, decided by the document before the file
/// is chosen and again before it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sizing {
    /// A new image: at its own size, one source pixel a dot and an SVG at its
    /// physical size, shrunk to fit each bounded side and never enlarged
    /// ([`relabel_render::artwork::import_within`]). The bounds are the usable
    /// area less a millimetre each side ([`crate::make::import_box`]).
    Within(Option<u32>, Option<u32>),
    /// An image imported again: for its frame as its content sees it
    /// ([`relabel_render::rasterise::local_size`]) and the fit it keeps
    /// ([`relabel_render::artwork::import`]).
    Frame { upright: (u32, u32), fit: ImageFit },
}

/// A decoded file, and its artwork as the element stores it: the base64 is made
/// where the decode ran, so landing it costs the document one string move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub imported: Imported,
    /// [`Imported::stored`] of `imported`.
    pub png: String,
}

/// One file to import: which file, what for, and at what pitch an SVG is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub path: PathBuf,
    pub sizing: Sizing,
    /// The label's own, never a connected printer's.
    pub dpmm: Dpmm,
}

/// Read `path` whole, or say why not, in words that follow "could not import
/// NAME: ".
///
/// The metadata is asked first, and anything but a regular file is refused before
/// it is opened: opening a FIFO waits for a writer, and a device such as
/// `/dev/zero` never ends. A length over [`MAX_INPUT`] is refused from the
/// metadata alone, and the read itself stops one byte past it, for a file that
/// grows between the two.
pub fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| unread(&e))?;
    if !meta.is_file() {
        return Err("it is not a regular file".into());
    }
    let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
    if len > MAX_INPUT {
        return Err(ArtworkError::InputTooLarge { bytes: len }.to_string());
    }
    let file = File::open(path).map_err(|e| unread(&e))?;
    read_at_most(file, MAX_INPUT, len)
}

/// Read `r` to its end, or stop one byte past `cap` and refuse it: what is in
/// memory never passes `cap + 1` bytes, however long the source turns out to be.
/// `expected` is the length the metadata gave, reserved up front.
fn read_at_most(r: impl Read, cap: usize, expected: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(expected.min(cap));
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    r.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| unread(&e))?;
    if bytes.len() > cap {
        return Err(ArtworkError::InputTooLarge { bytes: bytes.len() }.to_string());
    }
    Ok(bytes)
}

/// A read that failed, said as `disk` says one.
fn unread(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "it does not exist".into(),
        _ => e.to_string(),
    }
}

/// Decode `bytes` for `sizing` and make the base64 the element stores, or say why
/// not. `name` is the file's name, which only the wording of a refusal reads: the
/// decode sniffs the bytes and never trusts the name.
///
/// A new SVG too many dots for an SVG to be drawn into is shrunk into them,
/// keeping its shape: at its own size it can fit a large label at 300 dpi and
/// still be over [`artwork::SVG_MAX_DOTS`], and a new image has no frame yet
/// that a smaller one would contradict. The refusal is asked before anything is
/// drawn, so the second attempt costs one more parse.
pub fn prepare(bytes: &[u8], name: &str, sizing: &Sizing, dpmm: Dpmm) -> Result<Ready, String> {
    let imported = match sizing {
        Sizing::Within(w, h) => match artwork::import_within(bytes, (*w, *h), dpmm) {
            Err(ArtworkError::Svg(SvgRefusal::TooManyDots { w, h })) => {
                let (w, h) = within_budget(w, h);
                artwork::import_within(bytes, (Some(w), Some(h)), dpmm)
            }
            imported => imported,
        },
        Sizing::Frame { upright, fit } => artwork::import(bytes, *upright, fit, dpmm),
    }
    .map_err(|e| refusal(&e, name, bytes))?;
    let png = imported.stored().map_err(|e| e.to_string())?;
    Ok(Ready { imported, png })
}

/// `w` by `h` scaled down, keeping its shape, to at most
/// [`artwork::SVG_MAX_DOTS`] dots.
fn within_budget(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (u64::from(w), u64::from(h));
    let scale = (artwork::SVG_MAX_DOTS as f64 / (w * h) as f64).sqrt();
    let (mut w, mut h) = (
        ((w as f64 * scale) as u64).max(1),
        ((h as f64 * scale) as u64).max(1),
    );
    // The rounding down above can still leave it a row or so over.
    while w * h > artwork::SVG_MAX_DOTS {
        if w >= h {
            w -= 1;
        } else {
            h -= 1;
        }
    }
    (
        u32::try_from(w).unwrap_or(u32::MAX),
        u32::try_from(h).unwrap_or(u32::MAX),
    )
}

/// The whole of one import, as a worker runs it: the read, then the decode.
pub fn run(job: &Job) -> Result<Ready, String> {
    let bytes = read_capped(&job.path)?;
    prepare(&bytes, &crumb_of(&job.path), &job.sizing, job.dpmm)
}

/// An import refused, in the renderer's own words, except where those words would
/// mislead. Markup in a file not named `.svg` was never meant as an SVG, so the
/// XML parser's complaint about it is no help: a web page saved with an image's
/// name is the usual case. Likewise gzip data not named `.svg`, `.svgz` or
/// `.svg.gz`, which the renderer takes for a compressed SVG from its first two
/// bytes alone; so named, it most likely is one, and saving it uncompressed is
/// the step that works.
/// And a HEIC or JPEG XL photo, which a phone or a camera writes by default, gets
/// the one step that works.
pub fn refusal(e: &ArtworkError, name: &str, bytes: &[u8]) -> String {
    let named_svg = Path::new(name)
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("svg"));
    let lower = name.to_ascii_lowercase();
    let named_svgz = lower.ends_with(".svgz") || lower.ends_with(".svg.gz");
    match e {
        ArtworkError::Svg(SvgRefusal::Compressed) if !named_svg && !named_svgz => {
            "it is compressed data, and not an image this build can read: PNG, JPEG, GIF, BMP, \
             WebP or SVG"
                .into()
        }
        ArtworkError::Svg(SvgRefusal::Xml(_) | SvgRefusal::Parse(_) | SvgRefusal::NotUtf8)
            if !named_svg =>
        {
            "it starts like a web page or other markup, and is not an image this build can \
             read: PNG, JPEG, GIF, BMP, WebP or SVG"
                .into()
        }
        ArtworkError::NotAnImage if is_heif(bytes) => {
            format!("{e}; a HEIC photo has to be exported as JPEG or PNG first")
        }
        ArtworkError::NotAnImage if is_jxl(bytes) => {
            format!("{e}; a JPEG XL image has to be exported as PNG or JPEG first")
        }
        _ => e.to_string(),
    }
}

/// The brands an ISO-BMFF file names when it holds HEIC or HEIF stills.
const HEIF_BRANDS: [&[u8]; 9] = [
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"heif", b"mif1", b"msf1",
];

fn is_heif(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp".as_slice())
        && bytes
            .get(8..12)
            .is_some_and(|brand| HEIF_BRANDS.contains(&brand))
}

/// A bare JPEG XL codestream, or one in its container.
fn is_jxl(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0x0A]) || bytes.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n")
}

/// `f`, with a panic inside it turned into the sentence it becomes: an import
/// that panics has failed, and the window goes on. Nothing `f` touched is looked
/// at afterwards - a worker's closure owns everything it reads - so no broken
/// state can be seen, which is what makes asserting unwind safety sound.
pub fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| {
        Err(format!(
            "the importer stopped unexpectedly: {}",
            panic_message(p.as_ref())
        ))
    })
}

fn panic_message(p: &(dyn Any + Send)) -> &str {
    p.downcast_ref::<&str>()
        .copied()
        .or_else(|| p.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("it gave no reason")
}

/// What a [`Receiver`] resolves to when its [`Sender`] went without sending: a
/// worker that ended some way other than by answering.
pub const NO_ANSWER: &str = "the importer stopped without an answer";

/// The one-shot's shared state. `closed` is set once the sender has gone, by
/// sending or by being dropped, so the receiver is woken exactly once.
struct Slot<T> {
    value: Option<Result<T, String>>,
    waker: Option<Waker>,
    closed: bool,
}

/// The worker's end of a one-shot: sends one answer, from any thread.
pub struct Sender<T> {
    slot: Arc<Mutex<Slot<T>>>,
}

/// The loop's end of a one-shot: a future that resolves to the answer, or to
/// [`NO_ANSWER`] when the sender went without one.
pub struct Receiver<T> {
    slot: Arc<Mutex<Slot<T>>>,
}

/// A channel for one answer from a worker thread to a future on the event loop,
/// with std alone: a slot and a waker behind one lock.
pub fn one_shot<T>() -> (Sender<T>, Receiver<T>) {
    let slot = Arc::new(Mutex::new(Slot {
        value: None,
        waker: None,
        closed: false,
    }));
    (Sender { slot: slot.clone() }, Receiver { slot })
}

/// Nothing panics while the lock is held, and the state is whole between any two
/// of its statements, so a poisoned lock is taken as it stands.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<T> Sender<T> {
    pub fn send(self, value: Result<T, String>) {
        let waker = {
            let mut slot = lock(&self.slot);
            slot.value = Some(value);
            slot.closed = true;
            slot.waker.take()
        };
        // Woken with the lock released: the wake may poll at once.
        if let Some(w) = waker {
            w.wake();
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut slot = lock(&self.slot);
            if slot.closed {
                return;
            }
            slot.closed = true;
            slot.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, String>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut slot = lock(&self.slot);
        if let Some(value) = slot.value.take() {
            return Poll::Ready(value);
        }
        if slot.closed {
            return Poll::Ready(Err(NO_ANSWER.into()));
        }
        slot.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Called once with the import's answer, on the UI thread.
pub type Done = Box<dyn FnOnce(Result<Box<Ready>, String>)>;

/// Something that reads and decodes a file somewhere the window does not wait on.
pub trait Importer {
    /// Start `job`. `Ok` means `done` will be called exactly once, later or at
    /// once, so a caller holds no borrow across this; `Err` means it never will
    /// be, and nothing was started.
    fn start(&self, job: Job, done: Done) -> Result<(), NoWorker>;
}

/// Why an import could not even be started.
#[derive(Debug, thiserror::Error)]
pub enum NoWorker {
    #[error("the importer could not start: {0}")]
    Spawn(String),
    #[error("the importer could not start: another import is still running")]
    Busy,
}

/// The application's importer: each job on a thread of its own, named
/// `relabel-import`, and the answer delivered on the event loop. See the module
/// note.
pub struct ThreadImporter {
    run: fn(&Job) -> Result<Ready, String>,
    /// A worker's answer is owed: set when one starts, and cleared on the event
    /// loop when its answer, or its silence, arrives. A worker that hangs keeps it
    /// set, and so keeps every later import refused rather than stacked beside it.
    busy: Rc<Cell<bool>>,
}

impl Default for ThreadImporter {
    fn default() -> ThreadImporter {
        ThreadImporter::new()
    }
}

impl ThreadImporter {
    /// [`run`] on each job.
    pub fn new() -> ThreadImporter {
        ThreadImporter::with_run(run)
    }

    /// Any function on each job: one that panics, for the test that runs this
    /// importer on a real event loop.
    pub fn with_run(run: fn(&Job) -> Result<Ready, String>) -> ThreadImporter {
        ThreadImporter {
            run,
            busy: Rc::new(Cell::new(false)),
        }
    }
}

impl Importer for ThreadImporter {
    fn start(&self, job: Job, done: Done) -> Result<(), NoWorker> {
        if self.busy.get() {
            return Err(NoWorker::Busy);
        }
        let (tx, rx) = one_shot();
        // The listener first: without it no answer could arrive, and a worker
        // started anyway would decode for nothing.
        let busy = self.busy.clone();
        slint::spawn_local(async move {
            let result = rx.await;
            busy.set(false);
            done(result);
        })
        .map_err(|e| NoWorker::Spawn(e.to_string()))?;
        self.busy.set(true);
        // Held here until the worker has it, so a thread that cannot start still
        // answers, in its own words, through the listener already waiting.
        let handoff = Arc::new(Mutex::new(Some(tx)));
        let theirs = handoff.clone();
        let run = self.run;
        let spawned = std::thread::Builder::new()
            .name("relabel-import".into())
            .spawn(move || {
                let tx = lock(&theirs).take();
                let result = guarded(|| run(&job).map(Box::new));
                if let Some(tx) = tx {
                    tx.send(result);
                }
            });
        if let Err(e) = spawned
            && let Some(tx) = lock(&handoff).take()
        {
            tx.send(Err(NoWorker::Spawn(e.to_string()).to_string()));
        }
        Ok(())
    }
}

/// What an image element records of where its artwork came from: the chosen
/// file's name and nothing of its folder. Nothing ever opens it; a name tells a
/// person which file it was, and leaks no user name or folder layout into a label
/// that is passed on. Empty for a path with no name, which no dialog returns.
pub fn crumb_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What the file was, for the sentence an import is said in: "1200 by 400 pixels
/// of PNG, reduced", or "an SVG of 240 by 80 dots". Resampling is named when there
/// was any; a raster one pixel to a dot, cropped or not, needs no word, because the
/// size in dots is said just before it and a crop, by fit dots or by fit cover, has
/// a note of its own ([`relabel_render::artwork::ImportNote`]).
pub fn source_says(i: &Imported) -> String {
    let (sw, sh) = i.source_px;
    let (w, h) = (i.art.w, i.art.h);
    let how = if (w, h) == (sw, sh) || (i.source != SourceFormat::Svg && !i.resampled) {
        ""
    } else if w <= sw && h <= sh {
        ", reduced"
    } else if w >= sw && h >= sh {
        ", enlarged"
    } else {
        ", resampled to another shape"
    };
    match i.source {
        SourceFormat::Svg => format!("an SVG of {sw} by {sh} dots{how}"),
        f => format!("{sw} by {sh} pixels of {}{how}", format_name(f)),
    }
}

fn format_name(f: SourceFormat) -> &'static str {
    match f {
        SourceFormat::Png => "PNG",
        SourceFormat::Jpeg => "JPEG",
        SourceFormat::Gif => "GIF",
        SourceFormat::Bmp => "BMP",
        SourceFormat::WebP => "WebP",
        SourceFormat::Svg => "SVG",
    }
}

/// The halftone and threshold import chose, spelled as the inspector spells them,
/// and why - the rule in `relabel_render::artwork`'s module note, said back:
/// "threshold at 187, because it is line art that was resampled", and for line art
/// shrunk so far that strokes are thinner than a dot, which import draws into its
/// grey whole and one dot wide (`Imported::joined`), "..., and its strokes shrunk
/// thinner than a dot are drawn whole, one dot wide".
pub fn chosen_says(i: &Imported) -> String {
    let chose = settings_says(&i.halftone, i.threshold);
    let why = match (&i.halftone, i.threshold) {
        (ImageHalftone::FloydSteinberg, _) => "it is continuous tone",
        (ImageHalftone::Threshold, LINE_ART_THRESHOLD) if i.resampled && i.joined > 0 => {
            "it is line art that was resampled, and its strokes shrunk thinner than a dot are \
             drawn whole, one dot wide"
        }
        (ImageHalftone::Threshold, LINE_ART_THRESHOLD) if i.resampled => {
            "it is line art that was resampled"
        }
        (ImageHalftone::Threshold, LINE_ART_THRESHOLD) => "it is line art with soft edges",
        (ImageHalftone::Threshold, _) if i.source == SourceFormat::Svg => "it is line art",
        (ImageHalftone::Threshold, _) => "it is line art at one pixel to a dot",
        _ => return chose,
    };
    format!("{chose}, because {why}")
}

/// A halftone and the threshold it works at: "floyd_steinberg at 128".
pub fn settings_says(halftone: &ImageHalftone, threshold: u8) -> String {
    format!("{} at {threshold}", crate::ins::halftone_spelling(halftone))
}

/// Artwork for the document's tests, made in code: an image file's bytes, and
/// what a finished import of them hands the document.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::{Ready, Sizing};
    use relabel_render::artwork::{Artwork, Imported, SourceFormat};
    use relabel_render::document::ImageHalftone;
    use relabel_render::target::Dpmm;

    /// Grey dots from a function of the dot.
    pub fn art(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Artwork {
        Artwork {
            w,
            h,
            px: (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| f(x, y))
                .collect(),
        }
    }

    /// A grey PNG file.
    pub fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        art(w, h, f).to_png().expect("encoded")
    }

    /// Black bars on white: line art, with no grey at all.
    pub fn bars(x: u32, y: u32) -> u8 {
        if x % 40 < 8 || y % 40 < 8 { 0 } else { 255 }
    }

    /// The verify fixture's logo on white: 600 pixels square, drawn only in
    /// strokes 2 pixels wide - two rings, a square, a cross and twelve spokes - so
    /// shrunk into 80 dots no dot of it is more than about a third ink.
    pub fn thin_logo(x: u32, y: u32) -> u8 {
        let (dx, dy) = (f64::from(x) + 0.5 - 300.0, f64::from(y) + 0.5 - 300.0);
        let r = (dx * dx + dy * dy).sqrt();
        let rings = (258.0..260.0).contains(&r) || (178.0..180.0).contains(&r);
        let square = (200..402).contains(&x)
            && (200..402).contains(&y)
            && !((202..400).contains(&x) && (202..400).contains(&y));
        let cross = ((299..301).contains(&y) && (40..560).contains(&x))
            || ((299..301).contains(&x) && (40..560).contains(&y));
        let spokes = (0..12).any(|k| {
            let (s, c) = (f64::from(k) * std::f64::consts::PI / 6.0).sin_cos();
            (dy * c - dx * s).abs() < 1.0 && (180.0..260.0).contains(&(dx * c + dy * s))
        });
        if rings || square || cross || spokes {
            0
        } else {
            255
        }
    }

    /// The thin logo with a disc of pale grey 225 in its top left corner, clear
    /// of every stroke and a hundredth of the drawing: a fill that holds no ink.
    pub fn thin_logo_beside_pale_disc(x: u32, y: u32) -> u8 {
        let (dx, dy) = (f64::from(x) + 0.5 - 40.0, f64::from(y) + 0.5 - 40.0);
        if dx * dx + dy * dy < 34.0 * 34.0 {
            225
        } else {
            thin_logo(x, y)
        }
    }

    /// `bytes` imported as the application imports them for `sizing`.
    pub fn ready(bytes: &[u8], sizing: &Sizing, dpmm: Dpmm) -> Ready {
        super::prepare(bytes, "art.png", sizing, dpmm).expect("imported")
    }

    /// Artwork as an element stores it, exactly these dots.
    pub fn stored(a: Artwork) -> String {
        Imported {
            source_px: (a.w, a.h),
            art: a,
            source: SourceFormat::Png,
            halftone: ImageHalftone::Threshold,
            threshold: 128,
            resampled: false,
            joined: 0,
            notes: Vec::new(),
        }
        .stored()
        .expect("stored")
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    /// On a large label at 300 dpi an SVG that fits the label at its own size can
    /// be more dots than an SVG may be drawn into. A new one is shrunk into them,
    /// keeping its shape, and said to be reduced, as a PNG would be.
    #[test]
    fn a_new_svg_over_the_dot_budget_is_shrunk_into_it() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="70mm"><rect width="100%" height="100%"/></svg>"#;
        let within = Sizing::Within(Some(685), Some(1157));
        let r = prepare(svg.as_bytes(), "plan.svg", &within, Dpmm::DPI_300).expect("imported");
        let (w, h) = (r.imported.art.w, r.imported.art.h);
        assert_eq!(r.imported.source_px, (591, 827));
        assert!(
            u64::from(w) * u64::from(h) <= artwork::SVG_MAX_DOTS,
            "{w}x{h}"
        );
        assert!(
            w >= 430 && h >= 600,
            "no smaller than it has to be: {w}x{h}"
        );
        let shape = f64::from(w) / f64::from(h) - 591.0 / 827.0;
        assert!(shape.abs() < 0.005, "{w}x{h}");
        assert!(
            r.imported.art.px.iter().all(|&v| v == 0),
            "the whole drawing"
        );
        assert_eq!(
            source_says(&r.imported),
            "an SVG of 591 by 827 dots, reduced"
        );
        // A frame is the operator's own size, and is not shrunk behind their back.
        let frame = Sizing::Frame {
            upright: (591, 827),
            fit: ImageFit::Contain,
        };
        assert_eq!(
            prepare(svg.as_bytes(), "plan.svg", &frame, Dpmm::DPI_300).map(|_| ()),
            Err("a 591x827 SVG raster is too large; SVG artwork is limited to 262144 dots".into())
        );
    }

    /// A source longer than the metadata said - a file that grew after it was
    /// asked - is read one byte past the cap and no further, and refused.
    #[test]
    fn a_read_stops_one_byte_past_the_cap() {
        /// Zeros, `left` of them, counting how many were handed out.
        struct Zeros {
            left: usize,
            read: usize,
        }
        impl Read for Zeros {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = buf.len().min(self.left);
                buf[..n].fill(0);
                self.left -= n;
                self.read += n;
                Ok(n)
            }
        }
        let mut long = Zeros {
            left: 1000,
            read: 0,
        };
        assert_eq!(
            read_at_most(&mut long, 10, 4),
            Err(ArtworkError::InputTooLarge { bytes: 11 }.to_string())
        );
        assert_eq!(long.read, 11, "read no further than one byte past");
        let mut short = Zeros { left: 10, read: 0 };
        assert_eq!(read_at_most(&mut short, 10, 10), Ok(vec![0; 10]));
    }

    #[test]
    fn the_crumb_is_the_file_name_and_nothing_of_its_folder() {
        assert_eq!(
            crumb_of(Path::new("/home/someone/art/logo.png")),
            "logo.png"
        );
        assert_eq!(crumb_of(Path::new("logo.png")), "logo.png");
        assert_eq!(crumb_of(Path::new("/")), "");
    }

    /// The sentence names the file's own size and kind, and resampling only when
    /// there was some.
    #[test]
    fn the_source_is_said_with_its_size_and_whether_it_was_resampled() {
        let within = Sizing::Within(Some(304), Some(80));
        let big = ready(&png(1200, 400, bars), &within, Dpmm::DPI_203);
        assert_eq!(
            source_says(&big.imported),
            "1200 by 400 pixels of PNG, reduced"
        );
        assert_eq!(
            chosen_says(&big.imported),
            "threshold at 187, because it is line art that was resampled"
        );
        let small = ready(&png(64, 64, bars), &within, Dpmm::DPI_203);
        assert_eq!(source_says(&small.imported), "64 by 64 pixels of PNG");
        assert_eq!(
            chosen_says(&small.imported),
            "threshold at 128, because it is line art at one pixel to a dot"
        );
        let photo = ready(
            &png(64, 64, |x, y| (x * 2 + y) as u8),
            &within,
            Dpmm::DPI_203,
        );
        assert_eq!(
            chosen_says(&photo.imported),
            "floyd_steinberg at 128, because it is continuous tone"
        );
        let up = Sizing::Frame {
            upright: (96, 96),
            fit: ImageFit::Contain,
        };
        let enlarged = ready(&png(32, 32, bars), &up, Dpmm::DPI_203);
        assert_eq!(
            source_says(&enlarged.imported),
            "32 by 32 pixels of PNG, enlarged"
        );
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="30mm" height="10mm"><rect width="10mm" height="10mm"/></svg>"#;
        let drawn = ready(svg, &within, Dpmm::DPI_203);
        assert_eq!(source_says(&drawn.imported), "an SVG of 240 by 80 dots");
        assert_eq!(
            chosen_says(&drawn.imported),
            "threshold at 128, because it is line art"
        );
    }

    /// A logo whose strokes end up thinner than a dot keeps 187, and the sentence
    /// says its strokes were drawn whole and one dot wide, so a label that prints
    /// more ink than its source is never a mystery.
    #[test]
    fn a_shrunk_thin_stroked_logo_says_its_strokes_are_drawn_whole_one_dot_wide() {
        let within = Sizing::Within(Some(304), Some(80));
        let r = ready(&png(600, 600, thin_logo), &within, Dpmm::DPI_203);
        assert_eq!(r.imported.threshold, LINE_ART_THRESHOLD);
        assert_eq!(
            chosen_says(&r.imported),
            "threshold at 187, because it is line art that was resampled, and its strokes \
             shrunk thinner than a dot are drawn whole, one dot wide"
        );
    }

    /// A pale grey fill beside the thin logo holds none of its ink: the sentence is
    /// the same, the logo prints exactly the dots it prints alone, and the fill
    /// keeps its grey.
    #[test]
    fn a_pale_fill_beside_thin_strokes_changes_neither_the_sentence_nor_the_strokes() {
        let within = Sizing::Within(Some(304), Some(80));
        let plain = ready(&png(600, 600, thin_logo), &within, Dpmm::DPI_203);
        let r = ready(
            &png(600, 600, thin_logo_beside_pale_disc),
            &within,
            Dpmm::DPI_203,
        );
        assert_eq!(
            chosen_says(&r.imported),
            "threshold at 187, because it is line art that was resampled, and its strokes \
             shrunk thinner than a dot are drawn whole, one dot wide"
        );
        let prints = |px: &[u8]| {
            px.iter()
                .map(|&v| v <= LINE_ART_THRESHOLD)
                .collect::<Vec<_>>()
        };
        assert!(
            prints(&r.imported.art.px) == prints(&plain.imported.art.px),
            "the printed dots moved"
        );
        assert_eq!(
            r.imported.art.px[5 * 80 + 5],
            225,
            "the disc keeps its grey"
        );
    }

    /// A texture shrunk below a dot has no stroke to draw, and the sentence says
    /// nothing of one: a checkerboard of single pixels, a 1-bit dither of one
    /// pixel in a hundred, and a dense one of three pixels in ten shrunk only two
    /// times, whose clumps reach several dots, are line art that was resampled and
    /// no more.
    #[test]
    fn a_shrunk_texture_says_nothing_of_strokes_drawn_whole() {
        let within = Sizing::Within(Some(304), Some(80));
        let checker = |x: u32, y: u32| if (x + y).is_multiple_of(2) { 0 } else { 255 };
        // Scattered by a fixed mix of the pixel's index: `per_mille` thousandths
        // ink.
        let scattered = |per_mille: u64| {
            move |x: u32, y: u32| {
                let mut z = u64::from(y * 600 + x).wrapping_add(0x9e37_79b9_7f4a_7c15);
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                if (z ^ (z >> 31)) % 1000 < per_mille {
                    0
                } else {
                    255
                }
            }
        };
        for (bytes, what) in [
            (png(600, 600, checker), "a checkerboard"),
            (png(600, 600, scattered(10)), "a dither"),
            (png(160, 160, scattered(300)), "a dense dither"),
        ] {
            let r = ready(&bytes, &within, Dpmm::DPI_203);
            assert_eq!(r.imported.joined, 0, "{what}");
            assert_eq!(
                chosen_says(&r.imported),
                "threshold at 187, because it is line art that was resampled",
                "{what}"
            );
        }
    }

    /// A new image lands one source pixel to a dot, shrunk into the box and never
    /// enlarged, and an SVG at its own physical size; the header of what is stored
    /// says the same size.
    #[test]
    fn a_new_image_is_one_pixel_a_dot_and_never_enlarged() {
        let within = Sizing::Within(Some(304), Some(80));
        let big =
            prepare(&png(1200, 400, bars), "big.png", &within, Dpmm::DPI_203).expect("imported");
        assert_eq!((big.imported.art.w, big.imported.art.h), (240, 80));
        assert!(big.imported.resampled);
        assert_eq!(
            (&big.imported.halftone, big.imported.threshold),
            (&ImageHalftone::Threshold, LINE_ART_THRESHOLD)
        );
        let small =
            prepare(&png(64, 64, bars), "small.png", &within, Dpmm::DPI_203).expect("imported");
        assert_eq!((small.imported.art.w, small.imported.art.h), (64, 64));
        assert!(!small.imported.resampled);
        assert_eq!(
            (&small.imported.halftone, small.imported.threshold),
            (&ImageHalftone::Threshold, 128)
        );
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="30mm" height="10mm"><rect width="10mm" height="10mm"/></svg>"#;
        let drawn = prepare(svg, "logo.svg", &within, Dpmm::DPI_203).expect("imported");
        assert_eq!((drawn.imported.art.w, drawn.imported.art.h), (240, 80));
        assert_eq!(drawn.imported.source, SourceFormat::Svg);
        for r in [&big, &small, &drawn] {
            assert_eq!(
                artwork::stored_size(&r.png),
                Some((r.imported.art.w, r.imported.art.h))
            );
        }
    }

    /// Imported again, artwork is made for the frame and the fit the element keeps,
    /// so drawing it resamples nothing.
    #[test]
    fn a_reimport_is_sized_for_its_frame_and_fit() {
        let sizing = Sizing::Frame {
            upright: (320, 96),
            fit: ImageFit::Contain,
        };
        let r = prepare(&png(32, 32, bars), "logo.png", &sizing, Dpmm::DPI_203).expect("imported");
        assert_eq!((r.imported.art.w, r.imported.art.h), (96, 96));
        let placed = artwork::fit_box((96, 96), (320, 96), &ImageFit::Contain)
            .expect("it lands in the frame");
        assert_eq!((placed.bw, placed.bh), (96, 96));

        // A cover that only crops resamples nothing, and still says what it cut.
        let cover = Sizing::Frame {
            upright: (100, 100),
            fit: ImageFit::Cover,
        };
        let r = prepare(&png(400, 100, bars), "wide.png", &cover, Dpmm::DPI_203).expect("imported");
        assert_eq!(
            (r.imported.art.w, r.imported.art.h, r.imported.resampled),
            (100, 100, false)
        );
        assert_eq!(source_says(&r.imported), "400 by 100 pixels of PNG");
        assert_eq!(
            r.imported
                .notes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["to cover its frame it was cropped from 400x100 to the middle 100x100"]
        );

        let unknown = Sizing::Frame {
            upright: (320, 96),
            fit: ImageFit::Unknown("tile".into()),
        };
        let e = prepare(&png(32, 32, bars), "logo.png", &unknown, Dpmm::DPI_203)
            .expect_err("no fit this build knows");
        assert_eq!(
            e,
            ArtworkError::UnknownValue {
                what: "the fit",
                value: "tile".into()
            }
            .to_string()
        );
    }

    /// A web page saved under an image's name is said to be one; the XML parser's
    /// complaint is kept only for a file that set out to be an SVG.
    #[test]
    fn markup_not_named_svg_is_said_to_be_no_image() {
        let page = b"<!DOCTYPE html>\n<html><head><title>x</title></head><body><p>hi</body></html>";
        let within = Sizing::Within(Some(304), Some(80));
        let e = prepare(page, "page.png", &within, Dpmm::DPI_203).expect_err("not an image");
        assert!(e.contains("starts like a web page"), "{e}");
        assert!(!e.contains("well-formed"), "{e}");
        let e = prepare(page, "page.SVG", &within, Dpmm::DPI_203).expect_err("not an image");
        assert!(e.contains("the SVG is not well-formed XML"), "{e}");
    }

    /// Anything that starts like gzip is taken for a compressed SVG by the
    /// renderer; only a file named as one is told to save it uncompressed.
    #[test]
    fn compressed_data_not_named_as_an_svg_is_said_to_be_no_image() {
        let gz = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
        let within = Sizing::Within(Some(304), Some(80));
        let e = prepare(&gz, "archive.png", &within, Dpmm::DPI_203).expect_err("unread");
        assert_eq!(
            e,
            "it is compressed data, and not an image this build can read: PNG, JPEG, GIF, \
             BMP, WebP or SVG"
        );
        // A gzipped SVG saved as `.svg` is a compressed SVG too, and saving it
        // uncompressed is the step that works.
        for name in ["plan.svgz", "plan.SVG.gz", "plan.svg"] {
            let e = prepare(&gz, name, &within, Dpmm::DPI_203).expect_err("unread");
            assert_eq!(e, SvgRefusal::Compressed.to_string(), "{name}");
        }
    }

    /// The formats a phone writes by default are named, with the one step that
    /// works.
    #[test]
    fn a_heic_photo_is_told_to_be_exported() {
        let within = Sizing::Within(Some(304), Some(80));
        let mut heic = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic".to_vec();
        heic.resize(64, 0);
        let e = prepare(&heic, "IMG_0001.HEIC", &within, Dpmm::DPI_203).expect_err("unread");
        assert!(e.contains("not an image this build can read"), "{e}");
        assert!(e.contains("HEIC"), "{e}");
        let mut jxl = b"\0\0\0\x0cJXL \r\n\x87\n".to_vec();
        jxl.resize(64, 0);
        let e = prepare(&jxl, "photo.jxl", &within, Dpmm::DPI_203).expect_err("unread");
        assert!(e.contains("not an image this build can read"), "{e}");
        assert!(e.contains("JPEG XL"), "{e}");
        // Anything else unreadable keeps the renderer's words alone.
        let e = prepare(b"GIF87 no", "x.gif", &within, Dpmm::DPI_203).expect_err("unread");
        assert_eq!(e, ArtworkError::NotAnImage.to_string());
    }

    #[test]
    fn a_panic_while_importing_is_a_sentence() {
        let e = guarded::<()>(|| panic!("boom")).expect_err("a panic is an error");
        assert!(e.contains("the importer stopped unexpectedly"), "{e}");
        assert!(e.contains("boom"), "{e}");
        let e = guarded::<()>(|| panic!("{} bytes", 3)).expect_err("a panic is an error");
        assert!(e.ends_with("3 bytes"), "{e}");
        assert_eq!(guarded(|| Ok(7)), Ok(7));
        assert_eq!(guarded::<()>(|| Err("no".into())), Err("no".into()));
    }

    /// Counts its wakes.
    #[derive(Default)]
    struct Counted(AtomicUsize);

    impl Wake for Counted {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn the_one_shot_wakes_once_from_another_thread() {
        let (tx, mut rx) = one_shot::<u32>();
        let counted = Arc::new(Counted::default());
        let waker = Waker::from(counted.clone());
        let mut cx = Context::from_waker(&waker);
        assert_eq!(Pin::new(&mut rx).poll(&mut cx), Poll::Pending);
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            tx.send(Ok(7));
        });
        worker.join().expect("the worker ends");
        assert_eq!(counted.0.load(Ordering::SeqCst), 1);
        assert_eq!(Pin::new(&mut rx).poll(&mut cx), Poll::Ready(Ok(7)));
        assert_eq!(counted.0.load(Ordering::SeqCst), 1, "woken once only");
    }

    #[test]
    fn a_sender_dropped_unsent_is_an_answer() {
        let (tx, mut rx) = one_shot::<u32>();
        let counted = Arc::new(Counted::default());
        let waker = Waker::from(counted.clone());
        let mut cx = Context::from_waker(&waker);
        assert_eq!(Pin::new(&mut rx).poll(&mut cx), Poll::Pending);
        drop(tx);
        assert_eq!(counted.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            Pin::new(&mut rx).poll(&mut cx),
            Poll::Ready(Err(NO_ANSWER.to_string()))
        );
        assert_eq!(NO_ANSWER, "the importer stopped without an answer");
        // Dropped before it was ever polled, it answers the same at the first poll.
        let (tx, mut rx) = one_shot::<u32>();
        drop(tx);
        assert_eq!(
            Pin::new(&mut rx).poll(&mut cx),
            Poll::Ready(Err(NO_ANSWER.to_string()))
        );
    }
}
