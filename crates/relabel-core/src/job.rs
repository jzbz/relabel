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

use crate::error::JobError;
use crate::session::{PrinterEvent, Session};
use relabel_proto::{
    EncodedPage, LabelType, PrintFlow,
    parse::PrintStatus,
    payload::{self, PageColor},
};
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
    /// Overall ceiling for one job.
    pub job: Duration,
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
    pub timeouts: Timeouts,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            density: 2,
            label_type: LabelType::Gap,
            copies: 1,
            color: PageColor::SingleColor,
            timeouts: Timeouts::default(),
        }
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
    let total: u16 = pages
        .iter()
        .map(|_| opts.copies)
        .fold(0u16, |a, b| a.saturating_add(b));

    let result = run(session, flow, pages, opts, total, started, &mut on_progress).await;

    // Always, on every path. See the module note.
    let _ = session
        .request(payload::print_end(), opts.timeouts.print_end)
        .await;

    result
}

async fn run(
    session: &Session,
    flow: PrintFlow,
    pages: &[EncodedPage],
    opts: &PrintOptions,
    total: u16,
    started: Instant,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), JobError> {
    let t = &opts.timeouts;

    // Settings first. An unsupported value comes back as a fault report rather
    // than a refusal, and it is not fatal to the job.
    match session
        .request(payload::set_density(opts.density), t.settings)
        .await
    {
        Ok(_) => {}
        Err(e) if e.is_unsupported() => {}
        Err(e) => return Err(e.into()),
    }
    match session
        .request(payload::set_label_type(opts.label_type), t.settings)
        .await
    {
        Ok(_) => {}
        Err(e) if e.is_unsupported() => {}
        Err(e) => return Err(e.into()),
    }

    let start = match flow {
        PrintFlow::Base | PrintFlow::Mid => payload::print_start::simple(),
        PrintFlow::High => payload::print_start::with_pages(total),
        PrintFlow::V4 => payload::print_start::b1(total, opts.color),
        PrintFlow::V5 => payload::print_start::v4(total, opts.color, 0, 0),
    };
    session.request(start, t.print_start).await?;

    for page in pages {
        check_deadline(started, t.job)?;

        if flow.uses_print_clear() {
            let r = session.request(payload::print_clear(), t.default).await?;
            refused(&r, "PrintClear")?;
        }

        let r = session.request(payload::page_start(), t.page_start).await?;
        refused(&r, "PageStart")?;

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

        session.send_bulk(&page.frames).await?;

        let r = session.request(payload::page_end(), t.page_end).await?;
        refused(&r, "PageEnd")?;
    }

    wait_for_pages(session, total, started, opts, on_progress).await
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
    opts: &PrintOptions,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), JobError> {
    let t = &opts.timeouts;
    let mut events = session.events();
    let mut last_page = 0u16;

    loop {
        check_deadline(started, t.job)?;

        // Anything the printer volunteered since the last poll.
        loop {
            match timeout(Duration::from_millis(0), events.recv()).await {
                Ok(Ok(PrinterEvent::PageIndex(p))) => {
                    last_page = last_page.max(p);
                    on_progress(Progress {
                        page: last_page,
                        total,
                        print_percent: 0,
                        feed_percent: 0,
                    });
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
                on_progress(Progress {
                    page: last_page,
                    total,
                    print_percent: st.print_progress,
                    feed_percent: st.feed_progress,
                });
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
