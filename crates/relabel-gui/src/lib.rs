//! The relabel desktop application.
//!
//! Two panes: the label as dots, with a frame round every element, placement and
//! size by pointer or by keyboard, and a bar to add and delete elements; and beside
//! it the selected element's fields. Above them, a bar to make, open and save a
//! label. The document lives in Rust; the `.slint` side holds flat models of rows
//! and slots and no rules at all.

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
pub mod preview;
pub mod rows;
pub mod view;

use canvas::Canvas;
use choose::{Chooser, NativeChooser};
use doc::Doc;
use filing::Filing;
use fresh::NewSpec;
use rows::FileHost;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use view::Opening;

slint::include_modules!();

impl_canvas_host!(AppWindow, ElRow);
impl_ins_host!(AppWindow, InsSlot);
impl_view_host!(AppWindow);
impl_edit_host!(AppWindow, Tool, Grips);
impl_file_host!(AppWindow, SizePreset);

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
/// With no path, a new label at the registry's default size for a D110. With a
/// path, that label; or, when the path names a file that does not exist in a
/// folder that does, a new label whose file it becomes, so Control+S writes it
/// there with no dialog - which is also how a label is saved where no file dialog
/// can be shown. Any other failure opens the new label untitled, and the note
/// says why the file did not open: a release build on Windows has no console to
/// say it on (main.rs, `windows_subsystem`).
pub fn start(path: Option<&Path>) -> Started {
    let blank = || {
        let spec = NewSpec::stock(fresh::default_printer());
        let doc = Doc::open(fresh::build(&spec), spec.spec.direction)
            .expect("the registry's own D110 label opens");
        // The printer is assumed: nothing is open to take it from, and nothing is
        // connected.
        let note = format!(
            "{} (assumed: relabel is not connected to a printer): {}; Control+N makes one of \
             another size",
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
pub struct Wired<H: FileHost> {
    pub canvas: Rc<RefCell<Canvas<H>>>,
    pub filing: Rc<RefCell<Filing>>,
}

/// Wire every pane of a window to the document `started` holds: the canvas, the
/// fields, the edit bar, the view and the files, in that order. The application and
/// every file test call this, so both run the one wiring.
pub fn wire_window<H: FileHost>(
    host: &H,
    started: Started,
    chooser: Rc<dyn Chooser>,
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
    let filing = file::wire(host, &canvas, viewer, chooser, Filing::new(path));
    host.set_status(note.as_str().into());
    // So the window opens with the keyboard in the canvas: F6 is a capture, and
    // the capture list is built from the focused item's ancestor chain, which is
    // empty when nothing has focus.
    host.invoke_focus_canvas();
    Wired { canvas, filing }
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
    wire_window(&ui, started, chooser, Opening::Fit);
    ui.run()?;
    Ok(())
}
