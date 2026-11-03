//! Byte-precise text edits (PRD principle 7, spec section 10).
//!
//! Each function changes only the bytes of one element and leaves the rest of
//! the text byte-identical. Positions come from [`crate::ledger::Txn`].

use std::ops::Range;

use thiserror::Error;

use crate::import::render::escape;

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

/// Sets the payee to `new`: replaces the payee string at `payee_span`
/// (expected to read `old`), or, if there is no payee (`old` is `None`),
/// inserts `"<new>" ` before the narration string at `narration_span`.
/// Strings are escaped as the importer escapes them.
///
/// # Errors
///
/// [`EditError::Mismatch`] if the payee string is not `old`, if `payee_span`
/// and `old` disagree on whether there is a payee, or if `narration_span`
/// does not hold a string.
pub fn set_payee(
    text: &str,
    payee_span: Option<Range<usize>>,
    narration_span: Range<usize>,
    old: Option<&str>,
    new: &str,
) -> Result<String, EditError> {
    match (payee_span, old) {
        (Some(span), Some(old)) => replace(text, span, &quote(old), &quote(new)),
        (None, None)
            if text
                .get(narration_span.clone())
                .is_some_and(|s| s.starts_with('"') && s.ends_with('"')) =>
        {
            let at = narration_span.start;
            replace(text, at..at, "", &format!("{} ", quote(new)))
        }
        (span, _) => Err(EditError::Mismatch(span.unwrap_or(narration_span))),
    }
}

/// `text` as a string literal, escaped as the importer writes it.
pub(crate) fn quote(text: &str) -> String {
    format!("\"{}\"", escape(text))
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

/// Inserts ` #<tag>` for each of `tags`, in order, at `offset` (spec section
/// 10, `add-tag`); `offset` comes from [`crate::ledger::TagSpans`].
///
/// # Errors
///
/// [`EditError::Mismatch`] if `offset` is not a character boundary inside
/// `text`.
pub fn insert_tags(text: &str, offset: usize, tags: &[&str]) -> Result<String, EditError> {
    let (Some(before), Some(after)) = (text.get(..offset), text.get(offset..)) else {
        return Err(EditError::Mismatch(offset..offset));
    };
    let mut out =
        String::with_capacity(text.len() + tags.iter().map(|t| t.len() + 2).sum::<usize>());
    out.push_str(before);
    for tag in tags {
        out.push_str(" #");
        out.push_str(tag);
    }
    out.push_str(after);
    Ok(out)
}

/// Removes `#<tag>` at `span` with one separating blank (spec section 10,
/// `remove-tag`): the blank before it if an earlier token stands on its line,
/// otherwise the blank after it if there is one. A tag alone on its line is
/// removed with the whole line and its line break, or with the line break
/// before it if the line is the last.
///
/// # Errors
///
/// [`EditError::Mismatch`] if the text at `span` is not `#<tag>`.
pub fn remove_tag(text: &str, span: Range<usize>, tag: &str) -> Result<String, EditError> {
    let found = text.get(span.clone()).and_then(|t| t.strip_prefix('#'));
    if found != Some(tag) {
        return Err(EditError::Mismatch(span));
    }
    let line_start = text[..span.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[span.end..]
        .find('\n')
        .map_or(text.len(), |i| span.end + i + 1);
    let is_blank = |s: &str| s.trim_matches([' ', '\t', '\r', '\n']).is_empty();
    let cut = if is_blank(&text[line_start..span.start]) && is_blank(&text[span.end..line_end]) {
        if line_end == text.len() && !text.ends_with('\n') {
            let before = text[..line_start]
                .strip_suffix('\n')
                .map_or(line_start, |t| t.strip_suffix('\r').unwrap_or(t).len());
            before..text.len()
        } else {
            line_start..line_end
        }
    } else if !is_blank(&text[line_start..span.start]) && text[..span.start].ends_with([' ', '\t'])
    {
        span.start - 1..span.end
    } else if text[span.end..].starts_with([' ', '\t']) {
        span.start..span.end + 1
    } else {
        span
    };
    let mut out = String::with_capacity(text.len() - cut.len());
    out.push_str(&text[..cut.start]);
    out.push_str(&text[cut.end..]);
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
    fn insert_tags_adds_each_with_one_blank() {
        assert_eq!(
            insert_tags("x \"N\" ; c", 5, &["a", "b"]),
            Ok("x \"N\" #a #b ; c".to_owned())
        );
        assert_eq!(insert_tags("é", 1, &["a"]), Err(EditError::Mismatch(1..1)));
        assert_eq!(insert_tags("x", 2, &["a"]), Err(EditError::Mismatch(2..2)));
    }

    /// Removes the first `#<tag>` in `text`.
    fn remove_first(text: &str, tag: &str) -> Result<String, EditError> {
        let at = offset(text, &format!("#{tag}"));
        remove_tag(text, at..at + 1 + tag.len(), tag)
    }

    #[test]
    fn remove_tag_takes_the_blank_before() {
        assert_eq!(remove_first("\"N\" #a #b", "a"), Ok("\"N\" #b".to_owned()));
        assert_eq!(remove_first("\"N\" #a #b", "b"), Ok("\"N\" #a".to_owned()));
    }

    #[test]
    fn remove_tag_takes_the_blank_after_the_indent() {
        assert_eq!(remove_first("  #two ^l2", "two"), Ok("  ^l2".to_owned()));
        assert_eq!(remove_first("\t#two ^l2", "two"), Ok("\t^l2".to_owned()));
    }

    #[test]
    fn a_tag_alone_on_its_line_goes_with_the_line() {
        assert_eq!(
            remove_first("h\n  #three\n  m\n", "three"),
            Ok("h\n  m\n".to_owned())
        );
        assert_eq!(
            remove_first("h\r\n  #three\r\n  m", "three"),
            Ok("h\r\n  m".to_owned())
        );
        assert_eq!(remove_first("h\n  #three", "three"), Ok("h".to_owned()));
        assert_eq!(
            remove_first("h\r\n\t#three \t", "three"),
            Ok("h".to_owned())
        );
    }

    #[test]
    fn remove_tag_rejects_other_text() {
        let text = "\"N\" #other";
        let at = offset(text, "#other");
        assert_eq!(
            remove_tag(text, at..at + 6, "one"),
            Err(EditError::Mismatch(at..at + 6))
        );
        assert_eq!(
            remove_tag(text, at..at + 40, "other"),
            Err(EditError::Mismatch(at..at + 40))
        );
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

    fn span(text: &str, needle: &str) -> Range<usize> {
        let start = offset(text, needle);
        start..start + needle.len()
    }

    #[test]
    fn set_payee_replaces_only_the_payee_string() {
        let payee = span(TXN, "\"Shop\"");
        let narration = span(TXN, "\"x\"");
        let out = set_payee(TXN, Some(payee), narration, Some("Shop"), "Corner Grocery").unwrap();
        assert_eq!(out, TXN.replacen("\"Shop\"", "\"Corner Grocery\"", 1));
    }

    #[test]
    fn set_payee_escapes_quotes_and_backslashes() {
        let text = TXN.replacen("\"Shop\"", r#""Say \"hi\" \\""#, 1);
        let payee = span(&text, r#""Say \"hi\" \\""#);
        let narration = span(&text, "\"x\"");
        let out = set_payee(
            &text,
            Some(payee),
            narration,
            Some(r#"Say "hi" \"#),
            r#"A\"B"#,
        )
        .unwrap();
        assert_eq!(out, TXN.replacen("\"Shop\"", r#""A\\\"B""#, 1));
    }

    #[test]
    fn set_payee_inserts_a_payee_before_the_narration() {
        let text = TXN.replacen("\"Shop\" ", "", 1);
        let narration = span(&text, "\"x\"");
        let out = set_payee(&text, None, narration, None, "Sh\"op").unwrap();
        assert_eq!(out, TXN.replacen("\"Shop\"", r#""Sh\"op""#, 1));
    }

    #[test]
    fn set_payee_checks_the_old_payee() {
        let payee = span(TXN, "\"Shop\"");
        let narration = span(TXN, "\"x\"");
        for old in [Some("Shops"), Some("Sho"), None] {
            assert_eq!(
                set_payee(TXN, Some(payee.clone()), narration.clone(), old, "New"),
                Err(EditError::Mismatch(payee.clone())),
                "{old:?}"
            );
        }
        assert_eq!(
            set_payee(TXN, None, narration.clone(), Some("Shop"), "New"),
            Err(EditError::Mismatch(narration.clone()))
        );
    }

    #[test]
    fn set_payee_inserts_only_before_a_string() {
        let narration = span(TXN, "\"x\"");
        for partial in [
            narration.start + 1..narration.end,
            narration.start..narration.end - 1,
        ] {
            assert_eq!(
                set_payee(TXN, None, partial.clone(), None, "New"),
                Err(EditError::Mismatch(partial))
            );
        }
        assert!(set_payee(TXN, None, TXN.len()..TXN.len() + 1, None, "New").is_err());
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
