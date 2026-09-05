//! Which print flow a printer wants.
//!
//! The flows differ in two payload widths - `PrintStart` and `SetPageSize` -
//! and picking wrong does not produce an error. It produces a printer that
//! acknowledges everything and prints nothing, or a page rotated ninety
//! degrees, which is why this is a table rather than a guess.
//!
//! The selection rule and the widths are read from the vendor SDK; see
//! `docs/PROTOCOL-NOTES.md` for the provenance and for what it corrected. The
//! short version is that the flow is chosen from **the model family and the
//! negotiated protocol level together**, never from the model alone.

use crate::models::ModelSpec;

/// A set of payload widths and a job shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintFlow {
    /// One-byte `PrintStart`, four-byte `SetPageSize`. Sends `PrintClear`
    /// before each page and carries the copy count in `PrintQuantity`.
    Base,
    /// The same two widths as [`Base`](Self::Base), reached at protocol level 1
    /// on the D11 family. Kept distinct because it is a distinct implementation
    /// in the vendor SDK and is expected to diverge in how completion is
    /// detected, not in what it sends.
    Mid,
    /// Two-byte `PrintStart` (total pages), six-byte `SetPageSize`
    /// (rows, cols, copies).
    High,
    /// Seven-byte `PrintStart` (pages, four reserved zeros, page colour), and
    /// the same six-byte `SetPageSize`.
    V4,
    /// Nine-byte `PrintStart` (the V4 form plus speed and a flag) and the
    /// thirteen-byte `SetPageSize`.
    V5,
}

impl PrintFlow {
    /// The `PrintStart` payload width in bytes.
    pub fn print_start_len(self) -> usize {
        match self {
            PrintFlow::Base | PrintFlow::Mid => 1,
            PrintFlow::High => 2,
            PrintFlow::V4 => 7,
            PrintFlow::V5 => 9,
        }
    }

    /// The `SetPageSize` payload width in bytes.
    pub fn set_page_size_len(self) -> usize {
        match self {
            PrintFlow::Base | PrintFlow::Mid => 4,
            PrintFlow::High | PrintFlow::V4 => 6,
            PrintFlow::V5 => 13,
        }
    }

    /// Whether the flow issues `PrintClear` before each page.
    pub fn uses_print_clear(self) -> bool {
        matches!(self, PrintFlow::Base | PrintFlow::Mid)
    }

    /// Whether the copy count rides in `PrintQuantity` rather than in
    /// `SetPageSize`.
    pub fn copies_in_print_quantity(self) -> bool {
        matches!(self, PrintFlow::Base | PrintFlow::Mid)
    }
}

/// Model ids whose factory offers the three-level D11-family ladder.
///
/// Read from the vendor's own model-id switch. **None of these ever reaches the
/// seven- or nine-byte `PrintStart`** - which is the correction that matters,
/// because the most widely used open-source library maps D101 onto the
/// seven-byte flow and that mapping is wrong.
pub const D11_FAMILY: &[u16] = &[
    512,  // D11
    514,  // D11S
    1536, // shares the D11 factory; absent from the published catalogue
    2304, // D110
    2560, // D101
];

/// Model ids whose factory switches to [`PrintFlow::V4`] at protocol level 3.
pub const B1_FAMILY: &[u16] = &[
    4096, // B1
];

/// Models that silently print only one page however many copies are asked for.
///
/// The failure is quiet and expensive: the printer acknowledges the request,
/// prints once, parks its counter at one page and a hundred percent, refuses the
/// next `PageStart`, and then rejects everything that follows. A client that
/// merely waits sees nothing but a timeout.
///
/// The fix is to send the copies as that many complete jobs.
pub const ONE_PAGE_PER_JOB: &[u16] = &[
    2304, // D110
    3586, // N1
];

/// How many copies this model will honour in a single job.
///
/// `None` means no known limit.
pub fn copies_per_job(model_id: Option<u16>) -> Option<u16> {
    match model_id {
        Some(id) if ONE_PAGE_PER_JOB.contains(&id) => Some(1),
        _ => None,
    }
}

/// Pick the flow for a printer.
///
/// `protocol` is the level negotiated at connect time: the `Connect` reply gives
/// 0, 1 or "3 or more", and the `PrinterStatusData` read resolves the last of
/// those into 3, 4 or 5.
///
/// The fallback for an unrecognised model is deliberately not "the newest
/// thing": [`PrintFlow::V4`] is what people are told to try first and it is
/// reported working on a wide spread of hardware, so it is the default below
/// protocol level 5.
pub fn select_flow(model_id: Option<u16>, protocol: u8) -> PrintFlow {
    match model_id {
        Some(id) if D11_FAMILY.contains(&id) => match protocol {
            1 => PrintFlow::Mid,
            2 => PrintFlow::High,
            _ => PrintFlow::Base,
        },
        Some(id) if B1_FAMILY.contains(&id) => {
            if protocol >= 3 {
                PrintFlow::V4
            } else {
                PrintFlow::High
            }
        }
        _ => {
            if protocol >= 5 {
                PrintFlow::V5
            } else {
                PrintFlow::V4
            }
        }
    }
}

/// The flow for a model this crate knows, by spec rather than by raw id.
pub fn select_for(model: &ModelSpec, protocol: u8) -> PrintFlow {
    select_flow(model.wire_ids.first().copied(), protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::lookup;

    /// The correction. Every one of these is a target model, and the widely
    /// copied mapping puts D101 on the seven-byte flow.
    #[test]
    fn the_d11_family_never_reaches_the_seven_byte_print_start() {
        for id in D11_FAMILY {
            for protocol in 0..=5u8 {
                let flow = select_flow(Some(*id), protocol);
                assert!(
                    flow.print_start_len() <= 2,
                    "model {id} at protocol {protocol} chose {flow:?}"
                );
            }
        }
    }

    #[test]
    fn d11_family_ladder() {
        assert_eq!(select_flow(Some(2560), 0), PrintFlow::Base, "D101 level 0");
        assert_eq!(select_flow(Some(2560), 1), PrintFlow::Mid, "D101 level 1");
        assert_eq!(select_flow(Some(2560), 2), PrintFlow::High, "D101 level 2");
        assert_eq!(select_flow(Some(512), 0), PrintFlow::Base, "D11");
        assert_eq!(select_flow(Some(2304), 2), PrintFlow::High, "D110");
    }

    /// B1 below protocol 3 uses the two-byte form, not the seven-byte one.
    #[test]
    fn b1_only_reaches_v4_at_protocol_three() {
        assert_eq!(select_flow(Some(4096), 0), PrintFlow::High);
        assert_eq!(select_flow(Some(4096), 2), PrintFlow::High);
        assert_eq!(select_flow(Some(4096), 3), PrintFlow::V4);
        assert_eq!(select_flow(Some(4096), 5), PrintFlow::V4);
    }

    #[test]
    fn unknown_models_fall_back_to_the_widely_working_flow() {
        assert_eq!(select_flow(None, 0), PrintFlow::V4);
        assert_eq!(select_flow(Some(0xFFFF), 3), PrintFlow::V4);
        assert_eq!(select_flow(None, 5), PrintFlow::V5);
    }

    #[test]
    fn widths_match_the_payload_builders() {
        use crate::payload::{PageColor, print_start, set_page_size};
        assert_eq!(
            PrintFlow::Base.print_start_len(),
            print_start::simple().data.len()
        );
        assert_eq!(
            PrintFlow::High.print_start_len(),
            print_start::with_pages(1).data.len()
        );
        assert_eq!(
            PrintFlow::V4.print_start_len(),
            print_start::b1(1, PageColor::SingleColor).data.len()
        );
        assert_eq!(
            PrintFlow::V5.print_start_len(),
            print_start::v4(1, PageColor::SingleColor, 0, 0).data.len()
        );
        assert_eq!(
            PrintFlow::Base.set_page_size_len(),
            set_page_size::rows_cols(1, 1).data.len()
        );
        assert_eq!(
            PrintFlow::High.set_page_size_len(),
            set_page_size::rows_cols_copies(1, 1, 1).data.len()
        );
        assert_eq!(
            PrintFlow::V5.set_page_size_len(),
            set_page_size::v4(1, 1, 1, true, 0).data.len()
        );
    }

    #[test]
    fn the_d110_is_known_to_cap_a_job_at_one_page() {
        assert_eq!(copies_per_job(Some(2304)), Some(1), "D110");
        assert_eq!(copies_per_job(Some(3586)), Some(1), "N1");
        assert_eq!(copies_per_job(Some(512)), None, "D11 has no known cap");
        assert_eq!(copies_per_job(None), None);
    }

    #[test]
    fn the_registry_and_the_family_lists_agree() {
        for id in D11_FAMILY {
            if let Some(m) = lookup(*id) {
                assert!(
                    m.vendor.speaks_niimbot(),
                    "{} should be a NIIMBOT-protocol model",
                    m.name
                );
            }
        }
        assert_eq!(lookup(2560).map(|m| m.name), Some("D101"));
        assert_eq!(lookup(4096).map(|m| m.name), Some("B1"));
    }
}
