//! UniFFI bindings of `lentil-core` for the Android app.
//!
//! A thin layer without finance logic: it converts between the core's types
//! and FFI records and calls the core. Findings and conflicts cross as codes,
//! never as English text, and the app translates them.

uniffi::setup_scaffolding!();

mod error;
mod repo;
mod types;

pub use error::LentilError;
pub use repo::LentilRepo;
pub use types::{
    AccountItem, FfiAuthor, FfiChoiceSource, FfiCredentials, FfiFinding, FfiIdentity, FfiIntent,
    FfiOutcome, FfiPendingIntent, FfiSeverity, FfiTagChoice, FfiTagChoices, IntentOutcome,
    QueueItem, SyncReport,
};

/// A new device ID of spec section 9, created once on the device's first start.
#[uniffi::export]
#[must_use]
pub fn new_device_id() -> String {
    lentil_core::id::new_device_id(&mut lentil_core::env::OsRandom)
}

/// A new `lentil_id` for an entry this device creates, such as a trip event:
/// `<device>-<ulid>` (spec section 5).
#[uniffi::export]
#[must_use]
pub fn new_manual_id(device: &str) -> String {
    use lentil_core::env::{Clock, OsRandom, SystemClock};
    lentil_core::id::manual_id(device, SystemClock.now(), &mut OsRandom)
}

/// Whether `name` matches `[a-z0-9-]+`: a valid trip name, and what the user
/// may type after a tag family's prefix.
#[uniffi::export]
#[must_use]
pub fn is_plain_name(name: &str) -> bool {
    lentil_core::tags::is_plain_name(name)
}

/// Makes every later clone, fetch and push trust the CA certificates in the
/// PEM file at `path`. The app exports Android's trusted certificates there at
/// start, because the embedded OpenSSL cannot read Android's CA directories.
/// Call it before the first network operation; only the first call counts.
#[uniffi::export]
pub fn use_ca_bundle(path: &str) {
    lentil_core::sync::git::use_ca_bundle(std::path::Path::new(path));
}

/// The version of the embedded core.
#[uniffi::export]
#[must_use]
pub fn core_version() -> String {
    lentil_core::CORE_VERSION.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manual_ids_carry_the_device() {
        let first = new_manual_id("k7p2q9xd");
        assert_eq!(lentil_core::id::manual_device(&first), Some("k7p2q9xd"));
        assert_ne!(first, new_manual_id("k7p2q9xd"));
    }

    #[test]
    fn plain_names_come_from_the_core() {
        assert!(is_plain_name("lisbon"));
        assert!(!is_plain_name("Lisbon"));
    }
}
