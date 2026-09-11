//! The CSV reader, against the cases where spreadsheets, d3 and the `csv`
//! crate disagree.

use relabel_core::table::{CsvError, CsvTable, read_csv};

fn read(s: &str) -> CsvTable {
    read_csv(s.as_bytes(), ',', true).unwrap()
}

fn rows(t: &CsvTable) -> Vec<Vec<&str>> {
    t.rows
        .iter()
        .map(|r| r.iter().map(String::as_str).collect())
        .collect()
}

#[test]
fn quoted_fields_hold_delimiters_and_doubled_quotes() {
    let t = read("a,b\n\"x,y\",\"say \"\"hi\"\"\"");
    assert_eq!(t.columns, ["a", "b"]);
    assert_eq!(rows(&t), [["x,y", "say \"hi\""]]);
}

/// The reader does not judge a row's length: binding does, and names the row.
#[test]
fn ragged_rows_are_returned_as_they_are() {
    assert_eq!(rows(&read("a,b\n1")), [vec!["1"]]);
    assert_eq!(rows(&read("a,b\n1,2,3")), [vec!["1", "2", "3"]]);
}

/// d3 reads the blank line as a row, which prints a blank label.
#[test]
fn blank_lines_are_skipped() {
    let t = read("a,b\n1,2\n\n");
    assert_eq!(rows(&t), [["1", "2"]]);
    let t = read("\n\na,b\r\n\r\n1,2\r\r3,4");
    assert_eq!(rows(&t), [["1", "2"], ["3", "4"]]);
    assert_eq!(t.lines, [5, 7]);
}

#[test]
fn a_byte_order_mark_is_not_part_of_the_first_column() {
    assert_eq!(read("\u{feff}sku,x\n1,2").columns, ["sku", "x"]);
}

#[test]
fn a_quote_is_either_closed_and_followed_by_a_delimiter_or_an_error() {
    assert_eq!(
        read_csv(b"\"ab\"cd,e", ',', true),
        Err(CsvError::StrayQuote { line: 1 })
    );
    assert_eq!(
        read_csv(b"\"open", ',', true),
        Err(CsvError::Unterminated { line: 1 })
    );
    assert_eq!(
        read_csv(b"h\n1\n\"two\nlines\" x", ',', true),
        Err(CsvError::StrayQuote { line: 4 })
    );
    assert_eq!(
        read_csv(b"h\n1\n\"never\nclosed", ',', true),
        Err(CsvError::Unterminated { line: 3 })
    );
}

#[test]
fn only_utf8_is_read() {
    assert_eq!(read_csv(b"\xff\xfeh\0", ',', true), Err(CsvError::Utf16));
    assert_eq!(read_csv(b"\xfe\xff\0h", ',', true), Err(CsvError::Utf16));
    assert_eq!(
        read_csv(b"a\n\xff", ',', true),
        Err(CsvError::NotUtf8 { line: 2 })
    );
    assert_eq!(
        read_csv(b"a\r\nb\rc\n\"d\re\"\x80", ',', true),
        Err(CsvError::NotUtf8 { line: 5 })
    );
}

/// LF, CR and CRLF each end one record.
#[test]
fn every_line_end_ends_a_record() {
    let t = read("h\r\nb\rc\n");
    assert_eq!(rows(&t), [["b"], ["c"]]);
    assert_eq!(t.lines, [2, 3]);
}

/// Inside quotes a line end is part of the field, byte for byte, and the rows
/// after it still know their line.
#[test]
fn a_quoted_line_end_is_kept_and_counted() {
    let t = read("h\n\"l1\r\nl2\"\nz");
    assert_eq!(rows(&t), [["l1\r\nl2"], ["z"]]);
    assert_eq!(t.lines, [2, 4]);
}

#[test]
fn a_quote_after_the_start_is_text() {
    assert_eq!(rows(&read("h\nx\"y")), [["x\"y"]]);
    assert_eq!(rows(&read("h\n \"y\"")), [[" \"y\""]]);
}

#[test]
fn without_a_header_the_columns_are_numbered() {
    let t = read_csv(b"h1,h2\n3,4", ',', false).unwrap();
    assert_eq!(t.columns, ["1", "2"]);
    assert_eq!(rows(&t), [["h1", "h2"], ["3", "4"]]);
    assert_eq!(t.lines, [1, 2]);
    assert_eq!(read_csv(b"", ',', false).unwrap(), CsvTable::default());
}

#[test]
fn an_empty_field_is_kept_wherever_it_falls() {
    assert_eq!(
        rows(&read("a,b,c\n,,\n1,\"\",")),
        [["", "", ""], ["1", "", ""]]
    );
}

#[test]
fn other_delimiters() {
    let t = read_csv(b"a;b\n1,5;2", ';', true).unwrap();
    assert_eq!(rows(&t), [["1,5", "2"]]);
    let t = read_csv(b"a\tb\n1\t2", '\t', true).unwrap();
    assert_eq!(rows(&t), [["1", "2"]]);
    let t = read_csv("a§b\n1§2".as_bytes(), '§', true).unwrap();
    assert_eq!(rows(&t), [["1", "2"]]);
    for bad in ['"', '\r', '\n'] {
        assert_eq!(read_csv(b"a", bad, true), Err(CsvError::BadDelimiter(bad)));
    }
}

/// The frozen CSV beside the variable-data corpus reads as the table
/// relabel-render's preflight tests write out by hand.
#[test]
fn the_corpus_csv_reads_as_the_preflight_literal() {
    let t = read_csv(
        include_bytes!("../../relabel-render/tests/corpus/v1/variables.csv"),
        ',',
        true,
    )
    .unwrap();
    assert_eq!(t.columns, ["sku", "name", "$times"]);
    assert_eq!(
        rows(&t),
        [
            ["SKU-0412", "Widget", "2"],
            ["SKU-0413", "Gadget", ""],
            ["SKU-0414-EXTRA-LONG", "Gizmo", "1"],
            ["SKU-0415", "Doohickey", "3"],
            ["SKU-0416", "Spare", "0"],
        ]
    );
    assert_eq!(t.lines, [2, 3, 4, 5, 6]);
}

/// A megabyte of quotes, or of one unclosed field, reads in one pass.
#[test]
fn hostile_input_is_linear() {
    let start = std::time::Instant::now();
    let quotes = format!("h\n\"{}\"", "\"\"".repeat(500_000));
    assert_eq!(read(&quotes).rows[0][0].len(), 500_000);
    let open = format!("h\n\"{}", "a,\n".repeat(350_000));
    assert!(matches!(
        read_csv(open.as_bytes(), ',', true),
        Err(CsvError::Unterminated { line: 2 })
    ));
    assert!(start.elapsed().as_secs() < 2, "{:?}", start.elapsed());
}
