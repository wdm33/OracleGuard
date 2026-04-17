//! Evaluator replay.
//!
//! Owns: deterministically re-running `oracleguard-policy` over the
//! canonical inputs recorded in an evidence bundle and comparing the
//! result to the recorded decision.
//!
//! Does NOT own: the evaluator itself (policy crate), nor the
//! canonical decision type (schemas). This module only *compares*.
