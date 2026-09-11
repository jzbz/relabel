//! Two-dimensional symbols: QR and DataMatrix, as grids of modules.
//!
//! Like the linear barcodes, this module produces modules and nothing more;
//! the rasteriser draws each one as an exact square of whole dots. A module
//! size that is not a whole number of dots would make some modules one dot
//! wider than their neighbours, and a 2D decoder samples the grid by its
//! timing pattern, so uneven modules are read as the wrong ones.
//!
//! # Why DataMatrix matters here
//!
//! A QR quiet zone is four modules on every side; a DataMatrix one is one. On
//! a label 96 dots deep, that difference is most of the story: a QR at three
//! dots a module tops out at version 1, 21 modules square, while a rectangular
//! DataMatrix fits several times the data at a larger module.
//!
//! # Fences around both encoders
//!
//! `qrcode` 0.14.1 needs two. A Micro QR version handed to `with_version` can
//! underflow and panic, in release builds too, so the document cannot express
//! one and a test keeps this crate from naming one. And its segmenter takes
//! UTF-8 for Shift JIS often enough to matter, so [`qr`] builds each symbol
//! from the crate's parts rather than through its front door.
//!
//! `datamatrix` 0.3.3 needs three, each confirmed by running it:
//!
//! - **Macro headers are on by default and wrong.** Data that begins like an
//!   ISO/IEC 15434 envelope (`[)>` RS `05` GS) loses its last two bytes, and
//!   panics if it is shorter than nine. Every symbol here is built with macros
//!   off.
//! - **Choosing among several sizes fails on long data.** Given more than one
//!   candidate, it reports an empty size list well below the capacity of the
//!   largest. So sizes are tried here one at a time, each pinned.
//! - **Its string encoder adds a UTF-8 ECI for any control character**, a tab
//!   or newline included. Text that fits Latin-1 - DataMatrix's default
//!   character set - is encoded here as plain Latin-1 bytes with no ECI; only
//!   text beyond it gets one.

use crate::barcode::MIN_MODULE_DOTS;
use crate::document::{DmSize, QrEc, Rot};
use datamatrix::{DataMatrix, DataMatrixBuilder, SymbolList, SymbolSize};
use qrcode::bits::Bits;
use qrcode::optimize::{Optimizer, Parser, Segment};
use qrcode::types::{Mode, QrError};
use qrcode::{Color, EcLevel, QrCode, Version};

/// QR's quiet zone, in modules, on every side.
pub const QR_QUIET: u32 = 4;
/// DataMatrix's quiet zone, in modules, on every side.
pub const DM_QUIET: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    #[error("a 2D code needs some data")]
    Empty,
    #[error(
        "the symbol needs {}x{} dots with its quiet zone but its frame is {}x{}; it is refused rather than clipped or shrunk",
        needed.0, needed.1, available.0, available.1
    )]
    TooLarge {
        needed: (u32, u32),
        available: (u32, u32),
    },
    #[error(
        "a module of {0} dot is below the 2-dot floor; mark the code out of spec to print it anyway"
    )]
    TooNarrow(u16),
    #[error("a module must be at least one dot wide")]
    ZeroModule,
    #[error("QR version {0} does not exist; versions run from 1 to 40")]
    BadVersion(u8),
    #[error("the data does not fit {0}")]
    TooMuchData(String),
    #[error(
        "{0:?} is not a DataMatrix size; write one as rows by columns, like \"16x48\", or use \"square\", \"rectangle\" or \"any\""
    )]
    UnknownSize(String),
    #[error("the QR encoder refused the data: {0}")]
    Unencodable(String),
}

/// The module-size rule every 2D symbol follows: never zero, and never below
/// two dots unless the element is explicitly marked out of spec.
pub fn check_module(module_dots: u16, out_of_spec: bool) -> Result<(), CodeError> {
    if module_dots == 0 {
        Err(CodeError::ZeroModule)
    } else if module_dots < MIN_MODULE_DOTS && !out_of_spec {
        Err(CodeError::TooNarrow(module_dots))
    } else {
        Ok(())
    }
}

/// Encode a QR code, and report the version it came out as, so an editor can
/// pin it.
///
/// Built from the crate's parts rather than through `QrCode::new` or
/// `with_version`, for two reasons.
///
/// **Its segmenter takes UTF-8 for Shift JIS.** Any byte pair inside Shift
/// JIS's ranges becomes a Kanji-mode segment, and UTF-8 kana often qualifies:
/// "ああああ" goes out in Kanji mode, which a decoder that follows the standard
/// reads as Shift JIS and shows as "縺ゅ≠縺ゅ≠". relabel's text is always UTF-8,
/// so every Kanji segment is demoted to Byte before the segments are optimised.
///
/// **A version from its automatic path is not always pinnable.** That path
/// tunes the segmentation for version 9, 26 or 40 and fits the result into the
/// smallest version that holds it, while `with_version` re-tunes for the one
/// version it is given; the two occasionally disagree, and a version reported
/// for pinning then refuses the same data. Here one rule decides both: a
/// version holds the data if any tuning fits it, and the automatic choice is
/// the smallest version that holds it, so it is always pinnable. For data the
/// crate would not send in Kanji mode it is never larger than the crate's own
/// choice. For data it would - kana, CJK, anything whose UTF-8 bytes happen to
/// pair up as Shift JIS - it can be a version or two larger, because Byte mode
/// spends 16 bits on a pair of bytes where Kanji mode spends 13; the crate's
/// smaller symbol is the one that reads back wrong.
///
/// The bytes carry no ECI. Whether every scanner reads unmarked non-ASCII bytes
/// as UTF-8 is not known.
pub fn qr(data: &str, ec: QrEc, version: Option<u8>) -> Result<(ModuleGrid, u8), CodeError> {
    if data.is_empty() {
        // The encoder accepts empty data and produces a symbol holding
        // nothing, which is never what a label meant.
        return Err(CodeError::Empty);
    }
    let level = match ec {
        QrEc::L => EcLevel::L,
        QrEc::M => EcLevel::M,
        QrEc::Q => EcLevel::Q,
        QrEc::H => EcLevel::H,
    };
    let bytes = data.as_bytes();
    let (code, chosen) = match version {
        Some(v @ 1..=40) => (qr_at(bytes, v, level), v),
        Some(v) => return Err(CodeError::BadVersion(v)),
        // Refused before the walk: data that fits no tuning at version 40
        // fits nothing smaller, and data longer than any symbol's capacity
        // need not be parsed at all.
        None if bytes.len() > QR_MAX_BYTES || !qr_fits(bytes, 40, level) => {
            (Err(QrError::DataTooLong), 40)
        }
        None => (1..=40u8)
            .map(|v| (qr_at(bytes, v, level), v))
            .find(|(r, _)| !matches!(r, Err(QrError::DataTooLong)))
            .unwrap_or((Err(QrError::DataTooLong), 40)),
    };
    let code = code.map_err(|e| match e {
        QrError::DataTooLong => CodeError::TooMuchData(match version {
            Some(v) => format!("a version {v} QR code at level {ec:?}"),
            None => format!("any QR code at level {ec:?}"),
        }),
        other => CodeError::Unencodable(other.to_string()),
    })?;
    let w = code.width() as u32;
    let dark = code
        .into_colors()
        .into_iter()
        .map(|c| c == Color::Dark)
        .collect();
    Ok((ModuleGrid::new(w, w, QR_QUIET, dark), chosen))
}

/// The most bytes any QR symbol holds: 7089, all digits, at version 40 level L.
/// Numeric is the densest mode, so longer data cannot fit.
const QR_MAX_BYTES: usize = 7089;

/// The segmentations worth trying at version `v`, its own first.
///
/// The optimiser depends on a version only through its length-field class -
/// 1 to 9, 10 to 26, 27 to 40 - so these three are every segmentation it can
/// produce. The version's own class first reproduces `with_version` whenever
/// that fits.
fn tunings(v: i16) -> [i16; 3] {
    match v {
        1..=9 => [9, 26, 40],
        10..=26 => [26, 9, 40],
        _ => [40, 9, 26],
    }
}

/// The symbol at one version, if the data fits it at any tuning.
fn qr_at(bytes: &[u8], v: u8, level: EcLevel) -> Result<QrCode, QrError> {
    let v = i16::from(v);
    let mut result = Err(QrError::DataTooLong);
    for tune in tunings(v) {
        result = qr_bits(bytes, v, tune, level);
        if !matches!(result, Err(QrError::DataTooLong)) {
            break;
        }
    }
    // `with_bits` panics on a stream that was never terminated; `qr_bits`
    // always terminates it.
    QrCode::with_bits(result?, level)
}

/// Whether the data fits version `v` at any tuning, without building a symbol
/// or paying for its mask search.
fn qr_fits(bytes: &[u8], v: i16, level: EcLevel) -> bool {
    tunings(v)
        .into_iter()
        .any(|tune| qr_bits(bytes, v, tune, level).is_ok())
}

/// Segments optimised for version `tune`, Kanji demoted to Byte, pushed into
/// version `v` and terminated: everything but the symbol itself.
fn qr_bits(bytes: &[u8], v: i16, tune: i16, level: EcLevel) -> Result<Bits, QrError> {
    let segments = Parser::new(bytes).map(|s| match s.mode {
        Mode::Kanji => Segment {
            mode: Mode::Byte,
            ..s
        },
        _ => s,
    });
    let mut bits = Bits::new(Version::Normal(v));
    bits.push_segments(bytes, Optimizer::new(segments, Version::Normal(tune)))?;
    bits.push_terminator(level)?;
    Ok(bits)
}

/// Every DataMatrix size, as (symbol, rows, columns): the 24 squares and 6
/// rectangles of ISO/IEC 16022, then the 18 DMRE rectangles of ISO/IEC 21471.
///
/// The crate exposes no dimensions, so the table is relabel's own, and a test
/// checks every row against the bitmap the crate actually produces.
pub const DM_SIZES: [(SymbolSize, u8, u8); 48] = [
    (SymbolSize::Square10, 10, 10),
    (SymbolSize::Square12, 12, 12),
    (SymbolSize::Square14, 14, 14),
    (SymbolSize::Square16, 16, 16),
    (SymbolSize::Square18, 18, 18),
    (SymbolSize::Square20, 20, 20),
    (SymbolSize::Square22, 22, 22),
    (SymbolSize::Square24, 24, 24),
    (SymbolSize::Square26, 26, 26),
    (SymbolSize::Square32, 32, 32),
    (SymbolSize::Square36, 36, 36),
    (SymbolSize::Square40, 40, 40),
    (SymbolSize::Square44, 44, 44),
    (SymbolSize::Square48, 48, 48),
    (SymbolSize::Square52, 52, 52),
    (SymbolSize::Square64, 64, 64),
    (SymbolSize::Square72, 72, 72),
    (SymbolSize::Square80, 80, 80),
    (SymbolSize::Square88, 88, 88),
    (SymbolSize::Square96, 96, 96),
    (SymbolSize::Square104, 104, 104),
    (SymbolSize::Square120, 120, 120),
    (SymbolSize::Square132, 132, 132),
    (SymbolSize::Square144, 144, 144),
    (SymbolSize::Rect8x18, 8, 18),
    (SymbolSize::Rect8x32, 8, 32),
    (SymbolSize::Rect12x26, 12, 26),
    (SymbolSize::Rect12x36, 12, 36),
    (SymbolSize::Rect16x36, 16, 36),
    (SymbolSize::Rect16x48, 16, 48),
    (SymbolSize::Rect8x48, 8, 48),
    (SymbolSize::Rect8x64, 8, 64),
    (SymbolSize::Rect8x80, 8, 80),
    (SymbolSize::Rect8x96, 8, 96),
    (SymbolSize::Rect8x120, 8, 120),
    (SymbolSize::Rect8x144, 8, 144),
    (SymbolSize::Rect12x64, 12, 64),
    (SymbolSize::Rect12x88, 12, 88),
    (SymbolSize::Rect16x64, 16, 64),
    (SymbolSize::Rect20x36, 20, 36),
    (SymbolSize::Rect20x44, 20, 44),
    (SymbolSize::Rect20x64, 20, 64),
    (SymbolSize::Rect22x48, 22, 48),
    (SymbolSize::Rect24x48, 24, 48),
    (SymbolSize::Rect24x64, 24, 64),
    (SymbolSize::Rect26x40, 26, 40),
    (SymbolSize::Rect26x48, 26, 48),
    (SymbolSize::Rect26x64, 26, 64),
];

fn dm_dims(size: SymbolSize) -> (u8, u8) {
    DM_SIZES
        .iter()
        .find(|(s, _, _)| *s == size)
        .map(|&(_, r, c)| (r, c))
        .unwrap_or((0, 0))
}

/// The sizes a [`DmSize`] allows, smallest first in the crate's own order:
/// fewest data codewords, then the smaller diagonal.
fn dm_candidates(size: &DmSize) -> Result<Vec<SymbolSize>, CodeError> {
    Ok(match size {
        DmSize::Square => SymbolList::all().enforce_square().iter().collect(),
        DmSize::Rectangle => SymbolList::all().enforce_rectangular().iter().collect(),
        DmSize::Any => SymbolList::all().iter().collect(),
        DmSize::Fixed { rows, cols } => {
            let Some(&(s, _, _)) = DM_SIZES.iter().find(|&&(_, r, c)| (r, c) == (*rows, *cols))
            else {
                return Err(CodeError::UnknownSize(format!("{rows}x{cols}")));
            };
            vec![s]
        }
        DmSize::Unknown(s) => return Err(CodeError::UnknownSize(s.clone())),
    })
}

/// Encode a DataMatrix: the smallest allowed size that holds the data and
/// passes `fits`, and the size it chose, so an editor can pin it.
///
/// `fits` is how the frame takes part. On a label 96 dots deep the smallest
/// symbol by data capacity is often a long DMRE strip that fits only at a
/// smaller module than a squarer one a size up, so the rasteriser asks for
/// the smallest symbol that fits the frame at the element's module size, not
/// merely the smallest. When nothing fits, the smallest that holds the data is
/// returned, and drawing it reports the shortfall.
pub fn data_matrix(
    data: &str,
    size: &DmSize,
    fits: impl Fn(&ModuleGrid) -> bool,
) -> Result<(ModuleGrid, DmSize), CodeError> {
    if data.is_empty() {
        return Err(CodeError::Empty);
    }
    let latin1: Option<Vec<u8>> = data.chars().map(|c| u8::try_from(c).ok()).collect();
    let mut first = None;
    for s in dm_candidates(size)? {
        let b = DataMatrixBuilder::new()
            .with_macros(false)
            .with_symbol_list(s);
        let encoded = match &latin1 {
            Some(bytes) => b.encode(bytes),
            None => b.encode_str(data),
        };
        let Ok(dm) = encoded else { continue };
        let grid = dm_grid(&dm);
        let (rows, cols) = dm_dims(s);
        let chosen = DmSize::Fixed { rows, cols };
        if fits(&grid) {
            return Ok((grid, chosen));
        }
        first.get_or_insert((grid, chosen));
    }
    first.ok_or_else(|| {
        CodeError::TooMuchData(match size {
            DmSize::Fixed { rows, cols } => format!("a {rows}x{cols} DataMatrix"),
            DmSize::Square => "any square DataMatrix".into(),
            DmSize::Rectangle => "any rectangular DataMatrix".into(),
            _ => "any DataMatrix".into(),
        })
    })
}

fn dm_grid(dm: &DataMatrix) -> ModuleGrid {
    let bitmap = dm.bitmap();
    let (w, h) = (bitmap.width(), bitmap.height());
    let mut dark = vec![false; w * h];
    // `pixels` yields the dark modules as (column, row).
    for (x, y) in bitmap.pixels() {
        if x < w && y < h {
            dark[y * w + x] = true;
        }
    }
    ModuleGrid::new(w as u32, h as u32, DM_QUIET, dark)
}

/// A symbol laid out as modules, quiet zone excluded.
///
/// `w` columns by `h` rows, in the orientation the symbol reads. Where that
/// lands is the rasteriser's business: along the document's own axes on a
/// `Left` target, transposed on a `Top` one, and turned by the element's
/// rotation either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGrid {
    pub w: u32,
    pub h: u32,
    /// Required clear margin on every side, in modules.
    pub quiet: u32,
    /// Row-major, `w * h`, `true` for a dark module.
    dark: Vec<bool>,
}

impl ModuleGrid {
    /// Wrap a row-major module buffer. A buffer of the wrong length is padded
    /// with light modules or cut short, so a mismatch cannot become a panic
    /// while drawing - though no caller in this crate produces one.
    pub fn new(w: u32, h: u32, quiet: u32, mut dark: Vec<bool>) -> ModuleGrid {
        dark.resize(w as usize * h as usize, false);
        ModuleGrid { w, h, quiet, dark }
    }

    pub fn is_dark(&self, x: u32, y: u32) -> bool {
        x < self.w && y < self.h && self.dark[y as usize * self.w as usize + x as usize]
    }

    /// Modules across and down, quiet zone included.
    pub fn outer(&self) -> (u32, u32) {
        (
            self.w.saturating_add(self.quiet.saturating_mul(2)),
            self.h.saturating_add(self.quiet.saturating_mul(2)),
        )
    }

    /// Footprint in dots at a module size, quiet zone included.
    pub fn footprint(&self, module_dots: u16) -> (u32, u32) {
        let (w, h) = self.outer();
        let m = u32::from(module_dots);
        (w.saturating_mul(m), h.saturating_mul(m))
    }

    /// The largest whole module size at which the symbol and its quiet zone
    /// fit a box, or `None` if not even one dot a module does.
    ///
    /// This is the number the editor shows. Whether it is *enough* is a
    /// separate question - below two dots it will not scan - and is the
    /// caller's to ask.
    pub fn fit(&self, max_w: u32, max_h: u32) -> Option<u16> {
        let (w, h) = self.outer();
        if w == 0 || h == 0 {
            return None;
        }
        let m = (max_w / w).min(max_h / h);
        (m >= 1).then(|| m.min(u32::from(u16::MAX)) as u16)
    }

    /// The grid turned clockwise by the element's rotation, quantised to a
    /// quarter turn.
    ///
    /// Unlike a rectangle, a symbol has content to turn: a QR code on its side
    /// still scans, but it must be the same symbol on its side, not a
    /// transposed one, or the editor's preview and the printed label disagree
    /// about where the finder patterns are.
    pub fn turned(&self, rot: Rot) -> ModuleGrid {
        let quarters = match rot.quantised() {
            Rot::D90 => 1,
            Rot::D180 => 2,
            Rot::D270 => 3,
            _ => 0,
        };
        let mut g = self.clone();
        for _ in 0..quarters {
            g = g.quarter_cw();
        }
        g
    }

    /// Rows for columns: `dst(x, y) = src(y, x)`. A reflection, not a rotation;
    /// it exists to cancel the one a `Top` target's axis swap makes. See the
    /// rasteriser.
    pub fn transposed(&self) -> ModuleGrid {
        let (w, h) = (self.h, self.w);
        let mut dark = Vec::with_capacity(self.dark.len());
        for y in 0..h {
            for x in 0..w {
                dark.push(self.is_dark(y, x));
            }
        }
        ModuleGrid {
            w,
            h,
            quiet: self.quiet,
            dark,
        }
    }

    /// `dst(x, y) = src(x = y, y = h - 1 - x)`: the source's top-left module
    /// lands top-right, the same convention as `BitPlane::rotate_cw`.
    fn quarter_cw(&self) -> ModuleGrid {
        let (w, h) = (self.h, self.w);
        let mut dark = Vec::with_capacity(self.dark.len());
        for y in 0..h {
            for x in 0..w {
                dark.push(self.is_dark(y, self.h - 1 - x));
            }
        }
        ModuleGrid {
            w,
            h,
            quiet: self.quiet,
            dark,
        }
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    /// An L of three dark modules in a 3 x 2 grid:
    ///
    /// ```text
    /// X X .
    /// X . .
    /// ```
    fn ell() -> ModuleGrid {
        ModuleGrid::new(3, 2, 1, vec![true, true, false, true, false, false])
    }

    fn rows(g: &ModuleGrid) -> Vec<String> {
        (0..g.h)
            .map(|y| {
                (0..g.w)
                    .map(|x| if g.is_dark(x, y) { 'X' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_quarter_turn_is_clockwise() {
        let g = ell().turned(Rot::D90);
        assert_eq!((g.w, g.h), (2, 3));
        assert_eq!(rows(&g), ["XX", ".X", ".."]);
        assert_eq!(rows(&ell().turned(Rot::D180)), ["..X", ".XX"]);
        assert_eq!(rows(&ell().turned(Rot::D270)), ["..", "X.", "XX"]);
    }

    #[test]
    fn four_quarter_turns_are_the_identity() {
        let mut g = ell();
        for _ in 0..4 {
            g = g.turned(Rot::D90);
        }
        assert_eq!(g, ell());
    }

    #[test]
    fn a_free_angle_turns_to_the_nearest_quarter() {
        assert_eq!(ell().turned(Rot::Other(100)), ell().turned(Rot::D90));
        assert_eq!(ell().turned(Rot::Other(10)), ell());
    }

    #[test]
    fn footprint_includes_the_quiet_zone() {
        assert_eq!(ell().outer(), (5, 4));
        assert_eq!(ell().footprint(3), (15, 12));
    }

    #[test]
    fn fit_is_the_largest_whole_module_that_fits() {
        let g = ell(); // 5 x 4 modules with its quiet zone
        assert_eq!(g.fit(15, 12), Some(3));
        assert_eq!(g.fit(19, 15), Some(3), "never rounds up");
        assert_eq!(g.fit(20, 12), Some(3), "the tighter axis decides");
        assert_eq!(g.fit(4, 100), None);
    }

    /// Growing the box never shrinks the module.
    #[test]
    fn fit_is_monotonic() {
        let g = ell();
        let mut last = None;
        for side in 0..200 {
            let m = g.fit(side, side);
            assert!(m >= last, "{side}: {m:?} after {last:?}");
            last = m;
        }
    }

    /// Micro QR must stay unreachable: in `qrcode` 0.14.1 a Micro version can
    /// underflow and panic, in release builds too. The document cannot express
    /// one; this keeps the code from naming one, in every source file under
    /// `src/` - found by walking the directory, so a new module cannot be
    /// missed. Only comment lines are skipped, and the needle is assembled so
    /// that this test does not find itself.
    #[test]
    fn micro_qr_is_never_named_in_code() {
        let needle = concat!("Mic", "ro");
        let mut seen = 0;
        let mut dirs = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src"
        ))];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                seen += 1;
                let src = std::fs::read_to_string(&path).unwrap();
                for (i, line) in src.lines().enumerate() {
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    for (at, _) in line.match_indices(needle) {
                        // Any whole-word use outside a comment line fails: a
                        // path, a value, an aliased import in a brace list or
                        // out of one. Only a longer identifier that happens to
                        // contain the letters passes.
                        let ident =
                            |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                        let prev = line[..at].chars().next_back();
                        let next = line[at + needle.len()..].chars().next();
                        assert!(
                            ident(prev) || ident(next),
                            "{}:{}: {line}",
                            path.display(),
                            i + 1
                        );
                    }
                }
            }
        }
        assert!(seen >= 11, "only {seen} source files were scanned");
    }

    #[test]
    fn transposing_swaps_rows_for_columns() {
        let t = ell().transposed();
        assert_eq!((t.w, t.h), (2, 3));
        assert_eq!(rows(&t), ["XX", "X.", ".."]);
        assert_eq!(t.transposed(), ell());
        // A reflection: no quarter turn produces it.
        for r in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
            assert_ne!(ell().turned(r), t, "{r:?}");
        }
    }

    /// The automatic choice is always pinnable. This string came out as
    /// version 9 unpinned and was refused at version 9 pinned, when the two
    /// paths went through the crate's differently tuned front doors.
    #[test]
    fn the_automatic_qr_version_can_be_pinned() {
        let s = "DTOFGD9266806AAAAAAAAAXVBHMDVULTR24397607499869764904PGTMKSLJC8888888872555XFNTVQGFALGY7729097OLQUZAOKZUKMGMEFVAFK602BBBBB999MSFYCRPBRB44:::::::";
        for ec in [QrEc::L, QrEc::M, QrEc::Q, QrEc::H] {
            let (auto, v) = qr(s, ec, None).unwrap();
            let (pinned, _) =
                qr(s, ec, Some(v)).unwrap_or_else(|e| panic!("{ec:?}: version {v} refused: {e}"));
            assert_eq!(auto, pinned, "{ec:?}");
        }
        // No larger than the crate's own choice, version 9, for text it would
        // not send in Kanji mode...
        assert!(qr(s, QrEc::H, None).unwrap().1 <= 9);
        // ...and larger where it would, by design: the crate puts this in
        // version 1, where it reads back as Shift JIS.
        assert_eq!(qr("ありがとう", QrEc::M, None).unwrap().1, 2);
    }

    /// Latin-1 text reaches the crate as bytes with no ECI - control characters
    /// included, which its string encoder would mark with a UTF-8 ECI. Checked
    /// module by module, because a decoder returns the same text either way.
    #[test]
    fn latin1_text_gets_no_eci() {
        for (data, has_controls) in [
            ("a\tb\nc", true),
            ("[)>\u{1e}05\u{1d}ABCDE", true),
            // DEL and a C1 control: inside Latin-1, outside the crate's own
            // printable-only test.
            ("a\u{7f}b\u{85}c", true),
            ("Größe", false),
        ] {
            let (g, chosen) = data_matrix(data, &DmSize::Any, |_| true).unwrap();
            let DmSize::Fixed { rows, cols } = chosen else {
                panic!("{chosen:?}")
            };
            let s = DM_SIZES
                .iter()
                .find(|&&(_, r, c)| (r, c) == (rows, cols))
                .unwrap()
                .0;
            let b = || {
                DataMatrixBuilder::new()
                    .with_macros(false)
                    .with_symbol_list(s)
            };
            let bytes: Vec<u8> = data.chars().map(|c| c as u8).collect();
            assert_eq!(g, dm_grid(&b().encode(&bytes).unwrap()), "{data:?}");
            if has_controls && let Ok(marked) = b().encode_str(data) {
                assert_ne!(g, dm_grid(&marked), "{data:?} came out with an ECI");
            }
        }
    }

    #[test]
    fn datamatrix_shape_policies_are_honoured() {
        for data in ["RELABEL", "RELABEL-0042", "SKU-00412 lot 7"] {
            let (g, _) = data_matrix(data, &DmSize::Rectangle, |_| true).unwrap();
            assert_ne!(g.w, g.h, "{data:?} rectangle");
            let (g, _) = data_matrix(data, &DmSize::Square, |_| true).unwrap();
            assert_eq!(g.w, g.h, "{data:?} square");
        }
        // "any" takes a rectangle when one is smallest.
        let (_, c) = data_matrix("RELABEL-0042", &DmSize::Any, |_| true).unwrap();
        assert_eq!(c, DmSize::Fixed { rows: 8, cols: 32 });
        // 20x44 is a DMRE size (ISO/IEC 21471), reachable through "rectangle"
        // and "any" alike.
        let digits = "0123456789".repeat(10);
        for policy in [DmSize::Rectangle, DmSize::Any] {
            let (_, c) = data_matrix(&digits, &policy, |_| true).unwrap();
            assert_eq!(c, DmSize::Fixed { rows: 20, cols: 44 }, "{policy:?}");
        }
        // And "any" takes a square when one is smallest.
        let (_, c) = data_matrix("RELABEL", &DmSize::Any, |_| true).unwrap();
        assert_eq!(c, DmSize::Fixed { rows: 14, cols: 14 });
    }

    #[test]
    fn when_nothing_fits_the_smallest_that_holds_the_data_is_returned() {
        let (_, c) = data_matrix("RELABEL", &DmSize::Square, |_| false).unwrap();
        assert_eq!(c, DmSize::Fixed { rows: 14, cols: 14 });
    }

    /// The size table is relabel's, not the crate's, so check every row
    /// against a real symbol, and that it covers every size there is.
    #[test]
    fn the_datamatrix_size_table_matches_the_crate() {
        assert_eq!(SymbolList::all().iter().count(), DM_SIZES.len());
        for s in SymbolList::all().iter() {
            let (rows, cols) = dm_dims(s);
            let dm = DataMatrixBuilder::new()
                .with_macros(false)
                .with_symbol_list(s)
                .encode(b"")
                .unwrap();
            let b = dm.bitmap();
            assert_eq!(
                (b.height(), b.width()),
                (usize::from(rows), usize::from(cols)),
                "{s:?}"
            );
        }
    }

    /// Envelope-looking data used to lose bytes or panic with macros on.
    #[test]
    fn datamatrix_macros_are_off() {
        for data in [
            "[)>\u{1e}05\u{1d}",
            "[)>\u{1e}05\u{1d}ABCDE",
            "[)>\u{1e}06\u{1d}XY",
        ] {
            data_matrix(data, &DmSize::Any, |_| true).unwrap();
        }
    }

    /// Long data fails in the crate when several sizes are offered at once;
    /// trying them one at a time reaches the true capacity. 150 digits is past
    /// the point where the crate's own choice gives up, so handing it the
    /// whole list, which this module deliberately avoids, fails here.
    #[test]
    fn datamatrix_reaches_full_capacity_through_a_size_choice() {
        let long = "1".repeat(150);
        let whole_list = DataMatrixBuilder::new()
            .with_macros(false)
            .with_symbol_list(SymbolList::all().enforce_rectangular())
            .encode(long.as_bytes());
        assert!(whole_list.is_err(), "the crate now copes with a size list");
        assert!(data_matrix(&long, &DmSize::Rectangle, |_| true).is_ok());

        let (g, chosen) = data_matrix(&"1".repeat(98), &DmSize::Rectangle, |g| g.h <= 16).unwrap();
        assert_eq!(chosen, DmSize::Fixed { rows: 16, cols: 48 });
        assert_eq!((g.w, g.h), (48, 16));
        assert!(
            data_matrix(
                &"1".repeat(99),
                &DmSize::Fixed { rows: 16, cols: 48 },
                |_| true
            )
            .is_err()
        );
    }

    #[test]
    fn the_frame_steers_the_datamatrix_choice() {
        let data = "RELABEL-0042";
        let (_, smallest) = data_matrix(data, &DmSize::Rectangle, |_| true).unwrap();
        // Refuse anything longer than 30 modules and a squarer one is chosen.
        let (g, steered) = data_matrix(data, &DmSize::Rectangle, |g| g.w <= 30).unwrap();
        assert_ne!(smallest, steered);
        assert!(g.w <= 30);
    }

    #[test]
    fn qr_reports_the_version_it_chose() {
        let (g, v) = qr("RELABEL", QrEc::M, None).unwrap();
        assert_eq!((v, g.w, g.h, g.quiet), (1, 21, 21, QR_QUIET));
        let (g, v) = qr("RELABEL", QrEc::M, Some(3)).unwrap();
        assert_eq!((v, g.w), (3, 29));
        let (g, v) = qr("RELABEL", QrEc::M, Some(40)).unwrap();
        assert_eq!((v, g.w), (40, 177));
        // The automatic path reaches version 40 and refuses one byte more -
        // and anything past the densest capacity without trying a version.
        let (g, v) = qr(&"a".repeat(2953), QrEc::L, None).unwrap();
        assert_eq!((v, g.w), (40, 177));
        for len in [2954, QR_MAX_BYTES + 1, 1_000_000] {
            assert!(
                matches!(
                    qr(&"a".repeat(len), QrEc::L, None),
                    Err(CodeError::TooMuchData(_))
                ),
                "{len}"
            );
        }
        // Digits are the densest: 7089 fit, one more does not.
        assert_eq!(qr(&"1".repeat(QR_MAX_BYTES), QrEc::L, None).unwrap().1, 40);
        assert!(qr(&"1".repeat(QR_MAX_BYTES + 1), QrEc::L, None).is_err());
        assert_eq!(
            qr("RELABEL", QrEc::M, Some(0)),
            Err(CodeError::BadVersion(0))
        );
        assert_eq!(
            qr("RELABEL", QrEc::M, Some(41)),
            Err(CodeError::BadVersion(41))
        );
        assert!(matches!(
            qr(&"A".repeat(26), QrEc::L, Some(1)),
            Err(CodeError::TooMuchData(_))
        ));
        assert_eq!(qr("", QrEc::M, None), Err(CodeError::Empty));
    }

    #[test]
    fn unknown_datamatrix_sizes_are_reported() {
        for s in [
            DmSize::Unknown("hexagon".into()), // Not a size in either standard.
            DmSize::Fixed { rows: 20, cols: 21 },
        ] {
            assert!(matches!(
                data_matrix("x", &s, |_| true),
                Err(CodeError::UnknownSize(_))
            ));
        }
    }

    #[test]
    fn a_short_buffer_is_padded_not_a_panic() {
        let g = ModuleGrid::new(4, 4, 0, vec![true]);
        assert!(g.is_dark(0, 0));
        assert!(!g.is_dark(3, 3));
        assert!(!g.is_dark(99, 0));
    }
}
