//! Test patterns that answer questions no protocol command will.
//!
//! # The ruler
//!
//! A printer never reports how long its labels are. The tag on the roll carries
//! a barcode and a used/total count, not millimetres; the lookup from one to the
//! other is a vendor cloud service; and `GetPaperInfo` - the one command that
//! sounds like it should answer - replies on this hardware with four bytes
//! carrying what looks like a gap height and nothing else.
//!
//! So the label measures itself. Print a millimetre scale longer than any label
//! could be, and read off where the paper ran out. It needs no protocol work,
//! no cloud call and no calipers, and it is the same trick that established the
//! reference roll as 40 mm.
//!
//! There is no text renderer yet and the ruler does not need one: tick height
//! encodes the scale, and every centimetre is counted out in dots so the
//! reading is unambiguous without numerals.

use crate::canvas::Canvas;
use crate::document::Ink;
use crate::error::RenderError;
use crate::greymap::GreyMap;
use crate::target::Target;

/// Tick heights in dots, shortest first: every millimetre, every 5, every 10.
const TICK_MM: i32 = 6;
const TICK_5MM: i32 = 12;
const TICK_10MM: i32 = 20;
/// How far the centimetre counter sits above the baseline.
const COUNTER_GAP: i32 = 4;
const COUNTER_DOT: i32 = 3;
/// Depth of the leading-edge bar, in dots. See the note where it is drawn.
const LEAD_BAR: i32 = 3;

/// A millimetre rule running the length of the canvas.
///
/// Draws a solid bar across the full head axis at the leading edge - which also
/// shows whether the whole head is being addressed - then ticks rising from the
/// far edge, and above every centimetre tick a row of small squares counting
/// how many centimetres have passed.
///
/// `along_width` says which way the scale runs: true when the label reads
/// landscape, which is the case for the D11/D110/D101 family.
pub fn ruler(target: &Target, along_width: bool) -> Result<GreyMap, RenderError> {
    let mut c = Canvas::new(target)?;
    let dpmm = target.dpmm();
    let (length, across) = if along_width {
        (i32::from(target.width_px), i32::from(target.height_px))
    } else {
        (i32::from(target.height_px), i32::from(target.width_px))
    };

    // A bar across the whole other axis at the leading edge. Any gap in it is a
    // head element that did not fire.
    //
    // Deliberately only a few dots deep rather than a millimetre or two: the
    // scale is measured from the leading edge, so a thick bar would sit on top
    // of the first millimetre ticks and make the very reading the ruler exists
    // for ambiguous.
    plot_rect(&mut c, along_width, 0, 0, LEAD_BAR, across);

    let mut mm = 1;
    loop {
        let at = dpmm.dots_from_tenths_mm(mm * 10);
        if at >= length {
            break;
        }
        if at < LEAD_BAR {
            mm += 1;
            continue;
        }
        let (h, thick) = if mm % 10 == 0 {
            (TICK_10MM, 2)
        } else if mm % 5 == 0 {
            (TICK_5MM, 1)
        } else {
            (TICK_MM, 1)
        };
        // Ticks rise from the far edge so the solid bar at the start cannot be
        // confused with the scale.
        plot_rect(&mut c, along_width, at, across - h, thick, h);

        // Count the centimetres in dots, so the scale can be read with no
        // numerals: one square at 10 mm, two at 20, and so on.
        if mm % 10 == 0 {
            let n = mm / 10;
            let base = across - TICK_10MM - COUNTER_GAP - COUNTER_DOT;
            for i in 0..n {
                let y = base - i * (COUNTER_DOT + 2);
                if y < 0 {
                    break;
                }
                plot_rect(&mut c, along_width, at, y, COUNTER_DOT, COUNTER_DOT);
            }
        }
        mm += 1;
    }
    Ok(c.finish())
}

/// Place a rectangle given in scale coordinates - along the rule, then across
/// it - whichever way round the canvas happens to be.
fn plot_rect(c: &mut Canvas, along_width: bool, along: i32, across: i32, len: i32, depth: i32) {
    if along_width {
        c.fill_rect_dots(along, across, len, depth, Ink::Black);
    } else {
        c.fill_rect_dots(across, along, depth, len, Ink::Black);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relabel_proto::PrintDirection;

    fn landscape() -> Target {
        Target::for_label(96, 320, 203, PrintDirection::Left).unwrap()
    }

    #[test]
    fn the_leading_bar_spans_the_whole_head() {
        let g = ruler(&landscape(), true).unwrap();
        for y in 0..96u16 {
            assert_eq!(g.at(0, y), 0, "row {y} of the leading bar");
        }
    }

    /// The point of the whole exercise: a tick every millimetre, at exactly
    /// eight dots apart on a 203 dpi head.
    #[test]
    fn ticks_land_on_whole_millimetres() {
        let g = ruler(&landscape(), true).unwrap();
        let baseline = 95u16;
        for mm in 1..39u16 {
            let x = mm * 8;
            assert_eq!(g.at(x, baseline), 0, "tick at {mm} mm");
            // A millimetre tick is one dot wide, so its neighbour is clear -
            // except next to the wider 10 mm ticks.
            if mm % 10 != 0 && (mm + 1) % 10 != 0 {
                assert_eq!(g.at(x + 1, baseline), 255, "only one dot wide at {mm} mm");
            }
        }
    }

    #[test]
    fn tick_height_encodes_the_scale() {
        let g = ruler(&landscape(), true).unwrap();
        let top_of = |x: u16| (0..96u16).find(|y| g.at(x, *y) == 0).unwrap();
        assert_eq!(top_of(8), 96 - TICK_MM as u16, "1 mm");
        assert_eq!(top_of(5 * 8), 96 - TICK_5MM as u16, "5 mm");
        // The 10 mm tick carries the centimetre counter above it, so the
        // topmost ink is the counter rather than the tick.
        assert!(top_of(10 * 8) < 96 - TICK_10MM as u16, "10 mm is counted");
    }

    /// Three squares above the 30 mm tick, so the scale reads without numerals.
    #[test]
    fn centimetres_are_counted_in_dots() {
        let g = ruler(&landscape(), true).unwrap();
        let x = 30 * 8;
        let mut runs = 0;
        let mut prev = 255;
        for y in 0..(96 - TICK_10MM as u16) {
            let v = g.at(x, y);
            if v == 0 && prev != 0 {
                runs += 1;
            }
            prev = v;
        }
        assert_eq!(runs, 3, "30 mm is three centimetre squares");
    }

    #[test]
    fn the_scale_runs_the_other_way_for_a_top_direction_head() {
        let t = Target::for_label(96, 320, 203, PrintDirection::Top).unwrap();
        assert_eq!((t.width_px, t.height_px), (96, 320));
        let g = ruler(&t, false).unwrap();
        // The leading bar now spans the width.
        for x in 0..96u16 {
            assert_eq!(g.at(x, 0), 0);
        }
        assert_eq!(g.at(95, 8 * 5), 0, "a 5 mm tick, measured down the page");
    }
}
