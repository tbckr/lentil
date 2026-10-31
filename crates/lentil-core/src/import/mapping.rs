//! CSV mapping files under `sources/` (spec section 15).
//!
//! A mapping is data only: it names columns, formats and patterns, never a
//! program. Unknown and missing keys are rejected, so a mapping written for a
//! newer spec version never imports half-understood.

use std::borrow::Cow;

use jiff::civil::Date;
use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

/// A parsed mapping file. Unknown keys are rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// Character encoding of the export.
    pub encoding: Encoding,
    /// Field delimiter, one ASCII character.
    pub delimiter: u8,
    /// Cells that identify the header row, trimmed.
    pub header: Vec<String>,
    /// The first line after the table, if the export has a footer.
    pub end: Option<Pattern>,
    /// Decimal point of amounts.
    pub decimal: char,
    /// Thousands separator of amounts, dropped before parsing.
    pub thousands: Option<char>,
    /// The booking date column and its format.
    pub booking_date: DateColumn,
    /// Where the amount comes from.
    pub amount: AmountColumns,
    /// Currency column; without it the source's currency applies.
    pub currency: Option<String>,
    /// Columns joined into the counterparty.
    pub counterparty: Vec<String>,
    /// Columns joined into the purpose.
    pub purpose: Vec<String>,
    /// Column with the bank's unique reference.
    pub reference: Option<String>,
    /// Rows to skip, such as pending transactions.
    pub skip: Vec<SkipRule>,
    /// Where the balance comes from.
    pub balance: BalanceSource,
}

/// The four encodings of spec section 15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8, with an optional byte order mark.
    Utf8,
    /// Windows-1252.
    Windows1252,
    /// ISO-8859-1 (Latin-1): every byte is the code point of the same value.
    Iso88591,
    /// ISO-8859-15 (Latin-9).
    Iso885915,
}

impl Encoding {
    /// The mapping label of the encoding.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Windows1252 => "windows-1252",
            Self::Iso88591 => "iso-8859-1",
            Self::Iso885915 => "iso-8859-15",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        [
            Self::Utf8,
            Self::Windows1252,
            Self::Iso88591,
            Self::Iso885915,
        ]
        .into_iter()
        .find(|e| e.label() == label)
    }

    /// Decodes an export. Returns `None` if the bytes are not valid in this
    /// encoding; only UTF-8 can fail, the single-byte encodings map every byte.
    #[must_use]
    pub fn decode(self, bytes: &[u8]) -> Option<Cow<'_, str>> {
        match self {
            Self::Utf8 => {
                let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
                std::str::from_utf8(bytes).ok().map(Cow::Borrowed)
            }
            // encoding_rs maps the label iso-8859-1 to windows-1252 (WHATWG),
            // so Latin-1 is decoded here byte by byte.
            Self::Iso88591 => Some(Cow::Owned(bytes.iter().map(|&b| char::from(b)).collect())),
            Self::Windows1252 => {
                encoding_rs::WINDOWS_1252.decode_without_bom_handling_and_without_replacement(bytes)
            }
            Self::Iso885915 => {
                encoding_rs::ISO_8859_15.decode_without_bom_handling_and_without_replacement(bytes)
            }
        }
    }
}

/// A compiled regular expression that compares by its source text.
#[derive(Clone, Debug)]
pub struct Pattern(Regex);

impl Pattern {
    /// Compiles `source` in the syntax of spec section 16.
    ///
    /// # Errors
    ///
    /// Returns [`MappingError::Regex`] if it does not compile.
    pub fn new(source: &str) -> Result<Self, MappingError> {
        Regex::new(source)
            .map(Self)
            .map_err(|_| MappingError::Regex(source.to_owned()))
    }

    /// The compiled expression.
    #[must_use]
    pub fn regex(&self) -> &Regex {
        &self.0
    }

    /// Whether the expression matches anywhere in `text`.
    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text)
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Eq for Pattern {}

/// The booking date column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateColumn {
    /// Header name.
    pub column: String,
    /// Date format.
    pub format: DateFormat,
}

/// Where the amount comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AmountColumns {
    /// One signed column.
    Single {
        /// Header name.
        column: String,
        /// Flip the sign after reading.
        invert: bool,
    },
    /// Separate debit and credit columns; exactly one holds a number per row.
    DebitCredit {
        /// Header name of the debit column.
        debit: String,
        /// Header name of the credit column.
        credit: String,
        /// Flip the sign after reading.
        invert: bool,
    },
}

/// One `[[skip]]` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipRule {
    /// Header name.
    pub column: String,
    /// How the trimmed cell is compared.
    pub condition: SkipCondition,
}

/// How a `[[skip]]` entry compares the trimmed cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipCondition {
    /// `equals`: exact comparison.
    Equals(String),
    /// `matches`: regular expression.
    Matches(Pattern),
}

impl SkipRule {
    /// Whether the (untrimmed) cell matches this entry.
    #[must_use]
    pub fn matches(&self, cell: &str) -> bool {
        let cell = cell.trim();
        match &self.condition {
            SkipCondition::Equals(value) => cell == value,
            SkipCondition::Matches(pattern) => pattern.is_match(cell),
        }
    }
}

/// Where the balance comes from (`[balance]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BalanceSource {
    /// `from = "column"`: running balance after each row.
    Column(String),
    /// `from = "text"`: one balance stated before or after the table.
    Text {
        /// Pattern with the named group `amount` and optionally `date`.
        pattern: Pattern,
        /// Format of the `date` group; present exactly when the group is.
        date_format: Option<DateFormat>,
    },
    /// `from = "none"`: the CLI asks the user.
    None,
}

/// A date format made of `%d`, `%m`, `%Y` or `%y` and literal characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateFormat {
    source: String,
    items: Vec<DateItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateItem {
    Day,
    Month,
    Year4,
    Year2,
    Literal(char),
}

impl DateFormat {
    /// Parses a format. It needs exactly one day, one month and one year code.
    ///
    /// # Errors
    ///
    /// Returns [`MappingError::DateFormat`] for other codes or a missing or
    /// repeated part.
    pub fn new(source: &str) -> Result<Self, MappingError> {
        let error = || MappingError::DateFormat(source.to_owned());
        let mut items = Vec::new();
        let mut chars = source.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                items.push(match chars.next() {
                    Some('d') => DateItem::Day,
                    Some('m') => DateItem::Month,
                    Some('Y') => DateItem::Year4,
                    Some('y') => DateItem::Year2,
                    _ => return Err(error()),
                });
            } else {
                items.push(DateItem::Literal(c));
            }
        }
        let count = |f: fn(&DateItem) -> bool| items.iter().filter(|i| f(i)).count();
        if count(|i| *i == DateItem::Day) != 1
            || count(|i| *i == DateItem::Month) != 1
            || count(|i| matches!(i, DateItem::Year4 | DateItem::Year2)) != 1
        {
            return Err(error());
        }
        Ok(Self {
            source: source.to_owned(),
            items,
        })
    }

    /// The format as written in the mapping.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Parses a trimmed date. `%d` and `%m` take one or two digits, `%Y` four
    /// and `%y` two; `%y` maps 00–68 to 2000–2068 and 69–99 to 1969–1999, as
    /// POSIX `strptime` does. The whole text must be consumed.
    #[must_use]
    pub fn parse(&self, text: &str) -> Option<Date> {
        let mut rest = text.trim();
        let (mut day, mut month, mut year) = (0, 0, 0);
        for item in &self.items {
            match *item {
                DateItem::Literal(c) => rest = rest.strip_prefix(c)?,
                DateItem::Day => (day, rest) = digits(rest, 1, 2)?,
                DateItem::Month => (month, rest) = digits(rest, 1, 2)?,
                DateItem::Year4 => (year, rest) = digits(rest, 4, 4)?,
                DateItem::Year2 => {
                    let (yy, r) = digits(rest, 2, 2)?;
                    year = if yy < 69 { 2000 + yy } else { 1900 + yy };
                    rest = r;
                }
            }
        }
        if !rest.is_empty() {
            return None;
        }
        Date::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()
    }
}

/// Reads `min..=max` ASCII digits (as many as possible) from the start of `text`.
fn digits(text: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let len = text
        .bytes()
        .take(max)
        .take_while(u8::is_ascii_digit)
        .count();
    if len < min {
        return None;
    }
    let value = text[..len].parse().ok()?;
    Some((value, &text[len..]))
}

/// Why a mapping file is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum MappingError {
    /// Not TOML, an unknown or missing key, or a wrong type. Detail from the TOML parser.
    #[error("invalid mapping: {0}")]
    Syntax(String),
    /// A regular expression does not compile.
    #[error("invalid regular expression {0:?}")]
    Regex(String),
    /// An encoding other than the four of spec section 15.
    #[error("unsupported encoding {0:?}")]
    Encoding(String),
    /// A date format with codes other than `%d`, `%m`, `%Y`, `%y`.
    #[error("unsupported date format {0:?}")]
    DateFormat(String),
    /// Keys that do not fit together, such as `from = "column"` without `column`.
    #[error("inconsistent mapping: {0}")]
    Inconsistent(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMapping {
    file: RawFile,
    #[serde(default)]
    numbers: RawNumbers,
    columns: RawColumns,
    #[serde(default)]
    skip: Vec<RawSkip>,
    balance: RawBalance,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    encoding: Option<String>,
    delimiter: Option<String>,
    header: Vec<String>,
    end: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNumbers {
    decimal: Option<String>,
    thousands: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawColumns {
    booking_date: RawDateColumn,
    amount: RawAmount,
    currency: Option<String>,
    counterparty: Option<Vec<String>>,
    purpose: Option<Vec<String>>,
    reference: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDateColumn {
    column: String,
    format: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAmount {
    column: Option<String>,
    debit: Option<String>,
    credit: Option<String>,
    #[serde(default)]
    invert: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSkip {
    column: String,
    equals: Option<String>,
    matches: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBalance {
    from: RawBalanceFrom,
    column: Option<String>,
    pattern: Option<String>,
    date_format: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawBalanceFrom {
    Column,
    Text,
    None,
}

fn inconsistent(detail: &str) -> MappingError {
    MappingError::Inconsistent(detail.to_owned())
}

/// A single character from a one-character string.
fn one_char(value: &str, key: &str) -> Result<char, MappingError> {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(c),
        _ => Err(inconsistent(&format!("{key} must be one character"))),
    }
}

/// A column list for `counterparty` or `purpose`: absent or non-empty.
fn column_list(value: Option<Vec<String>>, key: &str) -> Result<Vec<String>, MappingError> {
    match value {
        Some(list) if list.is_empty() => {
            Err(inconsistent(&format!("{key} needs at least one column")))
        }
        Some(list) => Ok(list),
        None => Ok(Vec::new()),
    }
}

impl Mapping {
    /// Parses a mapping file.
    ///
    /// # Errors
    ///
    /// Returns a [`MappingError`] for any invalid content.
    pub fn parse(text: &str) -> Result<Self, MappingError> {
        let raw: RawMapping =
            toml::from_str(text).map_err(|e| MappingError::Syntax(e.message().to_owned()))?;

        let encoding = match raw.file.encoding.as_deref() {
            None => Encoding::Utf8,
            Some(label) => Encoding::from_label(label)
                .ok_or_else(|| MappingError::Encoding(label.to_owned()))?,
        };
        let delimiter = match raw.file.delimiter.as_deref() {
            None => b',',
            Some(d) => match d.as_bytes() {
                [b] if b.is_ascii() && !matches!(b, b'"' | b'\n' | b'\r') => *b,
                _ => return Err(inconsistent("delimiter must be one ASCII character")),
            },
        };
        let header: Vec<String> = raw
            .file
            .header
            .iter()
            .map(|h| h.trim().to_owned())
            .collect();
        if header.is_empty() || header.iter().any(String::is_empty) {
            return Err(inconsistent("header needs at least one non-empty cell"));
        }
        let end = raw.file.end.as_deref().map(Pattern::new).transpose()?;

        let decimal = match raw.numbers.decimal.as_deref() {
            None => '.',
            Some(d) => one_char(d, "decimal")?,
        };
        let thousands = raw
            .numbers
            .thousands
            .as_deref()
            .map(|t| one_char(t, "thousands"))
            .transpose()?;
        if thousands == Some(decimal) {
            return Err(inconsistent("thousands and decimal must differ"));
        }
        if decimal.is_ascii_digit() || matches!(decimal, '+' | '-') {
            return Err(inconsistent("decimal must not be a digit or sign"));
        }
        if thousands.is_some_and(|t| t.is_ascii_digit() || matches!(t, '+' | '-')) {
            return Err(inconsistent("thousands must not be a digit or sign"));
        }

        let columns = raw.columns;
        let booking_date = DateColumn {
            column: columns.booking_date.column,
            format: DateFormat::new(&columns.booking_date.format)?,
        };
        let amount = match columns.amount {
            RawAmount {
                column: Some(column),
                debit: None,
                credit: None,
                invert,
            } => AmountColumns::Single { column, invert },
            RawAmount {
                column: None,
                debit: Some(debit),
                credit: Some(credit),
                invert,
            } => AmountColumns::DebitCredit {
                debit,
                credit,
                invert,
            },
            _ => {
                return Err(inconsistent(
                    "amount needs either column or both debit and credit",
                ));
            }
        };

        let skip = raw
            .skip
            .into_iter()
            .map(|s| {
                let condition = match (s.equals, s.matches) {
                    (Some(value), None) => SkipCondition::Equals(value),
                    (None, Some(pattern)) => SkipCondition::Matches(Pattern::new(&pattern)?),
                    _ => return Err(inconsistent("skip needs either equals or matches")),
                };
                Ok(SkipRule {
                    column: s.column,
                    condition,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let b = raw.balance;
        let balance = match (b.from, b.column, b.pattern, b.date_format) {
            (RawBalanceFrom::Column, Some(column), None, None) => BalanceSource::Column(column),
            (RawBalanceFrom::Text, None, Some(pattern), date_format) => {
                let pattern = Pattern::new(&pattern)?;
                let names: Vec<&str> = pattern.regex().capture_names().flatten().collect();
                if !names.contains(&"amount") {
                    return Err(inconsistent("balance pattern needs a group named amount"));
                }
                let date_format = match (names.contains(&"date"), date_format) {
                    (true, Some(format)) => Some(DateFormat::new(&format)?),
                    (false, None) => None,
                    (true, None) => {
                        return Err(inconsistent("balance group date needs date_format"));
                    }
                    (false, Some(_)) => {
                        return Err(inconsistent("balance date_format needs a group named date"));
                    }
                };
                BalanceSource::Text {
                    pattern,
                    date_format,
                }
            }
            (RawBalanceFrom::None, None, None, None) => BalanceSource::None,
            (RawBalanceFrom::Column, ..) => {
                return Err(inconsistent(
                    "balance from column takes exactly the key column",
                ));
            }
            (RawBalanceFrom::Text, ..) => {
                return Err(inconsistent(
                    "balance from text takes pattern and date_format only",
                ));
            }
            (RawBalanceFrom::None, ..) => {
                return Err(inconsistent("balance from none takes no other key"));
            }
        };

        Ok(Self {
            encoding,
            delimiter,
            header,
            end,
            decimal,
            thousands,
            booking_date,
            amount,
            currency: columns.currency,
            counterparty: column_list(columns.counterparty, "counterparty")?,
            purpose: column_list(columns.purpose, "purpose")?,
            reference: columns.reference,
            skip,
            balance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of spec section 15, with the text balance variant.
    const SPEC_EXAMPLE: &str = r#"
[file]
encoding = "windows-1252"
delimiter = ";"
header = ["Buchungsdatum", "Betrag"]
end = "^Kontostand"

[numbers]
decimal = ","
thousands = "."

[columns]
booking_date = { column = "Buchungsdatum", format = "%d.%m.%Y" }
amount = { column = "Betrag" }
currency = "Währung"
counterparty = ["Zahlungsempfänger"]
purpose = ["Verwendungszweck"]
reference = "Kundenreferenz"

[[skip]]
column = "Status"
equals = "Vorgemerkt"

[balance]
from = "column"
column = "Saldo"
"#;

    const MINIMAL: &str = r#"
[file]
header = ["Date", "Amount"]

[columns]
booking_date = { column = "Date", format = "%Y-%m-%d" }
amount = { column = "Amount" }

[balance]
from = "none"
"#;

    fn err(text: &str) -> MappingError {
        Mapping::parse(text).unwrap_err()
    }

    #[test]
    fn parses_the_spec_example() {
        let m = Mapping::parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(m.encoding, Encoding::Windows1252);
        assert_eq!(m.delimiter, b';');
        assert_eq!(m.header, vec!["Buchungsdatum", "Betrag"]);
        assert!(m.end.as_ref().unwrap().is_match("Kontostand am 31.10.2026"));
        assert_eq!((m.decimal, m.thousands), (',', Some('.')));
        assert_eq!(m.booking_date.column, "Buchungsdatum");
        assert_eq!(m.booking_date.format.as_str(), "%d.%m.%Y");
        assert_eq!(
            m.amount,
            AmountColumns::Single {
                column: "Betrag".into(),
                invert: false
            }
        );
        assert_eq!(m.currency.as_deref(), Some("Währung"));
        assert_eq!(m.counterparty, vec!["Zahlungsempfänger"]);
        assert_eq!(m.purpose, vec!["Verwendungszweck"]);
        assert_eq!(m.reference.as_deref(), Some("Kundenreferenz"));
        assert_eq!(
            m.skip,
            vec![SkipRule {
                column: "Status".into(),
                condition: SkipCondition::Equals("Vorgemerkt".into())
            }]
        );
        assert_eq!(m.balance, BalanceSource::Column("Saldo".into()));
    }

    #[test]
    fn applies_defaults() {
        let m = Mapping::parse(MINIMAL).unwrap();
        assert_eq!(m.encoding, Encoding::Utf8);
        assert_eq!(m.delimiter, b',');
        assert_eq!((m.decimal, m.thousands), ('.', None));
        assert!(m.end.is_none());
        assert!(m.counterparty.is_empty() && m.purpose.is_empty());
        assert!(m.skip.is_empty());
        assert_eq!(m.balance, BalanceSource::None);
    }

    #[test]
    fn parses_debit_credit_with_invert() {
        let text = MINIMAL.replace(
            "amount = { column = \"Amount\" }",
            "amount = { debit = \"Out\", credit = \"In\", invert = true }",
        );
        assert_eq!(
            Mapping::parse(&text).unwrap().amount,
            AmountColumns::DebitCredit {
                debit: "Out".into(),
                credit: "In".into(),
                invert: true
            }
        );
    }

    #[test]
    fn rejects_unknown_keys_everywhere() {
        for (from, to) in [
            ("[file]\n", "[file]\ncommand = \"x\"\n"),
            ("[balance]\n", "[balance]\nscript = \"x\"\n"),
            ("[columns]\n", "[columns]\nvalue_date = \"x\"\n"),
            (
                "{ column = \"Amount\" }",
                "{ column = \"Amount\", scale = 2 }",
            ),
            (
                "format = \"%Y-%m-%d\"",
                "format = \"%Y-%m-%d\", tz = \"UTC\"",
            ),
        ] {
            let text = MINIMAL.replace(from, to);
            assert!(matches!(err(&text), MappingError::Syntax(_)), "{to}");
        }
        let text = format!("{MINIMAL}\n[converter]\nrun = \"x\"\n");
        assert!(matches!(err(&text), MappingError::Syntax(_)));
        let text = format!("{MINIMAL}\n[[skip]]\ncolumn = \"S\"\nequals = \"x\"\nexec = \"y\"\n");
        assert!(matches!(err(&text), MappingError::Syntax(_)));
    }

    #[test]
    fn rejects_missing_required_keys() {
        for remove in [
            "header = [\"Date\", \"Amount\"]\n",
            "booking_date = { column = \"Date\", format = \"%Y-%m-%d\" }\n",
            "amount = { column = \"Amount\" }\n",
            "[balance]\nfrom = \"none\"\n",
            "from = \"none\"\n",
        ] {
            let text = MINIMAL.replace(remove, "");
            assert!(matches!(err(&text), MappingError::Syntax(_)), "{remove}");
        }
        let text = MINIMAL.replace(", format = \"%Y-%m-%d\"", "");
        assert!(matches!(err(&text), MappingError::Syntax(_)));
    }

    #[test]
    fn accepts_the_four_encodings_only() {
        for (label, encoding) in [
            ("utf-8", Encoding::Utf8),
            ("windows-1252", Encoding::Windows1252),
            ("iso-8859-1", Encoding::Iso88591),
            ("iso-8859-15", Encoding::Iso885915),
        ] {
            let text = MINIMAL.replace("[file]\n", &format!("[file]\nencoding = {label:?}\n"));
            assert_eq!(Mapping::parse(&text).unwrap().encoding, encoding);
        }
        let text = MINIMAL.replace("[file]\n", "[file]\nencoding = \"utf-16\"\n");
        assert_eq!(err(&text), MappingError::Encoding("utf-16".into()));
    }

    #[test]
    fn decodes_each_encoding() {
        // 0xA4 is the currency sign in Latin-1 and the euro sign in Latin-9;
        // 0x80 is the euro sign in Windows-1252 and a C1 control in Latin-1.
        let bytes = b"\xE4\xA4\x80";
        assert_eq!(Encoding::Iso88591.decode(bytes).unwrap(), "ä\u{a4}\u{80}");
        assert_eq!(Encoding::Iso885915.decode(bytes).unwrap(), "ä€\u{80}");
        assert_eq!(Encoding::Windows1252.decode(bytes).unwrap(), "ä\u{a4}€");
        assert_eq!(
            Encoding::Utf8.decode(b"\xEF\xBB\xBFa\xC3\xA4").unwrap(),
            "aä"
        );
        assert_eq!(Encoding::Utf8.decode(b"\xE4x"), None);
    }

    #[test]
    fn rejects_bad_regular_expressions() {
        let text = MINIMAL.replace("[file]\n", "[file]\nend = \"(\"\n");
        assert_eq!(err(&text), MappingError::Regex("(".into()));
        let text = MINIMAL.replace("[file]\n", "[file]\nend = \"(?=x)\"\n");
        assert_eq!(err(&text), MappingError::Regex("(?=x)".into()));
        let text = format!("{MINIMAL}\n[[skip]]\ncolumn = \"S\"\nmatches = \"[\"\n");
        assert_eq!(err(&text), MappingError::Regex("[".into()));
    }

    #[test]
    fn date_formats_allow_only_four_codes() {
        for ok in ["%d.%m.%Y", "%Y-%m-%d", "%m/%d/%y", "%d%m%Y"] {
            assert!(DateFormat::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "%d.%m.%Y %H",
            "%e.%m.%Y",
            "%d.%m",
            "%d.%d.%m.%Y",
            "%Y%y%m%d",
            "%%%d%m%Y",
            "%d%m%Y%",
        ] {
            assert_eq!(
                DateFormat::new(bad),
                Err(MappingError::DateFormat(bad.into())),
                "{bad}"
            );
        }
        let text = MINIMAL.replace("%Y-%m-%d", "%F");
        assert_eq!(err(&text), MappingError::DateFormat("%F".into()));
    }

    #[test]
    fn parses_dates() {
        let f = DateFormat::new("%d.%m.%Y").unwrap();
        assert_eq!(f.parse("19.10.2026"), Some(Date::constant(2026, 10, 19)));
        assert_eq!(f.parse(" 1.2.2026 "), Some(Date::constant(2026, 2, 1)));
        assert_eq!(f.parse("31.02.2026"), None);
        assert_eq!(f.parse("19.10.26"), None);
        assert_eq!(f.parse("19.10.2026x"), None);
        assert_eq!(f.parse("19-10-2026"), None);
        assert_eq!(f.parse(""), None);
        let f = DateFormat::new("%y%m%d").unwrap();
        assert_eq!(f.parse("261019"), Some(Date::constant(2026, 10, 19)));
        assert_eq!(f.parse("681231"), Some(Date::constant(2068, 12, 31)));
        assert_eq!(f.parse("690101"), Some(Date::constant(1969, 1, 1)));
        let f = DateFormat::new("%m/%d/%Y").unwrap();
        assert_eq!(f.parse("10/19/2026"), Some(Date::constant(2026, 10, 19)));
    }

    #[test]
    fn balance_keys_must_fit_from() {
        let column = MINIMAL.replace("from = \"none\"", "from = \"column\"\ncolumn = \"Saldo\"");
        assert_eq!(
            Mapping::parse(&column).unwrap().balance,
            BalanceSource::Column("Saldo".into())
        );
        let text = MINIMAL.replace(
            "from = \"none\"",
            "from = \"text\"\npattern = 'Balance (?P<date>\\S+): (?P<amount>\\S+)'\ndate_format = \"%d.%m.%Y\"",
        );
        let BalanceSource::Text {
            pattern,
            date_format,
        } = Mapping::parse(&text).unwrap().balance
        else {
            panic!("expected text")
        };
        assert!(pattern.is_match("Balance 1.1.2026: 5"));
        assert_eq!(date_format.unwrap().as_str(), "%d.%m.%Y");

        for (balance, detail) in [
            ("from = \"column\"", "column"),
            (
                "from = \"column\"\ncolumn = \"S\"\npattern = \"(?P<amount>x)\"",
                "column",
            ),
            ("from = \"text\"", "text"),
            ("from = \"text\"\npattern = \"(?P<sum>x)\"", "amount"),
            (
                "from = \"text\"\npattern = \"(?P<amount>x) (?P<date>y)\"",
                "date_format",
            ),
            (
                "from = \"text\"\npattern = \"(?P<amount>x)\"\ndate_format = \"%d%m%Y\"",
                "date",
            ),
            ("from = \"none\"\ncolumn = \"S\"", "none"),
        ] {
            let text = MINIMAL.replace("from = \"none\"", balance);
            match err(&text) {
                MappingError::Inconsistent(d) => assert!(d.contains(detail), "{balance}: {d}"),
                other => panic!("{balance}: {other:?}"),
            }
        }
        let text = MINIMAL.replace("from = \"none\"", "from = \"bank\"");
        assert!(matches!(err(&text), MappingError::Syntax(_)));
    }

    #[test]
    fn amount_needs_column_or_debit_and_credit() {
        for amount in [
            "{ debit = \"Out\" }",
            "{ credit = \"In\" }",
            "{ column = \"A\", debit = \"Out\", credit = \"In\" }",
            "{ invert = true }",
        ] {
            let text = MINIMAL.replace("{ column = \"Amount\" }", amount);
            assert!(
                matches!(err(&text), MappingError::Inconsistent(_)),
                "{amount}"
            );
        }
    }

    #[test]
    fn skip_needs_exactly_one_condition() {
        let base = format!("{MINIMAL}\n[[skip]]\ncolumn = \"Status\"\n");
        assert!(matches!(err(&base), MappingError::Inconsistent(_)));
        let both = format!("{base}equals = \"a\"\nmatches = \"b\"\n");
        assert!(matches!(err(&both), MappingError::Inconsistent(_)));
        let m = Mapping::parse(&format!("{base}matches = \"^Pend\"\n")).unwrap();
        assert!(m.skip[0].matches("  Pending "));
        assert!(!m.skip[0].matches("Booked"));
    }

    #[test]
    fn skip_equals_compares_the_trimmed_cell_exactly() {
        let rule = SkipRule {
            column: "Status".into(),
            condition: SkipCondition::Equals("Vorgemerkt".into()),
        };
        assert!(rule.matches(" Vorgemerkt "));
        assert!(!rule.matches("vorgemerkt"));
        assert!(!rule.matches("Vorgemerkt (1)"));
    }

    #[test]
    fn rejects_bad_number_and_delimiter_settings() {
        for file in [
            "delimiter = \";;\"\nheader = [\"Date\", \"Amount\"]",
            "delimiter = \"\\\"\"\nheader = [\"Date\", \"Amount\"]",
            "delimiter = \"§\"\nheader = [\"Date\", \"Amount\"]",
            "header = []",
            "header = [\"Date\", \" \"]",
        ] {
            let text = MINIMAL.replace("header = [\"Date\", \"Amount\"]", file);
            assert!(
                matches!(err(&text), MappingError::Inconsistent(_)),
                "{file}"
            );
        }
        for numbers in [
            "decimal = \",\"\nthousands = \",\"",
            "decimal = \",,\"",
            "decimal = \"1\"",
            "thousands = \"-\"",
        ] {
            let text = format!("{MINIMAL}\n[numbers]\n{numbers}\n");
            assert!(
                matches!(err(&text), MappingError::Inconsistent(_)),
                "{numbers}"
            );
        }
        let text = MINIMAL.replace("[file]\n", "[file]\ndelimiter = \"\\t\"\n");
        assert_eq!(Mapping::parse(&text).unwrap().delimiter, b'\t');
    }

    #[test]
    fn column_lists_must_not_be_empty() {
        let text = MINIMAL.replace("[columns]\n", "[columns]\ncounterparty = []\n");
        assert!(matches!(err(&text), MappingError::Inconsistent(_)));
        let text = MINIMAL.replace("[columns]\n", "[columns]\npurpose = [\"A\", \"B\"]\n");
        assert_eq!(Mapping::parse(&text).unwrap().purpose, vec!["A", "B"]);
    }

    #[test]
    fn mappings_compare_by_pattern_source() {
        assert_eq!(
            Mapping::parse(SPEC_EXAMPLE).unwrap(),
            Mapping::parse(SPEC_EXAMPLE).unwrap()
        );
        let other = SPEC_EXAMPLE.replace("^Kontostand", "^Saldo");
        assert_ne!(
            Mapping::parse(SPEC_EXAMPLE).unwrap(),
            Mapping::parse(&other).unwrap()
        );
    }
}
