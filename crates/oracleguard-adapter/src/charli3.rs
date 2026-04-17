//! Charli3 oracle fetch and normalization shell.
//!
//! Owns: the imperative code that fetches data from Charli3 endpoints
//! and normalizes it into canonical types from `oracleguard-schemas`.
//!
//! Does NOT own: the canonical oracle observation type (schemas), nor
//! any decision based on the observation (policy).
