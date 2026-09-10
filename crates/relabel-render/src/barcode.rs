//! Linear barcodes: which symbologies, how their data is checked, and how long
//! they come out.
//!
//! This module turns a symbology and a string into a run of modules - bars in
//! units of the narrowest element, quiet zones included - and nothing more.
//! Placing them is the rasteriser's job, and it draws every bar as an exact
//! integer multiple of the element's module width in dots. A module width that
//! is not a whole number of dots rounds differently from bar to bar, and a
//! scanner cannot recover the narrow-element width from that.
//!
//! # Two encoders, one table, and the gaps in between
//!
//! Code 128 goes through `code128`, which chooses the character-set switches
//! itself. Code 39, ITF and the retail symbologies go through `barcoders`, and
//! two of its behaviours are fenced off here rather than discovered at a till:
//!
//! - **Its UPC-A is a type alias for EAN-13** that wants the twelve EAN digits
//!   in front of the check digit, so a real twelve-digit UPC-A encodes as a
//!   different, perfectly valid, wrong number. UPC-A is turned into EAN-13 here
//!   instead - a leading zero - which also lets the crate's own check-digit
//!   validation run on it.
//! - **Its ITF quietly appends a check digit to odd-length input**, so the value
//!   a scanner reads is not the value that was typed. Odd lengths are refused,
//!   with the fix in the message.
//!
//! Codabar is encoded here, from the standard's twenty-character table, because
//! `barcoders` 2.0.0 has three of those twenty wrong. Its B and C patterns are
//! swapped, so a symbol typed with a B stop scans with a C; and its `+` has
//! five wide elements where the standard has three, which zbar will not read at
//! all. It also never checked the A-D start and stop letters a Codabar needs;
//! those are checked here as well.
//!
//! Code 93 is not offered. `barcoders` emits a wrong check character for about
//! one input in twenty-three, and there is no independent decoder to verify a
//! patched version against.

use crate::target::Dpmm;
use code128::Code128;
use relabel_proto::PrintDirection;

/// The narrowest module printed without an explicit out-of-spec override.
///
/// Two dots is 0.25 mm at 203 dpi, which is the common 10-mil floor. One dot
/// is below every published minimum, and thermal bar growth at that width is a
/// large fraction of the bar itself.
pub const MIN_MODULE_DOTS: u16 = 2;

/// The starting module width for a new barcode on a printer.
///
/// Three dots where the head lies along the short axis - the whole
/// D11/D110/D101 family. A barcode running the length of the label there has
/// its bars across the paper's travel, so every bar edge is placed by the feed
/// motor stepping rather than by the fixed pitch of the head elements, and that
/// deserves a margin. Two dots elsewhere. Both are inference from the standards
/// and the mechanism, not measurement: the P4 print test in
/// `docs/RENDER-PLAN.md` settles them.
pub fn default_module_dots(direction: PrintDirection) -> u16 {
    match direction {
        PrintDirection::Left => 3,
        PrintDirection::Top => 2,
    }
}

/// The symbologies relabel prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbology {
    Code128,
    Code39,
    Itf,
    Codabar,
    Ean13,
    Ean8,
    UpcA,
}

impl Symbology {
    pub const ALL: [Symbology; 7] = [
        Symbology::Code128,
        Symbology::Code39,
        Symbology::Itf,
        Symbology::Codabar,
        Symbology::Ean13,
        Symbology::Ean8,
        Symbology::UpcA,
    ];

    /// Look up the name a document stores. `None` for anything else, which is
    /// a normal outcome: a file from a later version may name a symbology this
    /// build does not have.
    pub fn from_name(name: &str) -> Option<Symbology> {
        Symbology::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The name a document stores.
    pub fn name(self) -> &'static str {
        match self {
            Symbology::Code128 => "code128",
            Symbology::Code39 => "code39",
            Symbology::Itf => "itf",
            Symbology::Codabar => "codabar",
            Symbology::Ean13 => "ean13",
            Symbology::Ean8 => "ean8",
            Symbology::UpcA => "upca",
        }
    }

    /// The name a person reads.
    pub fn label(self) -> &'static str {
        match self {
            Symbology::Code128 => "Code 128",
            Symbology::Code39 => "Code 39",
            Symbology::Itf => "ITF",
            Symbology::Codabar => "Codabar",
            Symbology::Ean13 => "EAN-13",
            Symbology::Ean8 => "EAN-8",
            Symbology::UpcA => "UPC-A",
        }
    }

    /// Quiet zone before and after the symbol, in modules. Zero for Code 128,
    /// whose encoder already includes both.
    fn quiet(self) -> (u32, u32) {
        match self {
            Symbology::Code128 => (0, 0),
            Symbology::Code39 | Symbology::Itf | Symbology::Codabar => (10, 10),
            Symbology::Ean13 => (11, 7),
            Symbology::UpcA => (9, 9),
            Symbology::Ean8 => (7, 7),
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Symbology::Code128 => "at least one Latin-1 character",
            Symbology::Code39 => "1 to 255 characters",
            Symbology::Itf => "an even number of digits",
            Symbology::Codabar => "a start and a stop letter around its data",
            Symbology::Ean13 => "12 digits, or 13 including the check digit",
            Symbology::Ean8 => "7 digits, or 8 including the check digit",
            Symbology::UpcA => "11 digits, or 12 including the check digit",
        }
    }

    /// Nominal module width and bar height in micrometres, for the retail
    /// symbologies whose specification fixes both.
    fn retail(self) -> Option<(u64, u64)> {
        match self {
            Symbology::Ean13 | Symbology::UpcA => Some((330, 22_850)),
            Symbology::Ean8 => Some((330, 18_230)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BarcodeError {
    #[error("unknown symbology {0:?}")]
    UnknownSymbology(String),
    #[error("a barcode needs some data")]
    Empty,
    #[error("{sym} cannot encode {what}")]
    Character { sym: &'static str, what: String },
    #[error("{sym} needs {expected}, got {got} characters")]
    Length {
        sym: &'static str,
        expected: &'static str,
        got: usize,
    },
    #[error("the check digit in {data:?} is wrong for {sym}")]
    Checksum { sym: &'static str, data: String },
    #[error(
        "ITF encodes digits in pairs, so {len} digits cannot be printed as typed; add a leading zero"
    )]
    OddLength { len: usize },
    #[error("Codabar data must start and end with A, B, C or D, and use them nowhere else")]
    CodabarGuards,
    #[error(
        "a module width of {0} dot is below the 2-dot floor; mark the barcode out of spec to print it anyway"
    )]
    TooNarrow(u16),
    #[error("a module must be at least one dot wide")]
    ZeroModule,
    #[error("{sym} is not printable to its specification here: {reason}")]
    NotCompliant { sym: &'static str, reason: String },
    #[error(
        "the symbol is {needed} dots long but its frame allows {available}; it is refused rather than clipped, because a clipped barcode scans as a different value"
    )]
    TooWide { needed: u32, available: u32 },
}

/// A symbol laid out in modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modules {
    /// Each bar as (first module, width in modules), in reading order, measured
    /// from the start of the leading quiet zone.
    pub bars: Vec<(u32, u32)>,
    /// Total length in modules, both quiet zones included.
    pub len: u32,
}

impl Modules {
    /// Length in dots at a module width. This is the only figure a capacity
    /// readout should show: a Code 128's length depends on its character-set
    /// switches and cannot be predicted from the length of the data.
    pub fn width_dots(&self, module_dots: u16) -> u32 {
        self.len.saturating_mul(u32::from(module_dots))
    }
}

/// Encode, then measure.
pub fn width_dots(sym: Symbology, data: &str, module_dots: u16) -> Result<u32, BarcodeError> {
    Ok(encode(sym, data)?.width_dots(module_dots))
}

/// Turn data into modules, refusing anything that would not scan as typed.
pub fn encode(sym: Symbology, data: &str) -> Result<Modules, BarcodeError> {
    use barcoders::sym::{code39::Code39, ean8::EAN8, ean13::EAN13, tf::TF};

    if data.is_empty() {
        return Err(BarcodeError::Empty);
    }
    let fail = |e| from_barcoders(sym, data, e);
    let raw = match sym {
        Symbology::Code128 => {
            let code = Code128::encode_str(data).ok_or_else(|| BarcodeError::Character {
                sym: sym.label(),
                what: "characters outside Latin-1".into(),
            })?;
            return Ok(Modules {
                bars: code
                    .bar_coordinates()
                    .map(|b| (b.x, u32::from(b.width)))
                    .collect(),
                len: code.len() as u32,
            });
        }
        Symbology::Code39 => Code39::new(data).map_err(fail)?.encode(),
        Symbology::Itf => {
            if data.bytes().all(|b| b.is_ascii_digit()) && data.len() % 2 == 1 {
                return Err(BarcodeError::OddLength { len: data.len() });
            }
            TF::interleaved(data).map_err(fail)?.encode()
        }
        Symbology::Codabar => {
            check_codabar(data)?;
            codabar(data)?
        }
        Symbology::Ean13 => EAN13::new(data).map_err(fail)?.encode(),
        Symbology::Ean8 => EAN8::new(data).map_err(fail)?.encode(),
        Symbology::UpcA => {
            if !data.bytes().all(|b| b.is_ascii_digit()) {
                return Err(BarcodeError::Character {
                    sym: sym.label(),
                    what: format!("{data:?}; UPC-A is digits only"),
                });
            }
            if !(11..=12).contains(&data.len()) {
                return Err(BarcodeError::Length {
                    sym: sym.label(),
                    expected: sym.expected(),
                    got: data.len(),
                });
            }
            // A UPC-A is an EAN-13 with a leading zero, and the two check
            // digits coincide. Eleven digits become twelve and the crate
            // computes the check; twelve become thirteen and it verifies it.
            EAN13::new(format!("0{data}")).map_err(fail)?.encode()
        }
    };
    Ok(from_modules(sym, &raw))
}

/// Whether a retail symbol can be printed to its specification.
///
/// EAN and UPC fix the ratio of bar height to width, and permit 80 to 200
/// percent of a 0.33 mm module. On a 203 dpi head a module is a whole number of
/// dots, so only 3, 4 and 5 are compliant - two dots is 0.25 mm, 76 percent.
/// Height is what usually rules them out on these printers: even at 80 percent
/// an EAN-13 needs 18.3 mm of bar, against a 12 mm label.
///
/// Decided in integer micrometres; floats appear only in the message.
pub fn check_retail(
    sym: Symbology,
    module_dots: u16,
    bar_dots: u32,
    dpmm: Dpmm,
) -> Result<(), BarcodeError> {
    let Some((nominal_x, nominal_h)) = sym.retail() else {
        return Ok(());
    };
    let um = |dots: u64| dots * 1000 * u64::from(dpmm.den) / u64::from(dpmm.num.max(1));
    let x_um = um(u64::from(module_dots));
    let per_mille = x_um * 1000 / nominal_x;
    if !(800..=2000).contains(&per_mille) {
        return Err(BarcodeError::NotCompliant {
            sym: sym.label(),
            reason: format!(
                "a {module_dots}-dot module is {:.3} mm, {}% of the nominal 0.33 mm, and the specification allows 80 to 200%; use Code 128",
                x_um as f64 / 1000.0,
                per_mille / 10
            ),
        });
    }
    let need_um = nominal_h * per_mille / 1000;
    let have_um = um(u64::from(bar_dots));
    if have_um < need_um {
        return Err(BarcodeError::NotCompliant {
            sym: sym.label(),
            reason: format!(
                "its bars are {:.1} mm tall and at this width the specification needs {:.1} mm; Code 128 has no fixed height ratio and fits",
                have_um as f64 / 1000.0,
                need_um as f64 / 1000.0
            ),
        });
    }
    Ok(())
}

fn check_codabar(data: &str) -> Result<(), BarcodeError> {
    let guard = |c: &char| matches!(c, 'A'..='D');
    let chars: Vec<char> = data.chars().collect();
    let ok = chars.len() >= 2
        && guard(&chars[0])
        && guard(&chars[chars.len() - 1])
        && !chars[1..chars.len() - 1].iter().any(guard);
    if ok {
        Ok(())
    } else {
        Err(BarcodeError::CodabarGuards)
    }
}

/// Codabar's twenty characters as its standard gives them: seven elements,
/// alternating bar and space and starting with a bar, where a set bit is a wide
/// element. B, C and `+` are the three `barcoders` gets wrong.
const CODABAR: [(char, u8); 20] = [
    ('0', 0b000_0011),
    ('1', 0b000_0110),
    ('2', 0b000_1001),
    ('3', 0b110_0000),
    ('4', 0b001_0010),
    ('5', 0b100_0010),
    ('6', 0b010_0001),
    ('7', 0b010_0100),
    ('8', 0b011_0000),
    ('9', 0b100_1000),
    ('-', 0b000_1100),
    ('$', 0b001_1000),
    (':', 0b100_0101),
    ('/', 0b101_0001),
    ('.', 0b101_0100),
    ('+', 0b001_0101),
    ('A', 0b001_1010),
    ('B', 0b010_1001),
    ('C', 0b000_1011),
    ('D', 0b000_1110),
];

/// Codabar to modules, at the 2:1 wide-to-narrow ratio the other discrete
/// symbologies here use, with one narrow space between characters.
fn codabar(data: &str) -> Result<Vec<u8>, BarcodeError> {
    let mut out = Vec::new();
    for (i, c) in data.chars().enumerate() {
        let &(_, pattern) =
            CODABAR
                .iter()
                .find(|(k, _)| *k == c)
                .ok_or_else(|| BarcodeError::Character {
                    sym: Symbology::Codabar.label(),
                    what: format!("{c:?}"),
                })?;
        if i > 0 {
            out.push(0);
        }
        for e in 0..7 {
            let wide = (pattern >> (6 - e)) & 1 == 1;
            let ink = u8::from(e % 2 == 0);
            out.extend(std::iter::repeat_n(ink, if wide { 2 } else { 1 }));
        }
    }
    Ok(out)
}

fn from_barcoders(sym: Symbology, data: &str, e: barcoders::error::Error) -> BarcodeError {
    use barcoders::error::Error as E;
    match e {
        E::Length => BarcodeError::Length {
            sym: sym.label(),
            expected: sym.expected(),
            got: data.chars().count(),
        },
        E::Checksum => BarcodeError::Checksum {
            sym: sym.label(),
            data: data.to_string(),
        },
        E::Character | E::Generate => BarcodeError::Character {
            sym: sym.label(),
            what: format!("{data:?}"),
        },
    }
}

/// Bars from a module string, offset by the leading quiet zone.
fn from_modules(sym: Symbology, modules: &[u8]) -> Modules {
    let (lead, trail) = sym.quiet();
    let mut bars = Vec::new();
    let mut i = 0;
    while i < modules.len() {
        if modules[i] == 1 {
            let start = i;
            while i < modules.len() && modules[i] == 1 {
                i += 1;
            }
            bars.push((lead + start as u32, (i - start) as u32));
        } else {
            i += 1;
        }
    }
    Modules {
        bars,
        len: lead + modules.len() as u32 + trail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in Symbology::ALL {
            assert_eq!(Symbology::from_name(s.name()), Some(s));
        }
        assert_eq!(Symbology::from_name("code93"), None, "deliberately absent");
    }

    #[test]
    fn retail_compliance_at_203_dpi() {
        let d = Dpmm::DPI_203;
        // Two dots is 0.25 mm, 75.8% of nominal: below the 80% floor.
        assert!(check_retail(Symbology::Ean13, 2, 400, d).is_err());
        // Three dots is 113.6%, which needs about 26 mm of bar.
        assert!(
            check_retail(Symbology::Ean13, 3, 96, d).is_err(),
            "12 mm is not enough"
        );
        assert!(
            check_retail(Symbology::Ean13, 3, 208, d).is_ok(),
            "26 mm is"
        );
        // Six dots is 0.75 mm, 227%: above the 200% ceiling.
        assert!(check_retail(Symbology::Ean13, 6, 1000, d).is_err());
        // Anything that is not a retail symbology is never constrained this way.
        assert!(check_retail(Symbology::Code128, 1, 1, d).is_ok());
    }

    /// At 300 dpi the compliant widths shift, which is why this is computed
    /// rather than tabulated.
    #[test]
    fn retail_compliance_at_300_dpi() {
        let d = Dpmm::DPI_300;
        assert!(
            check_retail(Symbology::Ean13, 3, 400, d).is_err(),
            "0.254 mm, 77%"
        );
        assert!(
            check_retail(Symbology::Ean13, 4, 400, d).is_ok(),
            "0.339 mm, 103%"
        );
    }

    /// The three characters `barcoders` 2.0.0 gets wrong, pinned to the
    /// standard. Should a later `barcoders` fix them, nothing here changes:
    /// Codabar does not go through it.
    #[test]
    fn codabar_b_c_and_plus_follow_the_standard() {
        let m = |s: &str| {
            codabar(s)
                .unwrap()
                .iter()
                .map(|b| char::from(b'0' + b))
                .collect::<String>()
        };
        assert_eq!(m("B"), "1001001011");
        assert_eq!(m("C"), "1010010011");
        assert_eq!(m("+"), "1011011011");
        assert_eq!(m("A1"), "1011001001".to_owned() + "0" + "101011001");
    }

    #[test]
    fn the_ladder_orientation_gets_a_wider_default() {
        assert_eq!(default_module_dots(PrintDirection::Left), 3);
        assert_eq!(default_module_dots(PrintDirection::Top), 2);
        assert!(default_module_dots(PrintDirection::Top) >= MIN_MODULE_DOTS);
    }
}
