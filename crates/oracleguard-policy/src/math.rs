//! Deterministic integer math used by the evaluator.
//!
//! Owns: pure, total, integer-domain helpers used by the release-cap
//! evaluator. Every function here must be total and overflow-checked.
//!
//! Does NOT own: floating-point arithmetic, non-deterministic
//! calculations, or anything outside the evaluator's internal math
//! surface.
