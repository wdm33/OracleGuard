//! Bundle integrity checks.
//!
//! Owns: verifying that the bytes and digests in a loaded evidence
//! bundle match the canonical encoding defined in
//! `oracleguard-schemas`.
//!
//! Does NOT own: producing the bundle, nor re-executing the evaluator
//! (that is the replay module using `oracleguard-policy`).
