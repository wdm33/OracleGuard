//! OracleGuard offline verifier.
//!
//! This crate inspects evidence bundles and replays deterministic checks
//! using the public semantics defined in `oracleguard-schemas` and
//! `oracleguard-policy`. It must not redefine semantic meaning or introduce
//! alternate interpretations of canonical types.

pub mod bundle;
pub mod integrity;
pub mod replay;
pub mod report;
