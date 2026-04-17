//! Verifier report generation.
//!
//! Owns: structured, deterministic reports summarizing the outcome of
//! integrity and replay checks for judge-facing review.
//!
//! Does NOT own: rendering reports to external systems or terminals
//! beyond what the verifier binary target needs.
