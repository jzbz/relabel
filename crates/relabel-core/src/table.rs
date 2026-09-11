//! Reading a CSV file into the rows a label is bound to.
//!
//! Hand-written, with no dependency, to the rules spreadsheets export by and
//! d3 - which niimblue uses - reads by, except where d3 prints a wrong label
//! without a word:
//!
//! - A quote is special only at the start of a field, and `""` inside quotes
//!   is one quote. After the closing quote only the delimiter or the end of
//!   the line may follow; d3 drops the character that does, and the field
//!   reads as something other than what was written.
//! - A quote opened and never closed is an error, not a field running to the
//!   end of the file.
//! - Blank lines are skipped, as the `csv` crate skips them: d3 reads one as a
//!   row of empty fields, which prints a blank label.
//! - The file is UTF-8, with or without a byte-order mark. UTF-16, which is
//!   what a spreadsheet's "Unicode text" writes, is refused by name.
//!
//! LF, CR and CRLF each end a record, and are kept as they are inside quotes.
//! A row of the wrong length is returned as it is: binding refuses it, where
//! it can say which row it was.

/// A CSV file's header and rows, verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CsvTable {
    /// The header's fields, or `"1"` to `"n"` for a file without one, `n`
    /// being the length of its first row.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// The line each row starts on, counted from 1, for an error to point at.
    pub lines: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsvError {
    #[error("the file is UTF-16; save it as CSV UTF-8")]
    Utf16,
    #[error("line {line} is not UTF-8; save the file as CSV UTF-8")]
    NotUtf8 { line: u32 },
    #[error("{0:?} cannot separate fields")]
    BadDelimiter(char),
    #[error(
        "line {line}: a quoted field has more text after its closing quote; a quote inside a quoted field is written twice"
    )]
    StrayQuote { line: u32 },
    #[error("line {line}: a quote is opened and never closed")]
    Unterminated { line: u32 },
}

/// Reads `bytes` as CSV separated by `delimiter`, taking the first row as the
/// header when `has_header` is set.
pub fn read_csv(bytes: &[u8], delimiter: char, has_header: bool) -> Result<CsvTable, CsvError> {
    if matches!(delimiter, '"' | '\r' | '\n') {
        return Err(CsvError::BadDelimiter(delimiter));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(CsvError::Utf16);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| CsvError::NotUtf8 {
        line: line_of(&bytes[..e.valid_up_to()]),
    })?;
    let mut parser = Parser {
        chars: text.chars().peekable(),
        delimiter,
        line: 1,
    };
    let mut table = CsvTable::default();
    let mut header = has_header;
    while let Some((line, fields)) = parser.record()? {
        if header {
            table.columns = fields;
            header = false;
            continue;
        }
        if !has_header && table.rows.is_empty() {
            table.columns = (1..=fields.len()).map(|n| n.to_string()).collect();
        }
        table.rows.push(fields);
        table.lines.push(line);
    }
    Ok(table)
}

/// The line the end of `bytes` is on, counting LF, CR and CRLF as one line end
/// each.
fn line_of(bytes: &[u8]) -> u32 {
    let mut line = 1u32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => line += 1,
            b'\r' if bytes.get(i + 1) != Some(&b'\n') => line += 1,
            _ => {}
        }
        i += 1;
    }
    line
}

enum End {
    Delimiter,
    Line,
    Input,
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    delimiter: char,
    line: u32,
}

impl Parser<'_> {
    /// The next record and the line it starts on, blank lines skipped; `None`
    /// at the end of the input.
    fn record(&mut self) -> Result<Option<(u32, Vec<String>)>, CsvError> {
        loop {
            match self.chars.peek() {
                None => return Ok(None),
                Some('\r' | '\n') => self.line_end(),
                Some(_) => break,
            }
        }
        let start = self.line;
        let mut fields = Vec::new();
        loop {
            let (field, end) = self.field()?;
            fields.push(field);
            match end {
                End::Delimiter => {}
                End::Line | End::Input => return Ok(Some((start, fields))),
            }
        }
    }

    /// Consumes one LF, CR or CRLF.
    fn line_end(&mut self) {
        if self.chars.next() == Some('\r') && self.chars.peek() == Some(&'\n') {
            self.chars.next();
        }
        self.line += 1;
    }

    /// One field, and what ended it.
    fn field(&mut self) -> Result<(String, End), CsvError> {
        let mut s = String::new();
        if self.chars.peek() == Some(&'"') {
            let open = self.line;
            self.chars.next();
            loop {
                match self.chars.next() {
                    None => return Err(CsvError::Unterminated { line: open }),
                    Some('"') if self.chars.peek() == Some(&'"') => {
                        self.chars.next();
                        s.push('"');
                    }
                    Some('"') => break,
                    Some('\r') => {
                        s.push('\r');
                        if self.chars.peek() == Some(&'\n') {
                            self.chars.next();
                            s.push('\n');
                        }
                        self.line += 1;
                    }
                    Some('\n') => {
                        s.push('\n');
                        self.line += 1;
                    }
                    Some(c) => s.push(c),
                }
            }
            return match self.chars.peek() {
                None => Ok((s, End::Input)),
                Some('\r' | '\n') => {
                    self.line_end();
                    Ok((s, End::Line))
                }
                Some(&c) if c == self.delimiter => {
                    self.chars.next();
                    Ok((s, End::Delimiter))
                }
                Some(_) => Err(CsvError::StrayQuote { line: self.line }),
            };
        }
        loop {
            match self.chars.peek() {
                None => return Ok((s, End::Input)),
                Some('\r' | '\n') => {
                    self.line_end();
                    return Ok((s, End::Line));
                }
                Some(&c) if c == self.delimiter => {
                    self.chars.next();
                    return Ok((s, End::Delimiter));
                }
                Some(&c) => {
                    s.push(c);
                    self.chars.next();
                }
            }
        }
    }
}
