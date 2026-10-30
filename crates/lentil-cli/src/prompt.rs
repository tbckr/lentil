//! Questions on the terminal.

use std::io::{BufRead, IsTerminal, Write};

use jiff::civil::Date;
use rust_decimal::Decimal;

/// Whether the CLI may ask questions: stdin is a terminal.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// Prints `question` and reads one line, trimmed. `None` at end of input.
pub fn ask(question: &str) -> Option<String> {
    eprint!("{question}");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_owned()),
    }
}

/// Whether an answer means yes (`y`, `yes`, `j`, `ja`).
pub fn is_yes(answer: &str) -> bool {
    matches!(answer.to_lowercase().as_str(), "y" | "yes" | "j" | "ja")
}

/// Parses an amount as typed by a user: `1234.56`, `-1234,56`, `1.234,56` or `1,234.56`.
pub fn parse_amount(text: &str) -> Option<Decimal> {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\'')
        .collect();
    let normalized = match (cleaned.rfind(','), cleaned.rfind('.')) {
        (Some(comma), Some(dot)) if comma > dot => cleaned.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => cleaned.replace(',', ""),
        (Some(_), None) => cleaned.replace(',', "."),
        _ => cleaned,
    };
    normalized.parse().ok()
}

/// Parses `YYYY-MM-DD`.
pub fn parse_date(text: &str) -> Option<Date> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_amounts_in_both_notations() {
        let d = |s: &str| s.parse::<Decimal>().unwrap();
        assert_eq!(parse_amount("1234.56"), Some(d("1234.56")));
        assert_eq!(parse_amount("-1234,56"), Some(d("-1234.56")));
        assert_eq!(parse_amount("1.234,56"), Some(d("1234.56")));
        assert_eq!(parse_amount("1,234.56"), Some(d("1234.56")));
        assert_eq!(parse_amount("abc"), None);
    }

    #[test]
    fn recognizes_yes_in_english_and_german() {
        assert!(is_yes("y"));
        assert!(is_yes("Ja"));
        assert!(!is_yes(""));
        assert!(!is_yes("n"));
    }
}
