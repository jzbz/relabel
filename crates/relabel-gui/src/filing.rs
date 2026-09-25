//! New, open, save, save as, the print pane and closing the window: which of them
//! may go ahead, what has to be asked first, and what the operator is told.
//!
//! A synchronous state machine with no toolkit and no I/O in it, for `doc`'s
//! reason: one [`FileCmd`] in, one [`FileOutcome`] out, and every rule reached by
//! an ordinary unit test. The wiring reads the document into a [`Now`], applies
//! the command, and does whatever [`Next`] says - shows the question, opens a
//! dialog, writes or reads a file - then feeds the result back in.
//!
//! # One guard for every route
//!
//! A chord, a File bar button, an assistive technology's default action, the
//! window's close button, a dialog's late answer and the New label pane all come
//! through [`Filing::apply`], so unsaved work is asked about in one place.
//!
//! # The print pane shares the popups' one place
//!
//! The print pane is not a file command, and it is here anyway: it opens in the
//! same place as the question and the New label pane, only one of the three is
//! ever shown, and the window's one close handler runs through this machine. So
//! Control+P is refused while the others are open, a file command an assistive
//! technology presses behind the open pane is refused with "close the print pane
//! first", and closing the window with the pane open is an ordinary close. The
//! pane asks nothing about unsaved work: printing changes nothing in the
//! document. A close held for a print job is `print`'s, and reaches this machine
//! only once the job has ended.
//!
//! # Consent is to a revision, not to a moment
//!
//! "Don't save" discards the work as it stood when it was answered. A dialog or
//! the New label pane stays open while the operator goes on editing behind it,
//! so by the time a file is chosen or a size typed there may be work the answer
//! never covered. The answer records the document's revision; if the revision has
//! moved when the replacement is ready, the question is asked again, keeping the
//! file already chosen or the size already typed.

use crate::choose::{Ask, Picked};
use crate::fresh::{self, NewSpec};
use std::path::{Path, PathBuf};

/// What the operator asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    New,
    Open,
    Save,
    SaveAs,
    /// The print pane: what will print, the printer link, and Print. Named
    /// Preview inside, as the pane was before it printed.
    Preview,
    /// The window's close button, or anything else that asks the window to go.
    Close,
}

impl Want {
    /// The File bar's buttons, in order, which is also token order. Close has no
    /// button: the window's own close is the way to ask for it.
    pub const BAR: [Want; 5] = [
        Want::New,
        Want::Open,
        Want::Save,
        Want::SaveAs,
        Want::Preview,
    ];

    /// The integer the File bar sends back, for `Field::token`'s reason. `None`
    /// for Close, which has no button.
    pub fn token(self) -> Option<i32> {
        Want::BAR
            .iter()
            .position(|&w| w == self)
            .and_then(|i| i32::try_from(i).ok())
    }

    pub fn from_token(t: i32) -> Option<Want> {
        usize::try_from(t)
            .ok()
            .and_then(|i| Want::BAR.get(i))
            .copied()
    }

    /// The file command a key held with Control is, in either case. Save as is
    /// told from Save by Shift, never by the letter's case: one layout sends `S`
    /// with Shift held and another `s`. Control+Shift+O and Control+Shift+N are
    /// not file commands: the first adds a rectangle (`make::Kind`), and the
    /// second is left alone as the add chords leave U and E. Control+Shift+P is
    /// left alone the same way.
    pub fn of_key(text: &str, shift: bool) -> Option<Want> {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return None;
        };
        match (c.to_ascii_lowercase(), shift) {
            ('s', false) => Some(Want::Save),
            ('s', true) => Some(Want::SaveAs),
            ('o', false) => Some(Want::Open),
            ('n', false) => Some(Want::New),
            ('p', false) => Some(Want::Preview),
            _ => None,
        }
    }

    /// The button's face.
    pub fn caption(self) -> &'static str {
        match self {
            Want::New => "New",
            Want::Open => "Open",
            Want::Save => "Save",
            Want::SaveAs => "Save as",
            Want::Preview => "Print",
            Want::Close => "Close",
        }
    }

    /// The button's accessible name: what it does, not what it shows.
    pub fn button_name(self) -> &'static str {
        match self {
            Want::New => "New label",
            Want::Open => "Open label",
            Want::Save => "Save label",
            Want::SaveAs => "Save label as",
            Want::Preview => "Print the label",
            Want::Close => "Close the window",
        }
    }

    /// The chord, spelled the way the button's accessible description announces
    /// it. Empty for Close, which is the window manager's and not relabel's.
    pub fn keys(self) -> &'static str {
        match self {
            Want::New => "Control+N",
            Want::Open => "Control+O",
            Want::Save => "Control+S",
            Want::SaveAs => "Control+Shift+S",
            Want::Preview => "Control+P",
            Want::Close => "",
        }
    }

    /// What did not happen when the command was refused, said on its own.
    fn undone(self) -> &'static str {
        match self {
            Want::New => "no new label was started",
            Want::Open => "nothing was opened",
            Want::Save | Want::SaveAs => "nothing was saved",
            Want::Preview => "the print pane was not opened",
            Want::Close => "nothing was closed",
        }
    }
}

/// The three answers to the question about unsaved work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Save,
    DontSave,
    Cancel,
}

impl Reply {
    pub const ALL: [Reply; 3] = [Reply::Save, Reply::DontSave, Reply::Cancel];

    pub fn token(self) -> i32 {
        self as i32
    }

    pub fn from_token(t: i32) -> Option<Reply> {
        usize::try_from(t)
            .ok()
            .and_then(|i| Reply::ALL.get(i))
            .copied()
    }
}

/// What the question about unsaved work is protecting, and what goes ahead once
/// it is answered. `Make` and `Read` are New and Open with the size already typed
/// or the file already chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Intent {
    New,
    Open,
    Close,
    Make(NewSpec),
    Read(PathBuf),
}

impl Intent {
    /// The end of the question: "Save the changes to a.relabel before ...?"
    fn before(&self) -> &'static str {
        match self {
            Intent::Close => "closing",
            Intent::New | Intent::Make(_) => "starting a new label",
            Intent::Open | Intent::Read(_) => "opening another label",
        }
    }

    /// What did not happen when the intent was dropped, said on its own.
    fn undone(&self) -> &'static str {
        match self {
            Intent::Close => "nothing was closed",
            Intent::New | Intent::Make(_) => "no new label was started",
            Intent::Open | Intent::Read(_) => "nothing was opened",
        }
    }
}

/// The document as the file commands need it, read by the wiring after staged
/// text is committed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Now {
    /// `Doc::unsaved`: changed since the last save, or text typed and not yet
    /// committed.
    pub unsaved: bool,
    /// An element is held by the pointer, so the document is mid-gesture.
    pub pressed: bool,
    /// `Doc::revision`, which is what "Don't save" consents to.
    pub rev: u64,
    /// The document's file exists, so a clean document has nothing to write.
    pub on_disk: bool,
    /// Why `Label::writable` would refuse the label, when it would.
    pub unwritable: Option<String>,
    /// The name Save as offers when the label has none: `fresh::untitled_name`.
    pub suggest: String,
    /// A data binding's file, when it is a relative path, which is read from the
    /// label's own folder.
    pub csv: Option<String>,
}

/// One thing that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCmd {
    Want(Want),
    Reply(Reply),
    /// A file dialog answered. `ticket` is the one [`Next::Choose`] carried.
    Chosen {
        ticket: u64,
        picked: Picked,
    },
    /// The New label pane closed: with the label to make, or cancelled.
    Sized(Option<NewSpec>),
    /// What [`Next::Write`] came to.
    Wrote {
        to: PathBuf,
        result: Result<(), String>,
    },
    /// What [`Next::Read`] or [`Next::Fresh`] came to: `Ok` carries
    /// `Doc::opening_note` for the document now open.
    Opened {
        from: Source,
        result: Result<String, String>,
    },
    /// The print pane closed, by its Close button or Escape.
    PreviewClosed,
}

/// Where an opened document came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    New(NewSpec),
}

/// What the wiring does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// Let the window close. Only ever the answer to [`Want::Close`].
    Allow,
    /// Show the question about unsaved work, with this text.
    Ask(String),
    Choose {
        ticket: u64,
        ask: Ask,
    },
    /// Show the New label pane.
    Size,
    Write(PathBuf),
    Read(PathBuf),
    Fresh(NewSpec),
    /// Show the print pane, or show it again with what it says now.
    Preview,
    /// Close the window: the question was answered and the work is saved or let
    /// go.
    Hide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOutcome {
    pub next: Next,
    /// Pre-formatted for the status line. The UI never composes a message.
    pub note: Option<String>,
}

impl FileOutcome {
    fn then(next: Next) -> FileOutcome {
        FileOutcome { next, note: None }
    }

    fn said(note: impl Into<String>) -> FileOutcome {
        FileOutcome {
            next: Next::Nothing,
            note: Some(note.into()),
        }
    }

    fn quiet() -> FileOutcome {
        FileOutcome::then(Next::Nothing)
    }
}

/// A file dialog on screen. `then` is what a save goes on to once it is written;
/// `consent` is the revision "Don't save" was answered at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dialog {
    ticket: u64,
    for_open: bool,
    then: Option<Intent>,
    consent: Option<u64>,
}

impl Dialog {
    /// The command that asks for this dialog again.
    fn again(&self) -> &'static str {
        if self.for_open { "Open" } else { "Save as" }
    }

    fn undone(&self) -> &'static str {
        if self.for_open {
            "nothing was opened"
        } else {
            "nothing was saved"
        }
    }
}

/// What is open or under way.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wait {
    Idle,
    Question(Intent),
    Dialog(Dialog),
    Sizing {
        consent: Option<u64>,
    },
    /// A write the wiring is doing, and what goes ahead once it has.
    Writing {
        then: Option<Intent>,
    },
    /// The print pane is open.
    Previewing,
}

/// Which file the document is, and what is open or under way.
#[derive(Debug)]
pub struct Filing {
    path: Option<PathBuf>,
    /// The last ticket handed out. An answer carrying any other is stale.
    ticket: u64,
    wait: Wait,
    /// A dialog still on screen while the question about closing is asked over
    /// it. Nothing can take a native dialog down - rfd's builder has no way to
    /// close one (rfd file_dialog.rs:187-333) - so it stays pending until it
    /// answers: it is the pending dialog again when the question ends with the
    /// window still open, and an answer that arrives while the question is
    /// showing is said to have been dropped.
    shelved: Option<Dialog>,
    /// Whether a path is taken. Injected, so the machine does no I/O of its own
    /// and a test says what exists; asked only when a name chosen for Save as
    /// lacks the extension.
    exists: fn(&Path) -> bool,
}

const NO_DIALOG: &str =
    "no file dialog could be shown here (it needs a file-chooser portal or zenity)";

impl Filing {
    /// `path` is the document's file, when it has one; it need not exist yet.
    pub fn new(path: Option<PathBuf>) -> Filing {
        Filing {
            path,
            ticket: 0,
            wait: Wait::Idle,
            shelved: None,
            exists: Path::exists,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The document's file name, as it is shown: never the whole path.
    pub fn name(&self) -> Option<String> {
        self.path.as_deref().map(file_name)
    }

    /// Whether the print pane is open, which is when the wiring may move the
    /// keyboard inside it or draw it again.
    pub fn previewing(&self) -> bool {
        self.wait == Wait::Previewing
    }

    pub fn apply(&mut self, cmd: FileCmd, now: &Now) -> FileOutcome {
        let out = match cmd {
            FileCmd::Want(w) => self.want(w, now),
            FileCmd::Reply(r) => self.reply(r, now),
            FileCmd::Chosen { ticket, picked } => self.chosen(ticket, picked, now),
            FileCmd::Sized(spec) => self.sized(spec, now),
            FileCmd::Wrote { to, result } => self.wrote(to, result, now),
            FileCmd::Opened { from, result } => self.opened(from, result),
            FileCmd::PreviewClosed => {
                if self.wait == Wait::Previewing {
                    self.wait = Wait::Idle;
                }
                FileOutcome::quiet()
            }
        };
        // A shelved dialog goes with the window, and is pending again once the
        // question over it has ended any other way.
        if out.next == Next::Hide {
            self.shelved = None;
        } else if self.wait == Wait::Idle
            && let Some(d) = self.shelved.take()
        {
            self.wait = Wait::Dialog(d);
        }
        out
    }

    fn want(&mut self, w: Want, now: &Now) -> FileOutcome {
        // Refused, not queued: nothing runs at the release, so the note says what
        // did not happen.
        if now.pressed {
            return FileOutcome::said(held(w.undone()));
        }
        match (&mut self.wait, w) {
            // An assistive technology can reach the buttons behind the popup - an
            // action goes straight to its target node, with no check for a popup
            // above it (i-slint-backend-winit accesskit.rs:145-175) - so a command
            // can arrive with the question showing: it is shown again, not lost.
            (Wait::Question(intent), Want::Close) => {
                *intent = Intent::Close;
                return FileOutcome::then(Next::Ask(self.question()));
            }
            (Wait::Question(_), _) => {
                return FileOutcome {
                    next: Next::Ask(self.question()),
                    note: Some("answer the question about unsaved changes first".into()),
                };
            }
            // Closing is never held up by a dialog. With nothing unsaved the window
            // goes and the dialog's answer is ignored when it comes; otherwise the
            // question is asked over the dialog, which is shelved beneath it.
            (Wait::Dialog(_), Want::Close) if !now.unsaved => self.ticket += 1,
            (Wait::Dialog(_), Want::Close) => {
                let Wait::Dialog(d) =
                    std::mem::replace(&mut self.wait, Wait::Question(Intent::Close))
                else {
                    unreachable!("matched as a dialog above");
                };
                self.shelved = Some(d);
                return FileOutcome::then(Next::Ask(self.question()));
            }
            (Wait::Dialog(_), _) => return FileOutcome::said("a file dialog is already open"),
            (Wait::Sizing { .. }, Want::Close) => {}
            (Wait::Sizing { .. }, _) => {
                return FileOutcome::said("finish or cancel the new label first");
            }
            // Closing is an ordinary close, which takes the pane down with the
            // window or with the question asked in its place.
            (Wait::Previewing, Want::Close) => {}
            // Asked again, it is drawn again: nothing is lost by it.
            (Wait::Previewing, Want::Preview) => return FileOutcome::then(Next::Preview),
            // Pointer and keys cannot reach the File bar behind the pane, and an
            // assistive technology's action can, for the question's reason above.
            (Wait::Previewing, _) => return FileOutcome::said("close the print pane first"),
            (Wait::Idle | Wait::Writing { .. }, _) => {}
        }
        self.wait = Wait::Idle;
        let intent = match w {
            Want::Save => return self.save(now, None),
            Want::SaveAs => return self.save_as(now, None),
            // Nothing about unsaved work is asked: the pane changes nothing.
            Want::Preview => {
                self.wait = Wait::Previewing;
                return FileOutcome::then(Next::Preview);
            }
            Want::New => Intent::New,
            Want::Open => Intent::Open,
            Want::Close => Intent::Close,
        };
        if now.unsaved {
            self.wait = Wait::Question(intent);
            return FileOutcome::then(Next::Ask(self.question()));
        }
        match intent {
            // The one answer that lets the close request itself go through;
            // every later close is the wiring's, after the handler has returned.
            Intent::Close => FileOutcome::then(Next::Allow),
            intent => self.proceed(intent, None),
        }
    }

    fn reply(&mut self, r: Reply, now: &Now) -> FileOutcome {
        let Wait::Question(intent) = std::mem::replace(&mut self.wait, Wait::Idle) else {
            // A late answer to a question already gone.
            return FileOutcome::quiet();
        };
        match r {
            Reply::Cancel => FileOutcome::said(intent.undone()),
            // The question has gone with the answer, and nothing is kept for the
            // release.
            Reply::Save if now.pressed => {
                FileOutcome::said(held("nothing was saved") + &dropped(&intent))
            }
            Reply::DontSave if now.pressed => FileOutcome::said(held(intent.undone())),
            Reply::Save => self.save(now, Some(intent)),
            Reply::DontSave => self.proceed(intent, Some(now.rev)),
        }
    }

    fn chosen(&mut self, ticket: u64, picked: Picked, now: &Now) -> FileOutcome {
        // The question about closing is showing over this dialog, so what it
        // chose cannot go ahead; a file chosen is said to be dropped, and the
        // question stays.
        if let Some(d) = self.shelved.take_if(|d| d.ticket == ticket) {
            return match picked {
                Picked::Path(p) => FileOutcome::said(format!(
                    "{}: {} was chosen while the window was asking whether to close; choose \
                     {} again{}",
                    d.undone(),
                    file_name(&p),
                    d.again(),
                    d.then.as_ref().map(dropped).unwrap_or_default()
                )),
                Picked::Nothing | Picked::NoDialog => FileOutcome::quiet(),
            };
        }
        let Wait::Dialog(d) = &self.wait else {
            return FileOutcome::quiet();
        };
        if d.ticket != ticket {
            return FileOutcome::quiet();
        }
        let Wait::Dialog(d) = std::mem::replace(&mut self.wait, Wait::Idle) else {
            unreachable!("matched as a dialog above");
        };
        let lost = d.then.as_ref().map(dropped).unwrap_or_default();
        // Dropped, not queued: the document is mid-gesture, and nothing may
        // replace or write it.
        if now.pressed
            && let Picked::Path(p) = &picked
        {
            return FileOutcome::said(format!(
                "{}: {} was chosen while an element was held; let go, then choose {} \
                 again{lost}",
                d.undone(),
                file_name(p),
                d.again()
            ));
        }
        let Dialog {
            for_open,
            then,
            consent,
            ..
        } = d;
        match (picked, for_open) {
            (Picked::Nothing, true) => FileOutcome::said("nothing was opened: no file was chosen"),
            (Picked::NoDialog, true) => FileOutcome::said(format!(
                "nothing was opened: {NO_DIALOG}; a label opens from the command line: \
                 relabel path/to/label.relabel"
            )),
            (Picked::Path(p), true) => self.go_ahead(Intent::Read(p), consent, now),
            (Picked::Nothing, false) => {
                FileOutcome::said(format!("nothing was saved: no file was chosen{lost}"))
            }
            (Picked::NoDialog, false) => FileOutcome::said(format!(
                "nothing was saved: {NO_DIALOG}; relabel path/to/new.relabel starts a label \
                 that Control+S saves there{lost}"
            )),
            (Picked::Path(p), false) => match with_extension(p, self.exists) {
                Ok(to) => {
                    self.wait = Wait::Writing { then };
                    FileOutcome::then(Next::Write(to))
                }
                Err(why) => FileOutcome::said(why + &lost),
            },
        }
    }

    fn sized(&mut self, spec: Option<NewSpec>, now: &Now) -> FileOutcome {
        let Wait::Sizing { consent } = self.wait else {
            return FileOutcome::quiet();
        };
        self.wait = Wait::Idle;
        match spec {
            None => FileOutcome::said("no new label was started"),
            Some(_) if now.pressed => FileOutcome::said(held("no new label was started")),
            Some(spec) => self.go_ahead(Intent::Make(spec), consent, now),
        }
    }

    fn wrote(&mut self, to: PathBuf, result: Result<(), String>, now: &Now) -> FileOutcome {
        let then = match std::mem::replace(&mut self.wait, Wait::Idle) {
            Wait::Writing { then } => then,
            _ => None,
        };
        let name = file_name(&to);
        if let Err(why) = result {
            let lost = then.as_ref().map(dropped).unwrap_or_default();
            return FileOutcome::said(format!(
                "could not save {name}: {why}; nothing was written{lost}"
            ));
        }
        let mut note = format!("saved {name}");
        let folder = to.parent();
        if let Some(csv) = &now.csv
            && self.path.as_deref().and_then(Path::parent) != folder
        {
            let folder = folder.map(file_name).unwrap_or_default();
            note.push_str(&format!(
                "; its data file {csv} is now looked for in {folder}"
            ));
        }
        self.path = Some(to);
        let out = match then {
            Some(intent) => self.proceed(intent, None),
            None => FileOutcome::quiet(),
        };
        FileOutcome {
            next: out.next,
            note: Some(note),
        }
    }

    fn opened(&mut self, from: Source, result: Result<String, String>) -> FileOutcome {
        self.wait = Wait::Idle;
        match (from, result) {
            (Source::File(p), Ok(note)) => {
                let said = format!("opened {}: {note}", file_name(&p));
                self.path = Some(p);
                FileOutcome::said(said)
            }
            (Source::New(spec), Ok(note)) => {
                self.path = None;
                FileOutcome::said(format!("{}: {note}", fresh::made_says(&spec)))
            }
            (from, Err(why)) => {
                let current = match self.name() {
                    Some(name) => name,
                    None => "the untitled label".into(),
                };
                let what = match from {
                    Source::File(p) => format!("could not open {}", file_name(&p)),
                    Source::New(_) => "no new label was started".into(),
                };
                FileOutcome::said(format!("{what}: {why}; {current} is still open"))
            }
        }
    }

    /// Save, or Save as when the document has no file yet; `then` goes ahead once
    /// the write has succeeded.
    fn save(&mut self, now: &Now, then: Option<Intent>) -> FileOutcome {
        let Some(path) = self.path.clone() else {
            return self.save_as(now, then);
        };
        if !now.unsaved && now.on_disk {
            // Nothing is written, so a hand-written file that has not been
            // touched is never re-serialised.
            return match then {
                Some(intent) => self.proceed(intent, None),
                None => FileOutcome::said(format!("no changes to save in {}", file_name(&path))),
            };
        }
        if let Some(refused) = self.unwritable(now, then.as_ref()) {
            return refused;
        }
        self.wait = Wait::Writing { then };
        FileOutcome::then(Next::Write(path))
    }

    /// Ask where to save. A label that cannot be saved is refused before a
    /// dialog opens, rather than after the operator has chosen a place for it.
    fn save_as(&mut self, now: &Now, then: Option<Intent>) -> FileOutcome {
        if let Some(refused) = self.unwritable(now, then.as_ref()) {
            return refused;
        }
        // Only reached from the question about closing, asked over a Save as or
        // Open dialog on a label with no file yet: that dialog is still on screen,
        // and a second is not opened beside it.
        if self.shelved.is_some() {
            self.wait = Wait::Idle;
            let lost = then.as_ref().map(dropped).unwrap_or_default();
            return FileOutcome::said(format!(
                "nothing was saved: a file dialog is already open{lost}"
            ));
        }
        // The folder and the name separately: the portal backend never sets a
        // whole starting path (rfd xdg_desktop_portal.rs:249-262).
        let ask = Ask::SaveAs {
            folder: self.folder(),
            name: self.name().unwrap_or_else(|| now.suggest.clone()),
        };
        self.choose(ask, false, then, None)
    }

    fn unwritable(&mut self, now: &Now, then: Option<&Intent>) -> Option<FileOutcome> {
        let why = now.unwritable.as_ref()?;
        self.wait = Wait::Idle;
        let name = self.name().unwrap_or_else(|| "this label".into());
        let lost = then.map(dropped).unwrap_or_default();
        Some(FileOutcome::said(format!(
            "{name} cannot be saved: {why}; nothing was written{lost}"
        )))
    }

    fn choose(
        &mut self,
        ask: Ask,
        for_open: bool,
        then: Option<Intent>,
        consent: Option<u64>,
    ) -> FileOutcome {
        self.ticket += 1;
        self.wait = Wait::Dialog(Dialog {
            ticket: self.ticket,
            for_open,
            then,
            consent,
        });
        FileOutcome::then(Next::Choose {
            ticket: self.ticket,
            ask,
        })
    }

    /// A replacement is ready - a file chosen, a size typed. It goes ahead if
    /// nothing is unsaved or "Don't save" covered every change; otherwise the
    /// question comes back with the replacement kept.
    fn go_ahead(&mut self, intent: Intent, consent: Option<u64>, now: &Now) -> FileOutcome {
        if now.unsaved && consent != Some(now.rev) {
            self.wait = Wait::Question(intent);
            return FileOutcome::then(Next::Ask(self.question()));
        }
        self.proceed(intent, consent)
    }

    /// The intent goes ahead: its work is saved, or let go at `consent`. By now
    /// the close request that asked has been answered, so a close is a hide.
    fn proceed(&mut self, intent: Intent, consent: Option<u64>) -> FileOutcome {
        self.wait = Wait::Idle;
        match intent {
            Intent::Close => FileOutcome::then(Next::Hide),
            Intent::New => {
                self.wait = Wait::Sizing { consent };
                FileOutcome::then(Next::Size)
            }
            Intent::Open => {
                let ask = Ask::Open {
                    folder: self.folder(),
                };
                self.choose(ask, true, None, consent)
            }
            Intent::Make(spec) => FileOutcome::then(Next::Fresh(spec)),
            Intent::Read(p) => FileOutcome::then(Next::Read(p)),
        }
    }

    /// The question for what is waiting on it.
    fn question(&self) -> String {
        let Wait::Question(intent) = &self.wait else {
            return String::new();
        };
        let before = intent.before();
        match self.name() {
            Some(name) => format!("Save the changes to {name} before {before}?"),
            None => format!("Save this label before {before}?"),
        }
    }

    fn folder(&self) -> Option<PathBuf> {
        self.path
            .as_deref()
            .and_then(Path::parent)
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
    }
}

/// The window's title: the file's name, marked when there is unsaved work.
pub fn title(name: Option<&str>, unsaved: bool) -> String {
    let mark = if unsaved { "*" } else { "" };
    let name = name.unwrap_or("Untitled label");
    format!("{mark}{name} \u{2014} relabel")
}

/// The path Save as writes: as chosen when it ends in `.relabel`, in any case,
/// and with `.relabel` added otherwise - unless that name is already taken. Then
/// it is refused: the dialog asked about replacing the name as typed, not this
/// one.
pub fn with_extension(p: PathBuf, exists: impl Fn(&Path) -> bool) -> Result<PathBuf, String> {
    let has = p
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("relabel"));
    if has {
        return Ok(p);
    }
    let mut name = p.file_name().unwrap_or_default().to_os_string();
    name.push(".relabel");
    let q = p.with_file_name(name);
    if exists(&q) {
        return Err(format!(
            "{} already exists and the dialog did not ask about replacing it; choose Save as \
             again and type the name in full",
            file_name(&q)
        ));
    }
    Ok(q)
}

/// A path as it is shown: the file name only. Inside a sandbox the full path
/// is a document-portal path that means nothing to the operator.
fn file_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or(p.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// A command refused because an element is held. Nothing is kept for the
/// release, so the note says what did not happen and what to do instead.
fn held(undone: &str) -> String {
    format!("{undone}: let go of the element first")
}

/// The clause added when a save fails or is refused on the way to something else.
fn dropped(intent: &Intent) -> String {
    format!("; {}", intent.undone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(p: &str) -> Filing {
        Filing::new(Some(PathBuf::from(p)))
    }

    fn unsaved(rev: u64) -> Now {
        Now {
            unsaved: true,
            rev,
            on_disk: true,
            ..Now::default()
        }
    }

    fn clean() -> Now {
        Now {
            on_disk: true,
            ..Now::default()
        }
    }

    fn want(f: &mut Filing, w: Want, now: &Now) -> FileOutcome {
        f.apply(FileCmd::Want(w), now)
    }

    fn reply(f: &mut Filing, r: Reply, now: &Now) -> FileOutcome {
        f.apply(FileCmd::Reply(r), now)
    }

    fn chosen(f: &mut Filing, ticket: u64, picked: Picked, now: &Now) -> FileOutcome {
        f.apply(FileCmd::Chosen { ticket, picked }, now)
    }

    fn ticket_of(out: &FileOutcome) -> u64 {
        match out.next {
            Next::Choose { ticket, .. } => ticket,
            ref other => panic!("expected a dialog, got {other:?}"),
        }
    }

    fn path(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    fn d110_m_at(feed_tenths: i32) -> NewSpec {
        NewSpec {
            spec: fresh::default_printer(),
            feed_tenths,
            assumed: None,
        }
    }

    #[test]
    fn a_clean_document_whose_file_exists_saves_nothing() {
        let out = want(&mut at("/l/a.relabel"), Want::Save, &clean());
        assert_eq!(
            out,
            FileOutcome {
                next: Next::Nothing,
                note: Some("no changes to save in a.relabel".into()),
            }
        );
    }

    /// The file was deleted or never written - a path named at launch that did
    /// not exist yet - so even a clean document has something to put there.
    #[test]
    fn a_clean_document_whose_file_is_gone_is_written() {
        let out = want(&mut at("/l/a.relabel"), Want::Save, &Now::default());
        assert_eq!(out.next, Next::Write(path("/l/a.relabel")));
    }

    #[test]
    fn an_untitled_save_asks_where_with_the_suggested_name() {
        let now = Now {
            unsaved: true,
            suggest: "untitled-30x12.relabel".into(),
            ..Now::default()
        };
        let out = Filing::new(None).apply(FileCmd::Want(Want::Save), &now);
        assert_eq!(
            out.next,
            Next::Choose {
                ticket: 1,
                ask: Ask::SaveAs {
                    folder: None,
                    name: "untitled-30x12.relabel".into(),
                },
            }
        );
    }

    #[test]
    fn save_as_offers_the_files_own_folder_and_name() {
        let out = want(&mut at("/l/a.relabel"), Want::SaveAs, &clean());
        assert_eq!(
            out.next,
            Next::Choose {
                ticket: 1,
                ask: Ask::SaveAs {
                    folder: Some(path("/l")),
                    name: "a.relabel".into(),
                },
            }
        );
    }

    #[test]
    fn open_over_unsaved_work_asks_and_dont_save_then_chooses() {
        let mut f = at("/l/a.relabel");
        let out = want(&mut f, Want::Open, &unsaved(5));
        assert_eq!(
            out.next,
            Next::Ask("Save the changes to a.relabel before opening another label?".into())
        );
        let out = reply(&mut f, Reply::DontSave, &unsaved(5));
        assert!(
            matches!(
                out.next,
                Next::Choose { ask: Ask::Open { folder: Some(ref d) }, .. } if d == Path::new("/l")
            ),
            "{out:?}"
        );
    }

    #[test]
    fn an_edit_made_while_the_dialog_was_open_is_asked_about_again_and_the_file_kept() {
        for (rev, asked_again) in [(6, true), (5, false)] {
            let mut f = at("/l/a.relabel");
            want(&mut f, Want::Open, &unsaved(5));
            let t = ticket_of(&reply(&mut f, Reply::DontSave, &unsaved(5)));
            let out = chosen(&mut f, t, Picked::Path(path("/l/b.relabel")), &unsaved(rev));
            if !asked_again {
                assert_eq!(out.next, Next::Read(path("/l/b.relabel")));
                continue;
            }
            assert_eq!(
                out.next,
                Next::Ask("Save the changes to a.relabel before opening another label?".into())
            );
            let out = reply(&mut f, Reply::DontSave, &unsaved(rev));
            assert_eq!(out.next, Next::Read(path("/l/b.relabel")));
        }
    }

    #[test]
    fn a_size_typed_after_an_edit_is_asked_about_again_and_kept() {
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::New, &unsaved(5));
        assert_eq!(reply(&mut f, Reply::DontSave, &unsaved(5)).next, Next::Size);
        let spec = d110_m_at(400);
        let out = f.apply(FileCmd::Sized(Some(spec)), &unsaved(6));
        assert_eq!(
            out.next,
            Next::Ask("Save the changes to a.relabel before starting a new label?".into())
        );
        assert_eq!(
            reply(&mut f, Reply::DontSave, &unsaved(6)).next,
            Next::Fresh(spec)
        );
    }

    #[test]
    fn an_answer_for_a_superseded_ticket_is_ignored() {
        let mut f = at("/l/a.relabel");
        let t1 = ticket_of(&want(&mut f, Want::Open, &clean()));
        chosen(&mut f, t1, Picked::Nothing, &clean());
        let t2 = ticket_of(&want(&mut f, Want::Open, &clean()));
        assert_ne!(t1, t2);
        assert_eq!(
            chosen(&mut f, t1, Picked::Path(path("/x.relabel")), &clean()),
            FileOutcome {
                next: Next::Nothing,
                note: None,
            }
        );
        assert_eq!(
            chosen(&mut f, t2, Picked::Path(path("/y.relabel")), &clean()).next,
            Next::Read(path("/y.relabel"))
        );
    }

    #[test]
    fn a_second_file_command_while_a_dialog_is_open_is_refused() {
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::Open, &clean()));
        for w in Want::BAR {
            assert_eq!(
                want(&mut f, w, &clean()),
                FileOutcome::said("a file dialog is already open"),
                "{w:?}"
            );
        }
        // The dialog is still the one pending.
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/y.relabel")), &clean()).next,
            Next::Read(path("/y.relabel"))
        );
    }

    /// Nothing is kept for the release, so the note is a refusal: a note that said
    /// the command waits promised a save or a close that never came.
    #[test]
    fn a_held_pointer_refuses_every_file_command_and_says_nothing_happened() {
        let now = Now {
            pressed: true,
            unsaved: true,
            ..Now::default()
        };
        for (w, note) in [
            (Want::Save, "nothing was saved: let go of the element first"),
            (
                Want::SaveAs,
                "nothing was saved: let go of the element first",
            ),
            (
                Want::Open,
                "nothing was opened: let go of the element first",
            ),
            (
                Want::New,
                "no new label was started: let go of the element first",
            ),
            (
                Want::Preview,
                "the print pane was not opened: let go of the element first",
            ),
            (
                Want::Close,
                "nothing was closed: let go of the element first",
            ),
        ] {
            let mut f = at("/l/a.relabel");
            let out = want(&mut f, w, &now);
            assert_eq!(out, FileOutcome::said(note), "{w:?}");
            // Nothing was left waiting: the release has nothing to run.
            assert_eq!(f.wait, Wait::Idle, "{w:?}");
        }

        // An answer to the question is refused the same way, and Cancel is what
        // it always is.
        for (r, note) in [
            (
                Reply::Save,
                "nothing was saved: let go of the element first; nothing was closed",
            ),
            (
                Reply::DontSave,
                "nothing was closed: let go of the element first",
            ),
            (Reply::Cancel, "nothing was closed"),
        ] {
            let mut f = at("/l/a.relabel");
            want(&mut f, Want::Close, &unsaved(1));
            assert_eq!(reply(&mut f, r, &now), FileOutcome::said(note), "{r:?}");
        }
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::New, &clean());
        assert_eq!(
            f.apply(FileCmd::Sized(Some(d110_m_at(400))), &now),
            FileOutcome::said("no new label was started: let go of the element first")
        );
    }

    /// A dialog answered while an element is held is dropped, not queued: the
    /// document is mid-gesture, and nothing may replace or write it. The note
    /// names the file that was let go of, and how to have it after all.
    #[test]
    fn a_dialog_answered_while_an_element_is_held_is_dropped_and_said() {
        let pressed = Now {
            pressed: true,
            ..clean()
        };
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::Open, &clean()));
        let out = chosen(&mut f, t, Picked::Path(path("/l/b.relabel")), &pressed);
        assert_eq!(
            out,
            FileOutcome::said(
                "nothing was opened: b.relabel was chosen while an element was held; let go, \
                 then choose Open again"
            )
        );
        assert_eq!(ticket_of(&want(&mut f, Want::Open, &clean())), t + 1);

        let mut f = Filing::new(None);
        want(&mut f, Want::Close, &unsaved(1));
        let t = ticket_of(&reply(&mut f, Reply::Save, &unsaved(1)));
        let out = chosen(&mut f, t, Picked::Path(path("/d/n.relabel")), &pressed);
        assert_eq!(
            out,
            FileOutcome::said(
                "nothing was saved: n.relabel was chosen while an element was held; let go, \
                 then choose Save as again; nothing was closed"
            )
        );

        // Nothing chosen loses nothing, and is said as it always is.
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::Open, &clean()));
        assert_eq!(
            chosen(&mut f, t, Picked::Nothing, &pressed),
            FileOutcome::said("nothing was opened: no file was chosen")
        );
    }

    #[test]
    fn close_is_allowed_when_clean_and_asked_when_not() {
        assert_eq!(
            want(&mut at("/l/a.relabel"), Want::Close, &clean()),
            FileOutcome::then(Next::Allow)
        );
        assert_eq!(
            want(&mut at("/l/a.relabel"), Want::Close, &unsaved(1)).next,
            Next::Ask("Save the changes to a.relabel before closing?".into())
        );
        assert_eq!(
            want(&mut Filing::new(None), Want::Close, &unsaved(1)).next,
            Next::Ask("Save this label before closing?".into())
        );
    }

    #[test]
    fn save_then_close_hides_only_after_the_write() {
        let mut f = at("/l/a.relabel");
        assert!(matches!(
            want(&mut f, Want::Close, &unsaved(3)).next,
            Next::Ask(_)
        ));
        assert_eq!(
            reply(&mut f, Reply::Save, &unsaved(3)).next,
            Next::Write(path("/l/a.relabel"))
        );
        let out = f.apply(
            FileCmd::Wrote {
                to: path("/l/a.relabel"),
                result: Err("disk full".into()),
            },
            &unsaved(3),
        );
        assert_eq!(out.next, Next::Nothing);
        let note = out.note.expect("a failed save is said");
        assert!(
            note.contains("could not save a.relabel: disk full")
                && note.contains("nothing was closed"),
            "{note}"
        );

        assert!(matches!(
            want(&mut f, Want::Close, &unsaved(3)).next,
            Next::Ask(_)
        ));
        assert_eq!(
            reply(&mut f, Reply::Save, &unsaved(3)).next,
            Next::Write(path("/l/a.relabel"))
        );
        let out = f.apply(
            FileCmd::Wrote {
                to: path("/l/a.relabel"),
                result: Ok(()),
            },
            &clean(),
        );
        assert_eq!(out.next, Next::Hide);
        assert_eq!(out.note.as_deref(), Some("saved a.relabel"));
    }

    #[test]
    fn an_untitled_save_from_the_question_that_chooses_nothing_closes_nothing() {
        let mut f = Filing::new(None);
        want(&mut f, Want::Close, &unsaved(1));
        let t = ticket_of(&reply(&mut f, Reply::Save, &unsaved(1)));
        let out = chosen(&mut f, t, Picked::Nothing, &unsaved(1));
        assert_eq!(out.next, Next::Nothing);
        let note = out.note.expect("said");
        assert!(
            note.contains("nothing was saved") && note.contains("nothing was closed"),
            "{note}"
        );
    }

    #[test]
    fn an_untitled_save_from_the_question_closes_once_written() {
        let mut f = Filing::new(None);
        want(&mut f, Want::Close, &unsaved(1));
        let t = ticket_of(&reply(&mut f, Reply::Save, &unsaved(1)));
        let out = chosen(&mut f, t, Picked::Path(path("/d/new.relabel")), &unsaved(1));
        assert_eq!(out.next, Next::Write(path("/d/new.relabel")));
        let out = f.apply(
            FileCmd::Wrote {
                to: path("/d/new.relabel"),
                result: Ok(()),
            },
            &clean(),
        );
        assert_eq!(out.next, Next::Hide);
        assert_eq!(f.name().as_deref(), Some("new.relabel"));
    }

    #[test]
    fn no_file_chosen_and_no_dialog_are_said_differently() {
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::Open, &clean()));
        assert_eq!(
            chosen(&mut f, t, Picked::Nothing, &clean()),
            FileOutcome::said("nothing was opened: no file was chosen")
        );
        let t = ticket_of(&want(&mut f, Want::Open, &clean()));
        let out = chosen(&mut f, t, Picked::NoDialog, &clean());
        assert_eq!(out.next, Next::Nothing);
        let note = out.note.expect("said");
        assert!(
            note.contains("no file dialog could be shown here")
                && note.contains("relabel path/to/label.relabel"),
            "{note}"
        );
        assert!(matches!(
            want(&mut f, Want::Open, &clean()).next,
            Next::Choose { .. }
        ));

        let mut f = Filing::new(None);
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(1)));
        let note = chosen(&mut f, t, Picked::NoDialog, &unsaved(1))
            .note
            .expect("said");
        assert!(
            note.starts_with("nothing was saved: no file dialog could be shown here")
                && note.contains("relabel path/to/new.relabel"),
            "{note}"
        );
    }

    #[test]
    fn closing_a_clean_document_while_a_dialog_is_open_abandons_it() {
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::Open, &clean()));
        assert_eq!(want(&mut f, Want::Close, &clean()).next, Next::Allow);
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/l/b.relabel")), &clean()),
            FileOutcome::quiet()
        );
    }

    /// A native dialog stays on screen under the question about closing; when
    /// the window stays open it is still the one dialog, and its answer counts.
    #[test]
    fn a_close_cancelled_over_a_dialog_leaves_that_dialog_pending() {
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(2)));
        assert_eq!(
            want(&mut f, Want::Close, &unsaved(2)).next,
            Next::Ask("Save the changes to a.relabel before closing?".into())
        );
        assert_eq!(
            reply(&mut f, Reply::Cancel, &unsaved(2)),
            FileOutcome::said("nothing was closed")
        );
        for w in Want::BAR {
            assert_eq!(
                want(&mut f, w, &unsaved(2)),
                FileOutcome::said("a file dialog is already open"),
                "{w:?}"
            );
        }
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/l/c.relabel")), &unsaved(2)).next,
            Next::Write(path("/l/c.relabel"))
        );

        // An Open answered "Don't save" keeps its consent under the question.
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Open, &unsaved(5));
        let t = ticket_of(&reply(&mut f, Reply::DontSave, &unsaved(5)));
        want(&mut f, Want::Close, &unsaved(5));
        reply(&mut f, Reply::Cancel, &unsaved(5));
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/l/b.relabel")), &unsaved(5)).next,
            Next::Read(path("/l/b.relabel"))
        );

        // A failed save from the question leaves the window, and the dialog, open.
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(2)));
        want(&mut f, Want::Close, &unsaved(2));
        assert_eq!(
            reply(&mut f, Reply::Save, &unsaved(2)).next,
            Next::Write(path("/l/a.relabel"))
        );
        let out = f.apply(
            FileCmd::Wrote {
                to: path("/l/a.relabel"),
                result: Err("disk full".into()),
            },
            &unsaved(2),
        );
        assert!(
            out.note
                .as_deref()
                .is_some_and(|n| n.ends_with("nothing was closed")),
            "{out:?}"
        );
        assert_eq!(
            want(&mut f, Want::Open, &unsaved(2)),
            FileOutcome::said("a file dialog is already open")
        );
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/l/c.relabel")), &unsaved(2)).next,
            Next::Write(path("/l/c.relabel"))
        );
    }

    #[test]
    fn a_dialog_answered_under_the_close_question_is_said_to_be_dropped() {
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(2)));
        want(&mut f, Want::Close, &unsaved(2));
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/l/c.relabel")), &unsaved(2)),
            FileOutcome::said(
                "nothing was saved: c.relabel was chosen while the window was asking whether \
                 to close; choose Save as again"
            )
        );
        // The question is still the one showing, and the dialog is gone from it.
        assert_eq!(reply(&mut f, Reply::DontSave, &unsaved(2)).next, Next::Hide);

        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(2)));
        want(&mut f, Want::Close, &unsaved(2));
        assert_eq!(
            chosen(&mut f, t, Picked::Nothing, &unsaved(2)),
            FileOutcome::quiet()
        );
        reply(&mut f, Reply::Cancel, &unsaved(2));
        // No dialog is left pending, so the next command goes ahead.
        assert!(matches!(
            want(&mut f, Want::SaveAs, &unsaved(2)).next,
            Next::Choose { .. }
        ));
    }

    /// Save from the question on a label with no file yet would need a second
    /// dialog beside the one already showing.
    #[test]
    fn an_untitled_save_from_the_close_question_over_a_dialog_opens_no_second() {
        let mut f = Filing::new(None);
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(1)));
        want(&mut f, Want::Close, &unsaved(1));
        assert_eq!(
            reply(&mut f, Reply::Save, &unsaved(1)),
            FileOutcome::said(
                "nothing was saved: a file dialog is already open; nothing was closed"
            )
        );
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/d/n.relabel")), &unsaved(1)).next,
            Next::Write(path("/d/n.relabel"))
        );
    }

    #[test]
    fn saving_from_the_close_question_over_a_dialog_closes_once_written() {
        let mut f = at("/l/a.relabel");
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(2)));
        want(&mut f, Want::Close, &unsaved(2));
        reply(&mut f, Reply::Save, &unsaved(2));
        let out = f.apply(
            FileCmd::Wrote {
                to: path("/l/a.relabel"),
                result: Ok(()),
            },
            &clean(),
        );
        assert_eq!(out.next, Next::Hide);
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/l/c.relabel")), &clean()),
            FileOutcome::quiet()
        );
    }

    #[test]
    fn closing_while_the_new_label_pane_is_open_closes_it() {
        let mut f = at("/l/a.relabel");
        assert_eq!(want(&mut f, Want::New, &clean()).next, Next::Size);
        for w in Want::BAR {
            assert_eq!(
                want(&mut f, w, &clean()),
                FileOutcome::said("finish or cancel the new label first"),
                "{w:?}"
            );
        }
        assert_eq!(want(&mut f, Want::Close, &clean()).next, Next::Allow);
        // The pane's own answer, if it comes, finds nothing waiting for it.
        assert_eq!(
            f.apply(FileCmd::Sized(Some(d110_m_at(400))), &clean()),
            FileOutcome::quiet()
        );
    }

    #[test]
    fn an_unwritable_label_is_refused_before_any_dialog() {
        let why = "element 3 has a size of 0, which cannot be saved";
        let now = Now {
            unsaved: true,
            unwritable: Some(why.into()),
            ..Now::default()
        };
        let said = format!("this label cannot be saved: {why}; nothing was written");
        for w in [Want::Save, Want::SaveAs] {
            let mut f = Filing::new(None);
            assert_eq!(
                want(&mut f, w, &now),
                FileOutcome::said(said.clone()),
                "{w:?}"
            );
            // Back to idle: nothing is left waiting.
            assert_eq!(
                want(&mut f, Want::Close, &now).next,
                Next::Ask("Save this label before closing?".into())
            );
        }
        let mut f = Filing::new(None);
        want(&mut f, Want::Close, &now);
        // The question went away with the answer, so the window staying open is
        // said as well.
        assert_eq!(
            reply(&mut f, Reply::Save, &now),
            FileOutcome::said(format!("{said}; nothing was closed"))
        );

        let mut f = at("/l/a.relabel");
        assert_eq!(
            want(&mut f, Want::Save, &now),
            FileOutcome::said(format!(
                "a.relabel cannot be saved: {why}; nothing was written"
            ))
        );
    }

    #[test]
    fn a_close_during_the_question_turns_it_into_the_close_question() {
        let mut f = at("/l/a.relabel");
        assert_eq!(
            want(&mut f, Want::New, &unsaved(4)).next,
            Next::Ask("Save the changes to a.relabel before starting a new label?".into())
        );
        assert_eq!(
            want(&mut f, Want::Close, &unsaved(4)).next,
            Next::Ask("Save the changes to a.relabel before closing?".into())
        );
        assert_eq!(reply(&mut f, Reply::DontSave, &unsaved(4)).next, Next::Hide);
    }

    /// An assistive technology can press a button behind the popup; the question
    /// is shown again rather than the command lost or run under it.
    #[test]
    fn a_command_behind_the_question_shows_it_again() {
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Open, &unsaved(4));
        let out = want(&mut f, Want::Save, &unsaved(4));
        assert_eq!(
            out,
            FileOutcome {
                next: Next::Ask(
                    "Save the changes to a.relabel before opening another label?".into()
                ),
                note: Some("answer the question about unsaved changes first".into()),
            }
        );
    }

    #[test]
    fn cancel_says_what_did_not_happen() {
        for (w, note) in [
            (Want::Close, "nothing was closed"),
            (Want::New, "no new label was started"),
            (Want::Open, "nothing was opened"),
        ] {
            let mut f = at("/l/a.relabel");
            want(&mut f, w, &unsaved(1));
            assert_eq!(
                reply(&mut f, Reply::Cancel, &unsaved(1)),
                FileOutcome::said(note),
                "{w:?}"
            );
            // A second answer to a question already gone does nothing.
            assert_eq!(
                reply(&mut f, Reply::DontSave, &unsaved(1)),
                FileOutcome::quiet()
            );
        }
    }

    #[test]
    fn the_file_chords_leave_control_shift_o_to_the_rectangle() {
        for (text, shift, want) in [
            ("s", false, Some(Want::Save)),
            ("S", true, Some(Want::SaveAs)),
            ("s", true, Some(Want::SaveAs)),
            ("o", false, Some(Want::Open)),
            ("O", true, None),
            ("n", false, Some(Want::New)),
            ("N", true, None),
            ("z", false, None),
            ("ss", false, None),
            ("", false, None),
        ] {
            assert_eq!(Want::of_key(text, shift), want, "{text:?} {shift}");
        }
        for w in Want::BAR {
            assert_eq!(w.token().and_then(Want::from_token), Some(w));
        }
        assert_eq!(Want::Close.token(), None);
        assert_eq!(Want::from_token(5), None);
        assert_eq!(Want::from_token(-1), None);
        for r in Reply::ALL {
            assert_eq!(Reply::from_token(r.token()), Some(r));
        }
    }

    /// The fifth command opens the print pane: named Print, on Control+P, with
    /// token 4. Its refusals name the print pane.
    #[test]
    fn print_is_the_fifth_command() {
        for (text, shift, want) in [
            ("p", false, Some(Want::Preview)),
            ("P", false, Some(Want::Preview)),
            ("p", true, None),
            ("P", true, None),
        ] {
            assert_eq!(Want::of_key(text, shift), want, "{text:?} {shift}");
        }
        assert_eq!(Want::Preview.token(), Some(4));
        assert_eq!(Want::from_token(4), Some(Want::Preview));
        assert_eq!(Want::from_token(5), None);
        assert_eq!(
            (
                Want::Preview.caption(),
                Want::Preview.button_name(),
                Want::Preview.keys()
            ),
            ("Print", "Print the label", "Control+P")
        );
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Preview, &clean());
        assert_eq!(
            want(&mut f, Want::Save, &clean()),
            FileOutcome::said("close the print pane first")
        );
        let held = Now {
            pressed: true,
            ..clean()
        };
        let mut f = at("/l/a.relabel");
        assert_eq!(
            want(&mut f, Want::Preview, &held),
            FileOutcome::said("the print pane was not opened: let go of the element first")
        );
    }

    #[test]
    fn preview_opens_without_asking_about_unsaved_work() {
        let mut f = at("/l/a.relabel");
        assert!(!f.previewing());
        assert_eq!(
            want(&mut f, Want::Preview, &unsaved(3)),
            FileOutcome::then(Next::Preview)
        );
        assert!(f.previewing());
    }

    #[test]
    fn preview_is_refused_while_an_element_is_held() {
        let now = Now {
            pressed: true,
            ..unsaved(1)
        };
        let mut f = at("/l/a.relabel");
        assert_eq!(
            want(&mut f, Want::Preview, &now),
            FileOutcome::said("the print pane was not opened: let go of the element first")
        );
        assert!(!f.previewing());
    }

    /// Only an assistive technology can reach the File bar behind the pane, and
    /// what it presses there is refused, not run under the pane.
    #[test]
    fn file_commands_are_refused_while_the_preview_is_open() {
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Preview, &unsaved(1));
        for w in [Want::New, Want::Open, Want::Save, Want::SaveAs] {
            assert_eq!(
                want(&mut f, w, &unsaved(1)),
                FileOutcome::said("close the print pane first"),
                "{w:?}"
            );
            assert!(f.previewing(), "{w:?}");
        }
        assert_eq!(
            want(&mut f, Want::Preview, &unsaved(1)),
            FileOutcome::then(Next::Preview)
        );
        assert!(f.previewing());
    }

    #[test]
    fn closing_with_the_preview_open_asks_or_allows() {
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Preview, &clean());
        assert_eq!(want(&mut f, Want::Close, &clean()).next, Next::Allow);

        let mut f = Filing::new(None);
        want(&mut f, Want::Preview, &unsaved(2));
        assert_eq!(
            want(&mut f, Want::Close, &unsaved(2)).next,
            Next::Ask("Save this label before closing?".into())
        );
        assert!(!f.previewing());
        // Cancelled, the window stays with no pane, and the commands are free.
        assert_eq!(
            reply(&mut f, Reply::Cancel, &unsaved(2)),
            FileOutcome::said("nothing was closed")
        );
        assert!(matches!(
            want(&mut f, Want::SaveAs, &unsaved(2)).next,
            Next::Choose { .. }
        ));
    }

    #[test]
    fn the_preview_closing_frees_the_file_commands() {
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Preview, &clean());
        assert_eq!(
            f.apply(FileCmd::PreviewClosed, &clean()),
            FileOutcome::quiet()
        );
        assert!(!f.previewing());
        assert_eq!(want(&mut f, Want::New, &clean()).next, Next::Size);
        // A late close, with the New label pane showing, leaves it showing.
        assert_eq!(
            f.apply(FileCmd::PreviewClosed, &clean()),
            FileOutcome::quiet()
        );
        assert_eq!(
            want(&mut f, Want::Preview, &clean()),
            FileOutcome::said("finish or cancel the new label first")
        );
    }

    /// The question and the dialog keep the one place: Control+P over either is
    /// refused with their own words.
    #[test]
    fn preview_is_refused_while_a_question_or_dialog_is_open() {
        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Open, &unsaved(4));
        let out = want(&mut f, Want::Preview, &unsaved(4));
        assert_eq!(
            out.note.as_deref(),
            Some("answer the question about unsaved changes first")
        );
        assert!(!f.previewing());

        let mut f = at("/l/a.relabel");
        want(&mut f, Want::Open, &clean());
        assert_eq!(
            want(&mut f, Want::Preview, &clean()),
            FileOutcome::said("a file dialog is already open")
        );
        assert!(!f.previewing());
    }

    #[test]
    fn the_title_marks_unsaved_work() {
        assert_eq!(
            title(Some("a.relabel"), false),
            "a.relabel \u{2014} relabel"
        );
        assert_eq!(
            title(Some("a.relabel"), true),
            "*a.relabel \u{2014} relabel"
        );
        assert_eq!(title(None, false), "Untitled label \u{2014} relabel");
        assert_eq!(title(None, true), "*Untitled label \u{2014} relabel");
    }

    #[test]
    fn a_typed_name_gets_the_extension_unless_that_name_is_taken() {
        let nothing = |_: &Path| false;
        for (chosen, written) in [
            ("/d/a", "/d/a.relabel"),
            ("/d/a.relabel", "/d/a.relabel"),
            ("/d/a.RELABEL", "/d/a.RELABEL"),
            ("/d/a.json", "/d/a.json.relabel"),
        ] {
            assert_eq!(with_extension(path(chosen), nothing), Ok(path(written)));
        }
        let taken = |p: &Path| p == Path::new("/d/a.relabel");
        let e = with_extension(path("/d/a"), taken).expect_err("the name is taken");
        assert!(
            e.contains("a.relabel already exists and the dialog did not ask about replacing it"),
            "{e}"
        );

        // Through the machine, which asks its own injected question.
        let mut f = Filing::new(None);
        f.exists = |p| p == Path::new("/d/a.relabel");
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(1)));
        let out = chosen(&mut f, t, Picked::Path(path("/d/a")), &unsaved(1));
        assert_eq!(out.next, Next::Nothing);
        assert!(out.note.expect("said").contains("already exists"));
        let t = ticket_of(&want(&mut f, Want::SaveAs, &unsaved(1)));
        let out = chosen(&mut f, t, Picked::Path(path("/d/b")), &unsaved(1));
        assert_eq!(out.next, Next::Write(path("/d/b.relabel")));
    }

    #[test]
    fn save_as_to_another_folder_says_where_a_relative_csv_is_now_read() {
        let now = Now {
            csv: Some("people.csv".into()),
            ..unsaved(1)
        };
        let write = |from: &str, to: &str| {
            let mut f = at(from);
            let t = ticket_of(&want(&mut f, Want::SaveAs, &now));
            assert_eq!(
                chosen(&mut f, t, Picked::Path(path(to)), &now).next,
                Next::Write(path(to))
            );
            let out = f.apply(
                FileCmd::Wrote {
                    to: path(to),
                    result: Ok(()),
                },
                &now,
            );
            assert_eq!(f.path(), Some(Path::new(to)));
            out.note.expect("said")
        };
        assert_eq!(
            write("/one/a.relabel", "/two/b.relabel"),
            "saved b.relabel; its data file people.csv is now looked for in two"
        );
        assert_eq!(write("/one/a.relabel", "/one/c.relabel"), "saved c.relabel");
    }

    #[test]
    fn opening_says_what_opened_and_a_failure_keeps_the_current_file() {
        let mut f = at("/l/a.relabel");
        let out = f.apply(
            FileCmd::Opened {
                from: Source::File(path("/l/bad.relabel")),
                result: Err("it is empty".into()),
            },
            &clean(),
        );
        assert_eq!(
            out,
            FileOutcome::said("could not open bad.relabel: it is empty; a.relabel is still open")
        );
        assert_eq!(f.path(), Some(Path::new("/l/a.relabel")));

        let out = f.apply(
            FileCmd::Opened {
                from: Source::File(path("/m/b.relabel")),
                result: Ok("the head runs across the top of the canvas".into()),
            },
            &clean(),
        );
        assert_eq!(
            out,
            FileOutcome::said("opened b.relabel: the head runs across the top of the canvas")
        );
        assert_eq!(f.path(), Some(Path::new("/m/b.relabel")));

        let out = f.apply(
            FileCmd::Opened {
                from: Source::New(d110_m_at(400)),
                result: Ok("the head runs down the side".into()),
            },
            &clean(),
        );
        assert_eq!(
            out.note.as_deref(),
            Some("new 40 \u{d7} 12 mm label for a D110_M: the head runs down the side")
        );
        assert_eq!(f.path(), None);
        // A printer the New label pane said was assumed is said to be here too.
        let out = f.apply(
            FileCmd::Opened {
                from: Source::New(NewSpec {
                    assumed: Some(fresh::NOT_CONNECTED),
                    ..d110_m_at(300)
                }),
                result: Ok("the head runs down the side".into()),
            },
            &clean(),
        );
        assert_eq!(
            out.note.as_deref(),
            Some(
                "new 30 \u{d7} 12 mm label for a D110_M (assumed: relabel is not connected to a \
                 printer): the head runs down the side"
            )
        );
        let out = f.apply(
            FileCmd::Opened {
                from: Source::File(path("/m/c.relabel")),
                result: Err("it does not exist".into()),
            },
            &clean(),
        );
        assert!(
            out.note
                .expect("said")
                .ends_with("; the untitled label is still open")
        );
    }

    #[test]
    fn a_clean_open_and_new_go_straight_ahead() {
        let mut f = at("/l/a.relabel");
        assert_eq!(want(&mut f, Want::New, &clean()).next, Next::Size);
        assert_eq!(
            f.apply(FileCmd::Sized(None), &clean()),
            FileOutcome::said("no new label was started")
        );
        want(&mut f, Want::New, &clean());
        assert_eq!(
            f.apply(FileCmd::Sized(Some(d110_m_at(300))), &clean()).next,
            Next::Fresh(d110_m_at(300))
        );
        let t = ticket_of(&want(&mut f, Want::Open, &clean()));
        assert_eq!(
            chosen(&mut f, t, Picked::Path(path("/l/b.relabel")), &clean()).next,
            Next::Read(path("/l/b.relabel"))
        );
    }
}
