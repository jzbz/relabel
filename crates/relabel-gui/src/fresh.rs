//! A new label: which printer it is for, how long it is along the feed, and the
//! blank document that makes.
//!
//! No toolkit type in any signature, for `geom`'s reason: every rule here is
//! reached by an ordinary unit test, and the New label pane only shows what these
//! functions say.
//!
//! # Only the feed is asked for
//!
//! The canvas has to be exactly as wide as the print head across the head axis
//! (`Target::require_head`), and nothing draws media narrower than the head, so
//! the one length an operator chooses is the one along the feed. The printer never
//! reports it - the roll's tag carries a barcode and a count, not millimetres
//! (PROTOCOL-NOTES.md, "The label on this roll is 40 mm x 12 mm") - so the
//! length is typed, or taken from a preset.
//!
//! # Which printer
//!
//! The printer relabel is connected to, when it has one and this build can draw
//! for it. Otherwise the model the open label was made for, when the registry
//! knows it and this build can draw for it, and a D110_M otherwise, which is said
//! out loud.

use relabel_proto::ModelSpec;
use relabel_proto::models::by_name;
use relabel_render::document::{AuthoredFor, Label};
use relabel_render::target::Dpmm;

/// The printer a new label is for when nothing says otherwise: the D110_M is the
/// printer relabel has been run against (PROTOCOL-NOTES.md, wire id 2320).
pub const DEFAULT_MODEL: &str = "D110_M";

/// The one roll whose length has been measured rather than read from the
/// registry: 40 by 12 mm on a D110_M (PROTOCOL-NOTES.md, "The label on this roll
/// is 40 mm x 12 mm"). Offered wherever the stock is 12 mm across.
const MEASURED_ROLL: (i32, i32) = (400, 120);

/// A printer, and where it came from.
#[derive(Debug, Clone, Copy)]
pub struct Printer {
    pub spec: &'static ModelSpec,
    /// True when nothing connected or open names a model this build can draw for,
    /// so the printer is [`DEFAULT_MODEL`] because something had to be.
    pub assumed: bool,
    /// True when it is the printer relabel is connected to.
    pub connected: bool,
    /// True when relabel is connected to a printer it cannot make a label for,
    /// so this is another one.
    pub connected_elsewhere: bool,
}

/// The printer relabel is connected to, as a new label sees it.
#[derive(Debug, Clone, Copy)]
pub struct Connected {
    /// Its model, when relabel can print on it: `None` for an id the registry
    /// does not know, or one that several models with different heads share.
    pub model: Option<&'static ModelSpec>,
}

/// What the New label pane hands back: the printer, the length along the feed in
/// tenths of a millimetre, and why the printer was assumed when it was, so the
/// status line says what the pane said.
///
/// `PartialEq` by hand: [`ModelSpec`] has none, and the registry's names are
/// unique, so the name is the model.
#[derive(Debug, Clone, Copy)]
pub struct NewSpec {
    pub spec: &'static ModelSpec,
    pub feed_tenths: i32,
    /// [`assumed_why`] for the printer the pane was opened for.
    pub assumed: Option<&'static str>,
}

impl PartialEq for NewSpec {
    fn eq(&self, other: &NewSpec) -> bool {
        self.spec.name == other.spec.name
            && self.feed_tenths == other.feed_tenths
            && self.assumed == other.assumed
    }
}

impl Eq for NewSpec {}

impl NewSpec {
    /// The registry's own default label for this printer, assumed for no reason.
    pub fn stock(spec: &'static ModelSpec) -> NewSpec {
        NewSpec {
            spec,
            feed_tenths: stock(spec).0,
            assumed: None,
        }
    }

    /// A label of `feed_tenths` for the printer the New label pane was opened for.
    pub fn of(p: &Printer, feed_tenths: i32) -> NewSpec {
        NewSpec {
            spec: p.spec,
            feed_tenths,
            assumed: assumed_why(p),
        }
    }
}

/// Why [`DEFAULT_MODEL`] is assumed when nothing is connected and nothing open
/// names a model this build can draw for.
pub const NOT_CONNECTED: &str = "relabel is not connected to a printer";

/// Why [`DEFAULT_MODEL`] is assumed when the printer connected is one this build
/// cannot make a label for.
const CANNOT_USE_CONNECTED: &str = "relabel cannot make a label for the printer it is connected to";

/// Why the printer was assumed, in the words that follow "assumed: ", or `None`
/// when it was not.
pub fn assumed_why(p: &Printer) -> Option<&'static str> {
    match (p.assumed, p.connected_elsewhere) {
        (false, _) => None,
        (true, true) => Some(CANNOT_USE_CONNECTED),
        (true, false) => Some(NOT_CONNECTED),
    }
}

/// The registry's entry for [`DEFAULT_MODEL`].
pub fn default_printer() -> &'static ModelSpec {
    // A build-time fact rather than a runtime one: the registry is generated
    // from vendor/devices.json, and a test pins that it holds a usable D110_M.
    by_name(DEFAULT_MODEL).expect("the vendored registry has a D110_M")
}

/// The printer a new label is for: the one relabel is connected to when this
/// build can draw for it, else the one the open label was made for when this
/// build can draw for that, otherwise [`DEFAULT_MODEL`], assumed.
pub fn printer_for(label: &Label, connected: Option<Connected>) -> Printer {
    let usable_connected = connected.and_then(|c| c.model).filter(|spec| usable(spec));
    if let Some(spec) = usable_connected {
        return Printer {
            spec,
            assumed: false,
            connected: true,
            connected_elsewhere: false,
        };
    }
    let connected_elsewhere = connected.is_some();
    let named = label
        .meta
        .authored_for
        .as_ref()
        .and_then(|a| a.model.as_deref())
        .and_then(by_name)
        .filter(|spec| usable(spec));
    match named {
        Some(spec) => Printer {
            spec,
            assumed: false,
            connected: false,
            connected_elsewhere,
        },
        None => Printer {
            spec: default_printer(),
            assumed: true,
            connected: false,
            connected_elsewhere,
        },
    }
}

/// Whether this build can make a correct canvas for the model: a head of whole
/// bytes, because rows are packed eight dots to a byte and the canvas must equal
/// the head exactly, and a label length the registry actually gives. 22 of the
/// 300-dpi heads are not whole bytes (the D11_H's is 142 dots).
pub fn usable(spec: &ModelSpec) -> bool {
    spec.printhead_px > 0 && spec.printhead_px.is_multiple_of(8) && spec.max_label_mm > 0
}

/// The registry's default label as (feed, head), in tenths of a millimetre.
///
/// The registry does not fix which of its two figures is which axis - the D110
/// lists (30, 12) and the A1 Pro (25, 38) - so the one nearer the print width is
/// the head and the other the feed; on a tie the first is the feed. The feed is
/// held between 1 mm and the longest label the printer takes.
pub fn stock(spec: &ModelSpec) -> (i32, i32) {
    let (a, b) = (
        i32::from(spec.default_label_mm.0),
        i32::from(spec.default_label_mm.1),
    );
    let width = i32::from(spec.print_width_mm);
    let (feed, head) = if (b - width).abs() <= (a - width).abs() {
        (a, b)
    } else {
        (b, a)
    };
    let longest = (i32::from(spec.max_label_mm) * 10).max(10);
    ((feed * 10).clamp(10, longest), head * 10)
}

/// The lengths along the feed the pane offers as buttons, in tenths, ascending:
/// the registry's default, and the measured 40 mm roll where the stock is 12 mm
/// across and the printer takes 40 mm. `devices.json` lists no rolls, so there is
/// nothing else to offer.
pub fn presets(spec: &ModelSpec) -> Vec<i32> {
    let (feed, head) = stock(spec);
    let mut out = vec![feed];
    if head == MEASURED_ROLL.1 && i32::from(spec.max_label_mm) * 10 >= MEASURED_ROLL.0 {
        out.push(MEASURED_ROLL.0);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// One preset button, as the pane shows and announces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The length along the feed, in tenths, which is also the token the button
    /// hands back.
    pub feed_tenths: i32,
    /// The face: `30 × 12 mm`.
    pub text: String,
    /// The accessible name, said in words a screen reader reads as words: `New 30
    /// by 12 millimetre label`.
    pub name: String,
    /// The accessible description: `240 by 96 dots`.
    pub says: String,
}

/// The pane's preset buttons, one for each of [`presets`].
pub fn offers(spec: &ModelSpec) -> Vec<Offer> {
    let head = stock(spec).1;
    let dpmm = Dpmm::for_dpi(spec.dpi);
    presets(spec)
        .into_iter()
        .map(|feed| Offer {
            feed_tenths: feed,
            text: format!("{} \u{d7} {} mm", mm(feed), mm(head)),
            name: format!("New {} by {} millimetre label", mm(feed), mm(head)),
            says: format!(
                "{} by {} dots",
                dpmm.dots_from_tenths_mm(feed),
                spec.printhead_px
            ),
        })
        .collect()
}

/// The line below the field: how long a label may be, and where to find the length
/// of a roll.
pub fn hint_says(spec: &ModelSpec) -> String {
    format!(
        "Up to {} mm. The length is printed on the roll's box; relabel-cli ruler prints a \
         ruler that measures a roll you do not know.",
        spec.max_label_mm
    )
}

/// A length along the feed as typed: millimetres, to a tenth, with an optional
/// `mm` and either decimal mark. The error is what the pane says.
pub fn parse_feed(text: &str, spec: &ModelSpec) -> Result<i32, String> {
    let unreadable = || "type the length along the feed in millimetres, such as 40".to_string();
    let text = text.trim();
    let number = match text.len().checked_sub(2) {
        Some(at) if text.is_char_boundary(at) && text[at..].eq_ignore_ascii_case("mm") => {
            text[..at].trim_end()
        }
        _ => text,
    };
    let (whole, tenth) = match number.split_once(['.', ',']) {
        Some((whole, tenth)) => (whole, Some(tenth)),
        None => (number, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || tenth.is_some_and(|t| !digits(t)) {
        return Err(unreadable());
    }
    if tenth.is_some_and(|t| t.len() > 1) {
        return Err("lengths go to a tenth of a millimetre, such as 40.5".into());
    }
    let longest = i32::from(spec.max_label_mm) * 10;
    let too_long = || {
        format!(
            "{} mm is the longest label the {} takes",
            spec.max_label_mm, spec.name
        )
    };
    // Digits only, so the one way the parse fails is a number too long for an
    // i32, which is too long a label as well.
    let whole: i32 = whole.parse().map_err(|_| too_long())?;
    let tenth = tenth.map_or(0, |t| i32::from(t.as_bytes()[0] - b'0'));
    let tenths = whole
        .checked_mul(10)
        .and_then(|t| t.checked_add(tenth))
        .ok_or_else(too_long)?;
    if tenths < 10 {
        return Err("a label has to be at least 1 mm long".into());
    }
    if tenths > longest {
        return Err(too_long());
    }
    Ok(tenths)
}

/// What a length along the feed comes to in dots, for the pane's live note.
pub fn feed_says(tenths: i32, spec: &ModelSpec) -> String {
    let dots = Dpmm::for_dpi(spec.dpi).dots_from_tenths_mm(tenths);
    format!("{} mm along the feed is {dots} dots", mm(tenths))
}

/// The head axis, which is not a choice: the canvas is the whole print head.
pub fn across_says(spec: &ModelSpec) -> String {
    format!(
        "Across the head: {} mm, {} dots, the whole print head",
        spec.print_width_mm, spec.printhead_px
    )
}

/// The feed field's accessible description: the printer and the head, which the
/// pane shows as plain lines that no key reaches, so a screen reader hears them
/// when the pane opens with the keyboard in the field.
pub fn about_says(p: &Printer) -> String {
    format!("{}. {}", printer_says(p), across_says(p.spec))
}

/// Which printer the pane is making a label for, and why that one.
pub fn printer_says(p: &Printer) -> String {
    if p.connected {
        format!("For the {} relabel is connected to", p.spec.name)
    } else if let Some(why) = assumed_why(p) {
        format!("For a {} (assumed: {why})", p.spec.name)
    } else {
        format!(
            "For a {}, the printer the open label was made for",
            p.spec.name
        )
    }
}

/// What the status line calls a label [`build`] has just made, saying the printer
/// was assumed, and why, when the New label pane said so.
pub fn made_says(n: &NewSpec) -> String {
    let mut says = format!(
        "new {} \u{d7} {} mm label for a {}",
        mm(n.feed_tenths),
        mm(stock(n.spec).1),
        n.spec.name
    );
    if let Some(why) = n.assumed {
        says.push_str(&format!(" (assumed: {why})"));
    }
    says
}

/// A blank label for a printer and a length along the feed.
///
/// The canvas is the whole head and the length in dots. The stock is recorded in
/// millimetres rather than derived back from the dots, which at 300 dpi would
/// record 11.5 mm for a 12 mm head. The model is recorded as well, so reopening
/// the file knows which way round the canvas goes without assuming.
pub fn build(n: &NewSpec) -> Label {
    let spec = n.spec;
    let dpmm = Dpmm::for_dpi(spec.dpi);
    let mut label = Label::new(
        dpmm.dots_from_tenths_mm(n.feed_tenths),
        i32::from(spec.printhead_px),
        dpmm,
    );
    label.media.feed_tenths_mm = n.feed_tenths;
    label.media.head_tenths_mm = stock(spec).1;
    label.meta.authored_for = Some(AuthoredFor {
        model: Some(spec.name.into()),
        head_px: spec.printhead_px,
        dpi: spec.dpi,
    });
    label
}

/// Tenths of a millimetre as the pane writes them: `40`, or `40.5`.
pub fn mm(tenths: i32) -> String {
    let sign = if tenths < 0 { "-" } else { "" };
    let t = tenths.unsigned_abs();
    match t % 10 {
        0 => format!("{sign}{}", t / 10),
        d => format!("{sign}{}.{d}", t / 10),
    }
}

/// The name Save as offers for a label that has none yet, from its stock:
/// `untitled-40x12.relabel`.
pub fn untitled_name(label: &Label) -> String {
    format!(
        "untitled-{}x{}.relabel",
        mm(label.media.feed_tenths_mm),
        mm(label.media.head_tenths_mm)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use relabel_proto::PrintDirection;
    use relabel_proto::models::MODELS;
    use relabel_render::document::Repeat;

    fn model(name: &str) -> &'static ModelSpec {
        by_name(name).expect("the model is in the registry")
    }

    fn d110() -> &'static ModelSpec {
        model("D110")
    }

    #[test]
    fn a_d110_40_mm_label_is_320_by_96_with_its_stock_and_model() {
        let l = build(&NewSpec {
            spec: d110(),
            feed_tenths: 400,
            assumed: None,
        });
        assert_eq!((l.canvas.feed, l.canvas.head), (320, 96));
        assert_eq!(l.canvas.dpmm, Dpmm::DPI_203);
        assert_eq!((l.media.feed_tenths_mm, l.media.head_tenths_mm), (400, 120));
        assert_eq!(
            l.meta.authored_for,
            Some(AuthoredFor {
                model: Some("D110".into()),
                head_px: 96,
                dpi: 203,
            })
        );
        assert!(l.media.tail.is_none() && l.media.fold.is_none());
        assert_eq!(l.repeat, Repeat::None);
        assert!(l.meta.title.is_empty());
        assert!(l.elements.is_empty() && l.data.is_none());
    }

    /// The model is recorded so the file knows its own orientation: reopened with
    /// the other direction as the fallback, it still turns the right way and does
    /// not say it assumed.
    #[test]
    fn a_new_label_reopens_knowing_its_direction() {
        let l = build(&NewSpec {
            spec: d110(),
            feed_tenths: 400,
            assumed: None,
        });
        let text = l.to_json().expect("a new label saves");
        let back = Label::from_json(&text).expect("and loads");
        let doc = Doc::open(back, PrintDirection::Top).expect("a printable label");
        assert_eq!(
            doc.orientation_note(),
            "the head runs down the side, so the feed is across the canvas"
        );
    }

    #[test]
    fn the_printer_follows_the_open_label_and_otherwise_assumes_a_d110_m() {
        let mut l = Label::new(320, 96, Dpmm::DPI_203);
        let p = printer_for(&l, None);
        assert_eq!(
            (p.spec.name, p.assumed, p.connected),
            ("D110_M", true, false)
        );
        for (named, want, assumed) in [
            ("B18", "B18", false),
            ("D110", "D110", false),
            // 142 dots is not a whole number of bytes.
            ("D11_H", "D110_M", true),
            ("nonesuch", "D110_M", true),
        ] {
            l.meta.authored_for = Some(AuthoredFor {
                model: Some(named.into()),
                head_px: 96,
                dpi: 203,
            });
            let p = printer_for(&l, None);
            assert_eq!((p.spec.name, p.assumed), (want, assumed), "{named}");
            assert!(!p.connected, "{named}");
        }
    }

    /// The author's printer, which a new label is for when nothing says
    /// otherwise: whole bytes of head, the landscape strip, and the measured roll.
    #[test]
    fn the_default_printer_is_a_usable_d110_m() {
        let spec = default_printer();
        assert_eq!(spec.name, "D110_M");
        assert!(usable(spec));
        assert_eq!(
            (spec.printhead_px, spec.dpi, spec.direction),
            (96, 203, PrintDirection::Left)
        );
        assert_eq!(stock(spec), (300, 120));
        assert_eq!(presets(spec), [300, 400]);
        assert_eq!(spec.max_label_mm, 100);
    }

    #[test]
    fn printer_for_prefers_a_usable_connected_model() {
        let mut l = Label::new(320, 96, Dpmm::DPI_203);
        l.meta.authored_for = Some(AuthoredFor {
            model: Some("B18".into()),
            head_px: 96,
            dpi: 203,
        });
        let p = printer_for(&l, Some(on(model("D110_M"))));
        assert_eq!(
            (p.spec.name, p.assumed, p.connected),
            ("D110_M", false, true)
        );
        assert_eq!(printer_says(&p), "For the D110_M relabel is connected to");
        assert_eq!(
            about_says(&p),
            "For the D110_M relabel is connected to. Across the head: 12 mm, 96 dots, the \
             whole print head"
        );

        // 142 dots is not a whole number of bytes, so the label's own model is
        // used, and then the default.
        let p = printer_for(&l, Some(on(model("D11_H"))));
        assert_eq!((p.spec.name, p.assumed, p.connected), ("B18", false, false));
        assert_eq!(
            printer_says(&p),
            "For a B18, the printer the open label was made for"
        );
        let blank = Label::new(320, 96, Dpmm::DPI_203);
        let p = printer_for(&blank, Some(on(model("D11_H"))));
        assert_eq!(
            (p.spec.name, p.assumed, p.connected),
            ("D110_M", true, false)
        );
        // Connected, so the pane does not say that nothing is.
        let unusable = "For a D110_M (assumed: relabel cannot make a label for the printer it \
                        is connected to)";
        assert_eq!(printer_says(&p), unusable);
        // An id the registry does not know is the same.
        let p = printer_for(&blank, Some(Connected { model: None }));
        assert_eq!((p.spec.name, p.assumed), ("D110_M", true));
        assert_eq!(printer_says(&p), unusable);
    }

    fn on(spec: &'static ModelSpec) -> Connected {
        Connected { model: Some(spec) }
    }

    #[test]
    fn feed_lengths_parse_in_millimetres_to_a_tenth() {
        let d110 = d110();
        for (typed, tenths) in [
            ("40", 400),
            ("40.5", 405),
            ("40,5", 405),
            (" 40 mm ", 400),
            ("40mm", 400),
            ("40 MM", 400),
            ("100", 1000),
            ("1", 10),
        ] {
            assert_eq!(parse_feed(typed, d110), Ok(tenths), "{typed:?}");
        }
        assert_eq!(
            parse_feed("101", d110),
            Err("100 mm is the longest label the D110 takes".into())
        );
        assert_eq!(
            parse_feed("99999999999", d110),
            Err("100 mm is the longest label the D110 takes".into())
        );
        for typed in ["0.9", "0"] {
            assert_eq!(
                parse_feed(typed, d110),
                Err("a label has to be at least 1 mm long".into()),
                "{typed:?}"
            );
        }
        // The one thing wrong is said, rather than the length being unreadable:
        // the pane's note is where the operator hears what to change.
        for typed in ["40.55", "40,55 mm", "40.123"] {
            assert_eq!(
                parse_feed(typed, d110),
                Err("lengths go to a tenth of a millimetre, such as 40.5".into()),
                "{typed:?}"
            );
        }
        for typed in [
            "-3", "", "abc", "40.", ".5", "mm", "4 0", "٤٠", "40.5.1", "40.5x",
        ] {
            let e = parse_feed(typed, d110).expect_err(typed);
            assert!(
                e.starts_with("type the length along the feed"),
                "{typed:?}: {e}"
            );
        }
    }

    #[test]
    fn the_stock_splits_the_registry_default_by_the_print_width() {
        assert_eq!(stock(d110()), (300, 120));
        assert_eq!(stock(model("B1")), (300, 500));
        assert_eq!(stock(model("B21")), (300, 500));
        assert_eq!(stock(model("A1 Pro")), (380, 250));
        assert_eq!(stock(model("Hi-NB-D11")), (300, 120));
        for spec in MODELS.iter().filter(|m| usable(m)) {
            let (feed, _) = stock(spec);
            assert!(
                (10..=i32::from(spec.max_label_mm) * 10).contains(&feed),
                "{}: {feed}",
                spec.name
            );
        }
    }

    #[test]
    fn presets_are_the_registry_default_and_the_measured_roll() {
        assert_eq!(presets(d110()), [300, 400]);
        assert_eq!(presets(model("B18")), [300, 400]);
        assert_eq!(presets(model("B1")), [300]);
        assert_eq!(feed_says(400, d110()), "40 mm along the feed is 320 dots");
        assert_eq!(feed_says(405, d110()), "40.5 mm along the feed is 324 dots");
        assert_eq!(
            across_says(d110()),
            "Across the head: 12 mm, 96 dots, the whole print head"
        );
    }

    /// The buttons say their size in both units, and in words for a screen reader,
    /// and each hands back the length it makes.
    #[test]
    fn the_preset_buttons_name_their_size_and_their_dots() {
        let offers = offers(d110());
        assert_eq!(
            offers,
            [
                Offer {
                    feed_tenths: 300,
                    text: "30 \u{d7} 12 mm".into(),
                    name: "New 30 by 12 millimetre label".into(),
                    says: "240 by 96 dots".into(),
                },
                Offer {
                    feed_tenths: 400,
                    text: "40 \u{d7} 12 mm".into(),
                    name: "New 40 by 12 millimetre label".into(),
                    says: "320 by 96 dots".into(),
                },
            ]
        );
        assert_eq!(
            hint_says(d110()),
            "Up to 100 mm. The length is printed on the roll's box; relabel-cli ruler prints \
             a ruler that measures a roll you do not know."
        );
    }

    #[test]
    fn the_pane_says_which_printer_and_why() {
        let assumed = printer_for(&Label::new(320, 96, Dpmm::DPI_203), None);
        assert_eq!(
            printer_says(&assumed),
            "For a D110_M (assumed: relabel is not connected to a printer)"
        );
        let known = Printer {
            spec: model("B18"),
            assumed: false,
            connected: false,
            connected_elsewhere: false,
        };
        assert_eq!(
            printer_says(&known),
            "For a B18, the printer the open label was made for"
        );
        assert_eq!(
            made_says(&NewSpec {
                spec: d110(),
                feed_tenths: 405,
                assumed: None,
            }),
            "new 40.5 \u{d7} 12 mm label for a D110"
        );
        assert_eq!(NewSpec::stock(d110()).feed_tenths, 300);
        // The status line says the printer was assumed, and why, as the pane did.
        assert_eq!(
            made_says(&NewSpec::of(&assumed, 300)),
            "new 30 \u{d7} 12 mm label for a D110_M (assumed: relabel is not connected to a \
             printer)"
        );
        let elsewhere = Printer {
            connected_elsewhere: true,
            ..assumed
        };
        assert_eq!(
            made_says(&NewSpec::of(&elsewhere, 300)),
            "new 30 \u{d7} 12 mm label for a D110_M (assumed: relabel cannot make a label for \
             the printer it is connected to)"
        );
        assert!(!made_says(&NewSpec::of(&known, 300)).contains("assumed"));
    }

    #[test]
    fn the_untitled_name_says_the_stock() {
        for (feed, name) in [
            (400, "untitled-40x12.relabel"),
            (405, "untitled-40.5x12.relabel"),
        ] {
            let l = build(&NewSpec {
                spec: d110(),
                feed_tenths: feed,
                assumed: None,
            });
            assert_eq!(untitled_name(&l), name);
        }
        assert_eq!(mm(-5), "-0.5");
    }
}
