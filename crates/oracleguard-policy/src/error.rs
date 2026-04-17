//! Evaluator error types.
//!
//! Owns: the closed set of structured error values the evaluator can
//! return alongside a deny decision.
//!
//! Does NOT own: panics (the evaluator must not panic), I/O error
//! propagation (there is no I/O in this crate), or human-readable
//! error formatting beyond `Display` impls.
