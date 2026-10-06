//! The print pane's wiring and the Printer line beside the File bar, driven
//! through [`Printing`] and [`prove`].
//!
//! Every route into the link is one call to `drive`, which does the same four
//! things in order: apply the command to the machine, say its note on the status
//! line, hand the link the order it carries, and push what the machine and the
//! proof now say - and, when a job the window's close was held for has ended,
//! ask for the close again. No `RefCell` is borrowed while the order is handed
//! over or the close asked for: a plug may deliver news from inside it - a test's
//! does, and the real one does when the link cannot start - and that news is
//! another `drive`. This is `file`'s rule for the file dialog.
//!
//! # When the pane is drawn again
//!
//! Only while it is open (`Filing::previewing`), and only when what it shows is
//! stale: the proof records the document's revision and the link's generation it
//! was made at, and either can move behind the pane - an assistive technology's
//! edit reaches the canvas through its popup, and news arrives whenever the link
//! has some. The pane is a popup, so pointer and keys cannot edit behind it.
//!
//! # What is printed
//!
//! The page of the proof the pane shows, as it was encoded for that proof and
//! never again: the job slot gathers it only from a proof that is printable,
//! drawn for the connected printer and for the document as it now stands. A
//! proof that is stale is drawn again, shown, and nothing is printed, so what
//! prints is always what the operator last saw. Behind those gates a fresh
//! encode would give the same bytes, so no test can tell a cloned page from one
//! encoded again: that it is the proof's own rests on `job_cmd` alone. The
//! proof whose page was sent is kept while the job runs, and the pane shows it,
//! opened again or not, whatever is edited, opened or made behind it; when the
//! job ends it is let go, and the document is drawn again if the pane is open.
//! A job's news repaints nothing but the Printer line and the job line.
//!
//! # Closing the window during a job
//!
//! The close is held ([`Prints::hold_close`]), and no label starts after the one
//! in hand. Once the job has ended the close is asked for again, through the file
//! commands, as the close button would have asked: unsaved work is asked about
//! then, after the label, and a held element still refuses it.
//!
//! # Where the keyboard goes
//!
//! Where [`Printing`] says, and only while the pane is open: on opening; to the
//! first printer on entering a choice; and to Close whenever the primary slot or
//! the job slot changes face under it. Connect, and opening a printer, chosen or
//! the only one found, make the primary slot Stop waiting; Stop waiting, Cancel,
//! none found, a failure and a drop make it Connect; connecting makes it
//! Disconnect; and disconnecting and the link ending hide it. Print makes the job
//! slot Stop after this label, or hides it; the last label's start and a stop
//! hide it; and a job's end makes it Print again. So a second Return, or Return
//! held down, never stops, starts or drops a link, never prints twice and never
//! stops the job it started, and neither does a Return meant for the face a slot
//! had a moment before. Otherwise it stays where the operator left it. The issue
//! list leaves the pane while a printer is chosen, when the keyboard is on a
//! choice. Its rows are one model for the life of the window, written row by row,
//! so an update never takes the keyboard off the row that holds it
//! (`canvas::Canvas`, `rows`, for the reason).
//!
//! # The grey half
//!
//! When the proof keeps a grey ([`Proof::grey`]), the pane shows it under the
//! dots, each beside its caption. The dots are fitted to a box of their own: as
//! wide as the pane less its padding, the caption column and the spacing beside
//! it, 96 pixels in all where the single image's box is the pane less 60, and
//! half of the single image's height once the spacing between the two halves is
//! taken out, 96 pixels. The grey is painted at the scale the dots were fitted
//! at. So the two are the same size, left edges aligned, and each column of dots
//! sits straight above the grey it was cut from. Without a grey the dots have
//! the single image's larger box. The grey travels inside the proof, so it is
//! never older or newer than the dots beside it, and the pane changes nothing: a
//! halftone or a threshold is the inspector's to change, per image.

use crate::canvas::{Canvas, PanePush, said_again};
use crate::doc::Doc;
use crate::file::CloseReplay;
use crate::filing::{FileCmd, Filing, Now};
use crate::fresh;
use crate::plug::{News, Plug};
use crate::preview::{paint_fit, paint_grey_at};
use crate::printing::{Focus, JobSlot, LinkView, NotReady, PrintCmd, Printing};
use crate::proof::{Proof, prove};
use crate::rows::{ChoiceItem, IssueItem, PrintHost};
use relabel_core::link::Heard;
use slint::{Model, ModelRc, VecModel};
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

/// The pane's widest, in logical pixels, and what it keeps from the window's
/// edges when the window is narrower. shell.slint places it with the same two
/// numbers: `Math.min(920px, root.width - 40px)`.
const PANE_W: f32 = 920.0;
const PANE_MARGIN: f32 = 40.0;
/// The box the single image is fitted into, in logical pixels: this much
/// narrower than the pane for its padding, and this tall. The author's 40 by 12
/// mm label fits it at two pixels to the dot at scale 1, and a 100 mm label at
/// one.
const IMAGE_INSET: f32 = 60.0;
const IMAGE_H: f32 = 200.0;
/// The box each half is fitted into when the grey is shown, in logical pixels:
/// this much narrower than the pane, for its 32 of padding, the 56-pixel caption
/// column and the 8 between caption and image; and this tall, so the two halves
/// and the 8 between them are the single image's 200. A 100 mm label fits it at
/// one pixel to the dot at scale 1.
const SPLIT_INSET: f32 = 96.0;
const SPLIT_H: f32 = 96.0;

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
    /// The document's proof, drawn last.
    proof: RefCell<Option<Proof>>,
    /// The proof whose page a running job is printing, which the pane shows in
    /// place of the document's until the job ends.
    printing_proof: RefCell<Option<Proof>>,
    /// The close button's request again, through the file commands: set once by
    /// the wiring, and called when a job the close was held for has ended.
    close_replay: RefCell<Option<CloseReplay<H>>>,
    /// The issue list's one model, for `rows`' reason.
    issues: Rc<VecModel<H::Issue>>,
}

/// Wire the Printer line and the print pane to a window whose canvas is wired
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
        printing_proof: RefCell::new(None),
        close_replay: RefCell::new(None),
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
                    debug_assert_eq!(out.note, None, "closing the pane is quiet");
                }
                // The job slot. What it holds is the machine's to say, never the
                // face the button had when it was pressed.
                2 => {
                    let cmd = prints.job_cmd();
                    prints.drive(&host, cmd);
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
    {
        let prints = prints.clone();
        host.on_preview_copies_edited(move |text| {
            let Some(host) = prints.host.upgrade() else {
                return;
            };
            prints.drive(&host, PrintCmd::Copies(text.to_string()));
        });
    }

    prints.show(host, None);
    prints
}

impl<H: PrintHost> Prints<H> {
    /// The proof the pane shows, or showed last: a running job's own, else the
    /// document's.
    pub fn proof(&self) -> Option<Proof> {
        self.shown().clone()
    }

    fn shown(&self) -> Ref<'_, Option<Proof>> {
        let job = self.printing_proof.borrow();
        if job.is_some() {
            return job;
        }
        drop(job);
        self.proof.borrow()
    }

    /// Whether a job is running, which holds the window's close.
    pub fn busy(&self) -> bool {
        self.printing.borrow().busy()
    }

    /// The window's close button, before the file commands see it. While a job
    /// runs the close is held: no label starts after the one in hand, the status
    /// line says so, and this answers true, for the window to stay. Otherwise it
    /// does nothing and answers false.
    pub fn hold_close(&self, host: &H) -> bool {
        if !self.busy() {
            return false;
        }
        self.drive(host, PrintCmd::WindowClosing);
        true
    }

    /// What asks for the close again once a job it was held for has ended.
    pub fn set_close_replay(&self, replay: CloseReplay<H>) {
        *self.close_replay.borrow_mut() = Some(replay);
    }

    /// Open the pane, drawn afresh: the file commands' `Next::Preview`, once the
    /// machine is previewing and every other popup is closed.
    ///
    /// While a job runs it shows the job's own proof; the document's is drawn
    /// all the same, for the pane to show once the job has ended.
    pub fn open(&self, host: &H) {
        self.reprove(self.canvas.borrow().doc());
        let view = self.view();
        self.push_pane(host, &view);
        // Read by the pane as it opens.
        host.set_preview_focus_to(view.focus.token());
        host.invoke_show_preview();
    }

    /// What the job slot gives: Stop after this label while a job runs, which
    /// the machine takes only while it can still stop one; otherwise Print, with
    /// the page of the proof the pane shows when that proof is printable, drawn
    /// for the connected printer and for the document as it stands. A stale
    /// proof is drawn again, and the command says so, for the operator to check
    /// the new one first.
    fn job_cmd(&self) -> PrintCmd {
        if self.busy() {
            return PrintCmd::StopAfterLabel;
        }
        let stale = {
            let c = self.canvas.borrow();
            self.is_stale(c.doc())
        };
        if stale {
            self.reprove(self.canvas.borrow().doc());
            return PrintCmd::Print(Err(NotReady::Stale));
        }
        let generation = self.printing.borrow().generation();
        let proof = self.proof.borrow();
        let page = proof
            .as_ref()
            .filter(|p| p.printable() && p.printer.is_some() && p.generation == generation)
            .and_then(|p| p.page.clone());
        PrintCmd::Print(page.ok_or(NotReady::NotPrintable))
    }

    /// One printer command, start to finish.
    fn drive(&self, host: &H, cmd: PrintCmd) {
        // News of a job and a typed count change only the job's own lines,
        // unless the job slot changed face with them.
        let light = matches!(
            cmd,
            PrintCmd::Copies(_) | PrintCmd::Heard(Heard::Job { .. })
        );
        // Print sends the page of the proof shown now, which is the one kept.
        let sent = match &cmd {
            PrintCmd::Print(Ok(_)) => self.proof.borrow().clone(),
            _ => None,
        };
        let was_busy = self.busy();
        let out = self.printing.borrow_mut().apply(cmd);
        // Before the order is handed over: the job can end inside it.
        match (was_busy, self.busy()) {
            (false, true) => *self.printing_proof.borrow_mut() = sent,
            (true, false) => *self.printing_proof.borrow_mut() = None,
            _ => {}
        }
        self.connected.set(self.connected_now());
        if let Some(note) = out.note {
            host.set_status(said_again(&host.get_status(), note).as_str().into());
        }
        // Nothing borrowed: news can come back from inside this.
        if let Some((ticket, order)) = out.order {
            self.plug.order(ticket, order);
        }
        if light && out.focus.is_none() {
            self.show_job(host);
        } else {
            self.show(host, out.focus);
        }
        // Last, with nothing borrowed: the close goes through the file commands,
        // which may hide the window or ask about unsaved work.
        if out.replay_close {
            let replay = self.close_replay.borrow().clone();
            if let Some(replay) = replay {
                replay(host);
            }
        }
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

    /// Push the Printer line, and the pane's job line and slots when it is open:
    /// all that a job's news or a typed count changes. The image, the verdict
    /// and the issues stay as they are.
    fn show_job(&self, host: &H) {
        let view = self.view();
        host.set_printer_line(view.printer_line.as_str().into());
        if self.filing.borrow().previewing() {
            push_job(host, &view);
        }
    }

    /// What the machine says, with Print's description naming the size of the
    /// label the pane shows.
    fn view(&self) -> LinkView {
        let size = self.shown().as_ref().map(Proof::size_says);
        self.printing.borrow().view_for(size.as_deref())
    }

    /// Push the link as it now stands: the Printer line always, and the pane when
    /// it is open, drawn again when stale.
    fn show(&self, host: &H, focus: Option<Focus>) {
        let view = self.view();
        host.set_printer_line(view.printer_line.as_str().into());
        if !self.filing.borrow().previewing() {
            return;
        }
        let stale = {
            let c = self.canvas.borrow();
            self.is_stale(c.doc())
        };
        let view = if stale {
            self.reprove(self.canvas.borrow().doc());
            self.view()
        } else {
            view
        };
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
        let view = self.view();
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

    /// Everything the pane shows, from the proof it shows and `view`.
    fn push_pane(&self, host: &H, view: &LinkView) {
        let proof = self.shown();
        let Some(proof) = proof.as_ref() else {
            return;
        };
        host.set_preview_verdict(proof.verdict().as_str().into());
        host.set_preview_step(view.step.as_str().into());

        let grey = proof.grey.as_ref();
        let (box_w, box_h) = image_box(host.window(), grey.is_some());
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
                // At the dots' own scale, never fitted on its own.
                match grey.and_then(|g| paint_grey_at(&g.map, scale)) {
                    Some(painted) => {
                        host.set_preview_grey_px_w(painted.px_w);
                        host.set_preview_grey_px_h(painted.px_h);
                        host.set_preview_grey(painted.image);
                        let says = proof.grey_says(Some(scale.dots_per_pixel()));
                        host.set_preview_grey_says(says.as_str().into());
                    }
                    None => clear_grey(host),
                }
            }
            None => {
                host.set_preview_plane(slint::Image::default());
                host.set_preview_plane_px_w(0);
                host.set_preview_plane_px_h(0);
                host.set_preview_plane_says(proof.plane_says(None).as_str().into());
                clear_grey(host);
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
        push_job(host, view);
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

/// The job slot, the Copies field and the job line, from `view`.
fn push_job<H: PrintHost>(host: &H, view: &LinkView) {
    host.set_preview_print_face(view.job_slot.map_or("", JobSlot::face).into());
    host.set_preview_print_says(view.job_slot_says.as_str().into());
    host.set_preview_print_shown(view.job_slot.is_some());
    host.set_preview_copies_start(view.copies_start.as_str().into());
    host.set_preview_copies_says(view.copies_says.into());
    host.set_preview_copies_shown(view.copies_shown);
    match &view.job {
        Some(job) => {
            host.set_preview_job_done(job.done());
            host.set_preview_job_says(job.says.as_str().into());
            host.set_preview_job_shown(true);
        }
        None => {
            host.set_preview_job_shown(false);
            host.set_preview_job_done(0.0);
            host.set_preview_job_says("".into());
        }
    }
    host.set_preview_image_label(view.image_label.into());
}

/// No grey half: the pane shows the dots alone.
fn clear_grey<H: PrintHost>(host: &H) {
    host.set_preview_grey(slint::Image::default());
    host.set_preview_grey_px_w(0);
    host.set_preview_grey_px_h(0);
    host.set_preview_grey_says("".into());
}

/// The box the dots are fitted into, in whole physical pixels: the single
/// image's, or each half's when `split`.
fn image_box(window: &slint::Window, split: bool) -> (u32, u32) {
    let scale = window.scale_factor();
    image_box_at(window.size().width as f32 / scale, scale, split)
}

/// The pane's own arithmetic from shell.slint and print.slint, in logical
/// pixels for a window `window_w` wide, times the display scale `scale`.
fn image_box_at(window_w: f32, scale: f32, split: bool) -> (u32, u32) {
    let pane_w = PANE_W.min(window_w - PANE_MARGIN);
    let (w, h) = if split {
        (pane_w - SPLIT_INSET, SPLIT_H)
    } else {
        (pane_w - IMAGE_INSET, IMAGE_H)
    };
    let phys = |logical: f32| ((logical * scale).floor() as u32).max(1);
    (phys(w.max(1.0)), phys(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default 1200-pixel window at every scale the layout tests run: the
    /// single image's box is the pane less 60 by 200, each half's the pane less
    /// 96 by 96, floored to whole device pixels. A window narrower than the pane
    /// and its margins narrows both.
    #[test]
    fn the_image_box_at_every_scale() {
        let single = [(860, 200), (1075, 250), (1290, 300), (1720, 400)];
        let split = [(824, 96), (1030, 120), (1236, 144), (1648, 192)];
        for (i, s) in [1.0, 1.25, 1.5, 2.0].into_iter().enumerate() {
            assert_eq!(image_box_at(1200.0, s, false), single[i], "scale {s}");
            assert_eq!(image_box_at(1200.0, s, true), split[i], "scale {s}");
        }
        assert_eq!(image_box_at(700.0, 1.0, true), (564, 96));
        assert_eq!(image_box_at(700.0, 1.0, false), (600, 200));
    }
}
