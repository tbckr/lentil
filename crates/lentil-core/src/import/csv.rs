//! The CSV adapter (spec section 15).
//!
//! The adapter reads an export strictly: the header row must be found,
//! every column the mapping names must exist, and every table row must
//! either parse or match a `[[skip]]` entry. Anything else aborts the import
//! with the row's line, so a changed export format is never guessed.

use std::collections::HashMap;

use rust_decimal::Decimal;

use super::mapping::{AmountColumns, BalanceSource, Mapping, SkipRule};
use super::{ImportError, Record, StatedBalance, Statement};

/// Field names used in [`ImportError::RowInvalid`].
pub mod field {
    /// The booking date cell.
    pub const BOOKING_DATE: &str = "booking_date";
    /// The amount cell, or the debit and credit cells.
    pub const AMOUNT: &str = "amount";
    /// The currency cell.
    pub const CURRENCY: &str = "currency";
    /// The running balance cell, or the `amount` group of a balance text.
    pub const BALANCE: &str = "balance";
    /// The `date` group of a balance text.
    pub const BALANCE_DATE: &str = "balance_date";
}

/// Reads a CSV export into a statement.
///
/// `default_currency` is the source's currency, used when the mapping has no
/// currency column.
///
/// # Errors
///
/// Returns an [`ImportError`] if the export does not match the mapping. Rows
/// are never dropped silently.
pub fn read_statement(
    bytes: &[u8],
    mapping: &Mapping,
    default_currency: &str,
) -> Result<Statement, ImportError> {
    let text = mapping
        .encoding
        .decode(bytes)
        .ok_or_else(|| ImportError::Encoding(mapping.encoding.label().to_owned()))?;
    let lines: Vec<&str> = text.lines().collect();
    let index = LineIndex::new(&text);
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(mapping.delimiter)
        .from_reader(text.as_bytes());
    let mut rows = reader.records();

    let (header_line, columns) = loop {
        let Some(row) = rows.next() else {
            return Err(ImportError::HeaderNotFound);
        };
        let row = row.map_err(|e| malformed(&e, &index))?;
        let cells: Vec<&str> = row.iter().map(str::trim).collect();
        if mapping.header.iter().all(|h| cells.iter().any(|c| c == h)) {
            break (index.line_of(&row), Columns::resolve(mapping, &cells)?);
        }
    };

    let mut records = Vec::new();
    let mut skipped_rows = 0;
    let mut end_line = None;
    let mut references: HashMap<String, usize> = HashMap::new();
    for row in rows {
        let row = row.map_err(|e| malformed(&e, &index))?;
        let line = index.line_of(&row);
        if let Some(end) = &mapping.end
            && line
                .checked_sub(1)
                .and_then(|i| lines.get(i))
                .is_some_and(|l| end.is_match(l))
        {
            end_line = Some(line);
            break;
        }
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        if columns
            .skip
            .iter()
            .any(|(index, rule)| rule.matches(cell(&row, *index)))
        {
            skipped_rows += 1;
            continue;
        }
        let record = columns.record(&row, line, mapping, default_currency)?;
        if let Some(reference) = &record.reference
            && let Some(first_line) = references.insert(reference.clone(), line)
        {
            return Err(ImportError::DuplicateReference {
                line,
                first_line,
                reference: reference.clone(),
            });
        }
        records.push(record);
    }

    let stated_balance = match &mapping.balance {
        BalanceSource::Text {
            pattern,
            date_format,
        } => {
            let before = lines.iter().enumerate().take(header_line.saturating_sub(1));
            let after = end_line
                .map(|end| lines.iter().enumerate().skip(end.saturating_sub(1)))
                .into_iter()
                .flatten();
            let found = before
                .chain(after)
                .find_map(|(i, line)| pattern.regex().captures(line).map(|caps| (i + 1, caps)));
            let Some((line, caps)) = found else {
                return Err(ImportError::BalanceNotFound);
            };
            let amount_text = caps.name("amount").map_or("", |m| m.as_str());
            let amount = parse_amount(amount_text, mapping.decimal, mapping.thousands)
                .ok_or_else(|| invalid(line, field::BALANCE, amount_text))?;
            let date = match (caps.name("date"), date_format) {
                (Some(m), Some(format)) => Some(
                    format
                        .parse(m.as_str())
                        .ok_or_else(|| invalid(line, field::BALANCE_DATE, m.as_str()))?,
                ),
                _ => None,
            };
            Some(StatedBalance { amount, date })
        }
        BalanceSource::Column(_) | BalanceSource::None => None,
    };

    Ok(Statement {
        records,
        stated_balance,
        skipped_rows,
    })
}

/// Column indexes resolved against the header row.
struct Columns<'m> {
    booking_date: usize,
    amount: AmountIndexes,
    currency: Option<usize>,
    counterparty: Vec<usize>,
    purpose: Vec<usize>,
    reference: Option<usize>,
    skip: Vec<(usize, &'m SkipRule)>,
    balance: Option<usize>,
}

enum AmountIndexes {
    Single {
        column: usize,
        invert: bool,
    },
    DebitCredit {
        debit: usize,
        credit: usize,
        invert: bool,
    },
}

impl<'m> Columns<'m> {
    /// Finds every column the mapping names, in the mapping's order.
    fn resolve(mapping: &'m Mapping, header: &[&str]) -> Result<Self, ImportError> {
        let find = |name: &str| {
            header
                .iter()
                .position(|h| *h == name.trim())
                .ok_or_else(|| ImportError::ColumnNotFound(name.to_owned()))
        };
        let find_all =
            |names: &[String]| names.iter().map(|n| find(n)).collect::<Result<Vec<_>, _>>();
        Ok(Self {
            booking_date: find(&mapping.booking_date.column)?,
            amount: match &mapping.amount {
                AmountColumns::Single { column, invert } => AmountIndexes::Single {
                    column: find(column)?,
                    invert: *invert,
                },
                AmountColumns::DebitCredit {
                    debit,
                    credit,
                    invert,
                } => AmountIndexes::DebitCredit {
                    debit: find(debit)?,
                    credit: find(credit)?,
                    invert: *invert,
                },
            },
            currency: mapping.currency.as_deref().map(find).transpose()?,
            counterparty: find_all(&mapping.counterparty)?,
            purpose: find_all(&mapping.purpose)?,
            reference: mapping.reference.as_deref().map(find).transpose()?,
            skip: mapping
                .skip
                .iter()
                .map(|rule| Ok((find(&rule.column)?, rule)))
                .collect::<Result<Vec<_>, ImportError>>()?,
            balance: match &mapping.balance {
                BalanceSource::Column(column) => Some(find(column)?),
                BalanceSource::Text { .. } | BalanceSource::None => None,
            },
        })
    }

    /// Parses one table row.
    fn record(
        &self,
        row: &::csv::StringRecord,
        line: usize,
        mapping: &Mapping,
        default_currency: &str,
    ) -> Result<Record, ImportError> {
        let number = |index: usize, name: &str| {
            let value = cell(row, index).trim();
            parse_amount(value, mapping.decimal, mapping.thousands)
                .ok_or_else(|| invalid(line, name, value))
        };

        let date_cell = cell(row, self.booking_date).trim();
        let booking_date = mapping
            .booking_date
            .format
            .parse(date_cell)
            .ok_or_else(|| invalid(line, field::BOOKING_DATE, date_cell))?;

        let amount = match self.amount {
            AmountIndexes::Single { column, invert } => {
                flip(number(column, field::AMOUNT)?, invert)
            }
            AmountIndexes::DebitCredit {
                debit,
                credit,
                invert,
            } => {
                let debit_cell = cell(row, debit).trim();
                let credit_cell = cell(row, credit).trim();
                let amount = match (debit_cell.is_empty(), credit_cell.is_empty()) {
                    (true, false) => number(credit, field::AMOUNT)?,
                    (false, true) => -number(debit, field::AMOUNT)?.abs(),
                    _ => {
                        return Err(invalid(
                            line,
                            field::AMOUNT,
                            &format!("{debit_cell}|{credit_cell}"),
                        ));
                    }
                };
                flip(amount, invert)
            }
        };

        let currency = match self.currency {
            Some(index) => {
                let value = cell(row, index).trim();
                if !is_currency(value) {
                    return Err(invalid(line, field::CURRENCY, value));
                }
                value.to_owned()
            }
            None => default_currency.to_owned(),
        };

        let balance_after = self
            .balance
            .map(|index| number(index, field::BALANCE))
            .transpose()?;

        Ok(Record {
            booking_date,
            amount,
            currency,
            counterparty: joined(row, &self.counterparty),
            purpose: joined(row, &self.purpose),
            reference: self
                .reference
                .map(|index| cell(row, index).trim().to_owned())
                .filter(|r| !r.is_empty()),
            balance_after,
            line,
        })
    }
}

/// The cell at `index`, or `""` if the row is shorter.
fn cell(row: &::csv::StringRecord, index: usize) -> &str {
    row.get(index).unwrap_or_default()
}

/// The non-empty trimmed cells joined by one space, or `None`.
fn joined(row: &::csv::StringRecord, indexes: &[usize]) -> Option<String> {
    let parts: Vec<&str> = indexes
        .iter()
        .map(|&i| cell(row, i).trim())
        .filter(|c| !c.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn flip(amount: Decimal, invert: bool) -> Decimal {
    if invert { -amount } else { amount }
}

fn malformed(error: &::csv::Error, index: &LineIndex<'_>) -> ImportError {
    ImportError::Malformed {
        line: error.position().map_or(0, |p| index.line_at(p.byte())),
    }
}

/// 1-based line numbers from byte offsets.
///
/// The csv crate's own line counter skips blank lines and counts a CRLF as
/// two lines in some states, and a record's byte offset points at the blank
/// lines before it. So lines are computed here from the offset, after
/// skipping line breaks.
struct LineIndex<'t> {
    text: &'t str,
    starts: Vec<usize>,
}

impl<'t> LineIndex<'t> {
    fn new(text: &'t str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, starts }
    }

    fn line_at(&self, byte: u64) -> usize {
        let byte = usize::try_from(byte)
            .unwrap_or(usize::MAX)
            .min(self.text.len());
        let skipped = self.text.as_bytes()[byte..]
            .iter()
            .take_while(|b| matches!(b, b'\r' | b'\n'))
            .count();
        self.starts.partition_point(|&s| s <= byte + skipped)
    }

    fn line_of(&self, row: &::csv::StringRecord) -> usize {
        row.position().map_or(0, |p| self.line_at(p.byte()))
    }
}

fn invalid(line: usize, field: &str, value: &str) -> ImportError {
    ImportError::RowInvalid {
        line,
        field: field.to_owned(),
        value: value.to_owned(),
    }
}

/// Parses an amount: drops `thousands`, reads `decimal` as the decimal point
/// and accepts a leading `+` or `-`. Digits are required before the point and,
/// if there is a point, after it.
#[must_use]
pub fn parse_amount(text: &str, decimal: char, thousands: Option<char>) -> Option<Decimal> {
    let text = text.trim();
    let (negative, unsigned) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let plain: String = unsigned
        .chars()
        .filter(|&c| Some(c) != thousands)
        .map(|c| if c == decimal { '.' } else { c })
        .collect();
    let (int, frac) = match plain.split_once('.') {
        Some((int, frac)) => (int, Some(frac)),
        None => (plain.as_str(), None),
    };
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(int) || frac.is_some_and(|f| !all_digits(f)) {
        return None;
    }
    let value = Decimal::from_str_exact(&plain).ok()?;
    Some(if negative { -value } else { value })
}

/// Whether `text` is a Beancount currency: an uppercase letter, then up to
/// 22 of `A-Z 0-9 ' . _ -`, ending in a letter or digit.
fn is_currency(text: &str) -> bool {
    let bytes = text.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    bytes.len() <= 24
        && first.is_ascii_uppercase()
        && (last.is_ascii_uppercase() || last.is_ascii_digit())
        && bytes.iter().all(|b| {
            b.is_ascii_uppercase() || b.is_ascii_digit() || matches!(b, b'\'' | b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::Date;

    const MAPPING: &str = r#"
[file]
delimiter = ";"
header = ["Date", "Amount"]
end = "^Balance"

[numbers]
decimal = ","
thousands = "."

[columns]
booking_date = { column = "Date", format = "%d.%m.%Y" }
amount = { column = "Amount" }
counterparty = ["Name", "Name 2"]
purpose = ["Text"]
reference = "Ref"

[[skip]]
column = "Status"
equals = "Pending"

[balance]
from = "text"
pattern = 'Balance on (?P<date>\S+): (?P<amount>\S+)'
date_format = "%d.%m.%Y"
"#;

    const EXPORT: &str = "Account;Example Giro\n\
\n\
Status;Name;Date;Amount;Text;Ref;Name 2\n\
Booked;Corner  Grocery;18.10.2026;-42,17;Card payment;;\n\
Pending;Bakery;19.10.2026;-3,10;Bread;;\n\
;;;;;;\n\
Booked;City Utilities;19.10.2026;-1.064,00;Electricity;R-1;Ltd.\n\
Booked;;20.10.2026;+5;;;\n\
Balance on 20.10.2026: 1.234,56\n\
Booked;ignored after end;21.10.2026;x;;;\n";

    fn mapping(text: &str) -> Mapping {
        Mapping::parse(text).unwrap()
    }

    fn read(export: &str, mapping_text: &str) -> Result<Statement, ImportError> {
        read_statement(export.as_bytes(), &mapping(mapping_text), "EUR")
    }

    #[test]
    fn reads_records_skips_and_stated_balance() {
        let statement = read(EXPORT, MAPPING).unwrap();
        assert_eq!(statement.skipped_rows, 1);
        assert_eq!(
            statement.stated_balance,
            Some(StatedBalance {
                amount: Decimal::new(123_456, 2),
                date: Some(Date::constant(2026, 10, 20)),
            })
        );
        assert_eq!(
            statement.records,
            vec![
                Record {
                    booking_date: Date::constant(2026, 10, 18),
                    amount: Decimal::new(-4217, 2),
                    currency: "EUR".into(),
                    counterparty: Some("Corner  Grocery".into()),
                    purpose: Some("Card payment".into()),
                    reference: None,
                    balance_after: None,
                    line: 4,
                },
                Record {
                    booking_date: Date::constant(2026, 10, 19),
                    amount: Decimal::new(-106_400, 2),
                    currency: "EUR".into(),
                    counterparty: Some("City Utilities Ltd.".into()),
                    purpose: Some("Electricity".into()),
                    reference: Some("R-1".into()),
                    balance_after: None,
                    line: 7,
                },
                Record {
                    booking_date: Date::constant(2026, 10, 20),
                    amount: Decimal::new(5, 0),
                    currency: "EUR".into(),
                    counterparty: None,
                    purpose: None,
                    reference: None,
                    balance_after: None,
                    line: 8,
                },
            ]
        );
    }

    #[test]
    fn header_is_the_first_row_with_every_header_cell() {
        // The preamble mentions "Date" but not "Amount"; the header cells are trimmed.
        let export = "Date;of export\n Status ; Date ;Amount;Name;Name 2;Text;Ref\nBooked;18.10.2026;1;;;;\n";
        let statement = read(export, &MAPPING.replace("end = \"^Balance\"\n", "").replace(
            "from = \"text\"\npattern = 'Balance on (?P<date>\\S+): (?P<amount>\\S+)'\ndate_format = \"%d.%m.%Y\"",
            "from = \"none\"",
        ))
        .unwrap();
        assert_eq!(statement.records.len(), 1);
        assert_eq!(statement.records[0].line, 3);
    }

    #[test]
    fn missing_header_aborts() {
        assert_eq!(
            read("Datum;Betrag\n18.10.2026;1\n", MAPPING),
            Err(ImportError::HeaderNotFound)
        );
        assert_eq!(read("", MAPPING), Err(ImportError::HeaderNotFound));
    }

    #[test]
    fn columns_are_found_by_name_in_any_order() {
        let reordered = "Ref;Text;Amount;Date;Name 2;Name;Status\n\
;Card payment;-42,17;18.10.2026;;Corner Grocery;Booked\n\
Balance on 18.10.2026: 0\n";
        let statement = read(reordered, MAPPING).unwrap();
        assert_eq!(statement.records[0].amount, Decimal::new(-4217, 2));
        assert_eq!(
            statement.records[0].counterparty.as_deref(),
            Some("Corner Grocery")
        );
    }

    #[test]
    fn renamed_column_aborts_and_names_it() {
        let export = EXPORT.replace(";Text;", ";Purpose;");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::ColumnNotFound("Text".into()))
        );
        let export = EXPORT.replace("Status;", "State;");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::ColumnNotFound("Status".into()))
        );
    }

    #[test]
    fn unparsable_row_aborts_with_its_line() {
        let export = EXPORT.replace("+5", "five");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::RowInvalid {
                line: 8,
                field: "amount".into(),
                value: "five".into()
            })
        );
        let export = EXPORT.replace("18.10.2026", "2026-10-18");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::RowInvalid {
                line: 4,
                field: "booking_date".into(),
                value: "2026-10-18".into()
            })
        );
    }

    #[test]
    fn short_rows_fail_rather_than_vanish() {
        let export = EXPORT.replace("Booked;;20.10.2026;+5;;;", "Booked;x");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::RowInvalid {
                line: 8,
                field: "booking_date".into(),
                value: String::new()
            })
        );
    }

    #[test]
    fn skip_matches_by_regex() {
        let text = MAPPING.replace("equals = \"Pending\"", "matches = \"^Pend|^Vorg\"");
        let export = EXPORT.replace("Pending;", "Vorgemerkt;");
        assert_eq!(read(&export, &text).unwrap().skipped_rows, 1);
    }

    #[test]
    fn skipped_rows_need_not_parse() {
        let export = EXPORT.replace("Pending;Bakery;19.10.2026;-3,10", "Pending;Bakery;;");
        assert_eq!(read(&export, MAPPING).unwrap().records.len(), 3);
    }

    #[test]
    fn table_ends_at_eof_without_end() {
        let text = MAPPING
            .replace("end = \"^Balance\"\n", "")
            .replace("from = \"text\"\npattern = 'Balance on (?P<date>\\S+): (?P<amount>\\S+)'\ndate_format = \"%d.%m.%Y\"", "from = \"none\"");
        let export = "Status;Name;Date;Amount;Text;Ref;Name 2\nBooked;A;18.10.2026;1;;;\n\nBooked;B;19.10.2026;2;;;\n";
        let statement = read(export, &text).unwrap();
        assert_eq!(statement.records.len(), 2);
        assert_eq!(statement.records[1].line, 4);
        assert_eq!(statement.stated_balance, None);
    }

    #[test]
    fn text_balance_may_precede_the_header() {
        let export = "Balance on 31.10.2026: -12,50\nStatus;Name;Date;Amount;Text;Ref;Name 2\nBooked;A;18.10.2026;1;;;\n";
        assert_eq!(
            read(export, MAPPING).unwrap().stated_balance,
            Some(StatedBalance {
                amount: Decimal::new(-1250, 2),
                date: Some(Date::constant(2026, 10, 31)),
            })
        );
    }

    #[test]
    fn text_balance_ignores_table_rows() {
        // A matching line inside the table is a row, not the balance.
        let text = MAPPING.replace("end = \"^Balance\"\n", "");
        let export = "Status;Name;Date;Amount;Text;Ref;Name 2\nBooked;A;18.10.2026;1;;;\n";
        assert_eq!(read(export, &text), Err(ImportError::BalanceNotFound));
        let export = EXPORT.replace("Balance on 20.10.2026: 1.234,56", "Balance unknown");
        assert_eq!(read(&export, MAPPING), Err(ImportError::BalanceNotFound));
    }

    #[test]
    fn text_balance_without_date_and_bad_values() {
        let text = MAPPING.replace(
            "pattern = 'Balance on (?P<date>\\S+): (?P<amount>\\S+)'\ndate_format = \"%d.%m.%Y\"",
            "pattern = 'Balance on \\S+: (?P<amount>\\S+)'",
        );
        assert_eq!(
            read(EXPORT, &text).unwrap().stated_balance,
            Some(StatedBalance {
                amount: Decimal::new(123_456, 2),
                date: None
            })
        );
        let export = EXPORT.replace(": 1.234,56", ": n/a");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::RowInvalid {
                line: 9,
                field: "balance".into(),
                value: "n/a".into()
            })
        );
        let export = EXPORT.replace("on 20.10.2026:", "on 2026-10-20:");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::RowInvalid {
                line: 9,
                field: "balance_date".into(),
                value: "2026-10-20".into()
            })
        );
    }

    const DEBIT_CREDIT: &str = r#"
[file]
header = ["Date", "Debit", "Credit"]

[columns]
booking_date = { column = "Date", format = "%Y-%m-%d" }
amount = { debit = "Debit", credit = "Credit" }
currency = "Currency"

[balance]
from = "column"
column = "Balance"
"#;

    #[test]
    fn debit_is_made_negative_and_credit_kept() {
        let export = "Date,Debit,Credit,Currency,Balance\n\
2026-10-03,12.50,,EUR,87.50\n\
2026-10-02,-10.00,,EUR,100.00\n\
2026-10-01,,110.00,EUR,110.00\n";
        let records = read(export, DEBIT_CREDIT).unwrap().records;
        let amounts: Vec<_> = records.iter().map(|r| r.amount).collect();
        assert_eq!(
            amounts,
            vec![
                Decimal::new(-1250, 2),
                Decimal::new(-1000, 2),
                Decimal::new(11000, 2)
            ]
        );
        let balances: Vec<_> = records.iter().map(|r| r.balance_after.unwrap()).collect();
        assert_eq!(
            balances,
            vec![
                Decimal::new(8750, 2),
                Decimal::new(10000, 2),
                Decimal::new(11000, 2)
            ]
        );
    }

    #[test]
    fn debit_and_credit_need_exactly_one_number() {
        for (row, value) in [
            ("2026-10-01,1.00,2.00,EUR,0", "1.00|2.00"),
            ("2026-10-01,,,EUR,0", "|"),
        ] {
            let export = format!("Date,Debit,Credit,Currency,Balance\n{row}\n");
            assert_eq!(
                read(&export, DEBIT_CREDIT),
                Err(ImportError::RowInvalid {
                    line: 2,
                    field: "amount".into(),
                    value: value.into()
                }),
                "{row}"
            );
        }
        let export = "Date,Debit,Credit,Currency,Balance\n2026-10-01,,abc,EUR,0\n";
        assert!(matches!(
            read(export, DEBIT_CREDIT),
            Err(ImportError::RowInvalid { .. })
        ));
    }

    #[test]
    fn invert_flips_after_reading() {
        let text =
            DEBIT_CREDIT.replace("credit = \"Credit\"", "credit = \"Credit\", invert = true");
        let export =
            "Date,Debit,Credit,Currency,Balance\n2026-10-01,5,,EUR,1\n2026-10-02,,7,EUR,1\n";
        let amounts: Vec<_> = read(export, &text)
            .unwrap()
            .records
            .iter()
            .map(|r| r.amount)
            .collect();
        assert_eq!(amounts, vec![Decimal::new(5, 0), Decimal::new(-7, 0)]);

        let single = MAPPING.replace(
            "{ column = \"Amount\" }",
            "{ column = \"Amount\", invert = true }",
        );
        let records = read(EXPORT, &single).unwrap().records;
        assert_eq!(records[0].amount, Decimal::new(4217, 2));
        assert_eq!(records[2].amount, Decimal::new(-5, 0));
    }

    #[test]
    fn currency_column_must_hold_a_currency() {
        let export = "Date,Debit,Credit,Currency,Balance\n2026-10-01,5,,€,1\n";
        assert_eq!(
            read(export, DEBIT_CREDIT),
            Err(ImportError::RowInvalid {
                line: 2,
                field: "currency".into(),
                value: "€".into()
            })
        );
        let export = "Date,Debit,Credit,Currency,Balance\n2026-10-01,5,,USD,1\n";
        assert_eq!(
            read(export, DEBIT_CREDIT).unwrap().records[0].currency,
            "USD"
        );
    }

    #[test]
    fn balance_column_must_parse() {
        let export = "Date,Debit,Credit,Currency,Balance\n2026-10-01,5,,EUR,\n";
        assert_eq!(
            read(export, DEBIT_CREDIT),
            Err(ImportError::RowInvalid {
                line: 2,
                field: "balance".into(),
                value: String::new()
            })
        );
    }

    #[test]
    fn quoted_cells_and_crlf_line_endings() {
        let export = "Date,Debit,Credit,Currency,Balance\r\n\"2026-10-01\",\"1,234.50\",,EUR,\"-1,234.50\"\r\n";
        let text = DEBIT_CREDIT.replace("[columns]", "[numbers]\nthousands = \",\"\n\n[columns]");
        let record = &read(export, &text).unwrap().records[0];
        assert_eq!(record.amount, Decimal::new(-123_450, 2));
        assert_eq!(record.balance_after, Some(Decimal::new(-123_450, 2)));
        assert_eq!(record.line, 2);
    }

    #[test]
    fn repeated_reference_aborts() {
        let export = EXPORT
            .replace(";Electricity;R-1;", ";Electricity;R-2;")
            .replace("Booked;;20.10.2026;+5;;;", "Booked;;20.10.2026;+5;;R-2;");
        assert_eq!(
            read(&export, MAPPING),
            Err(ImportError::DuplicateReference {
                line: 8,
                first_line: 7,
                reference: "R-2".into()
            })
        );
    }

    #[test]
    fn line_numbers_count_blank_lines_and_crlf() {
        let text = MAPPING.replace("+5", "x");
        let export = EXPORT.replace('\n', "\r\n").replace("+5", "x");
        assert_eq!(
            read(&export, &text),
            Err(ImportError::RowInvalid {
                line: 8,
                field: "amount".into(),
                value: "x".into()
            })
        );
    }

    #[test]
    fn invalid_utf8_is_an_encoding_error() {
        assert_eq!(
            read_statement(b"Date;Amount\n\xE4\n", &mapping(MAPPING), "EUR"),
            Err(ImportError::Encoding("utf-8".into()))
        );
    }

    #[test]
    fn amounts_follow_the_number_settings() {
        let p = |s| parse_amount(s, ',', Some('.'));
        assert_eq!(p("1.234,56"), Some(Decimal::new(123_456, 2)));
        assert_eq!(p("-1.234,56"), Some(Decimal::new(-123_456, 2)));
        assert_eq!(p("+0,50"), Some(Decimal::new(50, 2)));
        assert_eq!(p(" 7 "), Some(Decimal::new(7, 0)));
        for bad in [
            "",
            "-",
            "1,2,3",
            ",50",
            "5,",
            "1.234.56,",
            "--1",
            "1e3",
            "12 EUR",
            "1,23-",
        ] {
            assert_eq!(p(bad), None, "{bad}");
        }
        let q = |s| parse_amount(s, '.', None);
        assert_eq!(q("-64.00"), Some(Decimal::new(-6400, 2)));
        assert_eq!(q("1,234.00"), None);
        assert_eq!(q("1.234,00"), None);
    }

    #[test]
    fn currency_syntax() {
        for ok in ["EUR", "USD", "X", "VBTLX", "A.B_C-D'E1"] {
            assert!(is_currency(ok), "{ok}");
        }
        for bad in ["", "eur", "1EUR", "EUR-", "€", "E UR", &"A".repeat(25)] {
            assert!(!is_currency(bad), "{bad}");
        }
    }
}
