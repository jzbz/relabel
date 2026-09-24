//! The file commands' wiring: the File bar, the chords, the question about unsaved
//! work, the New label pane, opening the Preview pane and the window's close
//! button, driven through [`Filing`].
//!
//! Every route is one call to `drive`, which does the same five things in order:
//! commit what a field is holding, unless an element is held; read the document
//! into a [`Now`]; apply the command; say the note; and do what [`Next`] says -
//! show the question or the pane, ask the chooser, write, read, make, or hide the
//! window - feeding a write's or an open's result back in as the next command.
//!
//! No `RefCell` is borrowed while the chooser is asked or a result is fed back:
//! a test's chooser answers at once, from inside `ask`, and that answer is another
//! `drive`. The keyboard goes to the canvas last of all, after a document has been
//! replaced, because the canvas surface commits a field when it gets the keyboard
//! and that borrows the canvas (canvas.slint `surface-focused`, ins.rs
//! `on_field_commit`).

use crate::canvas::{Canvas, PanePush, said_again};
use crate::choose::{Answer, Chooser, Picked};
use crate::disk;
use crate::doc::{Cmd, Doc};
use crate::filing::{self, FileCmd, Filing, Next, Now, Reply, Source, Want};
use crate::fresh::{self, NewSpec};
use crate::rows::{FileHost, PresetRow, ToolRow};
use crate::view::Viewer;
use relabel_proto::PrintDirection;
use relabel_render::document::{DataBinding, KnownBinding};
use slint::{CloseRequestResponse, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// Everything `drive` needs, shared by every callback.
struct Files<H: FileHost> {
    host: slint::Weak<H>,
    canvas: Rc<RefCell<Canvas<H>>>,
    viewer: Viewer<H>,
    chooser: Rc<dyn Chooser>,
    filing: Rc<RefCell<Filing>>,
    /// The file name the title shows, kept here for the canvas's push to read, so
    /// that push never borrows the `Filing`.
    name: Rc<RefCell<Option<String>>>,
    /// The printer the New label pane was opened for, which its answers are read
    /// against, and whether it was assumed, which the status line then says.
    printer: Cell<Option<fresh::Printer>>,
    /// Opens the Preview pane, which is `print`'s.
    preview: Rc<dyn Fn(&H)>,
    /// The connected printer, written by `print` and offered by the New label
    /// pane when a new label can be made for it.
    connected: Rc<Cell<Option<fresh::Connected>>>,
}

/// Wire the file commands to a window whose canvas, pane, edit bar, view and
/// printer are wired already.
///
/// `viewer` is the view's state from [`crate::view::wire`], which a document
/// opened in place of another starts afresh. `chooser` asks the operator for a
/// file: [`crate::choose::NativeChooser`] in the application, a script in a test.
/// `filing` is shared with the Preview pane, which closes through it; `preview`
/// opens that pane, and `connected` is the printer it has connected to.
pub fn wire<H: FileHost>(
    host: &H,
    canvas: &Rc<RefCell<Canvas<H>>>,
    viewer: Viewer<H>,
    chooser: Rc<dyn Chooser>,
    filing: Rc<RefCell<Filing>>,
    preview: Rc<dyn Fn(&H)>,
    connected: Rc<Cell<Option<fresh::Connected>>>,
) {
    let name = filing.borrow().name();
    let files = Rc::new(Files {
        host: host.as_weak(),
        canvas: canvas.clone(),
        viewer,
        chooser,
        name: Rc::new(RefCell::new(name)),
        filing,
        printer: Cell::new(None),
        preview,
        connected,
    });

    // Once, for `edit::wire`'s reason: the commands are this build's.
    let tools: Vec<H::Tool> = Want::BAR
        .into_iter()
        .filter_map(|w| {
            let token = w.token()?;
            Some(H::Tool::plain(
                token,
                w.caption(),
                w.button_name(),
                w.keys(),
            ))
        })
        .collect();
    host.set_file_tools(ModelRc::new(VecModel::from(tools)));

    // After every outcome: a nudge, an undo back to the saved state and a typed
    // digit all change whether there is unsaved work.
    let push: PanePush<H> = {
        let name = files.name.clone();
        Rc::new(move |host: &H, doc: &Doc| {
            let title = filing::title(name.borrow().as_deref(), doc.unsaved());
            host.set_doc_title(title.as_str().into());
        })
    };
    canvas.borrow_mut().tell_file(push);
    files.show_title(host);

    // Upgrade the weak handle or return, as every canvas callback does, then run
    // the body with the shared state and the window by the names given.
    macro_rules! on {
        ($setter:ident, |$files:ident, $host:ident $(, $arg:ident)*| $body:block) => {{
            let files = files.clone();
            host.$setter(move |$($arg),*| {
                let Some(host) = files.host.upgrade() else { return };
                let ($files, $host) = (&files, &host);
                $body
            });
        }};
    }

    on!(on_file_command, |files, host, token| {
        if let Some(w) = Want::from_token(token) {
            drive(files, host, FileCmd::Want(w));
        }
    });
    // Needs no document: which letters are chords is `Want`'s alone.
    host.on_file_of_key(|text, shift| {
        Want::of_key(&text, shift)
            .and_then(Want::token)
            .unwrap_or(-1)
    });
    on!(on_unsaved_reply, |files, host, token| {
        if let Some(r) = Reply::from_token(token) {
            drive(files, host, FileCmd::Reply(r));
        }
    });
    on!(on_new_feed_edited, |files, host, text| {
        if let Some(spec) = files.printer.get().map(|p| p.spec) {
            let note = match fresh::parse_feed(&text, spec) {
                Ok(tenths) => fresh::feed_says(tenths, spec),
                Err(why) => why,
            };
            host.set_new_note(note.as_str().into());
        }
    });
    {
        let files = files.clone();
        host.on_new_check(move |text| {
            let (Some(host), Some(spec)) =
                (files.host.upgrade(), files.printer.get().map(|p| p.spec))
            else {
                return false;
            };
            match fresh::parse_feed(&text, spec) {
                Ok(tenths) => {
                    host.set_new_note(fresh::feed_says(tenths, spec).as_str().into());
                    true
                }
                // Said again when it is said again, for `said_again`'s reason: the
                // note is the pane's live region.
                Err(why) => {
                    host.set_new_note(said_again(&host.get_new_note(), why).as_str().into());
                    false
                }
            }
        });
    }
    on!(on_new_make, |files, host, text| {
        // Checked by the pane before it closed; a length that fails here after all
        // starts nothing, and the machine says so.
        let spec = files.printer.get().and_then(|p| {
            let feed_tenths = fresh::parse_feed(&text, p.spec).ok()?;
            Some(NewSpec::of(&p, feed_tenths))
        });
        drive(files, host, FileCmd::Sized(spec));
    });
    on!(on_new_preset, |files, host, token| {
        // A token is a length the pane was offered, or nothing at all.
        let spec = files
            .printer
            .get()
            .filter(|p| fresh::presets(p.spec).contains(&token))
            .map(|p| NewSpec::of(&p, token));
        drive(files, host, FileCmd::Sized(spec));
    });
    on!(on_new_cancel, |files, host| {
        drive(files, host, FileCmd::Sized(None));
    });

    // The title bar's close, Alt+F4 and anything else the platform turns into a
    // close request. Only a clean document, or a close nothing is holding up,
    // answers HideWindow here; any other close is the question's, which hides the
    // window itself once it has been answered, after this handler has returned.
    // Nothing here dispatches another close request or sets a handler: the toolkit
    // has taken this handler out while it runs, so a nested request would be
    // answered HideWindow, and setting one panics (i-slint-core callbacks.rs:34-42).
    {
        let files = files.clone();
        host.window().on_close_requested(move || {
            let Some(host) = files.host.upgrade() else {
                return CloseRequestResponse::HideWindow;
            };
            match drive(&files, &host, FileCmd::Want(Want::Close)) {
                Next::Allow => CloseRequestResponse::HideWindow,
                _ => CloseRequestResponse::KeepWindowShown,
            }
        });
    }
}

/// One file command, start to finish. Returns what the command itself came to,
/// before anything it led to, which is what the close handler answers from.
fn drive<H: FileHost>(files: &Rc<Files<H>>, host: &H, cmd: FileCmd) -> Next {
    // Typing is work: a Save must write the digits in the field, and a close must
    // ask about them. Not while an element is held, where committing would write
    // inside the gesture; every file command is refused until the release anyway.
    {
        let mut c = files.canvas.borrow_mut();
        if !c.doc().pressed() {
            let out = c.doc_mut().apply(Cmd::CommitField);
            c.refresh(host, out);
        }
    }
    let now = files.now();
    let out = files.filing.borrow_mut().apply(cmd, &now);
    files.show_title(host);
    if let Some(note) = out.note {
        host.set_status(said_again(&host.get_status(), note).as_str().into());
    }
    files.perform(host, out.next.clone());
    out.next
}

impl<H: FileHost> Files<H> {
    /// The document as the file commands see it.
    fn now(&self) -> Now {
        let c = self.canvas.borrow();
        let (doc, label) = (c.doc(), c.doc().label());
        Now {
            unsaved: doc.unsaved(),
            pressed: doc.pressed(),
            rev: doc.revision(),
            on_disk: self.filing.borrow().path().is_some_and(Path::exists),
            unwritable: label.writable().err().map(|e| e.to_string()),
            suggest: fresh::untitled_name(label),
            // Only a relative path is read from the label's own folder, and so
            // moves when the label is saved somewhere else.
            csv: match &label.data {
                Some(DataBinding::Known(KnownBinding::CsvFile(f)))
                    if Path::new(&f.path).is_relative() =>
                {
                    Some(f.path.clone())
                }
                _ => None,
            },
        }
    }

    /// The file's name as the machine now has it, and the title from it.
    fn show_title(&self, host: &H) {
        let name = self.filing.borrow().name();
        let unsaved = self.canvas.borrow().doc().unsaved();
        host.set_doc_title(filing::title(name.as_deref(), unsaved).as_str().into());
        *self.name.borrow_mut() = name;
    }

    fn perform(self: &Rc<Self>, host: &H, next: Next) {
        match next {
            // The close handler answers Allow itself.
            Next::Nothing | Next::Allow => {}
            Next::Ask(question) => {
                host.invoke_close_file_popups();
                host.invoke_show_unsaved(question.as_str().into());
            }
            Next::Choose { ticket, ask } => {
                let answer: Answer = {
                    let files = self.clone();
                    Box::new(move |picked| {
                        if let Some(host) = files.host.upgrade() {
                            drive(&files, &host, FileCmd::Chosen { ticket, picked });
                        }
                    })
                };
                // Whatever stopped the dialog starting, the operator hears that no
                // dialog could be shown and what else works.
                if self.chooser.ask(ask, answer).is_err() {
                    let picked = Picked::NoDialog;
                    drive(self, host, FileCmd::Chosen { ticket, picked });
                }
            }
            Next::Size => {
                let printer =
                    fresh::printer_for(self.canvas.borrow().doc().label(), self.connected.get());
                let spec = printer.spec;
                self.printer.set(Some(printer));
                let presets: Vec<H::Preset> =
                    fresh::offers(spec).iter().map(H::Preset::of).collect();
                host.set_new_presets(ModelRc::new(VecModel::from(presets)));
                let feed = fresh::stock(spec).0;
                host.set_new_note(fresh::feed_says(feed, spec).as_str().into());
                host.invoke_show_new(
                    fresh::printer_says(&printer).as_str().into(),
                    fresh::across_says(spec).as_str().into(),
                    fresh::about_says(&printer).as_str().into(),
                    fresh::mm(feed).as_str().into(),
                    fresh::hint_says(spec).as_str().into(),
                );
            }
            Next::Write(to) => {
                let written = disk::save(&to, self.canvas.borrow().doc().label());
                let result = match written {
                    Ok(()) => {
                        self.canvas.borrow_mut().doc_mut().mark_saved();
                        Ok(())
                    }
                    Err(e) => Err(e.to_string()),
                };
                drive(self, host, FileCmd::Wrote { to, result });
            }
            Next::Read(from) => {
                // The only thing that says which way round the canvas goes is the
                // model the file was made for; `Left` is the assumption when it
                // says nothing, because it is the whole D11/D110/D101 family and
                // the one this has been run against. A connected printer that goes
                // the other way stops the preview rather than turning the label
                // (`proof`).
                let opened = disk::read_label(&from)
                    .map_err(|e| e.to_string())
                    .and_then(|label| {
                        Doc::open(label, PrintDirection::Left).map_err(|e| e.to_string())
                    });
                self.open(host, Source::File(from), opened);
            }
            // Shown again when it is already open, drawn afresh, so it is closed
            // first as a question is; and nothing else is ever shown with it.
            Next::Preview => {
                host.invoke_close_file_popups();
                (self.preview)(host);
            }
            Next::Fresh(spec) => {
                let made =
                    Doc::open(fresh::build(&spec), spec.spec.direction).map_err(|e| e.to_string());
                self.open(host, Source::New(spec), made);
            }
            Next::Hide => {
                host.invoke_close_file_popups();
                // Only fails when the platform is gone, and then the window is too.
                let _ = host.hide();
            }
        }
    }

    /// Put `opened` in the window, or leave everything as it is when it failed,
    /// and tell the machine which.
    fn open(self: &Rc<Self>, host: &H, from: Source, opened: Result<Doc, String>) {
        let result = opened.map(|doc| {
            let note = doc.opening_note();
            self.canvas.borrow_mut().replace_doc(host, doc);
            self.viewer.reopen(host);
            note
        });
        let replaced = result.is_ok();
        drive(self, host, FileCmd::Opened { from, result });
        // Last, with nothing borrowed: no row of the old document is left to hold
        // the keyboard, and with no focus item every chord in the window is dead
        // (shell.slint, `remove-selected`).
        if replaced {
            host.invoke_focus_canvas();
        }
    }
}
