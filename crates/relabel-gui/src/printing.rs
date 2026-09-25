//! The printer link as the editor sees it: looking for printers, choosing one,
//! connecting, what is connected, printing on it, and disconnecting - and every
//! sentence the operator hears about it.
//!
//! A synchronous state machine with no toolkit and no radio in it, for
//! `filing`'s reason: one [`PrintCmd`] in, one [`PrintOutcome`] out, and every
//! rule reached by an ordinary unit test. The wiring sends the order an outcome
//! carries to relabel-core's link actor, and feeds what the actor hears back in
//! as [`PrintCmd::Heard`].
//!
//! # Tickets
//!
//! Every order carries the machine's current ticket, and every answer the ticket
//! of the order it answers. Stop waiting takes a new ticket, so an answer to what
//! was stopped - a scan that finished anyway, a connect that came through late -
//! is ignored without a word rather than taken for news. Print takes none: a job
//! runs on the open's ticket, so the open's drop is still heard while a job is
//! waited for.
//!
//! # What is said, and where
//!
//! A note is for the status line, which is live: looking, connecting, connected
//! or one failure, each said once. The handshake's steps only change the step
//! line, which is not live, so the status line makes three announcements over a
//! connect and not fourteen. The pane's verdict is live as well, and is said
//! again whenever its words change, as they usually do once when a connect or
//! a disconnect draws the preview for another printer, or for none. Every
//! failure is plain words: no protocol detail, no command byte, and no error
//! type's name.
//!
//! # Printing
//!
//! Print is taken only when connected to a printer relabel can print on, with
//! nothing else running and Copies a whole number from 1 to [`MAX_LABELS`]; the
//! page comes from the wiring, which takes it from the proof and never encodes
//! it again. A job is four announcements at most: its start, a stop, a close
//! held for it, and its end. Its news - which label, and the printer's own
//! percentages - only changes the Printer line and the job line, which are not
//! live. An ending that failed is one sentence in three parts: the cause, then
//! how many labels printed and whether the one in hand may be part printed,
//! then what to do.
//!
//! The job slot holds Print while connected, and Stop after this label while a
//! job of more than one label can still be stopped. Whenever it changes face,
//! the keyboard goes to Close, for `stop_waiting`'s reason: a second Return, or
//! Return held down, must not stop the job it started, nor print again.
//!
//! # Generations
//!
//! [`Printing::generation`] moves whenever what [`Printing::facts`] returns does,
//! which is what a proof records so the wiring can tell it was drawn for another
//! printer, or for none. A job keeps the facts, so it moves only when the job
//! ends with the link gone.

use relabel_core::Timeouts;
use relabel_core::link::{
    CONNECT_WITHIN, Facts, Failure, Heard, LinkAfter, MAX_LABELS, NotPrintable, Order,
    PrintRefusal, Seen,
};
use relabel_core::{HandshakeStep, JobError, Reached, RunEnd, RunNews, RunReport, SessionError};
use relabel_proto::{EncodedPage, LabelType, PrinterErrorCode};

/// How long a scan listens, in seconds.
pub const FIND_SECS: u64 = 4;

/// How many printers are offered as choices. The pane has this many buttons.
pub const MAX_CHOICES: usize = 4;

/// Said when the link actor has stopped, which only a panic inside it does.
const BROKEN: &str =
    "The printer connection stopped working inside relabel; restart relabel to connect again.";

const NOT_CONNECTED: &str = "Not connected. Connect looks for your printer over Bluetooth.";

/// The Copies field's accessible description.
pub const COPIES_SAYS: &str = "How many labels to print, from 1 to 99";

const COPIES_REFUSED: &str = "Copies must be a whole number from 1 to 99, so nothing was printed.";

/// Said when Print finds the label changed since the pane drew it.
const STALE: &str =
    "Nothing printed: the label changed, so the preview was drawn again; check it, then print.";

/// What the operator did, or what the link said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrintCmd {
    Connect,
    /// One of the offered printers, by its place among them.
    Choose(usize),
    StopWaiting,
    Disconnect,
    /// Print the proof's page, or why the wiring has none to give.
    Print(Result<EncodedPage, NotReady>),
    /// Start no more labels once the one in hand has printed.
    StopAfterLabel,
    /// The Copies field's text, as typed. Kept, and read only by Print.
    Copies(String),
    /// The window's close button, pressed.
    WindowClosing,
    Heard(Heard),
    /// The link actor has ended, and nothing more will be heard.
    LinkEnded,
}

/// Why the wiring had no page for Print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotReady {
    /// The document or the link moved since the proof was drawn. The wiring
    /// has drawn it again, for the operator to check before printing.
    Stale,
    /// The proof stops it, preflight refuses it, or it has no page.
    NotPrintable,
}

/// The button in the pane's primary slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primary {
    Connect,
    StopWaiting,
    CancelChoice,
    Disconnect,
}

impl Primary {
    /// The button's face.
    pub fn face(self) -> &'static str {
        match self {
            Primary::Connect => "Connect",
            Primary::StopWaiting => "Stop waiting",
            Primary::CancelChoice => "Cancel",
            Primary::Disconnect => "Disconnect",
        }
    }

    /// Whether it is the pane's default button, and so drawn as the primary one:
    /// only Connect, so that Return in the pane can never disconnect.
    pub fn is_default(self) -> bool {
        self == Primary::Connect
    }

    /// The command the button gives.
    pub fn cmd(self) -> PrintCmd {
        match self {
            Primary::Connect => PrintCmd::Connect,
            Primary::StopWaiting | Primary::CancelChoice => PrintCmd::StopWaiting,
            Primary::Disconnect => PrintCmd::Disconnect,
        }
    }
}

/// The button in the pane's job slot. Never the default button: Return in the
/// pane never prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSlot {
    Print,
    StopAfterLabel,
}

impl JobSlot {
    /// The button's face.
    pub fn face(self) -> &'static str {
        match self {
            JobSlot::Print => "Print",
            JobSlot::StopAfterLabel => "Stop after this label",
        }
    }
}

/// The job line: how far a job has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLine {
    pub printed: u16,
    pub of: u16,
    /// What is happening now, in words. Not live.
    pub says: String,
}

impl JobLine {
    /// Labels printed as a share of the job, from 0 to 1: the bar's value.
    pub fn done(&self) -> f32 {
        f32::from(self.printed) / f32::from(self.of.max(1))
    }
}

/// Where the keyboard goes in the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Primary,
    Close,
    FirstChoice,
}

impl Focus {
    /// The integer the pane's `focus-to` takes, for `Field::token`'s reason.
    pub fn token(self) -> i32 {
        match self {
            Focus::Primary => 0,
            Focus::Close => 1,
            Focus::FirstChoice => 2,
        }
    }
}

/// What the wiring does after a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintOutcome {
    /// For the link actor, with the ticket it goes with.
    pub order: Option<(u64, Order)>,
    /// Pre-formatted for the status line.
    pub note: Option<String>,
    /// Where the keyboard goes now, when this command moves it. Only while the
    /// pane is open.
    pub focus: Option<Focus>,
    /// A job the window's close was held for has ended: close it now, the way
    /// the close button would have.
    pub replay_close: bool,
}

impl PrintOutcome {
    fn quiet() -> PrintOutcome {
        PrintOutcome {
            order: None,
            note: None,
            focus: None,
            replay_close: false,
        }
    }

    fn said(note: impl Into<String>) -> PrintOutcome {
        PrintOutcome {
            note: Some(note.into()),
            ..PrintOutcome::quiet()
        }
    }

    fn moving(mut self, focus: Focus) -> PrintOutcome {
        self.focus = Some(focus);
        self
    }

    /// A job ended: the close held for it, if one was, goes ahead.
    fn ended(mut self, job: &Job) -> PrintOutcome {
        self.replay_close = job.closing;
        self
    }
}

/// Everything the pane and the File-bar row show about the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkView {
    /// The Printer line beside the File bar: what is connected.
    pub printer_line: String,
    /// The pane's step line: what is happening now.
    pub step: String,
    /// `None` hides the primary slot.
    pub primary: Option<Primary>,
    /// The primary button's accessible description.
    pub primary_says: String,
    /// The offered printers, as (face, accessible description).
    pub choices: Vec<(String, String)>,
    /// Printers found and not offered.
    pub more: usize,
    /// Where the keyboard lands when the pane opens.
    pub focus: Focus,
    /// `None` hides the job slot.
    pub job_slot: Option<JobSlot>,
    /// The job slot's accessible description.
    pub job_slot_says: String,
    /// The Copies field shows with Print, and only then.
    pub copies_shown: bool,
    /// The Copies text as last typed, which the field starts from when the
    /// pane opens: kept for the life of the window.
    pub copies_start: String,
    pub copies_says: &'static str,
    /// `None` hides the job line.
    pub job: Option<JobLine>,
    /// The image's accessible label.
    pub image_label: &'static str,
}

/// Where a job's label has got, for the job line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Checking,
    Sending(u16),
    Printer(u16, u8, u8),
}

/// A job, as its news has left it.
#[derive(Debug, Clone)]
struct Job {
    of: u16,
    /// The last label started, 0 before the first.
    started: u16,
    printed: u16,
    /// How far the label in hand, or the next one, has got.
    reached: Reached,
    at: At,
    /// Stop after this label was ordered, from the slot or by a close.
    stopping: bool,
    /// The window's close is held until the job ends.
    closing: bool,
}

impl Job {
    fn new(of: u16) -> Job {
        Job {
            of,
            started: 0,
            printed: 0,
            reached: Reached::Check,
            at: At::Checking,
            stopping: false,
            closing: false,
        }
    }

    fn heard(&mut self, news: RunNews) {
        match news {
            RunNews::Checking => {
                self.reached = Reached::Check;
                self.at = At::Checking;
            }
            RunNews::Starting { n, .. } => {
                self.started = n;
                self.reached = Reached::Settings;
                self.at = At::Sending(n);
            }
            RunNews::Sending { n, .. } => {
                self.reached = Reached::Rows;
                self.at = At::Sending(n);
            }
            RunNews::Printer {
                n,
                print_percent,
                feed_percent,
                ..
            } => {
                self.reached = Reached::Waiting;
                self.at = At::Printer(n, print_percent, feed_percent);
            }
            // The job line keeps the printer's last figures: the next label
            // starts straight after, with nothing sent between.
            RunNews::Printed { n, .. } => {
                self.printed = n;
                self.reached = Reached::Settings;
            }
        }
    }

    /// Whether Stop after this label would still leave a label unprinted.
    fn can_stop(&self) -> bool {
        self.of > 1 && !self.stopping && self.started < self.of
    }

    /// How many labels printed, for a job that ends with no report.
    fn so_far(&self) -> String {
        if self.started > self.printed {
            clause(self.of, self.started, self.printed, self.reached)
        } else if self.printed >= self.of {
            match self.of {
                1 => "The label printed.".into(),
                of => format!("All {of} labels printed."),
            }
        } else {
            clause(self.of, self.printed + 1, self.printed, Reached::Settings)
        }
    }
}

#[derive(Debug, Clone)]
enum State {
    /// `last` is the last failure, which the step line keeps saying.
    Off {
        last: Option<String>,
    },
    Looking,
    Choosing {
        seen: Vec<Seen>,
    },
    Opening {
        name: String,
    },
    Asking {
        name: String,
        step: HandshakeStep,
    },
    /// `last` is how the last job ended, which the step line keeps saying.
    Ready {
        facts: Facts,
        last: Option<String>,
    },
    /// Disconnect was pressed. The facts stay until the link says Closed, so a
    /// proof drawn for the printer is not redrawn twice over one disconnect.
    Closing {
        facts: Facts,
    },
    /// A job is running on the printer these facts describe.
    Printing {
        facts: Facts,
        job: Job,
    },
    Broken,
}

/// The editor's side of the printer link.
#[derive(Debug, Clone)]
pub struct Printing {
    ticket: u64,
    generation: u64,
    state: State,
    /// The Copies field's text, kept for the life of the window.
    copies: String,
}

impl Default for Printing {
    fn default() -> Printing {
        Printing::new()
    }
}

impl Printing {
    pub fn new() -> Printing {
        Printing {
            ticket: 0,
            generation: 0,
            state: State::Off { last: None },
            copies: "1".into(),
        }
    }

    /// The printer connected and identified, when there is one.
    pub fn facts(&self) -> Option<&Facts> {
        match &self.state {
            State::Ready { facts, .. }
            | State::Closing { facts }
            | State::Printing { facts, .. } => Some(facts),
            _ => None,
        }
    }

    /// Moves whenever [`Printing::facts`] does.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a job is running, which holds the window's close.
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Printing { .. })
    }

    pub fn apply(&mut self, cmd: PrintCmd) -> PrintOutcome {
        let slot = self.job_slot();
        let mut out = match cmd {
            PrintCmd::Connect => self.connect(),
            PrintCmd::Choose(i) => self.choose(i),
            PrintCmd::StopWaiting => self.stop_waiting(),
            PrintCmd::Disconnect => self.disconnect(),
            PrintCmd::Print(page) => self.print(page),
            PrintCmd::StopAfterLabel => self.stop_after_label(),
            PrintCmd::Copies(text) => {
                self.copies = text;
                PrintOutcome::quiet()
            }
            PrintCmd::WindowClosing => self.window_closing(),
            PrintCmd::Heard(h) => self.heard(h),
            PrintCmd::LinkEnded => {
                if self.facts().is_some() {
                    self.generation += 1;
                }
                let state = std::mem::replace(&mut self.state, State::Broken);
                // The primary slot is gone, so the keyboard is not left on it.
                let out = match state {
                    State::Printing { job, .. } => {
                        PrintOutcome::said(format!("{BROKEN} {}", job.so_far())).ended(&job)
                    }
                    _ => PrintOutcome::said(BROKEN),
                };
                out.moving(Focus::Close)
            }
        };
        // The job slot changed face under the keyboard, which goes to Close:
        // Print became Stop, or Stop hid, or the job ended and Stop became
        // Print. A second Return must not stop what it started, or print again.
        if out.focus.is_none() && self.job_slot() != slot {
            out.focus = Some(Focus::Close);
        }
        out
    }

    fn order(&self, order: Order) -> Option<(u64, Order)> {
        Some((self.ticket, order))
    }

    /// The Copies text as a count of labels, when it is one relabel prints.
    fn labels(&self) -> Option<u16> {
        self.copies
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|n| (1..=MAX_LABELS).contains(n))
    }

    fn connect(&mut self) -> PrintOutcome {
        match self.state {
            State::Off { .. } => {
                self.ticket += 1;
                self.state = State::Looking;
                // Connect becomes Stop waiting under the keyboard that pressed it,
                // so the keyboard goes to Close, for `open`'s reason.
                PrintOutcome {
                    order: self.order(Order::Find { secs: FIND_SECS }),
                    ..PrintOutcome::said("Looking for printers.")
                }
                .moving(Focus::Close)
            }
            State::Broken => PrintOutcome::said(BROKEN),
            _ => PrintOutcome::quiet(),
        }
    }

    fn choose(&mut self, i: usize) -> PrintOutcome {
        let State::Choosing { seen } = &self.state else {
            return PrintOutcome::quiet();
        };
        match seen.iter().take(MAX_CHOICES).nth(i) {
            Some(s) => {
                let s = s.clone();
                self.open(&s)
            }
            None => PrintOutcome::quiet(),
        }
    }

    /// Connect to a printer a scan found. Never held back because it was not
    /// heard: what "heard" says after a scan is inferred, not verified.
    ///
    /// The primary slot is Stop waiting from here on, so the keyboard goes to
    /// Close: after a choice, a second Return, or Return held down, must not stop
    /// the connect it just started; and after one printer is opened unasked,
    /// Stop waiting would now stop a connect nobody pressed for.
    fn open(&mut self, s: &Seen) -> PrintOutcome {
        self.state = State::Opening {
            name: s.name.clone(),
        };
        let mut note = format!("Connecting to {}.", s.name);
        if !s.heard {
            note.push_str(&format!(
                " It was not heard in the scan, so this can take up to {} seconds.",
                CONNECT_WITHIN.as_secs()
            ));
        }
        PrintOutcome {
            order: self.order(Order::Open { key: s.key }),
            ..PrintOutcome::said(note)
        }
        .moving(Focus::Close)
    }

    /// Stop waiting, or Cancel while choosing. Either becomes Connect under the
    /// keyboard that pressed it, so the keyboard goes to Close, for `open`'s
    /// reason the other way round: a second Return, or Return held down, must not
    /// start the scan it just stopped.
    fn stop_waiting(&mut self) -> PrintOutcome {
        let note = match &self.state {
            State::Looking => "Stopped looking for printers.".to_string(),
            State::Opening { name } | State::Asking { name, .. } => {
                format!("Stopped connecting to {name}.")
            }
            State::Choosing { .. } => {
                self.state = State::Off { last: None };
                return PrintOutcome::said("No printer was chosen.").moving(Focus::Close);
            }
            _ => return PrintOutcome::quiet(),
        };
        self.ticket += 1;
        self.state = State::Off { last: None };
        PrintOutcome {
            order: self.order(Order::StopWaiting),
            ..PrintOutcome::said(note)
        }
        .moving(Focus::Close)
    }

    fn disconnect(&mut self) -> PrintOutcome {
        let State::Ready { facts, .. } = &self.state else {
            return PrintOutcome::quiet();
        };
        self.state = State::Closing {
            facts: facts.clone(),
        };
        // The primary slot hides while closing, so the keyboard is not left on it.
        PrintOutcome {
            order: self.order(Order::Close),
            ..PrintOutcome::quiet()
        }
        .moving(Focus::Close)
    }

    /// Print `page` as many labels as Copies says, each its own one-copy job.
    /// Taken only when connected, with nothing running, to a printer relabel can
    /// print on, and with Copies a whole number it prints. Anything else prints
    /// nothing and says why. The ticket stays the open's, so a drop is still
    /// heard.
    fn print(&mut self, page: Result<EncodedPage, NotReady>) -> PrintOutcome {
        let facts = match &self.state {
            State::Ready { facts, .. } => facts,
            State::Printing { .. } => {
                return PrintOutcome::said("Nothing printed: a print is already running.");
            }
            State::Broken => {
                return PrintOutcome::said(format!("Nothing printed: {}", lower_first(BROKEN)));
            }
            _ => return PrintOutcome::said("Nothing printed: no printer is connected."),
        };
        let page = match page {
            Ok(page) => page,
            Err(NotReady::Stale) => return PrintOutcome::said(STALE),
            Err(NotReady::NotPrintable) => {
                return PrintOutcome::said(
                    "Nothing printed: this label cannot print here, and the print pane says why.",
                );
            }
        };
        if let Err(why) = &facts.printable {
            return PrintOutcome::said(format!(
                "Nothing printed: relabel cannot print on {}, because {}.",
                facts.name,
                why_not(why)
            ));
        }
        let Some(labels) = self.labels() else {
            return PrintOutcome::said(COPIES_REFUSED);
        };
        let note = match labels {
            1 => format!("Printing on {}.", facts.name),
            n => format!("Printing {n} labels on {}.", facts.name),
        };
        self.state = State::Printing {
            facts: facts.clone(),
            job: Job::new(labels),
        };
        PrintOutcome {
            order: self.order(Order::Print { page, labels }),
            ..PrintOutcome::said(note)
        }
        .moving(Focus::Close)
    }

    /// Stop after this label: read by the link before every label, the first
    /// one included, so the label in hand always finishes. The slot hides under
    /// the keyboard that pressed it, which goes to Close.
    ///
    /// The note names no label. The link starts a label as soon as it has read
    /// the stop unset, and its news of that start reaches the machine later, so
    /// the label in hand may be one past `job.started`, or the first while
    /// `started` is still 0. The ending says which labels printed.
    fn stop_after_label(&mut self) -> PrintOutcome {
        let ticket = self.ticket;
        let State::Printing { job, .. } = &mut self.state else {
            return PrintOutcome::quiet();
        };
        if !job.can_stop() {
            return PrintOutcome::quiet();
        }
        job.stopping = true;
        let note = match job.started {
            0 => "Stopping after the label in progress, if any; no other label will start.",
            _ => "Stopping after the label in progress; no other label will start.",
        };
        PrintOutcome {
            order: Some((ticket, Order::StopAfterLabel)),
            ..PrintOutcome::said(note)
        }
        .moving(Focus::Close)
    }

    /// The window's close button while a job runs: the close is held until the
    /// job ends, and no label starts after the one in hand. Pressed again, it
    /// says the same again. With no job, nothing holds the close. The note names
    /// no label, for `stop_after_label`'s reason.
    fn window_closing(&mut self) -> PrintOutcome {
        let ticket = self.ticket;
        let State::Printing { job, .. } = &mut self.state else {
            return PrintOutcome::quiet();
        };
        job.closing = true;
        let order = (!job.stopping && job.started < job.of).then(|| {
            job.stopping = true;
            (ticket, Order::StopAfterLabel)
        });
        let note = match (job.started, job.of - job.started) {
            (0, _) => {
                "Closing once the label in progress, if any, has printed; no other label will \
                 start."
            }
            // The last label has started: no stop was needed.
            (_, 0) => "Closing once the label has printed.",
            _ => "Closing once the label in progress has printed; no other label will start.",
        };
        PrintOutcome {
            order,
            ..PrintOutcome::said(note)
        }
    }

    /// What the link said. News that turns the primary slot into Connect - none
    /// found, a failure, a drop - moves the keyboard to Close, for
    /// `stop_waiting`'s reason: a Return meant to stop the wait, or to disconnect,
    /// must not start a scan. Closed needs no move: the slot was hidden while
    /// closing, and `disconnect` has already moved it.
    fn heard(&mut self, h: Heard) -> PrintOutcome {
        if h.ticket() != self.ticket {
            return PrintOutcome::quiet();
        }
        let state = std::mem::replace(&mut self.state, State::Broken);
        let (state, out) = match (state, h) {
            (State::Looking, Heard::Found { mut seen, .. }) => {
                // Stable, so the link's own order holds within each group.
                seen.sort_by_key(|s| !s.heard);
                match seen.len() {
                    0 => (
                        State::Off { last: None },
                        PrintOutcome::said(
                            "No printer found. Check that it is switched on and nearby.",
                        )
                        .moving(Focus::Close),
                    ),
                    1 => {
                        let out = self.open(&seen[0]);
                        return out;
                    }
                    n => {
                        let mut note = format!("Found {n} printers: choose one.");
                        if let Some(more) = more_than_offered(n) {
                            note.push(' ');
                            note.push_str(&more);
                        }
                        (
                            State::Choosing { seen },
                            PrintOutcome::said(note).moving(Focus::FirstChoice),
                        )
                    }
                }
            }
            (
                State::Looking | State::Opening { .. } | State::Asking { .. },
                Heard::Failed { failure, .. },
            ) => {
                let said = failure_says(&failure);
                (
                    State::Off {
                        last: Some(said.clone()),
                    },
                    PrintOutcome::said(said).moving(Focus::Close),
                )
            }
            (State::Opening { name } | State::Asking { name, .. }, Heard::Step { step, .. }) => {
                (State::Asking { name, step }, PrintOutcome::quiet())
            }
            (State::Opening { .. } | State::Asking { .. }, Heard::Ready { facts, .. }) => {
                self.generation += 1;
                let note = ready_says(&facts);
                // Return then closes the pane, and can never disconnect.
                (
                    State::Ready { facts, last: None },
                    PrintOutcome::said(note).moving(Focus::Close),
                )
            }
            (State::Ready { facts, .. } | State::Closing { facts }, Heard::Dropped { .. }) => {
                self.generation += 1;
                let said = format!(
                    "The connection to {} dropped. Connect again to use it.",
                    facts.name
                );
                (
                    State::Off {
                        last: Some(said.clone()),
                    },
                    PrintOutcome::said(said).moving(Focus::Close),
                )
            }
            (State::Closing { facts }, Heard::Closed { .. }) => {
                self.generation += 1;
                (
                    State::Off { last: None },
                    PrintOutcome::said(format!("Disconnected from {}.", facts.name)),
                )
            }
            // A job's news changes the Printer line and the job line, which are
            // not live, and says nothing.
            (State::Printing { facts, mut job }, Heard::Job { news, .. }) => {
                job.heard(news);
                (State::Printing { facts, job }, PrintOutcome::quiet())
            }
            (State::Printing { facts, job }, Heard::JobEnded { report, link, .. }) => {
                let said = job_says(
                    &facts.name,
                    &report,
                    link,
                    (facts.print_density, facts.print_label_type),
                );
                let state = match link {
                    LinkAfter::Up => State::Ready {
                        facts,
                        last: Some(said.clone()),
                    },
                    LinkAfter::Gone => {
                        self.generation += 1;
                        State::Off {
                            last: Some(said.clone()),
                        }
                    }
                };
                (state, PrintOutcome::said(said).ended(&job))
            }
            // Heard before the link took the Print, which it will now refuse:
            // the drop came first, so nothing was sent.
            (State::Printing { facts, job }, Heard::Dropped { .. }) => {
                self.generation += 1;
                let said = format!(
                    "The connection to {} dropped, so nothing was printed. Connect again to use \
                     it.",
                    facts.name
                );
                (
                    State::Off {
                        last: Some(said.clone()),
                    },
                    PrintOutcome::said(said).ended(&job),
                )
            }
            // A second Print, which the machine never orders while one runs.
            (
                State::Printing { facts, job },
                Heard::PrintRefused {
                    why: PrintRefusal::Busy,
                    ..
                },
            ) => (State::Printing { facts, job }, PrintOutcome::quiet()),
            (State::Printing { facts, job }, Heard::PrintRefused { why, .. }) => {
                let said = refusal_says(&facts.name, why);
                let state = if why == PrintRefusal::NotConnected {
                    self.generation += 1;
                    State::Off {
                        last: Some(said.clone()),
                    }
                } else {
                    State::Ready {
                        facts,
                        last: Some(said.clone()),
                    }
                };
                (state, PrintOutcome::said(said).ended(&job))
            }
            // Anything else is an answer the machine is no longer waiting for.
            (state, _) => (state, PrintOutcome::quiet()),
        };
        self.state = state;
        out
    }

    fn job_slot(&self) -> Option<JobSlot> {
        match &self.state {
            State::Ready { facts, .. } if facts.printable.is_ok() => Some(JobSlot::Print),
            State::Printing { job, .. } if job.can_stop() => Some(JobSlot::StopAfterLabel),
            _ => None,
        }
    }

    pub fn view(&self) -> LinkView {
        self.view_for(None)
    }

    /// The view, with Print's description naming the label's size, as "30 ×
    /// 12 mm", when the wiring has a proof to take it from.
    pub fn view_for(&self, size: Option<&str>) -> LinkView {
        let (printer_line, step) = match &self.state {
            State::Off { last } => (
                "No printer connected".to_string(),
                last.clone().unwrap_or_else(|| NOT_CONNECTED.into()),
            ),
            State::Looking => (
                "Looking for printers".into(),
                format!("Looking for printers ({FIND_SECS} seconds)"),
            ),
            State::Choosing { seen } => {
                let mut step = format!("Found {} printers. Choose one.", seen.len());
                if let Some(more) = more_than_offered(seen.len()) {
                    step.push(' ');
                    step.push_str(&more);
                }
                ("Choose a printer (Control+P)".into(), step)
            }
            State::Opening { name } => (
                format!("Connecting to {name}"),
                format!(
                    "Connecting to {name} (up to {} seconds)",
                    CONNECT_WITHIN.as_secs()
                ),
            ),
            State::Asking { name, step } => (
                format!("Connecting to {name}"),
                format!("Connecting to {name}: {}", step.says()),
            ),
            State::Ready { facts, last } => (
                printer_line(facts),
                last.clone().unwrap_or_else(|| "Connected.".into()),
            ),
            State::Closing { facts } => (
                format!("Disconnecting from {}", facts.name),
                format!("Disconnecting from {}.", facts.name),
            ),
            State::Printing { facts, job } => (
                if job.started == 0 || job.of == 1 {
                    format!("{}: printing", facts.name)
                } else {
                    format!(
                        "{}: printing label {} of {}",
                        facts.name, job.started, job.of
                    )
                },
                format!("Printing on {}.", facts.name),
            ),
            State::Broken => ("No printer connected".into(), BROKEN.into()),
        };
        let primary = match &self.state {
            State::Off { .. } => Some(Primary::Connect),
            State::Looking | State::Opening { .. } | State::Asking { .. } => {
                Some(Primary::StopWaiting)
            }
            State::Choosing { .. } => Some(Primary::CancelChoice),
            State::Ready { .. } => Some(Primary::Disconnect),
            State::Closing { .. } | State::Printing { .. } | State::Broken => None,
        };
        let primary_says = match (primary, &self.state) {
            (Some(Primary::Connect), _) => {
                "Look for your printer over Bluetooth and connect to it".into()
            }
            (Some(Primary::StopWaiting), _) => "Stop looking or connecting".into(),
            (Some(Primary::CancelChoice), _) => "Choose no printer".into(),
            (Some(Primary::Disconnect), State::Ready { facts, .. }) => {
                format!("Close the Bluetooth link to {}", facts.name)
            }
            _ => String::new(),
        };
        let (choices, more) = match &self.state {
            State::Choosing { seen } => (
                seen.iter()
                    .take(MAX_CHOICES)
                    .map(|s| {
                        let face = if s.heard {
                            s.name.clone()
                        } else {
                            format!("{}, not heard just now", s.name)
                        };
                        (face, format!("Connect to {}", s.name))
                    })
                    .collect(),
                seen.len().saturating_sub(MAX_CHOICES),
            ),
            _ => (Vec::new(), 0),
        };
        // Where the keyboard lands when the pane opens: on Connect when it can
        // connect, on the first printer while choosing, and otherwise on Close,
        // so that Return never stops a connect, drops a link or prints by
        // accident.
        let focus = match &self.state {
            State::Off { .. } => Focus::Primary,
            State::Choosing { .. } => Focus::FirstChoice,
            _ => Focus::Close,
        };
        let job_slot = self.job_slot();
        let job_slot_says = match (job_slot, &self.state) {
            (Some(JobSlot::Print), State::Ready { facts, .. }) => self.print_says(facts, size),
            // No label named, for `stop_after_label`'s reason.
            (Some(JobSlot::StopAfterLabel), State::Printing { job, .. }) => match job.started {
                0 => "Finish the label in progress, if any, then print no more".into(),
                _ => "Finish the label in progress, then print no more".into(),
            },
            _ => String::new(),
        };
        let job = match &self.state {
            State::Printing { job, .. } => Some(JobLine {
                printed: job.printed,
                of: job.of,
                says: job_line(job.at, job.of),
            }),
            _ => None,
        };
        LinkView {
            printer_line,
            step,
            primary,
            primary_says,
            choices,
            more,
            focus,
            job_slot,
            job_slot_says,
            copies_shown: job_slot == Some(JobSlot::Print),
            copies_start: self.copies.clone(),
            copies_says: COPIES_SAYS,
            image_label: if job.is_some() {
                "What is printing"
            } else {
                "What will print"
            },
            job,
        }
    }

    /// Print's description: "Print 3 labels, 30 × 12 mm, on NAME at density 3
    /// on labels with gaps".
    fn print_says(&self, facts: &Facts, size: Option<&str>) -> String {
        let Some(labels) = self.labels() else {
            return "Nothing prints until Copies is a whole number from 1 to 99".into();
        };
        let size = size.map(|s| format!(", {s},")).unwrap_or_default();
        format!(
            "Print {}{size} on {} at density {} on {}",
            counted(u64::from(labels), "label", "labels"),
            facts.name,
            facts.print_density,
            label_type_says(facts.print_label_type)
        )
    }
}

/// "2 more were found and are not offered.", when more were found than offered.
fn more_than_offered(found: usize) -> Option<String> {
    match found.saturating_sub(MAX_CHOICES) {
        0 => None,
        1 => Some("1 more was found and is not offered.".into()),
        k => Some(format!("{k} more were found and are not offered.")),
    }
}

/// A count and its noun, the noun singular for one: "1 frame", "48 frames".
pub fn counted(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// `items` in a sentence: "a", "a and b", "a, b and c", with `and` or `or`.
pub fn joined(items: impl IntoIterator<Item = String>, word: &str) -> String {
    let mut items: Vec<String> = items.into_iter().collect();
    let Some(last) = items.pop() else {
        return String::new();
    };
    if items.is_empty() {
        return last;
    }
    format!("{} {word} {last}", items.join(", "))
}

/// A sentence with its first letter lower-cased, to follow a colon.
fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A clause with its first letter capitalised, to start a sentence.
fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Why a connected printer cannot be printed on, to follow "cannot print on it: ".
fn why_not(n: &NotPrintable) -> String {
    match n {
        NotPrintable::UnknownModel { .. } => {
            "it does not know this printer's print-head width".into()
        }
        NotPrintable::CandidatesDisagree { names } => format!(
            "its id is shared by {}, whose heads differ",
            joined(names.iter().map(|n| format!("the {n}")), "and")
        ),
    }
}

fn ready_says(facts: &Facts) -> String {
    match (&facts.printable, facts.model) {
        (Ok(()), Some(model)) => format!("Connected to {}, a {}.", facts.name, model.name),
        (Ok(()), None) => format!("Connected to {}.", facts.name),
        (Err(why), _) => format!(
            "Connected to {}, but relabel cannot print on it: {}.",
            facts.name,
            why_not(why)
        ),
    }
}

/// A battery level as the printer reports it, which is in quarters, in words.
fn battery_says(percent: u8) -> String {
    match percent {
        0 => "nearly empty".into(),
        25 => "about a quarter".into(),
        50 => "about half".into(),
        75 => "about three quarters".into(),
        100 => "full".into(),
        p => format!("about {p}%"),
    }
}

/// The Printer line for a connected printer. A part that is not known is left
/// out rather than said to be unknown.
fn printer_line(facts: &Facts) -> String {
    let id = facts
        .model_id
        .map(|id| format!(" (id {id})"))
        .unwrap_or_default();
    match (&facts.printable, facts.model) {
        (Ok(()), Some(model)) => {
            let mut parts = vec![
                format!("{}: a {}", facts.name, model.name),
                format!("{}-dot head at {} dpi", facts.head, facts.dpi),
            ];
            if let Some(fw) = &facts.firmware {
                parts.push(format!("firmware {fw}"));
            }
            if let Some(b) = facts.battery_percent {
                parts.push(format!("battery {}", battery_says(b)));
            }
            parts.join(", ")
        }
        (Err(NotPrintable::CandidatesDisagree { names }), _) => format!(
            "{}: {}{id}",
            facts.name,
            joined(names.iter().map(|n| format!("a {n}")), "or")
        ),
        _ => format!("{}: a printer relabel does not know{id}", facts.name),
    }
}

/// A find or an open that got nowhere, in plain words.
pub fn failure_says(f: &Failure) -> String {
    match f {
        Failure::NoBluetooth(_) => {
            if cfg!(target_os = "linux") {
                "relabel cannot reach this computer's Bluetooth service (bluetoothd, over the \
                 D-Bus system bus)."
                    .into()
            } else {
                "relabel cannot reach this computer's Bluetooth service.".into()
            }
        }
        Failure::NoAdapter => "This computer has no Bluetooth adapter.".into(),
        Failure::BluetoothOff => "Bluetooth is switched off.".into(),
        Failure::NotAllowed => {
            if cfg!(target_os = "macos") {
                "relabel is not allowed to use Bluetooth: allow it in System Settings, Privacy \
                 and Security, Bluetooth."
                    .into()
            } else {
                "relabel is not allowed to use Bluetooth.".into()
            }
        }
        Failure::ScanFailed(detail) => {
            format!("Looking for printers failed. Bluetooth said: {detail}")
        }
        Failure::NotFound { name } => {
            format!("{name} is no longer in Bluetooth's list. Connect again to look for it.")
        }
        Failure::TimedOut { name, secs } => format!(
            "{name} did not answer within {}. Check that it is switched on, nearby, and not \
             connected to another device.",
            counted(*secs, "second", "seconds")
        ),
        Failure::ConnectFailed { name, detail } => {
            format!("Could not connect to {name}. Bluetooth said: {detail}")
        }
        Failure::NoChannel { name } => format!(
            "{name} has no channel relabel knows how to use, so it is not a printer relabel can \
             talk to."
        ),
        Failure::Handshake { name, step } => format!(
            "{name} stopped answering while relabel was {}. Connect again.",
            step.says()
        ),
        Failure::DroppedWhileConnecting { name } => {
            format!("The connection to {name} dropped while connecting. Connect again.")
        }
    }
}

/// A label type in words, as the roll's packaging would say it.
pub fn label_type_says(t: LabelType) -> String {
    match t {
        LabelType::Gap => "labels with gaps".into(),
        LabelType::Black => "labels with black marks".into(),
        LabelType::Continuous => "continuous tape".into(),
        LabelType::Hole => "labels with holes".into(),
        LabelType::Transparent => "transparent labels".into(),
        LabelType::PvcTag => "PVC tags".into(),
        LabelType::BlackMarkGap => "labels with black marks and gaps".into(),
        LabelType::HeatShrinkTube => "heat-shrink tube".into(),
        LabelType::Unknown(c) => format!("label type {c}"),
    }
}

/// The job line: what the job is doing now.
fn job_line(at: At, of: u16) -> String {
    match (at, of) {
        (At::Checking, _) => "Checking that the printer answers.".into(),
        (At::Sending(_), 1) => "Sending the label.".into(),
        (At::Sending(n), of) => format!("Label {n} of {of}: sending."),
        (At::Printer(_, print, feed), 1) => {
            format!("The printer says {print}% printed, {feed}% fed.")
        }
        (At::Printer(n, print, feed), of) => {
            format!("Label {n} of {of}: the printer says {print}% printed, {feed}% fed.")
        }
    }
}

/// Which label, for a sentence: "the label" for a job of one, else "label 2
/// of 3".
fn which(label: u16, of: u16) -> String {
    if of == 1 {
        "the label".into()
    } else {
        format!("label {label} of {of}")
    }
}

/// When a failure came: "before printing started" at the liveness check,
/// otherwise "during the label" or "during label 2 of 3".
fn during(label: u16, of: u16, reached: Reached) -> String {
    if reached == Reached::Check {
        "before printing started".into()
    } else {
        format!("during {}", which(label, of))
    }
}

/// How many labels a failed job printed, and what became of `label`, the one
/// it failed on, with `printed` printed before it. Nothing of a label is sent
/// before its rows, so before [`Reached::Rows`] it was not sent, and from then
/// on it may have printed in part.
fn clause(of: u16, label: u16, printed: u16, reached: Reached) -> String {
    let before = (printed > 0).then(|| format!("{printed} of {of} labels printed before this; "));
    if reached < Reached::Rows {
        match (of, before) {
            (1, _) => "Nothing was printed.".into(),
            (of, None) => format!("None of the {of} labels was printed."),
            (of, Some(before)) if label >= of => format!("{before}label {label} was not sent."),
            (_, Some(before)) => format!("{before}label {label} and the rest were not sent."),
        }
    } else if of == 1 {
        "The label may be printed only in part.".into()
    } else {
        let rest = if label < of {
            ", and the rest were not sent"
        } else {
            ""
        };
        let said = format!("label {label} may be printed only in part{rest}.");
        match before {
            Some(before) => format!("{before}{said}"),
            None => upper_first(&said),
        }
    }
}

/// How long the job keeps asking a printer that refuses a step, in whole
/// seconds: the gaps between its tries.
fn refusing_secs() -> u64 {
    let t = Timeouts::default();
    let ms = t.refusal_gap.as_millis() as u64 * u64::from(t.refusals.saturating_sub(1));
    (ms + 500) / 1000
}

/// A printer's own fault, as a cause and a remedy. Neither reuses
/// `PrinterErrorCode::describe`'s remedies, which name the protocol log, a
/// pace and power-cycling.
fn fault_says(
    name: &str,
    code: PrinterErrorCode,
    label: &str,
    during: &str,
    settings: (u8, LabelType),
) -> (String, String) {
    use PrinterErrorCode::*;
    let (density, label_type) = settings;
    let label_type = label_type_says(label_type);
    let again = "Check the printer, then print again.".to_string();
    match code {
        CoverOpen => (
            format!("{name}'s cover is open."),
            "Close it, then print again.".into(),
        ),
        LackPaper | PaperOutException => (
            format!("{name} is out of labels."),
            "Load a roll, then print again.".into(),
        ),
        LowBattery => (
            format!("{name}'s battery is too low to print."),
            "Charge it, then print again.".into(),
        ),
        BatteryException => (
            format!("{name} reported a battery fault."),
            "Charge it; if that does not help, the battery may be failing.".into(),
        ),
        UserCancel => (
            format!("{name} says {label} was cancelled at the printer."),
            "Print again to print it.".into(),
        ),
        DataError => (
            format!("{name} refused a command {during}; it may have taken the label as finished."),
            "Print again.".into(),
        ),
        Overheat => (
            format!("{name}'s print head is too hot."),
            "Let it cool for a minute, then print again.".into(),
        ),
        PrinterBusy => (
            format!("{name} is busy with another job."),
            "Wait for it to finish, then print again; if nothing is printing, switch it off and \
             on."
            .into(),
        ),
        NoPrinterHead | PrinterHeadLoose => (
            format!("{name} says its print head is not seated."),
            "Open the cover, reseat the head and close it firmly, then print again.".into(),
        ),
        TemperatureLow => (
            format!("{name} is too cold to print."),
            "Let it warm to room temperature, then print again.".into(),
        ),
        NoRibbon | WrongRibbon | UsedRibbon | NonDedicatedRibbon => (
            format!("{name} reported a problem with its ribbon."),
            "Check the ribbon, then print again.".into(),
        ),
        WrongPaper | ECheckPaper => (
            format!("{name} says the loaded roll is not what it expected."),
            format!("relabel prints on {label_type}; check that the roll is one of those."),
        ),
        SetPaperFail
        | SetPrintModeFail
        | SetPrintDensityFail
        | SetMarginFail
        | SetPrintDensityNoSupport
        | SetPrintModeNoSupport
        | SetPrintLabelMaterialError
        | SetPrintLabelMaterialNoSupport => (
            format!(
                "{name} did not accept the settings relabel prints with (density {density}, \
                 {label_type})."
            ),
            "Switch it off and on, then print again.".into(),
        ),
        ReceiveDataTimeout => (
            format!("{name} stopped receiving {label} partway."),
            "Print again.".into(),
        ),
        Unknown(b) => (
            format!("{name} reported fault number {b}, which relabel does not know, {during}."),
            again,
        ),
        other => {
            let (what, _) = other.describe();
            let what = lower_first(what.trim_end_matches('.'));
            (format!("{name} reported a fault {during}: {what}."), again)
        }
    }
}

/// How a job ended, in one sentence for the status line. A failure is its
/// cause, then how many labels printed, then what to do. Every failure names
/// the printer; none names a command, a byte or an error type.
pub fn job_says(
    name: &str,
    report: &RunReport,
    link: LinkAfter,
    settings: (u8, LabelType),
) -> String {
    let RunReport { printed, of, end } = report;
    let (printed, of) = (*printed, *of);
    let (label, reached, error) = match end {
        RunEnd::Done | RunEnd::Stopped => {
            let mut said = match (end, printed, of) {
                (RunEnd::Done, _, 1) => format!("Printed on {name}"),
                (RunEnd::Done, _, of) => format!("Printed {of} labels on {name}"),
                (_, 0, _) => "Stopped before printing started: nothing was printed".into(),
                (_, p, of) => format!(
                    "Stopped after label {p} of {of}: {p} printed, {} not printed",
                    of - p
                ),
            };
            if link == LinkAfter::Gone {
                if *end == RunEnd::Done {
                    said.push_str("; then the connection dropped. Connect again to print more.");
                } else {
                    said.push_str(&format!(
                        "; then the connection to {name} dropped. Connect again to print more."
                    ));
                }
            } else {
                said.push('.');
            }
            return said;
        }
        RunEnd::Failed {
            label,
            reached,
            error,
        } => (*label, *reached, error),
    };
    let at_check = reached == Reached::Check;
    let during = during(label, of, reached);
    let which = which(label, of);
    let clause = if at_check {
        "Nothing was printed.".to_string()
    } else {
        clause(of, label, printed, reached)
    };
    let link_lost = matches!(
        error,
        JobError::Session(SessionError::Transport(_) | SessionError::Ended)
    );
    let mut said = match error {
        // A write failed or the session ended: the link went, whether or not
        // the platform said so first.
        _ if (link_lost || link == LinkAfter::Gone) && at_check => format!(
            "The connection to {name} dropped before printing started, so nothing was printed. \
             Connect again to use it."
        ),
        _ if link_lost => format!(
            "The connection to {name} dropped {during}. {clause} If the printer is still on, \
             switch it off and on, then connect again."
        ),
        JobError::Session(SessionError::Timeout { .. }) if at_check => format!(
            "{name} did not answer, so nothing was printed. Check that it is switched on and \
             nearby, then print again."
        ),
        JobError::Session(SessionError::Timeout { .. }) => format!(
            "{name} stopped answering {during}. {clause} Check that it is switched on and \
             nearby, then print again."
        ),
        JobError::Session(SessionError::Printer(code)) => {
            let (cause, remedy) = fault_says(name, *code, &which, &during, settings);
            format!("{cause} {clause} {remedy}")
        }
        JobError::Session(SessionError::NotSupported(_)) => format!(
            "{name} does not accept a command relabel prints with, so relabel cannot print on \
             it. {clause}"
        ),
        JobError::Session(SessionError::Proto(_)) => {
            format!("{name} sent a reply relabel could not read {during}. {clause} Print again.")
        }
        JobError::Refused { step: "PageEnd" } => {
            format!("{name} refused to finish {which} after it was sent. {clause} Print again.")
        }
        // PrintClear and PageStart are asked once; the settings and the job's
        // start are asked again for a while before the job gives up.
        JobError::Refused {
            step: "PrintClear" | "PageStart",
        } => format!(
            "{name} would not start {which}. {clause} Wait a few seconds and print again; if it \
             keeps refusing, switch it off and on."
        ),
        JobError::Refused { .. } => format!(
            "{name} would not start {which}: it kept refusing for about {}. {clause} Wait a few \
             seconds and print again; if it keeps refusing, switch it off and on.",
            counted(refusing_secs(), "second", "seconds")
        ),
        JobError::DeadlineExceeded { ms } => format!(
            "{name} did not report {which} printed within {}, so relabel ended it. {clause} \
             Check that a roll is loaded and the cover is shut, then print again.",
            counted(ms.div_ceil(1000), "second", "seconds")
        ),
        // Faults inside relabel, none of them the printer's doing. Transport
        // and Ended are the link's, and never reach here.
        JobError::Session(
            SessionError::OneWayCommand(_) | SessionError::Transport(_) | SessionError::Ended,
        )
        | JobError::Stalled { .. }
        | JobError::Cancelled => {
            if at_check {
                format!(
                    "relabel stopped before printing on {name} started, because of a fault \
                     inside relabel. {clause} Print again."
                )
            } else {
                format!(
                    "Printing on {name} stopped {during} because of a fault inside relabel. \
                     {clause} Print again."
                )
            }
        }
    };
    // The link went after a failure of another kind, which came first.
    if link == LinkAfter::Gone && !link_lost && !at_check {
        said.push_str(&format!(
            " The connection to {name} then dropped; connect again to use it."
        ));
    }
    said
}

/// A Print the link refused, with nothing sent.
fn refusal_says(name: &str, why: PrintRefusal) -> String {
    match why {
        PrintRefusal::NotConnected => format!(
            "Nothing printed: the connection to {name} had dropped. Connect again to use it."
        ),
        PrintRefusal::BadCount => format!(
            "Nothing printed: relabel sends {name} from 1 to {MAX_LABELS} labels at a time."
        ),
        PrintRefusal::WrongHead => format!(
            "Nothing printed: the label is wider than {name}'s print head, or relabel does not \
             know its head."
        ),
        // The machine never orders a second Print while one runs.
        PrintRefusal::Busy => format!("Nothing printed: {name} is already printing."),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use relabel_core::TransportError;
    use relabel_core::link::{DeviceKey, mock};
    use relabel_proto::payload::InfoField;
    use relabel_proto::{ProtoError, RequestId, ResponseId};

    const NAME: &str = mock::D110_M_NAME;

    fn seen(key: u32, name: &str, heard: bool) -> Seen {
        Seen {
            key: DeviceKey(key),
            name: name.into(),
            heard,
        }
    }

    fn found(p: &mut Printing, seen: Vec<Seen>) -> PrintOutcome {
        let ticket = p.ticket;
        p.apply(PrintCmd::Heard(Heard::Found { ticket, seen }))
    }

    fn step(p: &mut Printing, step: HandshakeStep) -> PrintOutcome {
        let ticket = p.ticket;
        p.apply(PrintCmd::Heard(Heard::Step { ticket, step }))
    }

    fn ready(p: &mut Printing, facts: Facts) -> PrintOutcome {
        let ticket = p.ticket;
        p.apply(PrintCmd::Heard(Heard::Ready { ticket, facts }))
    }

    /// Connected to the author's D110_M, the way the link reports it.
    fn connected() -> Printing {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, NAME, true)]);
        ready(&mut p, mock::facts(2320, NAME));
        p
    }

    #[test]
    fn connect_from_off_finds() {
        let mut p = Printing::new();
        let v = p.view();
        assert_eq!(
            (v.primary, v.focus, v.printer_line.as_str(), v.step.as_str()),
            (
                Some(Primary::Connect),
                Focus::Primary,
                "No printer connected",
                NOT_CONNECTED
            )
        );
        let out = p.apply(PrintCmd::Connect);
        assert_eq!(out.order, Some((1, Order::Find { secs: 4 })));
        assert_eq!(out.note.as_deref(), Some("Looking for printers."));
        // The primary slot becomes Stop waiting under the keyboard that pressed
        // Connect, so the keyboard goes to Close: a second Return, or Return held
        // down, must not stop the scan it just started.
        assert_eq!(out.focus, Some(Focus::Close));
        let v = p.view();
        assert_eq!(v.primary, Some(Primary::StopWaiting));
        assert_eq!(v.focus, Focus::Close);
        assert_eq!(v.printer_line, "Looking for printers");
        assert_eq!(v.step, "Looking for printers (4 seconds)");
        // A second press does nothing while it looks.
        assert_eq!(p.apply(PrintCmd::Connect), PrintOutcome::quiet());
    }

    #[test]
    fn one_printer_found_is_opened_unasked() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        let out = found(&mut p, vec![seen(7, NAME, true)]);
        assert_eq!(out.order, Some((1, Order::Open { key: DeviceKey(7) })));
        assert_eq!(
            out.note.as_deref(),
            Some("Connecting to D110_M-GB28010920.")
        );
        let v = p.view();
        assert_eq!(v.printer_line, "Connecting to D110_M-GB28010920");
        assert_eq!(v.step, "Connecting to D110_M-GB28010920 (up to 15 seconds)");
        assert_eq!(v.primary, Some(Primary::StopWaiting));
        // Off Stop waiting, for the reason choosing moves it: the operator did
        // not ask for this connect, and Return must not stop it by accident.
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(v.focus, Focus::Close);
    }

    #[test]
    fn one_unheard_printer_is_still_opened_and_said_to_be_slow() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        let out = found(&mut p, vec![seen(7, NAME, false)]);
        assert!(matches!(out.order, Some((1, Order::Open { .. }))));
        let note = out.note.expect("said");
        assert!(note.contains("was not heard in the scan"), "{note}");
        assert!(note.contains("up to 15 seconds"), "{note}");
    }

    #[test]
    fn several_printers_are_offered_at_most_four() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        let out = found(&mut p, vec![seen(1, "B", false), seen(2, "A", true)]);
        assert_eq!(out.order, None);
        assert_eq!(out.note.as_deref(), Some("Found 2 printers: choose one."));
        assert_eq!(out.focus, Some(Focus::FirstChoice));
        let v = p.view();
        assert_eq!(
            v.choices,
            [
                ("A".to_string(), "Connect to A".to_string()),
                (
                    "B, not heard just now".to_string(),
                    "Connect to B".to_string()
                ),
            ]
        );
        assert_eq!(v.focus, Focus::FirstChoice);
        assert_eq!(v.primary, Some(Primary::CancelChoice));
        assert_eq!(v.primary_says, "Choose no printer");
        assert_eq!(v.printer_line, "Choose a printer (Control+P)");
        assert_eq!(v.step, "Found 2 printers. Choose one.");

        // Choosing the second, which was listed second, opens it.
        let out = p.apply(PrintCmd::Choose(1));
        assert_eq!(out.order, Some((1, Order::Open { key: DeviceKey(1) })));
        // To Close, not to Stop waiting in the primary slot, for the reason
        // the pane opens on Close while connecting.
        assert_eq!(out.focus, Some(Focus::Close));

        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        let six = (0..6).map(|i| seen(i, &format!("P{i}"), true)).collect();
        let out = found(&mut p, six);
        assert_eq!(
            out.note.as_deref(),
            Some("Found 6 printers: choose one. 2 more were found and are not offered.")
        );
        let v = p.view();
        assert_eq!(v.choices.len(), 4);
        assert_eq!(v.more, 2);
        // A choice past the ones offered does nothing.
        assert_eq!(p.apply(PrintCmd::Choose(4)), PrintOutcome::quiet());
    }

    /// Every count a sentence here words takes the singular for one.
    #[test]
    fn one_more_printer_than_offered_is_said_in_the_singular() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        let five = (0..5).map(|i| seen(i, &format!("P{i}"), true)).collect();
        let out = found(&mut p, five);
        assert_eq!(
            out.note.as_deref(),
            Some("Found 5 printers: choose one. 1 more was found and is not offered.")
        );
        let v = p.view();
        assert_eq!(
            v.step,
            "Found 5 printers. Choose one. 1 more was found and is not offered."
        );
        assert_eq!(v.more, 1);
        assert_eq!(
            failure_says(&Failure::TimedOut {
                name: "X".into(),
                secs: 1
            }),
            "X did not answer within 1 second. Check that it is switched on, nearby, and not \
             connected to another device."
        );
    }

    #[test]
    fn none_found_is_said() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        let out = found(&mut p, vec![]);
        assert_eq!(
            out.note.as_deref(),
            Some("No printer found. Check that it is switched on and nearby.")
        );
        assert!(matches!(p.state, State::Off { .. }));
        assert_eq!(p.view().primary, Some(Primary::Connect));
        // Stop waiting has become Connect, so the keyboard goes to Close: a Return
        // meant to stop the scan must not start another.
        assert_eq!(out.focus, Some(Focus::Close));
    }

    #[test]
    fn steps_go_to_the_step_line_not_the_status() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, NAME, true)]);
        let out = step(
            &mut p,
            HandshakeStep::Info {
                field: InfoField::ModelId,
                n: 1,
            },
        );
        assert_eq!(out, PrintOutcome::quiet());
        assert_eq!(
            p.view().step,
            "Connecting to D110_M-GB28010920: reading its model id (1 of 8)"
        );
        assert_eq!(p.view().printer_line, "Connecting to D110_M-GB28010920");
    }

    #[test]
    fn ready_is_said_once_and_moves_the_keyboard_to_close() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, NAME, true)]);
        step(&mut p, HandshakeStep::Connect);
        let before = p.generation();
        assert!(p.facts().is_none());
        let out = ready(&mut p, mock::facts(2320, NAME));
        assert_eq!(
            out.note.as_deref(),
            Some("Connected to D110_M-GB28010920, a D110_M.")
        );
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(p.generation(), before + 1);
        let v = p.view();
        assert_eq!(
            v.printer_line,
            "D110_M-GB28010920: a D110_M, 96-dot head at 203 dpi, firmware 4.17, battery about \
             three quarters"
        );
        assert_eq!(v.step, "Connected.");
        assert_eq!(v.focus, Focus::Close);
        assert_eq!(v.primary, Some(Primary::Disconnect));
        assert_eq!(
            v.primary_says,
            "Close the Bluetooth link to D110_M-GB28010920"
        );
        assert!(!Primary::Disconnect.is_default());
        assert_eq!(p.facts().map(|f| f.name.as_str()), Some(NAME));
        // Said once: the same answer again finds nothing waiting for it.
        assert_eq!(
            ready(&mut p, mock::facts(2320, NAME)),
            PrintOutcome::quiet()
        );
        assert_eq!(p.generation(), before + 1);
    }

    #[test]
    fn ready_but_not_printable_says_why() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, "X", true)]);
        let out = ready(&mut p, mock::facts(9999, "X"));
        let note = out.note.expect("said");
        assert!(note.contains("but relabel cannot print on it"), "{note}");
        assert!(note.ends_with("print-head width."), "{note}");
        assert_eq!(
            p.view().printer_line,
            "X: a printer relabel does not know (id 9999)"
        );

        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, "Y", true)]);
        let note = ready(&mut p, mock::facts(2305, "Y")).note.expect("said");
        assert!(
            note.contains("its id is shared by the D110") && note.contains("the Hi-D110"),
            "{note}"
        );
        let line = p.view().printer_line;
        assert!(
            line.starts_with("Y: a D110")
                && line.contains(" or a Hi-D110")
                && line.ends_with("(id 2305)"),
            "{line}"
        );
    }

    #[test]
    fn a_late_answer_to_an_old_ticket_is_ignored() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        p.apply(PrintCmd::StopWaiting);
        let out = p.apply(PrintCmd::Heard(Heard::Found {
            ticket: 1,
            seen: vec![seen(7, NAME, true)],
        }));
        assert_eq!(out, PrintOutcome::quiet());
        assert!(matches!(p.state, State::Off { .. }));

        // Looking again, on ticket 3: whatever the stopped scan on ticket 1
        // still says is not taken for this one's answer.
        p.apply(PrintCmd::Connect);
        assert_eq!(p.ticket, 3);
        let late = [
            Heard::Found {
                ticket: 1,
                seen: vec![seen(7, NAME, true)],
            },
            Heard::Ready {
                ticket: 1,
                facts: mock::facts(2320, NAME),
            },
            Heard::Failed {
                ticket: 1,
                failure: Failure::NoAdapter,
            },
        ];
        for h in late {
            let out = p.apply(PrintCmd::Heard(h.clone()));
            assert_eq!(out, PrintOutcome::quiet(), "{h:?}");
            assert!(matches!(p.state, State::Looking), "{h:?}");
            assert_eq!(p.generation(), 0, "{h:?}");
        }
    }

    #[test]
    fn stop_waiting_says_what_stopped() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        let out = p.apply(PrintCmd::StopWaiting);
        assert_eq!(out.order, Some((2, Order::StopWaiting)));
        assert_eq!(out.note.as_deref(), Some("Stopped looking for printers."));
        // Stop waiting becomes Connect under the keyboard that pressed it, so the
        // keyboard goes to Close: a second Return, or Return held down, must not
        // start the scan it just stopped.
        assert_eq!(out.focus, Some(Focus::Close));

        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, "X", true)]);
        let out = p.apply(PrintCmd::StopWaiting);
        assert_eq!(out.order, Some((2, Order::StopWaiting)));
        assert_eq!(out.note.as_deref(), Some("Stopped connecting to X."));
        assert_eq!(out.focus, Some(Focus::Close));

        // Mid-handshake is the same.
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, "X", true)]);
        step(&mut p, HandshakeStep::Heartbeat);
        let out = p.apply(PrintCmd::StopWaiting);
        assert_eq!(out.note.as_deref(), Some("Stopped connecting to X."));
        assert_eq!(out.focus, Some(Focus::Close));

        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(1, "A", true), seen(2, "B", true)]);
        let out = p.apply(PrintCmd::StopWaiting);
        assert_eq!(out.order, None);
        assert_eq!(out.note.as_deref(), Some("No printer was chosen."));
        assert_eq!(p.view().primary, Some(Primary::Connect));
        // Cancel, likewise, is Connect now.
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(Primary::CancelChoice.cmd(), PrintCmd::StopWaiting);
    }

    #[test]
    fn disconnect_closes_and_says_so() {
        let mut p = connected();
        let out = p.apply(PrintCmd::Disconnect);
        assert_eq!(out.order, Some((1, Order::Close)));
        assert_eq!(out.note, None);
        assert_eq!(out.focus, Some(Focus::Close));
        let v = p.view();
        assert_eq!(v.primary, None);
        assert_eq!(v.printer_line, "Disconnecting from D110_M-GB28010920");
        let before = p.generation();
        let out = p.apply(PrintCmd::Heard(Heard::Closed { ticket: 1 }));
        assert_eq!(
            out.note.as_deref(),
            Some("Disconnected from D110_M-GB28010920.")
        );
        assert_eq!(p.view().printer_line, "No printer connected");
        assert_eq!(p.view().step, NOT_CONNECTED);
        assert_eq!(p.generation(), before + 1);
        assert!(p.facts().is_none());
    }

    #[test]
    fn a_dropped_link_is_said_once() {
        let mut p = connected();
        let before = p.generation();
        let out = p.apply(PrintCmd::Heard(Heard::Dropped { ticket: 1 }));
        assert_eq!(
            out.note.as_deref(),
            Some("The connection to D110_M-GB28010920 dropped. Connect again to use it.")
        );
        assert_eq!(p.generation(), before + 1);
        assert_eq!(p.view().printer_line, "No printer connected");
        // Disconnect has become Connect, so the keyboard goes to Close: a Return
        // meant to disconnect must not start a scan.
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(p.view().primary, Some(Primary::Connect));
        assert_eq!(
            p.apply(PrintCmd::Heard(Heard::Dropped { ticket: 1 })),
            PrintOutcome::quiet()
        );
        // And Connect looks again with a new ticket.
        assert_eq!(
            p.apply(PrintCmd::Connect).order,
            Some((2, Order::Find { secs: 4 }))
        );
    }

    #[test]
    fn every_failure_is_plain_words() {
        let name = || NAME.to_string();
        let failures = [
            Failure::NoBluetooth("org.freedesktop.DBus.Error.FileNotFound".into()),
            Failure::NoAdapter,
            Failure::BluetoothOff,
            Failure::NotAllowed,
            Failure::ScanFailed("Operation already in progress".into()),
            Failure::NotFound { name: name() },
            Failure::TimedOut {
                name: name(),
                secs: 15,
            },
            Failure::ConnectFailed {
                name: name(),
                detail: "le-connection-abort-by-local".into(),
            },
            Failure::NoChannel { name: name() },
            Failure::Handshake {
                name: name(),
                step: HandshakeStep::Info {
                    field: InfoField::SerialNumber,
                    n: 2,
                },
            },
            Failure::DroppedWhileConnecting { name: name() },
        ];
        for failure in failures {
            let mut p = Printing::new();
            p.apply(PrintCmd::Connect);
            let out = p.apply(PrintCmd::Heard(Heard::Failed {
                ticket: 1,
                failure: failure.clone(),
            }));
            // Stop waiting has become Connect, so the keyboard goes to Close.
            assert_eq!(out.focus, Some(Focus::Close), "{failure:?}");
            let note = out.note.expect("said");
            for word in [
                "0x",
                "ResponseId",
                " ms",
                "protocol log",
                "pace",
                "SessionError",
                "TransportError",
            ] {
                assert!(!note.contains(word), "{failure:?}: {note}");
            }
            assert_eq!(note, failure_says(&failure));
            // The step line keeps saying it, and Connect is offered again.
            assert_eq!(p.view().step, note);
            assert_eq!(p.view().primary, Some(Primary::Connect));
            if let Failure::TimedOut { .. } = failure {
                assert!(note.contains("15 seconds"), "{note}");
            }
        }
        // A failure while connecting, mid-handshake, moves it the same way.
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, NAME, true)]);
        step(&mut p, HandshakeStep::Heartbeat);
        let out = p.apply(PrintCmd::Heard(Heard::Failed {
            ticket: 1,
            failure: Failure::DroppedWhileConnecting { name: name() },
        }));
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(p.view().primary, Some(Primary::Connect));
        assert_eq!(
            failure_says(&Failure::Handshake {
                name: name(),
                step: HandshakeStep::Heartbeat
            }),
            "D110_M-GB28010920 stopped answering while relabel was checking the link. Connect \
             again."
        );
    }

    #[test]
    fn the_link_ending_is_final() {
        let mut p = connected();
        let before = p.generation();
        let out = p.apply(PrintCmd::LinkEnded);
        assert_eq!(
            out.note.as_deref(),
            Some(
                "The printer connection stopped working inside relabel; restart relabel to \
                 connect again."
            )
        );
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(p.generation(), before + 1);
        let v = p.view();
        assert_eq!(v.primary, None);
        assert_eq!(v.printer_line, "No printer connected");
        let again = p.apply(PrintCmd::Connect);
        assert_eq!(again.order, None);
        assert_eq!(again.note, out.note);
    }

    #[test]
    fn the_battery_is_said_in_words() {
        for (p, says) in [
            (0, "nearly empty"),
            (25, "about a quarter"),
            (50, "about half"),
            (75, "about three quarters"),
            (100, "full"),
            (60, "about 60%"),
        ] {
            assert_eq!(battery_says(p), says);
        }
        let mut facts = mock::facts(2320, NAME);
        facts.firmware = None;
        facts.battery_percent = None;
        assert_eq!(
            printer_line(&facts),
            "D110_M-GB28010920: a D110_M, 96-dot head at 203 dpi"
        );
        assert_eq!(joined(["a".to_string()], "and"), "a");
        assert_eq!(joined(["a", "b", "c"].map(String::from), "or"), "a, b or c");
    }

    const SETTINGS: (u8, LabelType) = (3, LabelType::Gap);

    /// Banned from every sentence the operator hears about a job, with every
    /// name in relabel-proto's command and reply tables (`banned`).
    const BANNED: [&str; 18] = [
        "0x",
        "RequestId",
        "ResponseId",
        " ms",
        "protocol log",
        "pace",
        "ProtoError",
        "SessionError",
        "TransportError",
        "JobError",
        "PrinterErrorCode",
        "NotSupported",
        "Timeout",
        "Refused",
        "DataError",
        "deadline",
        "power-cycle",
        "code",
    ];

    /// `BANNED`, and the name of every command relabel sends or reply it hears
    /// that relabel-proto's tables name - but Connect, a reply's name that is
    /// also the pane's own button, which a remedy may name.
    fn banned() -> Vec<&'static str> {
        let names = (0..=u8::MAX)
            .flat_map(|b| [RequestId(b).name(), ResponseId(b).name()])
            .filter(|n| !matches!(*n, "Unknown" | "Connect"));
        let mut all: Vec<_> = BANNED.into_iter().chain(names).collect();
        all.sort_unstable();
        all.dedup();
        all
    }

    fn page() -> EncodedPage {
        EncodedPage {
            rows: 240,
            cols: 96,
            frames: Vec::new(),
        }
    }

    fn copies(p: &mut Printing, text: &str) {
        assert_eq!(
            p.apply(PrintCmd::Copies(text.into())),
            PrintOutcome::quiet(),
            "Copies is quiet"
        );
    }

    fn print(p: &mut Printing) -> PrintOutcome {
        p.apply(PrintCmd::Print(Ok(page())))
    }

    fn news(p: &mut Printing, news: RunNews) -> PrintOutcome {
        let ticket = p.ticket;
        p.apply(PrintCmd::Heard(Heard::Job { ticket, news }))
    }

    fn ended(p: &mut Printing, report: RunReport, link: LinkAfter) -> PrintOutcome {
        let ticket = p.ticket;
        p.apply(PrintCmd::Heard(Heard::JobEnded {
            ticket,
            report,
            link,
        }))
    }

    fn report(printed: u16, of: u16, end: RunEnd) -> RunReport {
        RunReport { printed, of, end }
    }

    fn failed(label: u16, reached: Reached, error: JobError) -> RunEnd {
        RunEnd::Failed {
            label,
            reached,
            error,
        }
    }

    /// Connected to the D110_M, with a job of `of` labels just ordered.
    fn printing(of: u16) -> Printing {
        let mut p = connected();
        copies(&mut p, &of.to_string());
        let out = print(&mut p);
        assert!(
            matches!(out.order, Some((1, Order::Print { labels, .. })) if labels == of),
            "{out:?}"
        );
        p
    }

    /// Labels 1 to `through` printed, and `through + 1` started.
    fn up_to(p: &mut Printing, through: u16, of: u16) {
        news(p, RunNews::Checking);
        for n in 1..=through {
            news(p, RunNews::Starting { n, of });
            news(p, RunNews::Sending { n, of });
            news(p, RunNews::Printed { n, of });
        }
        if through < of {
            news(p, RunNews::Starting { n: through + 1, of });
        }
    }

    #[test]
    fn print_needs_ready_and_whole_copies() {
        let mut states = Vec::new();
        states.push(Printing::new());
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        states.push(p.clone());
        found(&mut p, vec![seen(1, "A", true), seen(2, "B", true)]);
        states.push(p);
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, NAME, true)]);
        states.push(p.clone());
        step(&mut p, HandshakeStep::Heartbeat);
        states.push(p);
        let mut p = connected();
        p.apply(PrintCmd::Disconnect);
        states.push(p);
        let mut p = Printing::new();
        p.apply(PrintCmd::LinkEnded);
        states.push(p);
        for mut p in states {
            copies(&mut p, "3");
            let state = format!("{:?}", p.state);
            let out = print(&mut p);
            assert_eq!(out.order, None, "{state}");
            let note = out.note.expect("said");
            assert!(note.starts_with("Nothing printed"), "{state}: {note}");
            assert!(!p.busy(), "{state}");
            assert_eq!(p.view().job_slot, None, "{state}");
        }

        let mut p = connected();
        assert_eq!(p.view().job_slot, Some(JobSlot::Print));
        assert!(p.view().copies_shown);
        copies(&mut p, "3");
        let (generation, facts) = (p.generation(), p.facts().cloned());
        let out = print(&mut p);
        assert_eq!(
            out,
            PrintOutcome {
                order: Some((
                    1,
                    Order::Print {
                        page: page(),
                        labels: 3
                    }
                )),
                note: Some("Printing 3 labels on D110_M-GB28010920.".into()),
                focus: Some(Focus::Close),
                replay_close: false,
            }
        );
        assert!(p.busy());
        assert!(matches!(p.state, State::Printing { .. }));
        assert_eq!(p.generation(), generation);
        assert_eq!(p.facts().cloned(), facts);

        let mut p = connected();
        assert_eq!(
            print(&mut p).note.as_deref(),
            Some("Printing on D110_M-GB28010920.")
        );

        // The wiring had no page to give.
        let mut p = connected();
        let out = p.apply(PrintCmd::Print(Err(NotReady::NotPrintable)));
        assert_eq!(out.order, None);
        assert!(out.note.expect("said").starts_with("Nothing printed: "));
        assert!(!p.busy());

        // A printer relabel cannot print on offers no Print, and refuses one.
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, "X", true)]);
        ready(&mut p, mock::facts(9999, "X"));
        assert_eq!(p.view().job_slot, None);
        assert!(!p.view().copies_shown);
        let out = print(&mut p);
        assert_eq!(out.order, None);
        assert_eq!(
            out.note.as_deref(),
            Some(
                "Nothing printed: relabel cannot print on X, because it does not know this \
                 printer's print-head width."
            )
        );
    }

    #[test]
    fn copies_out_of_range_print_nothing() {
        for text in ["0", "100", "", "two", "1.5", "-1"] {
            let mut p = connected();
            copies(&mut p, text);
            let out = print(&mut p);
            assert_eq!(out.order, None, "{text:?}");
            assert_eq!(out.note.as_deref(), Some(COPIES_REFUSED), "{text:?}");
            assert!(!p.busy(), "{text:?}");
            // Kept as typed, for the field to start from.
            assert_eq!(p.view().copies_start, text);
            assert_eq!(
                p.view().job_slot_says,
                "Nothing prints until Copies is a whole number from 1 to 99"
            );
        }
        let mut p = connected();
        copies(&mut p, " 3 ");
        assert!(matches!(
            print(&mut p).order,
            Some((1, Order::Print { labels: 3, .. }))
        ));
        // The field starts from what was typed for the life of the window.
        assert_eq!(p.view().copies_start, " 3 ");
        assert_eq!(p.view().copies_says, COPIES_SAYS);
        assert_eq!(Printing::new().view().copies_start, "1");
    }

    #[test]
    fn print_says_what_it_will_print() {
        let mut p = connected();
        copies(&mut p, "3");
        assert_eq!(
            p.view_for(Some("30 \u{d7} 12 mm")).job_slot_says,
            "Print 3 labels, 30 \u{d7} 12 mm, on D110_M-GB28010920 at density 3 on labels with \
             gaps"
        );
        copies(&mut p, "1");
        assert_eq!(
            p.view().job_slot_says,
            "Print 1 label on D110_M-GB28010920 at density 3 on labels with gaps"
        );
        assert_eq!(JobSlot::Print.face(), "Print");
        assert_eq!(JobSlot::StopAfterLabel.face(), "Stop after this label");
        assert_eq!(p.view().image_label, "What will print");
    }

    #[test]
    fn print_while_printing_and_stale_are_refused() {
        let mut p = printing(2);
        let out = print(&mut p);
        assert_eq!(out.order, None);
        assert_eq!(
            out.note.as_deref(),
            Some("Nothing printed: a print is already running.")
        );
        assert!(p.busy());

        let mut p = connected();
        let out = p.apply(PrintCmd::Print(Err(NotReady::Stale)));
        assert_eq!(out.order, None);
        let note = out.note.expect("said");
        assert!(
            note.starts_with("Nothing printed: the label changed"),
            "{note}"
        );
        assert_eq!(note, STALE);
        assert!(!p.busy());
    }

    #[test]
    fn stop_after_this_label_is_its_own_order() {
        let mut p = printing(3);
        news(&mut p, RunNews::Checking);
        news(&mut p, RunNews::Starting { n: 1, of: 3 });
        let v = p.view();
        assert_eq!(v.job_slot, Some(JobSlot::StopAfterLabel));
        assert_eq!(
            v.job_slot_says,
            "Finish the label in progress, then print no more"
        );
        assert!(!v.copies_shown);
        let out = p.apply(PrintCmd::StopAfterLabel);
        assert_eq!(
            out,
            PrintOutcome {
                order: Some((1, Order::StopAfterLabel)),
                note: Some(
                    "Stopping after the label in progress; no other label will start.".into()
                ),
                focus: Some(Focus::Close),
                replay_close: false,
            }
        );
        assert_eq!(p.view().job_slot, None);
        assert_eq!(p.apply(PrintCmd::StopAfterLabel), PrintOutcome::quiet());

        // Before the editor has heard of a label, one may still be on its way:
        // the link reads the stop only before each label, and its news of the
        // start lags it. So nothing is promised that the link may have passed.
        let mut p = printing(3);
        assert_eq!(
            p.view().job_slot_says,
            "Finish the label in progress, if any, then print no more"
        );
        let out = p.apply(PrintCmd::StopAfterLabel);
        assert_eq!(out.order, Some((1, Order::StopAfterLabel)));
        assert_eq!(
            out.note.as_deref(),
            Some("Stopping after the label in progress, if any; no other label will start.")
        );

        // A job of one label has nothing to stop after.
        let mut p = printing(1);
        assert_eq!(p.view().job_slot, None);
        news(&mut p, RunNews::Starting { n: 1, of: 1 });
        assert_eq!(p.view().job_slot, None);
        assert_eq!(p.apply(PrintCmd::StopAfterLabel), PrintOutcome::quiet());

        // The slot hides as the last label starts, and the keyboard leaves it.
        let mut p = printing(3);
        up_to(&mut p, 1, 3);
        let out = news(&mut p, RunNews::Printed { n: 2, of: 3 });
        assert_eq!(out, PrintOutcome::quiet(), "the slot has not changed");
        assert_eq!(p.view().job_slot, Some(JobSlot::StopAfterLabel));
        let out = news(&mut p, RunNews::Starting { n: 3, of: 3 });
        assert_eq!(out.note, None);
        assert_eq!(out.order, None);
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(p.view().job_slot, None);
        assert_eq!(p.apply(PrintCmd::StopAfterLabel), PrintOutcome::quiet());
    }

    #[test]
    fn link_commands_are_quiet_while_printing() {
        let mut p = printing(3);
        for cmd in [
            PrintCmd::Connect,
            PrintCmd::Choose(0),
            PrintCmd::StopWaiting,
            PrintCmd::Disconnect,
        ] {
            assert_eq!(p.apply(cmd.clone()), PrintOutcome::quiet(), "{cmd:?}");
            assert!(p.busy(), "{cmd:?}");
        }
        let v = p.view();
        assert_eq!(v.primary, None);
        assert_eq!(v.primary_says, "");
        assert_eq!(v.focus, Focus::Close);
    }

    #[test]
    fn job_news_updates_the_line_and_never_the_status() {
        let mut p = printing(3);
        let (generation, facts) = (p.generation(), p.facts().cloned());
        let job = |p: &Printing| p.view().job.expect("a job line");

        assert_eq!(news(&mut p, RunNews::Checking).note, None);
        assert_eq!(job(&p).says, "Checking that the printer answers.");
        assert_eq!(p.view().printer_line, "D110_M-GB28010920: printing");
        assert_eq!(p.view().step, "Printing on D110_M-GB28010920.");
        assert_eq!(p.view().image_label, "What is printing");
        assert_eq!(job(&p).done(), 0.0);

        let steps = [
            RunNews::Starting { n: 1, of: 3 },
            RunNews::Sending { n: 1, of: 3 },
            RunNews::Printer {
                n: 1,
                of: 3,
                print_percent: 100,
                feed_percent: 100,
            },
            RunNews::Printed { n: 1, of: 3 },
        ];
        for n in steps {
            assert_eq!(news(&mut p, n).note, None, "{n:?}");
        }
        assert_eq!(job(&p).done(), 1.0 / 3.0);
        assert_eq!(
            p.view().printer_line,
            "D110_M-GB28010920: printing label 1 of 3"
        );

        assert_eq!(news(&mut p, RunNews::Starting { n: 2, of: 3 }).note, None);
        assert_eq!(job(&p).says, "Label 2 of 3: sending.");
        assert_eq!(
            p.view().printer_line,
            "D110_M-GB28010920: printing label 2 of 3"
        );
        assert_eq!(news(&mut p, RunNews::Sending { n: 2, of: 3 }).note, None);
        let out = news(
            &mut p,
            RunNews::Printer {
                n: 2,
                of: 3,
                print_percent: 60,
                feed_percent: 0,
            },
        );
        assert_eq!(out, PrintOutcome::quiet());
        assert_eq!(
            job(&p).says,
            "Label 2 of 3: the printer says 60% printed, 0% fed."
        );
        assert_eq!(p.generation(), generation);
        assert_eq!(p.facts().cloned(), facts);

        // A job of one label says it without counting.
        let mut p = printing(1);
        news(&mut p, RunNews::Starting { n: 1, of: 1 });
        assert_eq!(job(&p).says, "Sending the label.");
        assert_eq!(p.view().printer_line, "D110_M-GB28010920: printing");
        news(
            &mut p,
            RunNews::Printer {
                n: 1,
                of: 1,
                print_percent: 60,
                feed_percent: 0,
            },
        );
        assert_eq!(job(&p).says, "The printer says 60% printed, 0% fed.");
    }

    #[test]
    fn job_endings_are_said_once() {
        let cases = [
            (
                1,
                report(1, 1, RunEnd::Done),
                "Printed on D110_M-GB28010920.",
            ),
            (
                3,
                report(3, 3, RunEnd::Done),
                "Printed 3 labels on D110_M-GB28010920.",
            ),
            (
                3,
                report(2, 3, RunEnd::Stopped),
                "Stopped after label 2 of 3: 2 printed, 1 not printed.",
            ),
            (
                3,
                report(0, 3, RunEnd::Stopped),
                "Stopped before printing started: nothing was printed.",
            ),
        ];
        for (of, report, says) in cases {
            let mut p = printing(of);
            news(&mut p, RunNews::Checking);
            let generation = p.generation();
            let out = ended(&mut p, report.clone(), LinkAfter::Up);
            assert_eq!(out.note.as_deref(), Some(says));
            assert_eq!(out.order, None);
            assert_eq!(out.focus, Some(Focus::Close));
            assert!(!out.replay_close);
            assert!(!p.busy());
            assert_eq!(p.generation(), generation);
            let v = p.view();
            assert_eq!(v.step, says, "the step line keeps saying it");
            assert_eq!(v.job_slot, Some(JobSlot::Print));
            assert_eq!(v.primary, Some(Primary::Disconnect));
            assert_eq!(v.job, None);
            assert_eq!(v.image_label, "What will print");
            assert_eq!(ended(&mut p, report, LinkAfter::Up), PrintOutcome::quiet());
        }

        // Done, and the link went after: the labels printed, and it is said.
        let mut p = printing(3);
        let generation = p.generation();
        let out = ended(&mut p, report(3, 3, RunEnd::Done), LinkAfter::Gone);
        assert_eq!(
            out.note.as_deref(),
            Some(
                "Printed 3 labels on D110_M-GB28010920; then the connection dropped. Connect \
                 again to print more."
            )
        );
        assert_eq!(p.generation(), generation + 1);
        assert!(p.facts().is_none());
        assert_eq!(p.view().primary, Some(Primary::Connect));
        assert_eq!(p.view().step, out.note.expect("said"));
    }

    #[test]
    fn the_accounting_clause_counts_labels() {
        use Reached::*;
        for (of, label, printed, reached, says) in [
            (1, 1, 0, Settings, "Nothing was printed."),
            (1, 1, 0, Rows, "The label may be printed only in part."),
            (3, 1, 0, Settings, "None of the 3 labels was printed."),
            (
                3,
                2,
                1,
                Settings,
                "1 of 3 labels printed before this; label 2 and the rest were not sent.",
            ),
            (
                3,
                3,
                2,
                Settings,
                "2 of 3 labels printed before this; label 3 was not sent.",
            ),
            (
                3,
                3,
                2,
                Waiting,
                "2 of 3 labels printed before this; label 3 may be printed only in part.",
            ),
            (
                3,
                1,
                0,
                Rows,
                "Label 1 may be printed only in part, and the rest were not sent.",
            ),
            (
                3,
                2,
                1,
                Rows,
                "1 of 3 labels printed before this; label 2 may be printed only in part, and \
                 the rest were not sent.",
            ),
        ] {
            assert_eq!(clause(of, label, printed, reached), says);
        }
        for of in 1..=4 {
            for label in 1..=of {
                for reached in [Rows, Waiting] {
                    let said = clause(of, label, label - 1, reached);
                    assert!(
                        !said.contains("Nothing was printed") && !said.contains("None of"),
                        "{said}"
                    );
                }
            }
        }
    }

    /// Every fault the printer can report, every way a session can fail, and
    /// every way a job can, at every point a label can reach.
    #[test]
    fn every_job_failure_is_plain_words() {
        let codes: Vec<PrinterErrorCode> = (0..=u8::MAX)
            .map(PrinterErrorCode::from_byte)
            .filter(|c| !matches!(c, PrinterErrorCode::Unknown(_)))
            .collect();
        assert_eq!(codes.len(), 38, "every named code");
        let banned = banned();
        for name in [
            "Heartbeat",
            "PrintStatus",
            "CancelPrint",
            "Pause",
            "PrintError",
        ] {
            assert!(banned.contains(&name), "{name} is banned");
        }
        let mut errors: Vec<JobError> = codes
            .into_iter()
            .chain([PrinterErrorCode::Unknown(71)])
            .map(|c| JobError::Session(SessionError::Printer(c)))
            .collect();
        errors.extend(
            [
                SessionError::Transport(TransportError::Disconnected),
                SessionError::Transport(TransportError::NotConnected),
                SessionError::Transport(TransportError::Write("org.bluez.Error.Failed".into())),
                SessionError::Proto(ProtoError::BadChecksum {
                    found: 0x12,
                    computed: 0x34,
                }),
                SessionError::NotSupported(ResponseId::PRINT_ERROR),
                SessionError::Timeout {
                    what: "PrintStatus",
                    ms: 900,
                },
                SessionError::OneWayCommand(0x85),
                SessionError::Ended,
            ]
            .map(JobError::Session),
        );
        errors.extend(
            [
                "SetLabelType",
                "SetDensity",
                "PrintStart",
                "PrintClear",
                "PageStart",
                "PageEnd",
            ]
            .map(|step| JobError::Refused { step }),
        );
        errors.extend([
            JobError::Stalled {
                reached: 0,
                expected: 1,
            },
            JobError::DeadlineExceeded { ms: 25_000 },
            JobError::Cancelled,
        ]);
        let mut sentences = 0;
        for error in &errors {
            for reached in [
                Reached::Check,
                Reached::Settings,
                Reached::Rows,
                Reached::Waiting,
            ] {
                for of in [1, 3] {
                    let (label, printed) = match (reached, of) {
                        (Reached::Check, _) | (_, 1) => (1, 0),
                        _ => (2, 1),
                    };
                    for link in [LinkAfter::Up, LinkAfter::Gone] {
                        let end = failed(label, reached, error.clone());
                        let said = job_says(NAME, &report(printed, of, end), link, SETTINGS);
                        let case = format!("{error:?} at {reached:?} of {of}, {link:?}");
                        assert!(said.contains(NAME), "{case}: {said}");
                        assert!(said.ends_with('.'), "{case}: {said}");
                        for word in &banned {
                            assert!(!said.contains(word), "{case}: {word:?} in {said}");
                        }
                        if reached >= Reached::Rows {
                            assert!(
                                !said.contains("Nothing was printed")
                                    && !said.contains("None of")
                                    && !said.contains("nothing was printed"),
                                "{case}: {said}"
                            );
                        }
                        sentences += 1;
                    }
                }
            }
        }
        assert_eq!(sentences, errors.len() * 16);

        // The endings that did not fail, with the link gone too, name it.
        for end in [RunEnd::Done, RunEnd::Stopped] {
            for printed in [0, 2] {
                let said = job_says(
                    NAME,
                    &report(printed, 3, end.clone()),
                    LinkAfter::Gone,
                    SETTINGS,
                );
                assert!(said.contains(NAME) && said.ends_with('.'), "{said}");
            }
        }
    }

    /// The sentences an operator is likeliest to meet, in full.
    #[test]
    fn the_commonest_failures_read_in_full() {
        let says =
            |printed, of, end| job_says(NAME, &report(printed, of, end), LinkAfter::Up, SETTINGS);
        assert_eq!(
            says(
                0,
                1,
                failed(
                    1,
                    Reached::Waiting,
                    JobError::DeadlineExceeded { ms: 25_000 }
                )
            ),
            "D110_M-GB28010920 did not report the label printed within 25 seconds, so relabel \
             ended it. The label may be printed only in part. Check that a roll is loaded and \
             the cover is shut, then print again."
        );
        assert_eq!(
            says(
                1,
                3,
                failed(
                    2,
                    Reached::Waiting,
                    JobError::Session(SessionError::Printer(PrinterErrorCode::LackPaper))
                )
            ),
            "D110_M-GB28010920 is out of labels. 1 of 3 labels printed before this; label 2 \
             may be printed only in part, and the rest were not sent. Load a roll, then print \
             again."
        );
        assert_eq!(
            says(
                0,
                3,
                failed(
                    1,
                    Reached::Check,
                    JobError::Session(SessionError::Timeout {
                        what: "Heartbeat",
                        ms: 1000
                    })
                )
            ),
            "D110_M-GB28010920 did not answer, so nothing was printed. Check that it is \
             switched on and nearby, then print again."
        );
        assert_eq!(
            says(
                0,
                3,
                failed(
                    1,
                    Reached::Check,
                    JobError::Session(SessionError::Printer(PrinterErrorCode::CoverOpen))
                )
            ),
            "D110_M-GB28010920's cover is open. Nothing was printed. Close it, then print again."
        );
        assert_eq!(
            says(
                1,
                3,
                failed(
                    2,
                    Reached::Settings,
                    JobError::Refused {
                        step: "SetLabelType"
                    }
                )
            ),
            "D110_M-GB28010920 would not start label 2 of 3: it kept refusing for about 2 \
             seconds. 1 of 3 labels printed before this; label 2 and the rest were not sent. \
             Wait a few seconds and print again; if it keeps refusing, switch it off and on."
        );
        assert_eq!(
            says(
                0,
                1,
                failed(
                    1,
                    Reached::Settings,
                    JobError::Session(SessionError::Printer(PrinterErrorCode::SetPrintDensityFail))
                )
            ),
            "D110_M-GB28010920 did not accept the settings relabel prints with (density 3, \
             labels with gaps). Nothing was printed. Switch it off and on, then print again."
        );
        assert_eq!(
            says(
                0,
                1,
                failed(
                    1,
                    Reached::Waiting,
                    JobError::Session(SessionError::Printer(PrinterErrorCode::Disconnect))
                )
            ),
            "D110_M-GB28010920 reported a fault during the label: the link to the printer \
             broke. The label may be printed only in part. Check the printer, then print again."
        );
        assert_eq!(
            says(
                0,
                1,
                failed(
                    1,
                    Reached::Rows,
                    JobError::Session(SessionError::Printer(PrinterErrorCode::Unknown(71)))
                )
            ),
            "D110_M-GB28010920 reported fault number 71, which relabel does not know, during \
             the label. The label may be printed only in part. Check the printer, then print \
             again."
        );
    }

    #[test]
    fn a_drop_mid_job_is_one_sentence() {
        let mut p = printing(3);
        up_to(&mut p, 1, 3);
        news(&mut p, RunNews::Sending { n: 2, of: 3 });
        news(
            &mut p,
            RunNews::Printer {
                n: 2,
                of: 3,
                print_percent: 30,
                feed_percent: 0,
            },
        );
        let generation = p.generation();
        let out = ended(
            &mut p,
            report(
                1,
                3,
                failed(2, Reached::Waiting, JobError::Session(SessionError::Ended)),
            ),
            LinkAfter::Gone,
        );
        let note = out.note.expect("said");
        assert!(
            note.starts_with("The connection to D110_M-GB28010920 dropped during label 2 of 3."),
            "{note}"
        );
        assert!(note.contains("1 of 3 labels printed before this"), "{note}");
        assert!(
            note.ends_with("If the printer is still on, switch it off and on, then connect again."),
            "{note}"
        );
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(p.generation(), generation + 1);
        assert!(p.facts().is_none());
        assert_eq!(p.view().primary, Some(Primary::Connect));
        assert_eq!(p.view().step, note);
        // No Dropped follows a job's own drop, and one that did is not news.
        assert_eq!(
            p.apply(PrintCmd::Heard(Heard::Dropped { ticket: 1 })),
            PrintOutcome::quiet()
        );

        // A drop heard before the link took the Print: nothing was sent.
        let mut p = printing(3);
        let generation = p.generation();
        let out = p.apply(PrintCmd::Heard(Heard::Dropped { ticket: 1 }));
        assert_eq!(
            out.note.as_deref(),
            Some(
                "The connection to D110_M-GB28010920 dropped, so nothing was printed. Connect \
                 again to use it."
            )
        );
        assert_eq!(out.focus, Some(Focus::Close));
        assert_eq!(p.generation(), generation + 1);
        assert!(!p.busy());
        // And the refusal that follows it is not news either.
        assert_eq!(
            p.apply(PrintCmd::Heard(Heard::PrintRefused {
                ticket: 1,
                why: PrintRefusal::NotConnected
            })),
            PrintOutcome::quiet()
        );

        // Dropped at the check.
        let said = job_says(
            NAME,
            &report(
                0,
                3,
                failed(
                    1,
                    Reached::Check,
                    JobError::Session(SessionError::Transport(TransportError::Disconnected)),
                ),
            ),
            LinkAfter::Gone,
            SETTINGS,
        );
        assert_eq!(
            said,
            "The connection to D110_M-GB28010920 dropped before printing started, so nothing \
             was printed. Connect again to use it."
        );
    }

    #[test]
    fn closing_during_a_job_holds_then_replays() {
        let mut p = printing(3);
        news(&mut p, RunNews::Checking);
        news(&mut p, RunNews::Starting { n: 1, of: 3 });
        let out = p.apply(PrintCmd::WindowClosing);
        assert!(p.busy());
        assert_eq!(out.order, Some((1, Order::StopAfterLabel)));
        let says = "Closing once the label in progress has printed; no other label will start.";
        assert_eq!(out.note.as_deref(), Some(says));
        assert!(!out.replay_close);
        // The slot hid: nothing more can be stopped.
        assert_eq!(p.view().job_slot, None);
        assert_eq!(out.focus, Some(Focus::Close));
        // Again: said again, and nothing ordered twice.
        let again = p.apply(PrintCmd::WindowClosing);
        assert_eq!(again.order, None);
        assert_eq!(again.note.as_deref(), Some(says));
        assert!(p.busy());
        let out = ended(&mut p, report(1, 3, RunEnd::Stopped), LinkAfter::Up);
        assert!(out.replay_close);
        assert!(!p.busy());
        assert_eq!(
            out.note.as_deref(),
            Some("Stopped after label 1 of 3: 1 printed, 2 not printed.")
        );

        // With no job, nothing is held.
        let mut p = connected();
        let out = p.apply(PrintCmd::WindowClosing);
        assert_eq!(out, PrintOutcome::quiet());
        assert!(!out.replay_close);
        assert!(!p.busy());

        // Before the editor has heard of a label, one may already be in hand,
        // for one label as for three.
        for of in [1, 3] {
            let mut p = printing(of);
            let out = p.apply(PrintCmd::WindowClosing);
            assert_eq!(out.order, Some((1, Order::StopAfterLabel)), "{of}");
            assert_eq!(
                out.note.as_deref(),
                Some(
                    "Closing once the label in progress, if any, has printed; no other label will start."
                ),
                "{of}"
            );
        }

        // A job of one label, and the last label of three: no more would start.
        let mut p = printing(1);
        news(&mut p, RunNews::Starting { n: 1, of: 1 });
        let out = p.apply(PrintCmd::WindowClosing);
        assert_eq!(out.order, None);
        assert_eq!(
            out.note.as_deref(),
            Some("Closing once the label has printed.")
        );
        let mut p = printing(3);
        up_to(&mut p, 2, 3);
        let out = p.apply(PrintCmd::WindowClosing);
        assert_eq!(out.order, None);
        assert_eq!(
            out.note.as_deref(),
            Some("Closing once the label has printed.")
        );
        let out = ended(&mut p, report(3, 3, RunEnd::Done), LinkAfter::Up);
        assert!(out.replay_close);

        // Stopped already from the slot: the close orders nothing more.
        let mut p = printing(3);
        up_to(&mut p, 0, 3);
        p.apply(PrintCmd::StopAfterLabel);
        let out = p.apply(PrintCmd::WindowClosing);
        assert_eq!(out.order, None);
        assert_eq!(
            out.note.as_deref(),
            Some("Closing once the label in progress has printed; no other label will start.")
        );
    }

    /// News, an ending or a refusal on an old ticket is from a link that has
    /// gone: it neither ends the job running now nor lets a held close go.
    /// Were an ending let through, the window could close mid-label and the
    /// link be given the idle time to finish in.
    #[test]
    fn job_news_on_an_old_ticket_is_ignored() {
        let mut p = Printing::new();
        p.apply(PrintCmd::Connect);
        p.apply(PrintCmd::StopWaiting);
        p.apply(PrintCmd::Connect);
        found(&mut p, vec![seen(7, NAME, true)]);
        ready(&mut p, mock::facts(2320, NAME));
        copies(&mut p, "3");
        let out = print(&mut p);
        assert!(
            matches!(out.order, Some((3, Order::Print { labels: 3, .. }))),
            "{out:?}"
        );
        news(&mut p, RunNews::Checking);
        news(&mut p, RunNews::Starting { n: 1, of: 3 });
        p.apply(PrintCmd::WindowClosing);
        let (view, generation) = (p.view(), p.generation());
        for ticket in [1, 2] {
            let late = [
                Heard::Job {
                    ticket,
                    news: RunNews::Starting { n: 2, of: 3 },
                },
                Heard::Job {
                    ticket,
                    news: RunNews::Printed { n: 1, of: 3 },
                },
                Heard::JobEnded {
                    ticket,
                    report: report(3, 3, RunEnd::Done),
                    link: LinkAfter::Up,
                },
                Heard::JobEnded {
                    ticket,
                    report: report(0, 3, RunEnd::Stopped),
                    link: LinkAfter::Gone,
                },
                Heard::PrintRefused {
                    ticket,
                    why: PrintRefusal::Busy,
                },
                Heard::PrintRefused {
                    ticket,
                    why: PrintRefusal::NotConnected,
                },
                Heard::Dropped { ticket },
                Heard::Closed { ticket },
            ];
            for h in late {
                let out = p.apply(PrintCmd::Heard(h.clone()));
                assert_eq!(out, PrintOutcome::quiet(), "{h:?}");
                assert!(p.busy(), "{h:?}");
                assert_eq!(p.view(), view, "{h:?}");
                assert_eq!(p.generation(), generation, "{h:?}");
            }
        }
        // The job's own ending ends it, and lets the close go.
        let out = ended(&mut p, report(1, 3, RunEnd::Stopped), LinkAfter::Up);
        assert!(out.replay_close);
        assert!(!p.busy());
    }

    #[test]
    fn link_ended_and_refusals_mid_job() {
        let mut p = printing(3);
        up_to(&mut p, 1, 3);
        news(&mut p, RunNews::Sending { n: 2, of: 3 });
        p.apply(PrintCmd::WindowClosing);
        let generation = p.generation();
        let out = p.apply(PrintCmd::LinkEnded);
        assert_eq!(
            out.note,
            Some(format!(
                "{BROKEN} 1 of 3 labels printed before this; label 2 may be printed only in \
                 part, and the rest were not sent."
            ))
        );
        assert!(out.replay_close);
        assert_eq!(out.focus, Some(Focus::Close));
        assert!(matches!(p.state, State::Broken));
        assert_eq!(p.generation(), generation + 1);
        assert!(!p.busy());

        // Without a close held, nothing is replayed.
        let mut p = printing(3);
        let out = p.apply(PrintCmd::LinkEnded);
        assert!(!out.replay_close);
        assert_eq!(
            out.note,
            Some(format!("{BROKEN} None of the 3 labels was printed."))
        );

        // Busy answers a second Print, which the machine never sends.
        let mut p = printing(3);
        let busy = p.apply(PrintCmd::Heard(Heard::PrintRefused {
            ticket: 1,
            why: PrintRefusal::Busy,
        }));
        assert_eq!(busy, PrintOutcome::quiet());
        assert!(p.busy());

        let mut p = printing(3);
        p.apply(PrintCmd::WindowClosing);
        let generation = p.generation();
        let out = p.apply(PrintCmd::Heard(Heard::PrintRefused {
            ticket: 1,
            why: PrintRefusal::NotConnected,
        }));
        let note = out.note.expect("said");
        assert!(note.starts_with("Nothing printed"), "{note}");
        assert!(out.replay_close);
        assert_eq!(p.generation(), generation + 1);
        assert_eq!(p.view().primary, Some(Primary::Connect));

        for why in [PrintRefusal::BadCount, PrintRefusal::WrongHead] {
            let mut p = printing(3);
            let generation = p.generation();
            let out = p.apply(PrintCmd::Heard(Heard::PrintRefused { ticket: 1, why }));
            let note = out.note.expect("said");
            assert!(note.starts_with("Nothing printed: "), "{note}");
            assert!(note.contains(NAME), "{note}");
            assert!(!p.busy());
            assert_eq!(p.generation(), generation, "still connected");
            assert_eq!(p.view().job_slot, Some(JobSlot::Print));
            assert_eq!(p.view().step, note);
        }
    }

    #[test]
    fn label_types_are_said_in_words() {
        for (t, says) in [
            (LabelType::Gap, "labels with gaps"),
            (LabelType::Black, "labels with black marks"),
            (LabelType::Continuous, "continuous tape"),
            (LabelType::Hole, "labels with holes"),
            (LabelType::Transparent, "transparent labels"),
            (LabelType::PvcTag, "PVC tags"),
            (LabelType::BlackMarkGap, "labels with black marks and gaps"),
            (LabelType::HeatShrinkTube, "heat-shrink tube"),
            (LabelType::Unknown(7), "label type 7"),
        ] {
            assert_eq!(label_type_says(t), says);
        }
    }
}
