//! The seam between the editor and the printer link: orders out, news back, and
//! the one implementation that starts a real link.
//!
//! No toolkit type and no runtime type appears in [`Plug`], so the wiring in
//! [`crate::print`] and every test of it are written against these types alone.
//! News arrives later, through the callback [`Plug::listen`] installs, on the UI
//! thread.
//!
//! This is the only module that names relabel-core's [`Link`] or polls a future
//! of printer origin, as [`crate::choose`] is for the file dialog:
//! [`NativePlug`] is the one implementation the application uses, and tests stand
//! in their own.
//!
//! # Threads
//!
//! The link actor runs on a thread and a runtime of its own (relabel-core
//! `link`, "Threads"). This side never enters that runtime. An order is a send
//! on an unbounded channel, which needs no runtime; the news is one future,
//! [`Events::next`], polled on the event loop by `slint::spawn_local`, which is
//! runtime agnostic (tokio-1.53.1 `sync/mod.rs:438-450`). Slint posts every wake
//! of that future back to the loop through its event-loop proxy (i-slint-core
//! `future.rs:60-86`), so no Send closure touches the editor's state and no
//! Slint handle is upgraded off the UI thread.

use relabel_core::link::{BleOpener, Events, Finished, Heard, Link, Opener, Order};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// What the link says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum News {
    Heard(Heard),
    /// The link has ended, and nothing more will be heard: it could not start,
    /// or its actor stopped, which only a panic inside it does.
    Ended,
}

/// Where news goes: called on the UI thread, once for each thing the link says.
pub type Deliver = Rc<dyn Fn(News)>;

/// Something that carries orders to a printer link and news back.
pub trait Plug {
    /// Where news goes, installed once by the wiring before any order.
    fn listen(&self, deliver: Deliver);
    /// Hand the link an order. News may be delivered before this returns: a
    /// caller holds no borrow across it.
    fn order(&self, ticket: u64, order: Order);
    /// Close the link, waiting up to `within`: called once, after the window has
    /// closed. Blocks the calling thread.
    fn finish(&self, within: Duration);
}

/// The real link: relabel-core's actor over Bluetooth.
///
/// Nothing is started until the first order, which is the first press of
/// Connect, so a launch opens no system bus and raises no permission prompt.
pub struct NativePlug {
    /// Taken by the first order, which starts the link with it.
    opener: RefCell<Option<Box<dyn Opener>>>,
    link: RefCell<Option<Link>>,
    deliver: RefCell<Option<Deliver>>,
    /// Set once `News::Ended` has been delivered, by the loop or by a failed
    /// order, so it is delivered once.
    ended: Rc<Cell<bool>>,
}

impl Default for NativePlug {
    fn default() -> NativePlug {
        NativePlug::new()
    }
}

impl NativePlug {
    /// Bluetooth Low Energy, through relabel-core's `BleOpener`.
    pub fn new() -> NativePlug {
        NativePlug::with_opener(Box::new(BleOpener::new()))
    }

    /// Any opener: relabel-core's in-memory one, for the one test that runs this
    /// plug on a real event loop.
    pub fn with_opener(opener: Box<dyn Opener>) -> NativePlug {
        NativePlug {
            opener: RefCell::new(Some(opener)),
            link: RefCell::new(None),
            deliver: RefCell::new(None),
            ended: Rc::new(Cell::new(false)),
        }
    }

    fn deliver(&self) -> Option<Deliver> {
        self.deliver.borrow().clone()
    }

    /// Say the link has ended, unless it has been said.
    fn end(&self) {
        if !self.ended.replace(true)
            && let Some(deliver) = self.deliver()
        {
            deliver(News::Ended);
        }
    }

    /// Start the link and the one loop that hears it. `false` when either could
    /// not start; nothing is left running then.
    fn start(&self) -> bool {
        let Some(opener) = self.opener.borrow_mut().take() else {
            return false;
        };
        let Some(deliver) = self.deliver() else {
            return false;
        };
        let (link, mut events): (Link, Events) = match Link::start(opener) {
            Ok(started) => started,
            Err(_) => return false,
        };
        let ended = self.ended.clone();
        let heard = slint::spawn_local(async move {
            while let Some(h) = events.next().await {
                deliver(News::Heard(h));
            }
            if !ended.replace(true) {
                deliver(News::Ended);
            }
        });
        if heard.is_err() {
            // Nothing would hear the link, so it goes: dropping its sender ends
            // the actor, which closes whatever it opened (relabel-core `link`,
            // `or_shutdown`). Nothing was opened yet.
            drop(link);
            return false;
        }
        *self.link.borrow_mut() = Some(link);
        true
    }
}

impl Plug for NativePlug {
    fn listen(&self, deliver: Deliver) {
        *self.deliver.borrow_mut() = Some(deliver);
    }

    fn order(&self, ticket: u64, order: Order) {
        let running = self.link.borrow().is_some();
        if !running && !self.start() {
            self.end();
            return;
        }
        let sent = self
            .link
            .borrow()
            .as_ref()
            .map(|link| link.send(ticket, order));
        // Released before anything is delivered: the delivery is another order's
        // way in.
        if !matches!(sent, Some(Ok(()))) {
            // The actor has gone, and the loop hearing it may not have noticed yet.
            self.end();
        }
    }

    fn finish(&self, within: Duration) {
        let link = self.link.borrow_mut().take();
        if let Some(link) = link {
            // Whether the thread ended in time or not, the process is leaving.
            let _: Finished = link.shutdown(within);
        }
    }
}
