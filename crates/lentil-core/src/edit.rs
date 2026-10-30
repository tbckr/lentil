//! Byte-precise text edits (PRD principle 7, spec section 10).
//!
//! Each function changes only the bytes of one element and leaves the rest of
//! the text byte-identical. Positions come from [`crate::ledger::Txn`].

use std::ops::Range;

use thiserror::Error;

/// Why an edit cannot be made at the given position.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum EditError {
    /// The bytes at the position are not what the caller expected.
    #[error("unexpected text at {0:?}")]
    Mismatch(Range<usize>),
}

/// Prefix that `dismiss` puts in front of every line (spec section 10).
pub(crate) const COMMENT_PREFIX: &str = "; ";

/// Replaces the flag character at `offset` (expected to be `old`) with `new`.
///
/// # Errors
///
/// [`EditError::Mismatch`] if the character at `offset` is not `old`.
pub fn set_flag(text: &str, offset: usize, old: char, new: char) -> Result<String, EditError> {
    let span = offset..offset.saturating_add(old.len_utf8());
    let (mut old_buf, mut new_buf) = ([0; 4], [0; 4]);
    replace(
        text,
        span,
        old.encode_utf8(&mut old_buf),
        new.encode_utf8(&mut new_buf),
    )
}

/// Replaces the account name at `span` (expected to be `old`) with `new`.
///
/// # Errors
///
/// [`EditError::Mismatch`] if the text at `span` is not `old`.
pub fn set_account(
    text: &str,
    span: Range<usize>,
    old: &str,
    new: &str,
) -> Result<String, EditError> {
    replace(text, span, old, new)
}

/// Comments out the lines of `span` by prepending `; ` to each (spec section 10, `dismiss`).
///
/// `span` must start at the beginning of a line; every line it touches is
/// commented out. Line breaks stay as they are, so a CRLF file stays CRLF. An
/// empty span touches no line and leaves the text unchanged.
///
/// # Errors
///
/// [`EditError::Mismatch`] if `span` does not start at a line start or does
/// not lie within the text.
pub fn comment_out(text: &str, span: Range<usize>) -> Result<String, EditError> {
    let (Some(before), Some(body), Some(after)) = (
        text.get(..span.start),
        text.get(span.clone()),
        text.get(span.end..),
    ) else {
        return Err(EditError::Mismatch(span));
    };
    if !(before.is_empty() || before.ends_with('\n')) {
        return Err(EditError::Mismatch(span));
    }
    let lines = body.split_inclusive('\n');
    let mut out = String::with_capacity(text.len() + COMMENT_PREFIX.len() * lines.clone().count());
    out.push_str(before);
    for line in lines {
        out.push_str(COMMENT_PREFIX);
        out.push_str(line);
    }
    out.push_str(after);
    Ok(out)
}

/// Appends `line` as a new last line, adding a line break before it if the
/// text does not end with one.
///
/// The appended line ends with a line break too, so the file stays terminated.
/// A line break at the end of `line` itself is not doubled. The text's line
/// ending is kept: if its first line ends with `\r\n`, so do the added ones.
#[must_use]
pub fn append_line(text: &str, line: &str) -> String {
    let eol = line_ending(text);
    let line = line
        .strip_suffix('\n')
        .map_or(line, |l| l.strip_suffix('\r').unwrap_or(l));
    let mut out = String::with_capacity(text.len() + line.len() + 2 * eol.len());
    out.push_str(text);
    if !text.is_empty() && !text.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(line);
    out.push_str(eol);
    out
}

/// The line ending of `text`: `\r\n` if its first line ends with one, else `\n`.
fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if text[..i].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}

/// Replaces the bytes at `span`, which must read `old`, with `new`.
fn replace(text: &str, span: Range<usize>, old: &str, new: &str) -> Result<String, EditError> {
    match (
        text.get(..span.start),
        text.get(span.clone()),
        text.get(span.end..),
    ) {
        (Some(before), Some(found), Some(after)) if found == old => {
            let mut out = String::with_capacity(text.len() - old.len() + new.len());
            out.push_str(before);
            out.push_str(new);
            out.push_str(after);
            Ok(out)
        }
        _ => Err(EditError::Mismatch(span)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TXN: &str = "; head\n2026-10-18 ! \"Shop\" \"x\"\n  lentil_id: \"giro-1\"\n  Assets:Bank:Giro  -1.00 EUR ; c\n  Expenses:Uncategorized\n\n; tail\n";

    fn offset(text: &str, needle: &str) -> usize {
        text.find(needle).unwrap()
    }

    #[test]
    fn set_flag_changes_only_the_flag() {
        let at = offset(TXN, "! ");
        let out = set_flag(TXN, at, '!', '*').unwrap();
        assert_eq!(out, TXN.replacen("! \"Shop\"", "* \"Shop\"", 1));
        assert_eq!(out.len(), TXN.len());
        assert_eq!(out[..at], TXN[..at]);
        assert_eq!(out[at + 1..], TXN[at + 1..]);
    }

    #[test]
    fn set_flag_checks_the_old_flag() {
        let at = offset(TXN, "! ");
        assert_eq!(
            set_flag(TXN, at, '*', '!'),
            Err(EditError::Mismatch(at..at + 1))
        );
        assert_eq!(
            set_flag(TXN, at + 1, '!', '*'),
            Err(EditError::Mismatch(at + 1..at + 2))
        );
        assert!(set_flag(TXN, TXN.len(), '!', '*').is_err());
        assert!(set_flag(TXN, usize::MAX, '!', '*').is_err());
    }

    #[test]
    fn set_flag_handles_multibyte_characters() {
        let text = "a€b";
        assert_eq!(set_flag(text, 1, '€', '*').unwrap(), "a*b");
        assert_eq!(set_flag("a*b", 1, '*', '€').unwrap(), "a€b");
        // Not on a character boundary.
        assert!(set_flag(text, 2, '€', '*').is_err());
    }

    #[test]
    fn set_account_changes_only_the_name() {
        let start = offset(TXN, "Expenses:Uncategorized");
        let span = start..start + "Expenses:Uncategorized".len();
        let out = set_account(TXN, span, "Expenses:Uncategorized", "Expenses:Food").unwrap();
        assert_eq!(out, TXN.replace("Expenses:Uncategorized", "Expenses:Food"));
        assert_eq!(out[..start], TXN[..start]);
        assert_eq!(
            out[start + "Expenses:Food".len()..],
            TXN[start + "Expenses:Uncategorized".len()..]
        );
    }

    #[test]
    fn set_account_checks_the_old_name() {
        let start = offset(TXN, "Expenses:Uncategorized");
        let span = start..start + "Expenses:Uncategorized".len();
        assert_eq!(
            set_account(TXN, span.clone(), "Expenses:Other", "Expenses:Food"),
            Err(EditError::Mismatch(span))
        );
        assert!(set_account(TXN, start..TXN.len() + 1, "x", "y").is_err());
        #[allow(clippy::reversed_empty_ranges)]
        let reversed = start..start - 1;
        assert!(set_account(TXN, reversed, "", "y").is_err());
    }

    #[test]
    fn comment_out_prefixes_every_line_of_the_span() {
        let start = offset(TXN, "2026-10-18");
        let end = offset(TXN, "Expenses:Uncategorized") + "Expenses:Uncategorized".len();
        let out = comment_out(TXN, start..end).unwrap();
        assert_eq!(
            out,
            "; head\n; 2026-10-18 ! \"Shop\" \"x\"\n;   lentil_id: \"giro-1\"\n;   Assets:Bank:Giro  -1.00 EUR ; c\n;   Expenses:Uncategorized\n\n; tail\n"
        );
    }

    #[test]
    fn comment_out_keeps_crlf() {
        let text = TXN.replace('\n', "\r\n");
        let start = offset(&text, "2026-10-18");
        let end = offset(&text, "Expenses:Uncategorized") + "Expenses:Uncategorized".len();
        let out = comment_out(&text, start..end).unwrap();
        let expected = "; head\n; 2026-10-18 ! \"Shop\" \"x\"\n;   lentil_id: \"giro-1\"\n;   Assets:Bank:Giro  -1.00 EUR ; c\n;   Expenses:Uncategorized\n\n; tail\n"
            .replace('\n', "\r\n");
        assert_eq!(out, expected);
    }

    #[test]
    fn comment_out_requires_a_line_start() {
        let start = offset(TXN, "2026-10-18") + 1;
        assert_eq!(
            comment_out(TXN, start..start + 3),
            Err(EditError::Mismatch(start..start + 3))
        );
        assert!(comment_out(TXN, 0..TXN.len() + 1).is_err());
    }

    #[test]
    fn comment_out_at_text_start_and_empty_span() {
        assert_eq!(comment_out("a\nb", 0..3).unwrap(), "; a\n; b");
        assert_eq!(comment_out("a\nb\n", 0..2).unwrap(), "; a\nb\n");
        assert_eq!(comment_out("a\nb\n", 2..2).unwrap(), "a\nb\n");
    }

    #[test]
    fn append_line_adds_line_breaks_as_needed() {
        assert_eq!(append_line("", "x"), "x\n");
        assert_eq!(append_line("a\n", "x"), "a\nx\n");
        assert_eq!(append_line("a", "x"), "a\nx\n");
        assert_eq!(append_line("a\n", "x\n"), "a\nx\n");
    }

    #[test]
    fn append_line_keeps_crlf() {
        assert_eq!(append_line("a\r\nb\r\n", "x"), "a\r\nb\r\nx\r\n");
        assert_eq!(append_line("a\r\nb", "x\r\n"), "a\r\nb\r\nx\r\n");
        assert_eq!(append_line("a\nb\r\n", "x"), "a\nb\r\nx\n");
    }

    mod props {
        use proptest::prelude::*;

        use super::super::*;

        /// Lines of mixed content with LF or CRLF endings.
        fn text() -> impl Strategy<Value = String> {
            (
                prop::collection::vec("[ a-zA-Z0-9:;!*\"é€-]{0,12}", 1..8),
                any::<bool>(),
                any::<bool>(),
            )
                .prop_map(|(lines, crlf, final_break)| {
                    let eol = if crlf { "\r\n" } else { "\n" };
                    let mut text = lines.join(eol);
                    if final_break {
                        text.push_str(eol);
                    }
                    text
                })
        }

        /// Byte offsets of the line starts of `text`.
        fn line_starts(text: &str) -> Vec<usize> {
            std::iter::once(0)
                .chain(text.match_indices('\n').map(|(i, _)| i + 1))
                .filter(|&i| i == 0 || i < text.len())
                .collect()
        }

        proptest! {
            #[test]
            fn comment_out_only_adds_prefixes(text in text(), a in any::<usize>(), b in any::<usize>()) {
                let starts = line_starts(&text);
                let start = starts[a % starts.len()];
                let mut end = start + b % (text.len() - start + 1);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let out = comment_out(&text, start..end).unwrap();
                prop_assert_eq!(&out[..start], &text[..start]);
                let lines = text[start..end].split_inclusive('\n').count();
                prop_assert_eq!(out.len(), text.len() + COMMENT_PREFIX.len() * lines);
                prop_assert_eq!(&out[out.len() - (text.len() - end)..], &text[end..]);
                let restored: String = out[start..out.len() - (text.len() - end)]
                    .split_inclusive('\n')
                    .map(|l| l.strip_prefix(COMMENT_PREFIX).unwrap())
                    .collect();
                prop_assert_eq!(restored, &text[start..end]);
            }

            #[test]
            fn set_account_round_trips(text in text(), a in any::<usize>(), new in "[A-Z][a-z]{0,8}(:[A-Z][a-z]{0,8}){1,3}") {
                let starts = line_starts(&text);
                let start = starts[a % starts.len()];
                let end = text[start..].find(['\r', '\n']).map_or(text.len(), |i| start + i);
                let old = &text[start..end];
                let out = set_account(&text, start..end, old, &new).unwrap();
                prop_assert_eq!(&out[..start], &text[..start]);
                prop_assert_eq!(&out[start + new.len()..], &text[end..]);
                let back = set_account(&out, start..start + new.len(), &new, old).unwrap();
                prop_assert_eq!(back, text);
            }

            #[test]
            fn append_line_keeps_the_text_as_prefix(text in text(), line in "[ a-zA-Z0-9:]{0,20}") {
                let out = append_line(&text, &line);
                prop_assert!(out.starts_with(&text));
                prop_assert!(out.ends_with('\n'));
                prop_assert_eq!(out.trim_end_matches(['\r', '\n']).ends_with(&line), true);
                let crlf = text.find('\n').is_some_and(|i| text[..i].ends_with('\r'));
                let added = &out[text.len()..];
                prop_assert_eq!(added.matches("\r\n").count(), if crlf { added.matches('\n').count() } else { 0 });
            }
        }
    }
}
