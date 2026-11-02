//! Entry identity (spec section 5) and device IDs (spec section 9).

use std::collections::HashMap;

use jiff::Timestamp;
use sha2::{Digest, Sha256};

use crate::env::Random;
use crate::import::Record;

/// Number of hex characters of the hash in an imported `lentil_id`.
const HASH_CHARS: usize = 12;

/// RFC 4648 base32 alphabet in lowercase, for device IDs.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Crockford base32 alphabet in lowercase, for ULIDs.
const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Collapses inner whitespace to one space and trims (spec section 5).
#[must_use]
pub fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The original text of a record (spec section 5). The importer looks up
/// learned suggestions by it and shows it for possible duplicates.
#[must_use]
pub fn description(record: &Record) -> String {
    original_text(record.counterparty.as_deref(), record.purpose.as_deref())
}

/// The original text (spec section 5): counterparty and purpose joined by one
/// space and normalized, a missing one counting as empty.
#[must_use]
pub fn original_text(counterparty: Option<&str>, purpose: Option<&str>) -> String {
    let counterparty = counterparty.unwrap_or_default();
    let purpose = purpose.unwrap_or_default();
    normalize_text(&format!("{counterparty} {purpose}"))
}

/// The deterministic `lentil_id` of an imported record (spec section 5).
///
/// With a reference: `<source-id>-` + first 12 hex chars of
/// SHA-256(`<source-id>|<reference>`). Without: SHA-256 over
/// `<source-id>|<booking date>|<amount>|<currency>|<counterparty>|<purpose>|<n>`,
/// where `occurrence` is `n`.
///
/// Amounts are hashed normalized, so `-64.00` and `-64` give the same ID;
/// text is trimmed with inner whitespace collapsed, and a missing
/// counterparty or purpose hashes as the empty string.
#[must_use]
pub fn import_id(source_id: &str, record: &Record, occurrence: usize) -> String {
    let input = match reference(record) {
        Some(reference) => format!("{source_id}|{reference}"),
        None => format!("{}|{occurrence}", field_key(source_id, record)),
    };
    hashed_id(source_id, &input)
}

/// Assigns IDs to records of one export, computing occurrence indexes in the
/// order the bank lists them.
///
/// Only records without a reference are hashed by their fields, so only they
/// count for each other's occurrence index.
#[must_use]
pub fn import_ids(source_id: &str, records: &[Record]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    records
        .iter()
        .map(|record| {
            if reference(record).is_some() {
                return import_id(source_id, record, 0);
            }
            let count = seen.entry(field_key(source_id, record)).or_insert(0);
            let occurrence = *count;
            *count += 1;
            import_id(source_id, record, occurrence)
        })
        .collect()
}

/// `<source-id>-` + the first 12 hex characters of SHA-256 over `input`.
#[must_use]
pub fn hashed_id(source_id: &str, input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let hex: String = digest
        .iter()
        .take(HASH_CHARS / 2)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{source_id}-{hex}")
}

/// Whether `id` has the shape of an imported ID of `source_id`:
/// `<source-id>-` followed by exactly 12 lowercase hex characters. A prefix
/// test alone would let a source `giro` claim the IDs of a source `giro-2`.
#[must_use]
pub fn is_import_id_of(source_id: &str, id: &str) -> bool {
    id.strip_prefix(source_id)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|hash| {
            hash.len() == HASH_CHARS && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// The normalized reference, if the record has a non-empty one.
fn reference(record: &Record) -> Option<String> {
    record
        .reference
        .as_deref()
        .map(normalize_text)
        .filter(|r| !r.is_empty())
}

/// The hash input without the occurrence index.
fn field_key(source_id: &str, record: &Record) -> String {
    format!(
        "{source_id}|{}|{}|{}|{}|{}",
        record.booking_date,
        record.amount.normalize(),
        record.currency.trim(),
        normalize_text(record.counterparty.as_deref().unwrap_or_default()),
        normalize_text(record.purpose.as_deref().unwrap_or_default()),
    )
}

/// A new device ID: 8 random lowercase base32 characters (spec section 9).
pub fn new_device_id(random: &mut dyn Random) -> String {
    // 40 random bits in the low bytes of a big-endian integer.
    let mut bytes = [0u8; 16];
    random.fill(&mut bytes[11..]);
    encode_base32(u128::from_be_bytes(bytes), 8, BASE32)
}

/// A lowercase ULID (Crockford base32, 26 characters): a 48-bit millisecond
/// timestamp followed by 80 random bits. Times before 1970 encode as zero.
pub fn new_ulid(now: Timestamp, random: &mut dyn Random) -> String {
    let millis = u64::try_from(now.as_millisecond()).unwrap_or(0);
    let mut bytes = [0u8; 16];
    // The low 6 bytes of the timestamp; jiff's range ends long before 2^48 ms.
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    random.fill(&mut bytes[6..]);
    encode_base32(u128::from_be_bytes(bytes), 26, CROCKFORD)
}

/// The lowest `5 * chars` bits of `value` in base32, most significant first.
fn encode_base32(value: u128, chars: usize, alphabet: &[u8; 32]) -> String {
    (0..chars)
        .rev()
        .map(|i| {
            let index = (value >> (5 * i)) & 0x1f;
            char::from(alphabet[usize::from(u8::try_from(index).unwrap_or(0))])
        })
        .collect()
}

/// `lentil_id` of a manual entry: `<device-id>-<ulid>` (spec section 5).
pub fn manual_id(device_id: &str, now: Timestamp, random: &mut dyn Random) -> String {
    format!("{device_id}-{}", new_ulid(now, random))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::CountingRandom;
    use jiff::civil::Date;
    use rust_decimal::Decimal;

    fn record(amount: Decimal, counterparty: Option<&str>, purpose: Option<&str>) -> Record {
        Record {
            booking_date: Date::constant(2026, 10, 19),
            amount,
            currency: "EUR".to_owned(),
            counterparty: counterparty.map(str::to_owned),
            purpose: purpose.map(str::to_owned),
            booking_type: None,
            reference: None,
            balance_after: None,
            line: 1,
        }
    }

    fn sha_prefix(input: &str) -> String {
        let digest = Sha256::digest(input.as_bytes());
        digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn normalize_trims_and_collapses_whitespace() {
        assert_eq!(
            normalize_text("  CORNER \t GROCERY\n 4711  "),
            "CORNER GROCERY 4711"
        );
        assert_eq!(normalize_text("   "), "");
        assert_eq!(normalize_text("a\u{a0}\u{a0}b"), "a b");
    }

    #[test]
    fn description_joins_counterparty_and_purpose() {
        let r = record(
            Decimal::ONE,
            Some(" CORNER  GROCERY "),
            Some("4711 CARD\tPAYMENT"),
        );
        assert_eq!(description(&r), "CORNER GROCERY 4711 CARD PAYMENT");
        let r = record(Decimal::ONE, None, Some(" Card payment "));
        assert_eq!(description(&r), "Card payment");
        let r = record(Decimal::ONE, Some("Bakery"), None);
        assert_eq!(description(&r), "Bakery");
        assert_eq!(description(&record(Decimal::ONE, None, None)), "");
    }

    #[test]
    fn id_without_reference_hashes_the_normalized_fields() {
        let r = record(
            Decimal::new(-6400, 2),
            Some("  City   Utilities "),
            Some("Electricity"),
        );
        let expected = format!(
            "giro-{}",
            sha_prefix("giro|2026-10-19|-64|EUR|City Utilities|Electricity|0")
        );
        assert_eq!(import_id("giro", &r, 0), expected);
        assert_eq!(expected.len(), "giro-".len() + 12);
        assert!(
            expected[5..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn amounts_hash_alike_regardless_of_trailing_zeros() {
        let a = record(Decimal::new(-6400, 2), None, Some("x"));
        let b = record(Decimal::new(-64, 0), None, Some("x"));
        let c = record(Decimal::new(-6410, 2), None, Some("x"));
        assert_eq!(import_id("giro", &a, 0), import_id("giro", &b, 0));
        assert_ne!(import_id("giro", &a, 0), import_id("giro", &c, 0));
    }

    #[test]
    fn missing_text_fields_hash_as_empty_strings() {
        let r = record(Decimal::new(5, 0), None, None);
        let expected = format!("giro-{}", sha_prefix("giro|2026-10-19|5|EUR|||0"));
        assert_eq!(import_id("giro", &r, 0), expected);
        let empty = record(Decimal::new(5, 0), Some("  "), Some(""));
        assert_eq!(import_id("giro", &empty, 0), expected);
    }

    #[test]
    fn booking_type_stays_out_of_the_id() {
        let mut r = record(Decimal::new(-6400, 2), Some("City Utilities"), None);
        let without = import_id("giro", &r, 0);
        r.booking_type = Some("Direct debit".to_owned());
        assert_eq!(import_id("giro", &r, 0), without);
        assert_eq!(import_ids("giro", &[r]), [without]);
    }

    #[test]
    fn id_with_reference_hashes_source_and_reference_only() {
        let mut r = record(Decimal::new(-6400, 2), Some("City Utilities"), None);
        r.reference = Some("2026101912345".to_owned());
        let expected = format!("giro-{}", sha_prefix("giro|2026101912345"));
        assert_eq!(import_id("giro", &r, 0), expected);
        assert_eq!(import_id("giro", &r, 3), expected);
        r.amount = Decimal::ONE;
        assert_eq!(import_id("giro", &r, 0), expected);
    }

    #[test]
    fn empty_reference_falls_back_to_fields() {
        let mut r = record(Decimal::ONE, None, Some("x"));
        let without = import_id("giro", &r, 0);
        r.reference = Some("  ".to_owned());
        assert_eq!(import_id("giro", &r, 0), without);
    }

    #[test]
    fn occurrence_index_counts_identical_records_in_export_order() {
        let a = record(Decimal::new(-310, 2), Some("Bakery"), Some("Bread"));
        let b = record(Decimal::new(-310, 2), Some("Bakery"), Some("Rolls"));
        let a_spaced = record(Decimal::new(-3100, 3), Some(" Bakery "), Some("Bread"));
        let ids = import_ids("giro", &[a.clone(), b.clone(), a_spaced, a.clone()]);
        assert_eq!(ids[0], import_id("giro", &a, 0));
        assert_eq!(ids[1], import_id("giro", &b, 0));
        assert_eq!(ids[2], import_id("giro", &a, 1));
        assert_eq!(ids[3], import_id("giro", &a, 2));
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 4);
    }

    #[test]
    fn records_with_reference_do_not_count_for_occurrences() {
        let a = record(Decimal::ONE, None, Some("x"));
        let mut with_ref = a.clone();
        with_ref.reference = Some("R1".to_owned());
        let ids = import_ids("giro", &[with_ref.clone(), a.clone()]);
        assert_eq!(ids[0], import_id("giro", &with_ref, 0));
        assert_eq!(ids[1], import_id("giro", &a, 0));
    }

    #[test]
    fn ids_depend_on_the_source() {
        let r = record(Decimal::ONE, None, Some("x"));
        assert!(import_id("card", &r, 0).starts_with("card-"));
        assert_ne!(import_id("card", &r, 0)[5..], import_id("giro", &r, 0)[5..]);
    }

    #[test]
    fn recognizes_import_ids_of_one_source() {
        let r = record(Decimal::ONE, None, Some("x"));
        assert!(is_import_id_of("giro", &import_id("giro", &r, 0)));
        assert!(!is_import_id_of("giro", &import_id("giro-2", &r, 0)));
        assert!(!is_import_id_of("giro-2", &import_id("giro", &r, 0)));
        assert!(!is_import_id_of("giro", "giro-3F9A1C2E7B4D"));
        assert!(!is_import_id_of("giro", "giro-3f9a1c2e7b4"));
        assert!(!is_import_id_of("giro", "giro3f9a1c2e7b4d0"));
        assert!(!is_import_id_of(
            "k7p2q9xd",
            "k7p2q9xd-01k81wf2g0000g40r40m30e209"
        ));
    }

    #[test]
    fn device_id_is_eight_lowercase_base32_chars() {
        let id = new_device_id(&mut CountingRandom(0));
        // Bytes 00 01 02 03 04 as 5-bit groups: 0 0 0 16 4 0 24 4.
        assert_eq!(id, "aaaqeaye");
        let id = new_device_id(&mut CountingRandom(200));
        assert_eq!(id.len(), 8);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        );
    }

    #[test]
    fn ulid_encodes_time_then_randomness() {
        let now = Timestamp::from_millisecond(1_761_000_000_000).unwrap();
        let ulid = new_ulid(now, &mut CountingRandom(0));
        assert_eq!(ulid.len(), 26);
        assert!(ulid.bytes().all(|b| CROCKFORD.contains(&b)));
        // The first 10 characters encode the 48-bit timestamp.
        assert_eq!(&ulid[..10], "01k81wf2g0");
        // Bytes 00..09 as 80 bits.
        assert_eq!(&ulid[10..], "000g40r40m30e209");
        let later = new_ulid(
            Timestamp::from_millisecond(1_761_000_000_001).unwrap(),
            &mut CountingRandom(0),
        );
        assert!(later > ulid);
    }

    #[test]
    fn ulid_of_the_epoch_starts_with_zeros() {
        let ulid = new_ulid(Timestamp::UNIX_EPOCH, &mut CountingRandom(0));
        assert_eq!(&ulid[..10], "0000000000");
        let before = Timestamp::from_second(-10).unwrap();
        assert_eq!(
            &new_ulid(before, &mut CountingRandom(0))[..10],
            "0000000000"
        );
    }

    #[test]
    fn manual_id_prefixes_the_device() {
        let id = manual_id("k7p2q9xd", Timestamp::UNIX_EPOCH, &mut CountingRandom(0));
        assert!(id.starts_with("k7p2q9xd-"));
        assert_eq!(id.len(), 8 + 1 + 26);
    }
}
