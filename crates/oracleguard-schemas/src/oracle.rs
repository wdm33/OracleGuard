//! Oracle-observation canonical types.
//!
//! Owns: the canonical representation of Charli3 oracle observations
//! once normalized — price, timestamp, confidence band, and associated
//! provenance fields.
//!
//! Does NOT own: fetching oracle data, retry logic, network transport,
//! or any shell code. All of that lives in `oracleguard-adapter` and
//! consumes these types.
