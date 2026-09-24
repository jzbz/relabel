//! The relabel desktop application.
//!
//! Two panes: the label as dots, with a frame round every element, placement and
//! size by pointer or by keyboard, and a bar to add and delete elements; and beside
//! it the selected element's fields. Above them, a bar to make, open, save and
//! preview a label, and a line saying which printer is connected. The Preview pane
//! shows what would print and connects to a printer; nothing prints yet. The
//! document lives in Rust; the `.slint` side holds flat models of rows and slots
//! and no rules at all.

pub mod a11y;
pub mod canvas;
pub mod choose;
pub mod disk;
pub mod doc;
pub mod edit;
pub mod file;
pub mod filing;
pub mod fresh;
pub mod geom;
pub mod ins;
pub mod make;
pub mod plug;
pub mod preview;
pub mod print;
pub mod printing;
pub mod proof;
pub mod rows;
pub mod view;

use canvas::Canvas;
use choose::{Chooser, NativeChooser};
use doc::Doc;
use filing::Filing;
use fresh::NewSpec;
use plug::{NativePlug, Plug};
use rows::PrintHost;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use view::Opening;

slint::include_modules!();

impl_canvas_host!(AppWindow, ElRow);
impl_ins_host!(AppWindow, InsSlot);
impl_view_host!(AppWindow);
impl_edit_host!(AppWindow, Tool, Grips);
impl_file_host!(AppWindow, SizePreset);
impl_print_host!(AppWindow, IssueRow, ChoiceRow);

/// What the window opens on: the document, its file when it has one, and the
/// status line's first sentence.
pub struct Started {
    pub doc: Doc,
    /// Absolute, so the dialogs start in its folder whatever the working directory.
    pub path: Option<PathBuf>,
    pub note: String,
}

/// The document the window opens on, from the command line's path or from none.
///
/// With no path, a new label at the registry's default size for a D110_M. With a
/// path, that label; or, when the path names a file that does not exist in a
/// folder that does, a new label whose file it becomes, so Control+S writes it
/// there with no dialog - which is also how a label is saved where no file dialog
/// can be shown. Any other failure opens the new label untitled, and the note
/// says why the file did not open: a release build on Windows has no console to
/// say it on (main.rs, `windows_subsystem`).
pub fn start(path: Option<&Path>) -> Started {
    let blank = || {
        // The printer is assumed: nothing is open to take it from, and nothing is
        // connected. Said in `made_says`'s words, which Create in the New label
        // pane uses too.
        let spec = NewSpec {
            assumed: Some(fresh::NOT_CONNECTED),
            ..NewSpec::stock(fresh::default_printer())
        };
        let doc = Doc::open(fresh::build(&spec), spec.spec.direction)
            .expect("the registry's own D110_M label opens");
        let note = format!(
            "{}: {}; Control+N makes one of another size",
            fresh::made_says(&spec),
            doc.opening_note()
        );
        (doc, note)
    };
    let Some(path) = path else {
        let (doc, note) = blank();
        return Started {
            doc,
            path: None,
            note,
        };
    };
    let whole = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let name = whole
        .file_name()
        .unwrap_or(whole.as_os_str())
        .to_string_lossy()
        .into_owned();
    let why = match disk::read_label(&whole) {
        Ok(label) => match Doc::open(label, relabel_proto::PrintDirection::Left) {
            Ok(doc) => {
                return Started {
                    note: format!("opened {name}: {}", doc.opening_note()),
                    doc,
                    path: Some(whole),
                };
            }
            Err(e) => e.to_string(),
        },
        Err(disk::ReadError::Missing) if whole.parent().is_some_and(Path::is_dir) => {
            let (doc, note) = blank();
            return Started {
                doc,
                path: Some(whole),
                note: format!("{name} does not exist yet, and Control+S saves it: {note}"),
            };
        }
        Err(e) => e.to_string(),
    };
    let (doc, note) = blank();
    Started {
        doc,
        path: None,
        note: format!("could not open {name}: {why}; {note}"),
    }
}

/// The document's state, as [`wire_window`] hands it back for a caller to read.
pub struct Wired<H: PrintHost> {
    pub canvas: Rc<RefCell<Canvas<H>>>,
    pub filing: Rc<RefCell<Filing>>,
    pub prints: Rc<print::Prints<H>>,
}

/// Wire every pane of a window to the document `started` holds: the canvas, the
/// fields, the edit bar, the view, the printer and the files, in that order. The
/// application and every file and print test call this, so all of them run the one
/// wiring.
///
/// `plug` carries orders to the printer link, and nothing is sent through it until
/// Connect is pressed.
pub fn wire_window<H: PrintHost>(
    host: &H,
    started: Started,
    chooser: Rc<dyn Chooser>,
    plug: Rc<dyn Plug>,
    opening: Opening,
) -> Wired<H> {
    let Started { doc, path, note } = started;
    let canvas = canvas::wire(host, doc);
    ins::wire(host, &canvas);
    edit::wire(host, &canvas);
    // The application opens fitted, and stays fitted until a zoom is chosen: the
    // window manager's first sizes are provisional, and a fit taken once here would
    // be stale.
    let viewer = view::wire(host, &canvas, opening);
    // One machine for the file commands and the preview, which share the popups'
    // one place (filing.rs); the printer before the files, which open its pane.
    let filing = Rc::new(RefCell::new(Filing::new(path)));
    let connected = Rc::new(Cell::new(None));
    let prints = print::wire(host, &canvas, &filing, plug, connected.clone());
    let preview: Rc<dyn Fn(&H)> = {
        let prints = prints.clone();
        Rc::new(move |host: &H| prints.open(host))
    };
    file::wire(
        host,
        &canvas,
        viewer,
        chooser,
        filing.clone(),
        preview,
        connected,
    );
    host.set_status(note.as_str().into());
    // So the window opens with the keyboard in the canvas: F6 is a capture, and
    // the capture list is built from the focused item's ancestor chain, which is
    // empty when nothing has focus.
    host.invoke_focus_canvas();
    Wired {
        canvas,
        filing,
        prints,
    }
}

/// Open the window on the label at `path`, or on a new one, and run it until it
/// closes.
pub fn run_app(path: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let started = start(path);
    if path.is_some() && started.path.is_none() {
        eprintln!("relabel: {}", started.note);
    }
    let ui = AppWindow::new()?;
    let chooser = Rc::new(NativeChooser::new(&ui));
    let plug = Rc::new(NativePlug::new());
    wire_window(&ui, started, chooser, plug.clone(), Opening::Fit);
    let ran = ui.run();
    // Whatever the loop ended with, the printer link is closed before the process
    // goes: a scan stopped, a connect abandoned, a session closed. Five seconds
    // covers each of those at its own cap (relabel-core `link`), and past it the
    // process leaves anyway rather than hang with no window.
    plug.finish(Duration::from_secs(5));
    ran?;
    Ok(())
}
