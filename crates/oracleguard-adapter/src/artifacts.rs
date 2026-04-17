//! Artifact persistence helpers.
//!
//! Owns: the imperative code that writes evidence artifacts to disk in
//! the canonical bundle layout.
//!
//! Does NOT own: the canonical evidence types (schemas), nor the
//! verification logic that later reads those artifacts (verifier).
