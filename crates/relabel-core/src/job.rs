//! Driving a print from start to finish.
//!
//! The shape of a job is the same everywhere - settings, start, then per page
//! (page start, geometry, rows, page end), then wait for the counter, then end -
//! and what differs between printers is two payload widths and whether the copy
//! count rides in `SetPageSize` or in its own command. That variation lives in
//! [`PrintFlow`], so this file has one code path rather than seven.
//!
//! # Two rules that are not optimisations
//!
//! **`PrintEnd` always runs**, including on the error path, because it is what
//! feeds the label out and retracts the paper. Skipping it on a failure leaves
//! the label parked under the print head.
//!
//! **Polling runs to completion before `PrintEnd`**, because `PrintEnd` arriving
//! mid-print produces a cut-off label. Completion is judged on the page counter
//! and never on the two progress percentages, which are not reliably 0-100
//! across models.
//!
//! # Two ways in
//!
//! [`print()`] is the command line's: one job for the pages it is given, with the
//! copies in it. [`print_labels`] is the editor's: a liveness check, then one
//! one-copy job per label, which is byte for byte what `relabel-cli label`
//! sends for the same labels. It exists so a run can be stopped between labels
//! and so a failure can say how many labels came out; neither needs a command
//! the printer has not been seen to take.

use crate::error::JobError;
use crate::printer::Identity;
use crate::session::{PrinterEvent, Session};
use relabel_proto::{
    EncodedPage, LabelType, PrintFlow,
    parse::PrintStatus,
    payload::{self, PageColor},
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, timeout};

/// Per-step deadlines.
///
/// Taken from the tighter of the two published sets, the one that came from
/// hardware rather than from a library's defaults. All of them are settings
/// because the only way to know is to try.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub default: Duration,
    pub settings: Duration,
    pub print_start: Duration,
    pub page_start: Duration,
    pub page_end: Duration,
    pub print_end: Duration,
    pub status: Duration,
    pub poll_interval: Duration,
    /// Overall ceiling for one page of one job.
    pub job: Duration,
    /// How many times a refused step is sent again before the job gives up,
    /// and how long to wait in between. See [`accepted_step`].
    pub refusals: u8,
    pub refusal_gap: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            default: Duration::from_millis(1000),
            settings: Duration::from_millis(1000),
            print_start: Duration::from_millis(2000),
            page_start: Duration::from_millis(1000),
            page_end: Duration::from_millis(3000),
            print_end: Duration::from_millis(2500),
            status: Duration::from_millis(900),
            poll_interval: Duration::from_millis(150),
            job: Duration::from_millis(25_000),
            // Ten is the vendor's own figure (`sRepeatRequestCounts`). A
            // D110_M answered the second job of a run within a few hundred
            // milliseconds.
            refusals: 10,
            refusal_gap: Duration::from_millis(200),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PrintOptions {
    pub density: u8,
    pub label_type: LabelType,
    /// Copies of each page.
    pub copies: u16,
    pub color: PageColor,
    /// How many copies this printer will honour in one job.
    ///
    /// `None` means no known limit. Set from
    /// [`relabel_proto::copies_per_job`], which knows the models that quietly
    /// print once and then refuse everything.
    pub copies_per_job: Option<u16>,
    pub timeouts: Timeouts,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            density: 2,
            label_type: LabelType::Gap,
            copies: 1,
            color: PageColor::SingleColor,
            copies_per_job: None,
            timeouts: Timeouts::default(),
        }
    }
}

impl PrintOptions {
    /// Fill in the per-model copy limit for a printer.
    pub fn for_model(mut self, model_id: Option<u16>) -> Self {
        self.copies_per_job = relabel_proto::copies_per_job(model_id);
        self
    }

    /// What `relabel-cli label` prints one label with on this printer: the
    /// model's default density unless `density` says otherwise, else 2; the
    /// first label type the model lists, else labels with gaps; one copy.
    ///
    /// The command line spells the same expression out itself; a unit test
    /// holds the two together.
    pub fn for_identity(id: &Identity, density: Option<u8>) -> PrintOptions {
        let model = id.model;
        PrintOptions {
            density: density.or(model.map(|m| m.density.2)).unwrap_or(2),
            label_type: model
                .and_then(|m| m.label_types.first().copied())
                .unwrap_or(LabelType::Gap),
            copies: 1,
            ..PrintOptions::default()
        }
        .for_model(id.model_id)
    }
}

/// How far along a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub page: u16,
    pub total: u16,
    /// The printer's own figures, passed through unchanged and labelled as such
    /// wherever they are shown.
    pub print_percent: u8,
    pub feed_percent: u8,
}

/// How far one job has got, as [`one_job`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// A page's rows are about to be sent. From here on a label may come out,
    /// in part or whole, whatever happens next.
    Rows,
    /// The printer's own figures, while the job waits for its counter.
    Waiting(Progress),
}

/// How far the label in hand had got when a run of labels ended. Ordered, so
/// `reached >= Reached::Rows` asks whether any of it may have printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reached {
    /// The liveness check before the first label.
    Check,
    /// The settings and the job's start: nothing of the label has been sent.
    Settings,
    /// Its rows were being sent.
    Rows,
    /// Its rows were sent and the printer was reporting on it.
    Waiting,
}

/// What a run of labels says as it goes. Labels count from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunNews {
    /// The liveness check is about to be sent.
    Checking,
    /// Label `n`'s job is about to start.
    Starting { n: u16, of: u16 },
    /// Label `n`'s rows are about to be sent.
    Sending { n: u16, of: u16 },
    /// The printer's own figures for label `n`, said only when they change.
    Printer {
        n: u16,
        of: u16,
        print_percent: u8,
        feed_percent: u8,
    },
    /// Label `n`'s job ended with its counter reached.
    Printed { n: u16, of: u16 },
}

/// How a run of labels ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// Every label printed.
    Done,
    /// The stop was read before a label started, so that label and every one
    /// after it were never sent.
    Stopped,
    /// Label `label` failed at `reached`. No label after it was sent.
    Failed {
        label: u16,
        reached: Reached,
        error: JobError,
    },
}

/// What a run of labels did: `printed` of `of` labels reached their counter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub printed: u16,
    pub of: u16,
    pub end: RunEnd,
}

/// Print one page as `labels` labels, the editor's way: a liveness check, then
/// one one-copy job per label.
///
/// The check is one heartbeat of `check` with the default timeout. A printer
/// that answers it, or says it does not support it, is alive; anything else
/// ends the run with nothing else sent - no job is started, so there is no
/// `PrintEnd` to send either.
///
/// Each label is then exactly what [`print()`] sends for one page with `opts`
/// and one copy, whatever `opts.copies` says: its own job, its own clock and
/// its own `PrintEnd`. Nothing is sent between labels. `stop` is read before
/// every label, the first one included, and a stop only ever takes effect
/// there, so the label in hand always finishes. No label is sent twice.
#[allow(clippy::too_many_arguments)]
pub async fn print_labels(
    session: &Session,
    flow: PrintFlow,
    page: &EncodedPage,
    opts: &PrintOptions,
    labels: u16,
    check: payload::HeartbeatKind,
    stop: &AtomicBool,
    mut on_news: impl FnMut(RunNews),
) -> RunReport {
    let of = labels;
    if of == 0 {
        return RunReport {
            printed: 0,
            of,
            end: RunEnd::Done,
        };
    }
    let one = PrintOptions { copies: 1, ..*opts };

    on_news(RunNews::Checking);
    match session
        .request(payload::heartbeat(check), one.timeouts.default)
        .await
    {
        Ok(_) => {}
        // The D110 answers every heartbeat this way, and prints.
        Err(e) if e.is_unsupported() => {}
        Err(e) => {
            return RunReport {
                printed: 0,
                of,
                end: RunEnd::Failed {
                    label: 1,
                    reached: Reached::Check,
                    error: e.into(),
                },
            };
        }
    }

    let mut printed = 0u16;
    for n in 1..=of {
        if stop.load(Ordering::SeqCst) {
            return RunReport {
                printed,
                of,
                end: RunEnd::Stopped,
            };
        }
        on_news(RunNews::Starting { n, of });
        let mut reached = Reached::Settings;
        let mut figures = None;
        let result = one_job(
            session,
            flow,
            std::slice::from_ref(page),
            &one,
            Instant::now(),
            &mut |stage: Stage| match stage {
                Stage::Rows => {
                    reached = Reached::Rows;
                    on_news(RunNews::Sending { n, of });
                }
                Stage::Waiting(p) => {
                    reached = Reached::Waiting;
                    let now = (p.print_percent, p.feed_percent);
                    if figures != Some(now) {
                        figures = Some(now);
                        on_news(RunNews::Printer {
                            n,
                            of,
                            print_percent: p.print_percent,
                            feed_percent: p.feed_percent,
                        });
                    }
                }
            },
        )
        .await;
        match result {
            Ok(()) => {
                printed += 1;
                on_news(RunNews::Printed { n, of });
            }
            Err(error) => {
                return RunReport {
                    printed,
                    of,
                    end: RunEnd::Failed {
                        label: n,
                        reached,
                        error,
                    },
                };
            }
        }
    }
    RunReport {
        printed,
        of,
        end: RunEnd::Done,
    }
}

/// Print one or more pages.
///
/// `flow` comes from [`relabel_proto::select_flow`], which needs the model id
/// and the protocol level negotiated at connect time.
pub async fn print(
    session: &Session,
    flow: PrintFlow,
    pages: &[EncodedPage],
    opts: &PrintOptions,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), JobError> {
    if pages.is_empty() {
        return Ok(());
    }
    let started = Instant::now();

    // Some printers print exactly one page whatever the copy count says, then
    // refuse every command that follows. For those the copies become that many
    // complete jobs instead - which is slower, and is the only thing that works.
    let per_job = opts.copies_per_job.unwrap_or(u16::MAX).max(1);
    if opts.copies > per_job {
        let mut done = 0u16;
        let grand_total = pages.len() as u16 * opts.copies;
        while done < opts.copies {
            let batch = per_job.min(opts.copies - done);
            let batch_opts = PrintOptions {
                copies: batch,
                ..*opts
            };
            let base = done * pages.len() as u16;
            // Its own clock: one deadline across every job of a split run
            // would fail the later ones for the time the earlier ones took.
            one_job(
                session,
                flow,
                pages,
                &batch_opts,
                Instant::now(),
                &mut |stage: Stage| {
                    if let Stage::Waiting(p) = stage {
                        on_progress(Progress {
                            page: base + p.page,
                            total: grand_total,
                            ..p
                        })
                    }
                },
            )
            .await?;
            done += batch;
        }
        return Ok(());
    }

    one_job(session, flow, pages, opts, started, &mut |stage: Stage| {
        if let Stage::Waiting(p) = stage {
            on_progress(p)
        }
    })
    .await
}

/// Send a step that is answered accepted-or-refused, sending it again while
/// the answer is a refusal.
///
/// `0x00` does not only mean "no": the firmware also answers it for "not yet",
/// while the previous label is still feeding out. Ignoring that byte is what
/// made the second label of a run silently never print on a D110_M - the
/// settings and `PrintStart` were all refused, a whole page was streamed into
/// a printer with no job open, and the job then waited out its deadline for a
/// page counter that could never move. The vendor's own sender resends on the
/// same byte, ten times, which is where the default comes from.
///
/// Only a bare `0x00` counts. A longer reply belongs to some other flow's
/// answer for this command and is passed back untouched, so a printer this has
/// never run against cannot be blocked by a byte that means something else to
/// it.
async fn accepted_step(
    session: &Session,
    frame: relabel_proto::Frame,
    wait: Duration,
    step: &'static str,
    t: &Timeouts,
) -> Result<relabel_proto::Frame, JobError> {
    let attempts = t.refusals.max(1);
    for attempt in 1..=attempts {
        let reply = session.request(frame.clone(), wait).await?;
        if reply.data != [0x00] {
            return Ok(reply);
        }
        if attempt < attempts {
            tokio::time::sleep(t.refusal_gap).await;
        }
    }
    Err(JobError::Refused { step })
}

/// One complete start-to-`PrintEnd` cycle.
async fn one_job(
    session: &Session,
    flow: PrintFlow,
    pages: &[EncodedPage],
    opts: &PrintOptions,
    started: Instant,
    on_stage: &mut impl FnMut(Stage),
) -> Result<(), JobError> {
    let total: u16 = pages
        .iter()
        .map(|_| opts.copies)
        .fold(0u16, |a, b| a.saturating_add(b));

    let result = run(session, flow, pages, opts, total, started, on_stage).await;

    // Always, on every path. See the module note.
    let _ = session
        .request(payload::print_end(), opts.timeouts.print_end)
        .await;
    after_print_end(session, flow).await;

    result
}

async fn run(
    session: &Session,
    flow: PrintFlow,
    pages: &[EncodedPage],
    opts: &PrintOptions,
    total: u16,
    started: Instant,
    on_stage: &mut impl FnMut(Stage),
) -> Result<(), JobError> {
    let t = &opts.timeouts;

    // Settings first, in the order this flow wants them. A setting the printer
    // answers with 0x00 (not supported) is skipped; a 0xDB fault report fails
    // the job before any row is sent, and PrintEnd still follows. (A bare 0x00
    // byte in the setting's own reply is a refusal, sent again by
    // `accepted_step`.)
    let density = payload::set_density(opts.density);
    let label_type = payload::set_label_type(opts.label_type);
    let ordered = if flow.label_type_before_density() {
        [(label_type, "SetLabelType"), (density, "SetDensity")]
    } else {
        [(density, "SetDensity"), (label_type, "SetLabelType")]
    };
    for (f, step) in ordered {
        match accepted_step(session, f, t.settings, step, t).await {
            Ok(_) => {}
            Err(JobError::Session(e)) if e.is_unsupported() => {}
            Err(e) => return Err(e),
        }
    }

    let start = match flow {
        PrintFlow::Base | PrintFlow::Mid => payload::print_start::simple(),
        PrintFlow::High => payload::print_start::with_pages(total),
        PrintFlow::V4 => payload::print_start::b1(total, opts.color),
        PrintFlow::V5 => payload::print_start::v4(total, opts.color, 0, 0),
    };
    accepted_step(session, start, t.print_start, "PrintStart", t).await?;

    // This flow's printers eat the first packet after PrintStart. Give them
    // something disposable so the one that gets eaten is not a real command.
    if flow.needs_sacrificial_sends() {
        let _ = session.send_oneway(payload::print_status()).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    let budget = budget(t, pages.len());
    for page in pages {
        check_deadline(started, budget)?;

        if flow.uses_print_clear() {
            let r = session.request(payload::print_clear(), t.default).await?;
            refused(&r, "PrintClear")?;
        }

        if flow.uses_page_start() {
            let r = session.request(payload::page_start(), t.page_start).await?;
            refused(&r, "PageStart")?;
        }

        let geometry = match flow {
            PrintFlow::Base | PrintFlow::Mid => {
                payload::set_page_size::rows_cols(page.rows, page.cols)
            }
            PrintFlow::High | PrintFlow::V4 => {
                payload::set_page_size::rows_cols_copies(page.rows, page.cols, opts.copies)
            }
            PrintFlow::V5 => payload::set_page_size::v4(page.rows, page.cols, opts.copies, true, 0),
        };
        session.request(geometry, t.default).await?;

        if flow.copies_in_print_quantity() {
            session
                .request(payload::print_quantity(opts.copies), t.default)
                .await?;
        }

        on_stage(Stage::Rows);
        session.send_bulk(&page.frames).await?;

        let r = session.request(payload::page_end(), t.page_end).await?;
        refused(&r, "PageEnd")?;
    }

    wait_for_pages(session, total, started, budget, opts, on_stage).await
}

/// The second discarded packet, sent after `PrintEnd`.
///
/// Separate from [`run`] because `PrintEnd` is issued by the caller on every
/// path, including the failure one, and this has to follow it.
pub(crate) async fn after_print_end(session: &Session, flow: PrintFlow) {
    if flow.needs_sacrificial_sends() {
        let _ = session
            .send_oneway(payload::heartbeat(
                relabel_proto::payload::HeartbeatKind::Advanced2,
            ))
            .await;
    }
}

/// What a job is allowed, in total: the per-page ceiling times the pages it
/// carries. One ceiling for a whole job would fail a long run part way
/// through, and the run is the thing worth protecting.
fn budget(t: &Timeouts, pages: usize) -> Duration {
    t.job
        .saturating_mul(u32::try_from(pages.max(1)).unwrap_or(u32::MAX))
}

/// Wait until the printer's page counter reaches `total`.
///
/// Two sources agree on this number and either is enough: the counter the
/// printer volunteers, and the counter in a polled status reply. Listening for
/// both means a flow that only volunteers it and a flow that only answers polls
/// are handled by the same code.
async fn wait_for_pages(
    session: &Session,
    total: u16,
    started: Instant,
    budget: Duration,
    opts: &PrintOptions,
    on_stage: &mut impl FnMut(Stage),
) -> Result<(), JobError> {
    let t = &opts.timeouts;
    let mut events = session.events();
    let mut last_page = 0u16;

    loop {
        check_deadline(started, budget)?;

        // Anything the printer volunteered since the last poll.
        loop {
            match timeout(Duration::from_millis(0), events.recv()).await {
                Ok(Ok(PrinterEvent::PageIndex(p))) => {
                    last_page = last_page.max(p);
                    on_stage(Stage::Waiting(Progress {
                        page: last_page,
                        total,
                        print_percent: 0,
                        feed_percent: 0,
                    }));
                }
                Ok(Ok(PrinterEvent::Error(code))) => {
                    return Err(crate::SessionError::Printer(code).into());
                }
                Ok(Ok(_)) => continue,
                // A slow consumer that lagged has missed events, not failed.
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => break,
                Err(_) => break,
            }
        }
        if last_page >= total {
            return Ok(());
        }

        match session.request(payload::print_status(), t.status).await {
            Ok(f) => {
                let st = PrintStatus::parse(&f.data).map_err(crate::SessionError::from)?;
                if let Some(code) = st.error {
                    return Err(crate::SessionError::Printer(code).into());
                }
                last_page = last_page.max(st.page);
                on_stage(Stage::Waiting(Progress {
                    page: last_page,
                    total,
                    print_percent: st.print_progress,
                    feed_percent: st.feed_progress,
                }));
                // Deliberately `>=` and not `==`: a printer that overshoots or
                // reports a page we did not count must still end the job.
                if last_page >= total {
                    return Ok(());
                }
            }
            // Some flows never answer a status poll; the volunteered counter is
            // the only signal, so silence here is not fatal.
            Err(crate::SessionError::Timeout { .. }) => {}
            Err(e) if e.is_unsupported() => {}
            Err(e) => return Err(e.into()),
        }

        tokio::time::sleep(t.poll_interval).await;
    }
}

fn refused(reply: &relabel_proto::Frame, step: &'static str) -> Result<(), JobError> {
    if relabel_proto::parse::accepted(&reply.data) {
        Ok(())
    } else {
        Err(JobError::Refused { step })
    }
}

fn check_deadline(started: Instant, job: Duration) -> Result<(), JobError> {
    if started.elapsed() >= job {
        Err(JobError::DeadlineExceeded {
            ms: job.as_millis() as u64,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_proto::models;

    fn identity(model_id: Option<u16>) -> Identity {
        let candidates: Vec<_> = model_id
            .map(models::by_wire_id)
            .into_iter()
            .flatten()
            .collect();
        Identity {
            model_id,
            model: candidates.first().copied(),
            candidates,
            ..Identity::default()
        }
    }

    /// relabel-cli's `label.rs`, spelled out as it is there.
    fn cli_formula(id: &Identity, density: Option<u8>) -> PrintOptions {
        let model = id.model;
        PrintOptions {
            density: density.or(model.map(|m| m.density.2)).unwrap_or(2),
            label_type: model
                .and_then(|m| m.label_types.first().copied())
                .unwrap_or(LabelType::Gap),
            copies: 1,
            ..PrintOptions::default()
        }
        .for_model(id.model_id)
    }

    fn fields(o: &PrintOptions) -> (u8, LabelType, u16, PageColor, Option<u16>, Duration) {
        (
            o.density,
            o.label_type,
            o.copies,
            o.color,
            o.copies_per_job,
            o.timeouts.job,
        )
    }

    #[test]
    fn for_identity_is_the_cli_formula() {
        let d110_m = PrintOptions::for_identity(&identity(Some(2320)), None);
        assert_eq!(d110_m.density, 3);
        assert_eq!(d110_m.label_type, LabelType::Gap);
        assert_eq!(d110_m.copies, 1);
        assert_eq!(d110_m.copies_per_job, None);

        let d110 = PrintOptions::for_identity(&identity(Some(2304)), None);
        assert_eq!(d110.density, 2);
        assert_eq!(d110.copies_per_job, Some(1));

        let nothing = PrintOptions::for_identity(&identity(None), None);
        assert_eq!(nothing.density, 2);
        assert_eq!(nothing.label_type, LabelType::Gap);

        let chosen = PrintOptions::for_identity(&identity(Some(2320)), Some(5));
        assert_eq!(chosen.density, 5);

        for model_id in [Some(2320), Some(2304), Some(2305), Some(9999), None] {
            for density in [None, Some(1), Some(5)] {
                let id = identity(model_id);
                assert_eq!(
                    fields(&PrintOptions::for_identity(&id, density)),
                    fields(&cli_formula(&id, density)),
                    "model {model_id:?}, density {density:?}"
                );
            }
        }
    }
}
