//! `parse_say` takes the number after the FIRST occurrence of "feed"/"head"
//! anywhere in the string - and the element's own name comes first in the
//! sentence `say` builds. A name containing either axis word captures the
//! parse.

use relabel_gui::a11y::{parse_say, say};
use relabel_render::document::RectDots;

fn frame(feed: i32, head: i32, feed_len: i32, head_len: i32) -> RectDots {
    RectDots {
        feed,
        head,
        feed_len,
        head_len,
    }
}

#[test]
fn a_name_containing_an_axis_word_round_trips() {
    let f = frame(12, 4, 96, 24);
    for name in [
        "SKU",
        "header",
        "feed rate",
        "feed 99",
        "head 7",
        "overhead label",
    ] {
        let sentence = say("Text", name, f, false, false, None);
        let got = parse_say(&sentence);
        println!("{:<16} -> {:?}   from {sentence:?}", name, got);
    }
    // The property the design asserts: what it says is what it reads back.
    for name in [
        "SKU",
        "header",
        "feed rate",
        "feed 99",
        "head 7",
        "overhead label",
    ] {
        let sentence = say("Text", name, f, false, false, None);
        assert_eq!(
            parse_say(&sentence),
            Some((12, 4)),
            "the sentence for an element named {name:?} did not read back as its own placement"
        );
    }
}

/// A name may contain the quote marks the sentence wraps it in. Cutting to the
/// first closing quote left the rest of the name to be read as the placement,
/// so an element could be named into lying about where it is.
#[test]
fn a_name_containing_the_quote_marks_cannot_hijack_the_parse() {
    let f = frame(12, 4, 96, 24);
    for name in [
        "a\u{2019} feed 99 head 88 x",
        "\u{2018}quoted\u{2019}",
        "\u{2019}\u{2019}\u{2019} feed 7",
        "plain",
    ] {
        let said = say("Text", name, f, false, false, None);
        assert_eq!(
            parse_say(&said),
            Some((12, 4)),
            "the sentence for {name:?} read back as something else: {said}"
        );
    }
}
