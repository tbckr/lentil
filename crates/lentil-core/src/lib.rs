//! Lentil core: everything every client computes, so all clients agree.
//!
//! Business logic (replay, ID generation, validation, import planning) is pure
//! and works on [`snapshot::Snapshot`]s. I/O happens only behind the boundary
//! traits in [`env`] and [`sync::GitBackend`].

pub mod config;
pub mod correction;
pub mod edit;
pub mod env;
pub mod finding;
pub mod id;
pub mod import;
pub mod intent;
pub mod layout;
pub mod learn;
pub mod ledger;
pub mod reapply;
pub mod snapshot;
pub mod status;
pub mod sync;
pub mod tags;
pub mod trips;
pub mod validate;

/// Name and version of this core, for `Lentil-Client` trailers of embedding clients.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
