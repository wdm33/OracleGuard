//! OracleGuard release-cap policy evaluation.
//!
//! This crate owns the pure deterministic evaluator over canonical semantic
//! inputs from `oracleguard-schemas`. It must not perform I/O, must not use
//! floating-point arithmetic in authoritative paths, and must not depend on
//! the adapter or verifier crates.

#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
#![deny(clippy::panic)]
#![deny(clippy::todo, clippy::unimplemented)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![deny(clippy::print_stdout, clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

pub mod error;
pub mod evaluate;
pub mod math;
