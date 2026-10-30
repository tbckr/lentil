//! Entry identity (spec section 5) and device IDs (spec section 9).

use jiff::Timestamp;

use crate::env::Random;
use crate::import::Record;

/// Collapses inner whitespace to one space and trims (spec section 5).
#[must_use]
pub fn normalize_text(text: &str) -> String {
    let _ = text;
    todo!("task B")
}

/// `lentil_desc`: counterparty and purpose joined by one space, normalized (spec section 15).
#[must_use]
pub fn description(record: &Record) -> String {
    let _ = record;
    todo!("task B")
}

/// The deterministic `lentil_id` of an imported record (spec section 5).
///
/// With a reference: `<source-id>-` + first 12 hex chars of
/// SHA-256(`<source-id>|<reference>`). Without: SHA-256 over
/// `<source-id>|<booking date>|<amount>|<currency>|<counterparty>|<purpose>|<n>`,
/// where `occurrence` is `n`.
#[must_use]
pub fn import_id(source_id: &str, record: &Record, occurrence: usize) -> String {
    let _ = (source_id, record, occurrence);
    todo!("task B")
}

/// Assigns IDs to records of one export, computing occurrence indexes in the
/// order the bank lists them.
#[must_use]
pub fn import_ids(source_id: &str, records: &[Record]) -> Vec<String> {
    let _ = (source_id, records);
    todo!("task B")
}

/// A new device ID: 8 random lowercase base32 characters (spec section 9).
pub fn new_device_id(random: &mut dyn Random) -> String {
    let _ = random;
    todo!("task B")
}

/// A lowercase ULID (Crockford base32, 26 characters).
pub fn new_ulid(now: Timestamp, random: &mut dyn Random) -> String {
    let _ = (now, random);
    todo!("task B")
}

/// `lentil_id` of a manual entry: `<device-id>-<ulid>` (spec section 5).
pub fn manual_id(device_id: &str, now: Timestamp, random: &mut dyn Random) -> String {
    format!("{device_id}-{}", new_ulid(now, random))
}
