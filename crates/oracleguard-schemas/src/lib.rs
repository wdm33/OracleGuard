//! OracleGuard canonical semantic types.
//!
//! This crate is the sole public owner of canonical semantic meaning for
//! OracleGuard. It defines versioned wire/domain structs, reason codes,
//! authorized-effect types, and evidence types. It performs no I/O and
//! contains no shell logic.

#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
#![deny(clippy::panic)]
#![deny(clippy::todo, clippy::unimplemented)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![deny(clippy::print_stdout, clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

pub mod effect;
pub mod evidence;
pub mod intent;
pub mod oracle;
pub mod reason;
