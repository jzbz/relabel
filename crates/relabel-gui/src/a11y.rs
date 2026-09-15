//! What the canvas tells an assistive technology about one element, and how it
//! reads an answer back.
//!
//! Both halves are here, in Rust, for one reason: they are a format and its
//! parser, and the place they go wrong is drifting apart. Slint can build the
//! string - it converts an int to a string implicitly - and cannot parse one
//! back beyond `to-float`, so half of the pair would have to be here anyway.
//!
//! The sentence speaks the document's own axes, feed and head, not the
//! canvas's x and y. Those are the names in the file, in the inspector, in
//! preflight's messages and in the command line's output, and a reader who
//! moves an element to "feed 12" has to find the same 12 in the report that
//! refuses it. The arrow keys are the one spatial exception, because an arrow
//! key is spatial; the sentence says which document axis moved.

use relabel_render::document::RectDots;
use relabel_render::media::Keepout;

/// The sentence, in the order a reader needs it: what the element is, where it
/// is, how big it is, and then only what is wrong with it.
pub fn say(
    kind: &str,
    name: &str,
    f: RectDots,
    locked: bool,
    off_canvas: bool,
    keepout: Option<Keepout>,
) -> String {
    let mut s = String::new();
    s.push_str(kind);
    if !name.is_empty() {
        s.push_str(" \u{2018}");
        // The two quote marks are the name's delimiters in this sentence, so a
        // name carrying one is spoken with a plain apostrophe in its place.
        // `parse_say` already cuts to the last closing quote and reads such a
        // sentence back correctly without this; neutralising keeps the marks
        // meaning one thing to whatever else reads the sentence - a reader who
        // hears punctuation, or a second parser that cuts at the first quote.
        // Only the sentence changes: the file keeps the name as it was typed.
        s.extend(name.chars().map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            c => c,
        }));
        s.push('\u{2019}');
    }
    s.push_str(&format!(
        ", feed {}, head {}, {} by {} dots",
        f.feed, f.head, f.feed_len, f.head_len
    ));
    if locked {
        s.push_str(", locked");
    }
    if off_canvas {
        s.push_str(", off the label");
    }
    if let Some(k) = keepout {
        s.push_str(", ");
        s.push_str(keepout_phrase(k));
    }
    s
}

/// Said aloud, so it is a phrase rather than a code. The four are
/// [`Keepout`]'s own order of how much the operator loses.
pub(crate) fn keepout_phrase(k: Keepout) -> &'static str {
    match k {
        Keepout::Repeated => "a repeated panel prints over it",
        Keepout::Tail => "in the tail, which the fold hides",
        Keepout::Inset => "outside the printable area",
        Keepout::Shape => "outside the die-cut",
    }
}

/// A placement read back out of a value an assistive technology wrote, as
/// `(feed, head)`.
///
/// Deliberately tolerant about the spelling and strict about the content.
/// Tolerant, because the prompt an assistive technology puts up is prefilled
/// with the whole sentence and the user may send back any of `feed 12, head 4`,
/// `12, 4`, `12 4` or the edited sentence itself; refusing the short forms
/// would be hostile to the only keyboard path this slice ships. Strict,
/// because a value with one number in it, or none, is a typo and not an
/// instruction: `None` leaves the element exactly where it was, which is the
/// house rule about not guessing.
pub fn parse_say(s: &str) -> Option<(i32, i32)> {
    // The element's own name comes first in the sentence, and it is quoted.
    // Cut it out before reading any number: a name like "header" or "feed 99"
    // would otherwise capture the search for the axis word that follows it,
    // and the sentence would not read back as the placement it states.
    // To the LAST closing quote, not the first: everything this function
    // appends after the name - the axes, the size, the keep-out - carries no
    // quote mark, so the last one in the sentence is always the name's. A name
    // holding a quote of its own would otherwise close it early and leave its
    // own digits to be read as the placement.
    let without_name = match (s.find('\u{2018}'), s.rfind('\u{2019}')) {
        (Some(open), Some(close)) if open < close => {
            let mut t = String::with_capacity(s.len());
            t.push_str(&s[..open]);
            t.push_str(&s[close + '\u{2019}'.len_utf8()..]);
            t
        }
        _ => s.to_string(),
    };
    let s = without_name.as_str();
    if let (Some(feed), Some(head)) = (labelled(s, "feed"), labelled(s, "head")) {
        return Some((feed, head));
    }
    let mut numbers = s
        .split(|c: char| !(c.is_ascii_digit() || c == '-'))
        .filter(|t| !t.is_empty())
        .map(str::parse::<i32>);
    match (numbers.next(), numbers.next()) {
        (Some(Ok(feed)), Some(Ok(head))) => Some((feed, head)),
        _ => None,
    }
}

/// The number after a word, wherever in the string that word appears.
fn labelled(s: &str, word: &str) -> Option<i32> {
    let at = s.to_ascii_lowercase().find(word)? + word.len();
    s[at..]
        .split(|c: char| !(c.is_ascii_digit() || c == '-'))
        .find(|t| !t.is_empty())?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
        RectDots {
            feed,
            head,
            feed_len,
            head_len,
        }
    }

    #[test]
    fn an_element_says_what_it_is_then_where_it_is_then_what_is_wrong() {
        assert_eq!(
            say("Text", "SKU", frame(12, 4, 96, 24), false, false, None),
            "Text \u{2018}SKU\u{2019}, feed 12, head 4, 96 by 24 dots"
        );
        assert_eq!(
            say(
                "Barcode",
                "",
                frame(0, 0, 8, 8),
                true,
                false,
                Some(Keepout::Inset)
            ),
            "Barcode, feed 0, head 0, 8 by 8 dots, locked, outside the printable area"
        );
    }

    #[test]
    fn the_sentence_it_says_is_one_it_can_read_back() {
        let f = frame(-3, 40, 96, 24);
        let said = say("Text", "Part no.", f, false, false, Some(Keepout::Tail));
        assert_eq!(parse_say(&said), Some((-3, 40)));
    }

    #[test]
    fn the_short_spellings_a_reader_is_likely_to_type_are_all_accepted() {
        for s in ["feed 12, head 4", "12,4", "12, 4", "12 4", "FEED 12 HEAD 4"] {
            assert_eq!(parse_say(s), Some((12, 4)), "{s:?}");
        }
    }

    #[test]
    fn a_value_with_one_number_or_none_moves_nothing() {
        for s in ["", "12", "feed 12", "somewhere over there", "head 4"] {
            assert_eq!(parse_say(s), None, "{s:?}");
        }
    }

    /// The words win over the order, so an element whose *name* is a number
    /// does not shift the parse.
    #[test]
    fn a_named_axis_is_read_in_preference_to_the_first_two_numbers() {
        let said = say("Text", "2024", frame(12, 4, 96, 24), false, false, None);
        assert_eq!(parse_say(&said), Some((12, 4)));
    }

    /// Off the label is its own fault. `Bounds::keepout` reads any off-canvas
    /// frame as the tail, which would send the operator looking for a fold.
    #[test]
    fn an_element_off_the_label_says_so_rather_than_naming_a_keep_out() {
        let said = say("Text", "SKU", frame(-40, 4, 96, 24), false, true, None);
        assert!(said.ends_with(", off the label"), "{said}");
        assert!(!said.contains("tail"), "{said}");
    }

    /// A name field makes any name reachable, including one written to look like
    /// the end of the name and the start of a placement.
    #[test]
    fn a_name_carrying_the_sentences_own_quote_marks_still_reads_back_as_its_placement() {
        let f = frame(5, 6, 96, 24);
        for name in [
            "\u{2019}, feed 77, head 88",
            "\u{2018}feed 1 head 2\u{2019}",
            "Joe\u{2019}s \u{2018}best\u{2019}",
        ] {
            let said = say("Text", name, f, false, false, None);
            assert_eq!(parse_say(&said), Some((5, 6)), "{said}");
            assert_eq!(said.matches('\u{2018}').count(), 1, "{said}");
            assert_eq!(said.matches('\u{2019}').count(), 1, "{said}");
        }
    }

    /// The sentence is a rendering of the name, not the name: what is saved is
    /// what the operator typed, quote marks and all.
    #[test]
    fn the_file_keeps_the_name_the_operator_typed_even_when_the_sentence_cannot() {
        use relabel_render::document::{Common, Element, ElementOrRaw, Label, RectEl};
        use relabel_render::target::Dpmm;
        let typed = "Joe\u{2019}s \u{2018}best\u{2019}";
        let mut common = Common::new(1, frame(5, 6, 40, 24));
        common.name = typed.to_string();
        let mut label = Label::new(320, 96, Dpmm::DPI_203);
        label.elements = vec![ElementOrRaw::Known(Element::Rect(RectEl {
            common,
            stroke_dots: 0,
            radius_dots: 0,
        }))];
        let saved = label.to_json().expect("a writable label");
        let back = Label::from_json(&saved).expect("a readable label");
        let name = &back.elements[0].known().expect("a rectangle").common().name;
        assert_eq!(name, typed);
        let said = say("Rectangle", name, frame(5, 6, 40, 24), false, false, None);
        assert!(said.contains("Joe's 'best'"), "{said}");
        assert!(!said.contains(typed), "{said}");
    }
}
