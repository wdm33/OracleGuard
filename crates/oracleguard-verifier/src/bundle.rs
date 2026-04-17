//! Evidence bundle loading.
//!
//! Owns: parsing an on-disk evidence bundle into canonical in-memory
//! values using the types defined in `oracleguard-schemas`.
//!
//! Does NOT own: alternate canonical encodings, a second evidence type
//! definition, or any step that happens before the bundle is on disk
//! (that is the adapter).
