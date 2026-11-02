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
    AccountItem, FfiAuthor, FfiCredentials, FfiFinding, FfiIdentity, FfiIntent, FfiOutcome,
    FfiPendingIntent, FfiSeverity, IntentOutcome, QueueItem, SyncReport,
};

/// A new device ID of spec section 9, created once on the device's first start.
#[uniffi::export]
#[must_use]
pub fn new_device_id() -> String {
    lentil_core::id::new_device_id(&mut lentil_core::env::OsRandom)
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
