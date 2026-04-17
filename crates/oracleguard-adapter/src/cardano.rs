//! Cardano settlement shell.
//!
//! Owns: the imperative code that settles authorized effects on
//! Cardano, including transaction assembly, submission, and confirmation
//! polling.
//!
//! Does NOT own: the canonical authorized-effect type (schemas), nor
//! any decision about whether a settlement should occur (policy).
