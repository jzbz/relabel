//! Errors, and the printer's own error codes.

use core::fmt;

/// Something went wrong encoding or decoding, entirely locally.
///
/// Distinct from [`PrinterErrorCode`], which is the printer telling us it will
/// not do something. A `ProtoError` is always our fault or a corrupt stream; a
/// `PrinterErrorCode` is a fact about the hardware.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    /// A payload longer than 255 bytes was offered to the encoder.
    ///
    /// The length field is a single byte, so this cannot be represented. The
    /// JavaScript implementations write `arr[pos] = data.length` into a
    /// `Uint8Array`, which wraps silently mod 256 and emits a frame the printer
    /// will mis-parse; refusing is the only honest option. Nothing legitimate
    /// hits this: the widest printhead in the catalogue is ET10 at 1600 px,
    /// giving a 200-byte row plus a 6-byte header.
    #[error("payload of {0} bytes exceeds the 255-byte frame limit")]
    PayloadTooLong(usize),

    /// The frame's trailing XOR did not match the bytes before it.
    #[error("checksum mismatch: frame carries {found:#04x}, computed {computed:#04x}")]
    BadChecksum { found: u8, computed: u8 },

    /// A frame did not start with `55 55` where one was expected.
    #[error("expected frame head 55 55, found {0:02x?}")]
    BadHead([u8; 2]),

    /// A frame did not end with `AA AA` where its length said it would.
    ///
    /// This is the signature of a desynchronised stream rather than a corrupt
    /// frame: the length byte was read out of the middle of something else.
    #[error("expected frame tail AA AA, found {0:02x?}")]
    BadTail([u8; 2]),

    /// Fewer bytes than the frame's own length field promises.
    ///
    /// Not an error in a streaming context - the accumulator treats it as
    /// "wait for more" - but it is one when decoding a slice that was supposed
    /// to hold a whole frame.
    #[error("truncated frame: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },

    /// A parser ran off the end of a payload.
    ///
    /// Every parser in this crate returns `Result` rather than indexing, because
    /// every byte it reads came off a wire from a device we do not control. This
    /// is the variant that would otherwise have been a panic.
    #[error("payload too short for {what}: need {need} bytes, have {have}")]
    ShortPayload {
        what: &'static str,
        need: usize,
        have: usize,
    },

    /// The page is wider than the print head can address.
    ///
    /// The vendor's client clamps instead, and the printer drops the columns it
    /// cannot reach - but a truncated label that reports success is worse than a
    /// refusal, and the caller always knows the head width.
    #[error("page is {cols} dots wide but the print head is only {printhead_px}")]
    PageTooWide { cols: u16, printhead_px: u16 },

    /// A field held a value outside the range its type allows.
    #[error("invalid {what}: {value:#04x}")]
    InvalidValue { what: &'static str, value: u8 },
}

/// The one-byte code carried by [`ResponseId::PrintError`](crate::ResponseId::PrintError).
///
/// The printer can send this in place of *any* expected response, so the session
/// layer short-circuits a pending request when one arrives.
///
/// [`Self::DataError`] deserves its own note: in practice it does not mean the
/// packet was malformed. It means "refused, cannot service this now", and it
/// typically lands on whatever command arrives after the printer considers the
/// job over. Surfacing it by its literal name sends people hunting for an
/// encoder bug that is not there, so [`Self::describe`] words it differently.
/// Generates the enum alongside both directions of its byte mapping.
///
/// Written as a macro because the `Unknown(u8)` variant makes the enum
/// data-carrying, and a data-carrying enum cannot be cast to its discriminant
/// with `as` however it is `repr`-ed. Two hand-written 38-arm matches that have
/// to stay in agreement is exactly the sort of thing that silently rots.
macro_rules! printer_error_codes {
    ($($name:ident = $val:expr;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum PrinterErrorCode {
            $($name,)*
            /// A code the printer sent that this table does not know.
            ///
            /// Kept rather than rejected: an unrecognised error is still an
            /// error, and showing the user the raw byte is more useful than a
            /// parse failure stacked on top of whatever actually went wrong.
            Unknown(u8),
        }

        impl PrinterErrorCode {
            pub fn from_byte(b: u8) -> Self {
                match b {
                    $($val => PrinterErrorCode::$name,)*
                    other => PrinterErrorCode::Unknown(other),
                }
            }

            pub fn as_byte(self) -> u8 {
                match self {
                    $(PrinterErrorCode::$name => $val,)*
                    PrinterErrorCode::Unknown(b) => b,
                }
            }
        }
    };
}

printer_error_codes! {
    CoverOpen = 0x01;
    LackPaper = 0x02;
    LowBattery = 0x03;
    BatteryException = 0x04;
    UserCancel = 0x05;
    DataError = 0x06;
    Overheat = 0x07;
    PaperOutException = 0x08;
    PrinterBusy = 0x09;
    NoPrinterHead = 0x0a;
    TemperatureLow = 0x0b;
    PrinterHeadLoose = 0x0c;
    NoRibbon = 0x0d;
    WrongRibbon = 0x0e;
    UsedRibbon = 0x0f;
    WrongPaper = 0x10;
    SetPaperFail = 0x11;
    SetPrintModeFail = 0x12;
    SetPrintDensityFail = 0x13;
    WriteRfidFail = 0x14;
    SetMarginFail = 0x15;
    CommunicationException = 0x16;
    Disconnect = 0x17;
    CanvasParameterError = 0x18;
    RotationParameterException = 0x19;
    JsonParameterException = 0x1a;
    B3sAbnormalPaperOutput = 0x1b;
    ECheckPaper = 0x1c;
    RfidTagNotWritten = 0x1d;
    SetPrintDensityNoSupport = 0x1e;
    SetPrintModeNoSupport = 0x1f;
    SetPrintLabelMaterialError = 0x20;
    SetPrintLabelMaterialNoSupport = 0x21;
    NotSupportWrittenRfid = 0x22;
    IllegalPage = 0x32;
    IllegalRibbonPage = 0x33;
    ReceiveDataTimeout = 0x34;
    NonDedicatedRibbon = 0x35;
}

impl PrinterErrorCode {
    /// Whether this ends the job, or is advisory.
    ///
    /// Only the "no support" family is recoverable in the sense that the job can
    /// continue with a different value; everything else stops the print.
    pub fn is_fatal(self) -> bool {
        !matches!(
            self,
            PrinterErrorCode::SetPrintDensityNoSupport
                | PrinterErrorCode::SetPrintModeNoSupport
                | PrinterErrorCode::SetPrintLabelMaterialNoSupport
                | PrinterErrorCode::NotSupportWrittenRfid
        )
    }

    /// What to tell the user, and what they can do about it.
    ///
    /// Returned as a pair so the GUI can show the cause and the remedy in
    /// different weights, and so the CLI can print only the first half.
    pub fn describe(self) -> (&'static str, &'static str) {
        use PrinterErrorCode::*;
        match self {
            CoverOpen => ("The printer's cover is open.", "Close it and print again."),
            LackPaper | PaperOutException => (
                "The printer is out of labels.",
                "Load a roll, then print again.",
            ),
            LowBattery => (
                "The printer's battery is too low to print.",
                "Charge it, or run it from USB power.",
            ),
            BatteryException => (
                "The printer reported a battery fault.",
                "Charge it from a different supply; if it persists the cell may be failing.",
            ),
            UserCancel => ("The print was cancelled.", ""),
            // Deliberately not "data error". See the type-level note.
            DataError => (
                "The printer refused the command.",
                "It may consider the job already finished. Check the protocol log.",
            ),
            Overheat => (
                "The print head is too hot.",
                "Wait a minute for it to cool, then print again.",
            ),
            TemperatureLow => (
                "The printer is too cold to print.",
                "Let it warm up to room temperature.",
            ),
            PrinterBusy => (
                "The printer is busy with another job.",
                "Wait for it to finish, or power-cycle it if nothing is printing.",
            ),
            NoPrinterHead | PrinterHeadLoose => (
                "The print head is not seated.",
                "Open the cover, reseat the head, and close it firmly.",
            ),
            NoRibbon | WrongRibbon | UsedRibbon | NonDedicatedRibbon => (
                "There is a problem with the ribbon.",
                "Fit a fresh, compatible ribbon cartridge.",
            ),
            WrongPaper | ECheckPaper => (
                "The loaded labels are not what the printer expected.",
                "Check the label type setting matches the roll.",
            ),
            SetPaperFail
            | SetPrintModeFail
            | SetPrintDensityFail
            | SetMarginFail
            | SetPrintLabelMaterialError => (
                "The printer rejected a settings change.",
                "The value may be out of range for this model.",
            ),
            SetPrintDensityNoSupport => (
                "This printer does not support that print density.",
                "Choose a value inside the model's own range.",
            ),
            SetPrintModeNoSupport => ("This printer does not support that print mode.", ""),
            SetPrintLabelMaterialNoSupport => {
                ("This printer does not support that label material.", "")
            }
            WriteRfidFail | RfidTagNotWritten | NotSupportWrittenRfid => (
                "The printer could not write the label tag.",
                "relabel never writes tags, so this is unexpected - please report it.",
            ),
            CommunicationException | Disconnect => {
                ("The link to the printer broke.", "Reconnect and try again.")
            }
            ReceiveDataTimeout => (
                "The printer timed out waiting for data.",
                "This usually means writes are being sent too fast; try a longer pace.",
            ),
            CanvasParameterError | RotationParameterException | JsonParameterException => (
                "The printer rejected the page geometry.",
                "The label may be wider than the print head.",
            ),
            B3sAbnormalPaperOutput => (
                "The paper did not feed correctly.",
                "Check for a jam and reload the roll.",
            ),
            IllegalPage | IllegalRibbonPage => ("The printer rejected the page count.", ""),
            Unknown(b) => {
                // A &'static str cannot carry the byte, and this type is Copy
                // and allocation-free by design, so the code goes in the second
                // half where the caller can append it. Callers that want the
                // number in the headline use `as_byte`.
                let _ = b;
                (
                    "The printer reported an error.",
                    "See the protocol log for the raw code.",
                )
            }
        }
    }
}

impl fmt::Display for PrinterErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, _) = self.describe();
        match self {
            PrinterErrorCode::Unknown(b) => write!(f, "{what} (code {b:#04x})"),
            _ => f.write_str(what),
        }
    }
}
