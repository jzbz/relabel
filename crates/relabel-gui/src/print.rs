//! The Preview pane's wiring and the Printer line beside the File bar, driven
//! through [`Printing`] and [`prove`].
//!
//! Every route into the link is one call to `drive`, which does the same four
//! things in order: apply the command to the machine, say its note on the status
//! line, hand the link the order it carries, and push what the machine and the
//! proof now say. No `RefCell` is borrowed while the order is handed over: a plug
//! may deliver news from inside it - a test's does, and the real one does when the
//! link cannot start - and that news is another `drive`. This is `file`'s rule for
//! the file dialog.
//!
//! # When the pane is drawn again
//!
//! Only while it is open (`Filing::previewing`), and only when what it shows is
//! stale: the proof records the document's revision and the link's generation it
//! was made at, and either can move behind the pane - an assistive technology's
//! edit reaches the canvas through its popup, and news arrives whenever the link
//! has some. The pane is a popup, so pointer and keys cannot edit behind it.
//!
//! # Where the keyboard goes
//!
//! Where [`Printing`] says, and only while the pane is open: on opening; to the
//! first printer on entering a choice; and to Close whenever the primary slot
//! changes face under it. Connect, and opening a printer, chosen or the only one
//! found, make it Stop waiting; Stop waiting, Cancel, none found, a failure and a
//! drop make it Connect; connecting makes it Disconnect; and disconnecting and the
//! link ending hide it. So a second Return, or Return held down, never stops,
//! starts or drops a link, and neither does a Return meant for the face the slot
//! had a moment before. Otherwise it stays where the operator left it. The issue
//! list leaves the pane while a printer is chosen, when the keyboard is on a
//! choice. Its rows are one model for the life of the window, written row by row,
//! so an update never takes the keyboard off the row that holds it
//! (`canvas::Canvas`, `rows`, for the reason).

use crate::canvas::{Canvas, PanePush, said_again};
use crate::doc::Doc;
use crate::filing::{FileCmd, Filing, Now};
use crate::fresh;
use crate::plug::{News, Plug};
use crate::preview::paint_fit;
use crate::printing::{Focus, LinkView, PrintCmd, Printing};
use crate::proof::{Proof, prove};
use crate::rows::{ChoiceItem, IssueItem, PrintHost};
use slint::{Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The pane's widest, in logical pixels, and what it keeps from the window's
/// edges when the window is narrower. shell.slint places it with the same two
/// numbers.
const PANE_W: f32 = 700.0;
const PANE_MARGIN: f32 = 40.0;
/// The box the image is fitted into, in logical pixels: at most this wide, and
/// this much narrower than the pane for its padding; and this tall. The author's
/// 40 by 12 mm label fits it at two pixels to the dot at scale 1.
const IMAGE_W: f32 = 640.0;
const IMAGE_INSET: f32 = 60.0;
const IMAGE_H: f32 = 200.0;

/// Everything `drive` needs, shared by every callback.
pub struct Prints<H: PrintHost> {
    host: slint::Weak<H>,
    canvas: Rc<RefCell<Canvas<H>>>,
    filing: Rc<RefCell<Filing>>,
    plug: Rc<dyn Plug>,
    printing: RefCell<Printing>,
    /// The connected printer, as the New label pane sees it
    /// (`fresh::printer_for`).
    connected: Rc<Cell<Option<fresh::Connected>>>,
    /// What the pane shows, or showed last.
    proof: RefCell<Option<Proof>>,
    /// The issue list's one model, for `rows`' reason.
    issues: Rc<VecModel<H::Issue>>,
}

/// Wire the Printer line and the Preview pane to a window whose canvas is wired
/// already. Before the files: the file commands open the pane through
/// [`Prints::open`].
///
/// `plug` carries orders to the printer link: [`crate::plug::NativePlug`] in the
/// application, a stand-in in a test. `connected` is written here and read by the
/// New label pane.
pub fn wire<H: PrintHost>(
    host: &H,
    canvas: &Rc<RefCell<Canvas<H>>>,
    filing: &Rc<RefCell<Filing>>,
    plug: Rc<dyn Plug>,
    connected: Rc<Cell<Option<fresh::Connected>>>,
) -> Rc<Prints<H>> {
    let prints = Rc::new(Prints {
        host: host.as_weak(),
        canvas: canvas.clone(),
        filing: filing.clone(),
        plug,
        printing: RefCell::new(Printing::new()),
        connected,
        proof: RefCell::new(None),
        issues: Rc::new(VecModel::default()),
    });
    host.set_preview_issues(ModelRc::from(prints.issues.clone()));

    // Weak, both: the plug and the canvas are held by `prints`, and a strong
    // handle from either back to it would be a cycle nothing ever frees.
    {
        let weak = Rc::downgrade(&prints);
        prints.plug.listen(Rc::new(move |news| {
            let Some(prints) = weak.upgrade() else { return };
            let Some(host) = prints.host.upgrade() else {
                return;
            };
            let cmd = match news {
                News::Heard(h) => PrintCmd::Heard(h),
                News::Ended => PrintCmd::LinkEnded,
            };
            prints.drive(&host, cmd);
        }));
    }
    {
        let weak = Rc::downgrade(&prints);
        let push: PanePush<H> = Rc::new(move |host: &H, doc: &Doc| {
            if let Some(prints) = weak.upgrade() {
                prints.edited(host, doc);
            }
        });
        canvas.borrow_mut().tell_print(push);
    }

    {
        let prints = prints.clone();
        host.on_preview_act(move |what| {
            let Some(host) = prints.host.upgrade() else {
                return;
            };
            match what {
                0 => {
                    let primary = prints.printing.borrow().view().primary;
                    if let Some(p) = primary {
                        prints.drive(&host, p.cmd());
                    }
                }
                // The pane has closed itself. The preview reads nothing of the
                // document to close, so no `Now` is read for it.
                1 => {
                    let out = prints
                        .filing
                        .borrow_mut()
                        .apply(FileCmd::PreviewClosed, &Now::default());
                    debug_assert_eq!(out.note, None, "closing the preview is quiet");
                }
                _ => {}
            }
        });
    }
    {
        let prints = prints.clone();
        host.on_preview_choose(move |i| {
            let (Some(host), Ok(i)) = (prints.host.upgrade(), usize::try_from(i)) else {
                return;
            };
            prints.drive(&host, PrintCmd::Choose(i));
        });
    }

    prints.show(host, None);
    prints
}

impl<H: PrintHost> Prints<H> {
    /// The proof the pane shows, or showed last.
    pub fn proof(&self) -> Option<Proof> {
        self.proof.borrow().clone()
    }

    /// Open the pane, drawn afresh: the file commands' `Next::Preview`, once the
    /// machine is previewing and every other popup is closed.
    pub fn open(&self, host: &H) {
        self.reprove(self.canvas.borrow().doc());
        let view = self.printing.borrow().view();
        self.push_pane(host, &view);
        // Read by the pane as it opens.
        host.set_preview_focus_to(view.focus.token());
        host.invoke_show_preview();
    }

    /// One printer command, start to finish.
    fn drive(&self, host: &H, cmd: PrintCmd) {
        let out = self.printing.borrow_mut().apply(cmd);
        self.connected.set(self.connected_now());
        if let Some(note) = out.note {
            host.set_status(said_again(&host.get_status(), note).as_str().into());
        }
        // Nothing borrowed: news can come back from inside this.
        if let Some((ticket, order)) = out.order {
            self.plug.order(ticket, order);
        }
        self.show(host, out.focus);
    }

    /// The connected printer as a new label sees it: its model only when
    /// relabel can print on it. Whether a new label can be drawn for that model
    /// is `fresh::printer_for`'s to decide.
    fn connected_now(&self) -> Option<fresh::Connected> {
        let printing = self.printing.borrow();
        printing.facts().map(|f| fresh::Connected {
            model: f.model.filter(|_| f.printable.is_ok()),
        })
    }

    /// Push the link as it now stands: the Printer line always, and the pane when
    /// it is open, drawn again when stale.
    fn show(&self, host: &H, focus: Option<Focus>) {
        let view = self.printing.borrow().view();
        host.set_printer_line(view.printer_line.as_str().into());
        if !self.filing.borrow().previewing() {
            return;
        }
        let stale = {
            let c = self.canvas.borrow();
            self.is_stale(c.doc())
        };
        if stale {
            self.reprove(self.canvas.borrow().doc());
        }
        self.push_pane(host, &view);
        if let Some(focus) = focus {
            host.set_preview_focus_to(focus.token());
            // A change the pane's `changed` handler sees even when the target is
            // the one it had.
            host.set_preview_focus_serial(host.get_preview_focus_serial().wrapping_add(1));
        }
    }

    /// The canvas's push, after every outcome. Handed the document: the canvas
    /// is borrowed while this runs.
    fn edited(&self, host: &H, doc: &Doc) {
        if !self.filing.borrow().previewing() || !self.is_stale(doc) {
            return;
        }
        self.reprove(doc);
        let view = self.printing.borrow().view();
        self.push_pane(host, &view);
    }

    fn is_stale(&self, doc: &Doc) -> bool {
        let generation = self.printing.borrow().generation();
        self.proof
            .borrow()
            .as_ref()
            .is_none_or(|p| p.rev != doc.revision() || p.generation != generation)
    }

    fn reprove(&self, doc: &Doc) {
        let proof = {
            let printing = self.printing.borrow();
            prove(doc, printing.facts(), printing.generation())
        };
        *self.proof.borrow_mut() = Some(proof);
    }

    /// Everything the pane shows, from the proof and `view`.
    fn push_pane(&self, host: &H, view: &LinkView) {
        let proof = self.proof.borrow();
        let Some(proof) = proof.as_ref() else {
            return;
        };
        host.set_preview_verdict(proof.verdict().as_str().into());
        host.set_preview_step(view.step.as_str().into());

        let (box_w, box_h) = image_box(host.window());
        match proof
            .plane
            .as_ref()
            .and_then(|plane| paint_fit(plane, box_w, box_h))
        {
            Some((painted, scale)) => {
                host.set_preview_plane_px_w(painted.px_w);
                host.set_preview_plane_px_h(painted.px_h);
                host.set_preview_plane(painted.image);
                let says = proof.plane_says(Some(scale.dots_per_pixel()));
                host.set_preview_plane_says(says.as_str().into());
            }
            None => {
                host.set_preview_plane(slint::Image::default());
                host.set_preview_plane_px_w(0);
                host.set_preview_plane_px_h(0);
                host.set_preview_plane_says(proof.plane_says(None).as_str().into());
            }
        }

        let facts = proof.facts_line();
        host.set_preview_facts(facts.as_str().into());
        host.set_preview_close_says(facts.as_str().into());
        self.write_issues(&proof.issue_rows());

        let choices: Vec<H::Choice> = view
            .choices
            .iter()
            .map(|(face, says)| H::Choice::of(face, says))
            .collect();
        host.set_preview_choices(ModelRc::new(VecModel::from(choices)));
        host.set_preview_primary(view.primary.map_or("", |p| p.face()).into());
        host.set_preview_primary_says(view.primary_says.as_str().into());
        host.set_preview_primary_shown(view.primary.is_some());
        host.set_preview_primary_default(view.primary.is_some_and(|p| p.is_default()));
    }

    /// Bring the issue list to `rows`, row by row: rows past the new end go from
    /// the end, new rows are pushed, and only a row whose words changed is written.
    fn write_issues(&self, rows: &[String]) {
        while self.issues.row_count() > rows.len() {
            self.issues.remove(self.issues.row_count() - 1);
        }
        for (i, text) in rows.iter().enumerate() {
            let row = H::Issue::of(text);
            if i < self.issues.row_count() {
                if self.issues.row_data(i).as_ref().map(IssueItem::text) != Some(text.as_str()) {
                    self.issues.set_row_data(i, row);
                }
            } else {
                self.issues.push(row);
            }
        }
    }
}

/// The box the image is fitted into, in whole physical pixels: the pane's own
/// arithmetic from shell.slint, in logical pixels, times the display scale.
fn image_box(window: &slint::Window) -> (u32, u32) {
    let scale = window.scale_factor();
    let window_w = window.size().width as f32 / scale;
    let pane_w = PANE_W.min(window_w - PANE_MARGIN);
    let w = IMAGE_W.min(pane_w - IMAGE_INSET).max(1.0);
    let phys = |logical: f32| ((logical * scale).floor() as u32).max(1);
    (phys(w), phys(IMAGE_H))
}
