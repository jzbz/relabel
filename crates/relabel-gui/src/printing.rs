//! The printer link as the editor sees it: looking for printers, choosing one,
//! connecting, what is connected, and disconnecting - and every sentence the
//! operator hears about it.
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
//! is ignored without a word rather than taken for news.
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
//! # Generations
//!
//! [`Printing::generation`] moves whenever what [`Printing::facts`] returns does,
//! which is what a proof records so the wiring can tell it was drawn for another
//! printer, or for none.

use relabel_core::HandshakeStep;
use relabel_core::link::{CONNECT_WITHIN, Facts, Failure, Heard, NotPrintable, Order, Seen};

/// How long a scan listens, in seconds.
pub const FIND_SECS: u64 = 4;

/// How many printers are offered as choices. The pane has this many buttons.
pub const MAX_CHOICES: usize = 4;

/// Said when the link actor has stopped, which only a panic inside it does.
const BROKEN: &str =
    "The printer connection stopped working inside relabel; restart relabel to connect again.";

const NOT_CONNECTED: &str = "Not connected. Connect looks for your printer over Bluetooth.";

/// What the operator did, or what the link said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrintCmd {
    Connect,
    /// One of the offered printers, by its place among them.
    Choose(usize),
    StopWaiting,
    Disconnect,
    Heard(Heard),
    /// The link actor has ended, and nothing more will be heard.
    LinkEnded,
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
}

impl PrintOutcome {
    fn quiet() -> PrintOutcome {
        PrintOutcome {
            order: None,
            note: None,
            focus: None,
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
    Ready {
        facts: Facts,
    },
    /// Disconnect was pressed. The facts stay until the link says Closed, so a
    /// proof drawn for the printer is not redrawn twice over one disconnect.
    Closing {
        facts: Facts,
    },
    Broken,
}

/// The editor's side of the printer link.
#[derive(Debug, Clone)]
pub struct Printing {
    ticket: u64,
    generation: u64,
    state: State,
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
        }
    }

    /// The printer connected and identified, when there is one.
    pub fn facts(&self) -> Option<&Facts> {
        match &self.state {
            State::Ready { facts } | State::Closing { facts } => Some(facts),
            _ => None,
        }
    }

    /// Moves whenever [`Printing::facts`] does.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn apply(&mut self, cmd: PrintCmd) -> PrintOutcome {
        match cmd {
            PrintCmd::Connect => self.connect(),
            PrintCmd::Choose(i) => self.choose(i),
            PrintCmd::StopWaiting => self.stop_waiting(),
            PrintCmd::Disconnect => self.disconnect(),
            PrintCmd::Heard(h) => self.heard(h),
            PrintCmd::LinkEnded => {
                if self.facts().is_some() {
                    self.generation += 1;
                }
                self.state = State::Broken;
                // The primary slot is gone, so the keyboard is not left on it.
                PrintOutcome::said(BROKEN).moving(Focus::Close)
            }
        }
    }

    fn order(&self, order: Order) -> Option<(u64, Order)> {
        Some((self.ticket, order))
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
        let State::Ready { facts } = &self.state else {
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
                    State::Ready { facts },
                    PrintOutcome::said(note).moving(Focus::Close),
                )
            }
            (State::Ready { facts } | State::Closing { facts }, Heard::Dropped { .. }) => {
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
            // Anything else is an answer the machine is no longer waiting for.
            (state, _) => (state, PrintOutcome::quiet()),
        };
        self.state = state;
        out
    }

    pub fn view(&self) -> LinkView {
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
            State::Ready { facts } => (printer_line(facts), "Connected.".into()),
            State::Closing { facts } => (
                format!("Disconnecting from {}", facts.name),
                format!("Disconnecting from {}.", facts.name),
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
            State::Closing { .. } | State::Broken => None,
        };
        let primary_says = match (primary, &self.state) {
            (Some(Primary::Connect), _) => {
                "Look for your printer over Bluetooth and connect to it".into()
            }
            (Some(Primary::StopWaiting), _) => "Stop looking or connecting".into(),
            (Some(Primary::CancelChoice), _) => "Choose no printer".into(),
            (Some(Primary::Disconnect), State::Ready { facts }) => {
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
        // so that Return never stops a connect or drops a link by accident.
        let focus = match &self.state {
            State::Off { .. } => Focus::Primary,
            State::Choosing { .. } => Focus::FirstChoice,
            _ => Focus::Close,
        };
        LinkView {
            printer_line,
            step,
            primary,
            primary_says,
            choices,
            more,
            focus,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_core::link::{DeviceKey, mock};
    use relabel_proto::payload::InfoField;

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
}
