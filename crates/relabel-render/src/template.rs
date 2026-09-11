//! Placeholders: niimblue's grammar, exactly.
//!
//! niimblue substitutes `{key}` and `{key|filter}` with one JavaScript regular
//! expression, `/{\s*(\$?\w+)\s*(?:\|\s*(.*?)\s*)?}/g`. A label brought over
//! from it has to resolve the same way, and three of that expression's pieces
//! mean something different in Rust:
//!
//! - JavaScript's `\s` includes U+FEFF and not U+0085, where Rust's
//!   `char::is_whitespace` is the other way round;
//! - without the `u` flag `\w` is ASCII: `[A-Za-z0-9_]`, so `{skú}` never
//!   matches;
//! - `.` stops only at `\n`, `\r`, U+2028 and U+2029, so a filter can hold any
//!   other character, `{` and `|` included, but never crosses a line.
//!
//! So this is a scanner, not a regex. It finds the same matches, in the same
//! places, with the same key and filter - checked against a backtracking
//! transliteration of the expression in the tests - and it runs in linear time,
//! where the expression backtracks quadratically on a run of unclosed `{a|`.
//!
//! Matches never overlap, the scan runs left to right, and text substituted in
//! is never scanned again.

use std::borrow::Cow;
use std::ops::Range;

/// JavaScript's `\s`.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// What JavaScript's `.` refuses to match.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// JavaScript's `\w` without the `u` flag.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// One `{key}` or `{key|filter}` in a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder<'a> {
    /// The whole placeholder, braces included, in bytes.
    pub range: Range<usize>,
    /// The key, with its leading `$` if it has one.
    pub key: &'a str,
    /// `None` for no `|`; `Some("")` for an empty filter.
    pub filter: Option<&'a str>,
}

/// The placeholders in a template, in order.
pub fn placeholders(s: &str) -> Placeholders<'_> {
    Placeholders {
        s,
        pos: 0,
        close: Cursor::default(),
        line: Cursor::default(),
        after_line: Cursor::default(),
    }
}

/// A search that only ever moves forward: the first position at or after
/// `from` that satisfies a test, remembered, so later searches from a later
/// position start where the last one stopped.
#[derive(Default, Clone, Copy)]
struct Cursor {
    from: usize,
    found: Option<usize>,
    /// Whether `found` holds a completed search from `from`.
    valid: bool,
}

impl Cursor {
    fn first(&mut self, s: &str, from: usize, test: impl Fn(char) -> bool) -> Option<usize> {
        if self.valid && from >= self.from && self.found.is_none_or(|f| f >= from) {
            return self.found;
        }
        let start = if self.valid && from >= self.from {
            // Everything before the old answer was already rejected.
            self.found.map_or(s.len(), |f| f.max(from))
        } else {
            from
        };
        let found = s[start..]
            .char_indices()
            .find(|&(_, c)| test(c))
            .map(|(i, _)| start + i);
        *self = Cursor {
            from,
            found,
            valid: true,
        };
        found
    }
}

pub struct Placeholders<'a> {
    s: &'a str,
    pos: usize,
    close: Cursor,
    line: Cursor,
    after_line: Cursor,
}

fn skip_space(s: &str, mut i: usize) -> usize {
    while let Some(c) = s[i..].chars().next() {
        if !is_js_space(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

impl<'a> Placeholders<'a> {
    /// The placeholder starting at the `{` at `o`, if one does.
    fn at(&mut self, o: usize) -> Option<Placeholder<'a>> {
        let s = self.s;
        let mut i = skip_space(s, o + 1);
        let key_start = i;
        if s[i..].starts_with('$') {
            i += 1;
        }
        let word_start = i;
        while s[i..].chars().next().is_some_and(is_word) {
            i += 1;
        }
        if i == word_start {
            return None;
        }
        let key = &s[key_start..i];
        let r = skip_space(s, i);
        match s[r..].chars().next() {
            Some('}') => Some(Placeholder {
                range: o..r + 1,
                key,
                filter: None,
            }),
            Some('|') => {
                let p = skip_space(s, r + 1);
                let c = self.close.first(s, p, |c| c == '}')?;
                let lt = self.line.first(s, p, is_line_terminator);
                if let Some(lt) = lt
                    && lt < c
                {
                    // A line ends before the brace: the filter cannot cross it,
                    // unless only spaces lie between it and the brace.
                    let n = self.after_line.first(s, lt, |c| !is_js_space(c));
                    if n.is_some_and(|n| n < c) {
                        return None;
                    }
                }
                // The filter ends where the spaces before the brace begin.
                let mut k = c;
                while k > p {
                    let prev = s[..k].chars().next_back().unwrap_or('}');
                    if !is_js_space(prev) {
                        break;
                    }
                    k -= prev.len_utf8();
                }
                Some(Placeholder {
                    range: o..c + 1,
                    key,
                    filter: Some(&s[p..k]),
                })
            }
            _ => None,
        }
    }
}

impl<'a> Iterator for Placeholders<'a> {
    type Item = Placeholder<'a>;

    fn next(&mut self) -> Option<Placeholder<'a>> {
        while self.pos < self.s.len() {
            let o = self.pos + self.s[self.pos..].find('{')?;
            match self.at(o) {
                Some(p) => {
                    self.pos = p.range.end;
                    return Some(p);
                }
                None => self.pos = o + 1,
            }
        }
        None
    }
}

/// Why a template could not be expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandError<E> {
    /// The result would pass the length limit.
    TooLong,
    /// The resolver refused a placeholder.
    Resolve(E),
}

/// Replace each placeholder the resolver answers for, leaving the others as
/// written, spacing included. `Ok(None)` when nothing was replaced, so the
/// caller can keep the original; the limit, in bytes, is checked before every
/// piece is added, so a hostile template cannot allocate past it.
pub fn expand<'r, E>(
    s: &str,
    limit: usize,
    mut resolve: impl FnMut(&Placeholder<'_>) -> Result<Option<Cow<'r, str>>, E>,
) -> Result<Option<String>, ExpandError<E>> {
    fn push<E>(out: &mut String, piece: &str, limit: usize) -> Result<(), ExpandError<E>> {
        if out.len() + piece.len() > limit {
            return Err(ExpandError::TooLong);
        }
        out.push_str(piece);
        Ok(())
    }
    let mut out: Option<String> = None;
    let mut last = 0;
    for p in placeholders(s) {
        let Some(value) = resolve(&p).map_err(ExpandError::Resolve)? else {
            continue;
        };
        let o = out.get_or_insert_with(String::new);
        push(o, &s[last..p.range.start], limit)?;
        push(o, &value, limit)?;
        last = p.range.end;
    }
    match out {
        None => Ok(None),
        Some(mut o) => {
            push(&mut o, &s[last..], limit)?;
            Ok(Some(o))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expression, transliterated with its backtracking, for short
    /// strings: `{`, `\s*` greedy, `\$?`, `\w+`, `\s*` greedy, then either
    /// `}` or `|\s*(.*?)\s*}` with the leading spaces greedy and the filter
    /// lazy. Deliberately naive; the scanner must agree with it.
    fn reference(s: &str) -> Vec<(usize, usize, String, Option<String>)> {
        let c: Vec<(usize, char)> = s.char_indices().collect();
        let at = |i: usize| c.get(i).map(|x| x.1);
        let byte = |i: usize| c.get(i).map_or(s.len(), |x| x.0);
        let mut out = Vec::new();
        let mut i = 0;
        'scan: while i < c.len() {
            if at(i) != Some('{') {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while at(j).is_some_and(is_js_space) {
                j += 1;
            }
            let ks = j;
            if at(j) == Some('$') {
                j += 1;
            }
            let ws = j;
            while at(j).is_some_and(is_word) {
                j += 1;
            }
            if j == ws {
                i += 1;
                continue;
            }
            let key: String = c[ks..j].iter().map(|x| x.1).collect();
            let mut r = j;
            while at(r).is_some_and(is_js_space) {
                r += 1;
            }
            // Greedy `\s*` may give back characters, but a space is neither
            // `}` nor `|`, so only the longest run can succeed.
            if at(r) == Some('|') {
                let mut lead_end = r + 1;
                while at(lead_end).is_some_and(is_js_space) {
                    lead_end += 1;
                }
                for p in (r + 1..=lead_end).rev() {
                    let mut f = p;
                    loop {
                        let mut t = f;
                        while at(t).is_some_and(is_js_space) {
                            t += 1;
                        }
                        let mut u = t;
                        loop {
                            if at(u) == Some('}') {
                                let filter: String = c[p..f].iter().map(|x| x.1).collect();
                                out.push((byte(i), byte(u + 1), key, Some(filter)));
                                i = u + 1;
                                continue 'scan;
                            }
                            if u == f {
                                break;
                            }
                            u -= 1;
                        }
                        match at(f) {
                            Some(ch) if !is_line_terminator(ch) => f += 1,
                            _ => break,
                        }
                    }
                }
            }
            if at(r) == Some('}') {
                out.push((byte(i), byte(r + 1), key, None));
                i = r + 1;
                continue;
            }
            i += 1;
        }
        out
    }

    fn scanned(s: &str) -> Vec<(usize, usize, String, Option<String>)> {
        placeholders(s)
            .map(|p| {
                (
                    p.range.start,
                    p.range.end,
                    p.key.to_string(),
                    p.filter.map(str::to_string),
                )
            })
            .collect()
    }

    #[test]
    fn the_scanner_agrees_with_the_expression() {
        let alphabet = [
            '{', '}', '|', '$', '[', ']', 'a', 'd', 't', '1', '_', '\t', '\n', '\r', ' ', '\u{a0}',
            '\u{feff}', '\u{85}', '\u{2028}', '\u{200b}', 'é', '😀',
        ];
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..200_000 {
            let len = (next() % 14) as usize;
            let s: String = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            assert_eq!(scanned(&s), reference(&s), "{s:?}");
        }
    }

    #[test]
    fn known_matches() {
        let one = |s: &str| {
            scanned(s)
                .into_iter()
                .map(|m| (m.2, m.3))
                .collect::<Vec<_>>()
        };
        let k = |key: &str, f: Option<&str>| (key.to_string(), f.map(str::to_string));
        assert_eq!(one("{sku}"), vec![k("sku", None)]);
        assert_eq!(one("{ sku }"), vec![k("sku", None)]);
        assert_eq!(one("{\u{a0}sku\u{feff}}"), vec![k("sku", None)]);
        assert_eq!(one("{\u{200b}sku}"), vec![]);
        assert_eq!(one("{\u{85}sku}"), vec![]);
        assert_eq!(one("{a |  b  c  }"), vec![k("a", Some("b  c"))]);
        assert_eq!(one("{a|}"), vec![k("a", Some(""))]);
        assert_eq!(one("{a|b|c}"), vec![k("a", Some("b|c"))]);
        assert_eq!(one("{dt|[}]}"), vec![k("dt", Some("["))]);
        assert_eq!(one("{dt|YYYY\nMM}"), vec![]);
        assert_eq!(one("{dt|YYYY\n}"), vec![k("dt", Some("YYYY"))]);
        assert_eq!(one("{a\n|\nb\n}"), vec![k("a", Some("b"))]);
        assert_eq!(one("{dt|YYYY\u{85}}"), vec![k("dt", Some("YYYY\u{85}"))]);
        assert_eq!(one("{$times}"), vec![k("$times", None)]);
        for none in [
            "{}",
            "{ }",
            "{a b}",
            "{a-b}",
            "{$}",
            "{a$}",
            "{$ times}",
            "{$$times}",
            "{skú}",
        ] {
            assert_eq!(one(none), vec![], "{none:?}");
        }
        assert_eq!(
            scanned("{{sku}}")[0].0,
            1,
            "the match starts at the inner brace"
        );
    }

    #[test]
    fn expansion_keeps_what_it_does_not_resolve() {
        let r = |p: &Placeholder<'_>| -> Result<Option<Cow<'static, str>>, ()> {
            Ok((p.key == "sku").then_some(Cow::Borrowed("A1")))
        };
        assert_eq!(expand("{ missing | f }", 100, r), Ok(None));
        assert_eq!(
            expand("x{sku}{ missing | f }y", 100, r),
            Ok(Some("xA1{ missing | f }y".into()))
        );
        assert_eq!(expand("{sku}", 2, r), Ok(Some("A1".into())));
        assert_eq!(expand("{sku}!", 2, r), Err(ExpandError::TooLong));
        let fails = |_: &Placeholder<'_>| -> Result<Option<Cow<'static, str>>, u8> { Err(7) };
        assert_eq!(expand("{sku}", 100, fails), Err(ExpandError::Resolve(7)));
    }

    /// A megabyte of each shape that makes the expression backtrack is
    /// scanned in linear time.
    #[test]
    fn hostile_templates_scan_in_linear_time() {
        for s in [
            "{a|".repeat(350_000),
            "{ ".repeat(500_000),
            format!("{{a|x{}", " ".repeat(1 << 20)),
            "{a|\n  ".repeat(170_000),
            format!("{}}}", "{a|".repeat(350_000)),
        ] {
            let start = std::time::Instant::now();
            let n = placeholders(&s).count();
            assert!(n <= 1, "{n}");
            assert!(start.elapsed().as_secs() < 2, "{:?}", start.elapsed());
        }
    }
}
