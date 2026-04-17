//! Evidence bundle canonical types.
//!
//! Owns: the canonical shape of evidence records and bundles — the
//! inputs, decisions, and authorized effects that must be preserved so
//! an offline verifier can replay the disbursement.
//!
//! Does NOT own: bundle assembly (adapter), bundle verification
//! (verifier), or any storage format beyond the canonical in-memory
//! representation.
