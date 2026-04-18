//! OracleGuard adapter shell.
//!
//! This crate hosts imperative shell logic: Charli3 oracle fetch and
//! normalization, Ziranity CLI submission, Cardano settlement, and artifact
//! persistence helpers. It consumes public semantics from
//! `oracleguard-schemas` and `oracleguard-policy`; it must not redefine
//! semantic meaning or own evaluator decisions.

pub mod artifacts;
pub mod cardano;
pub mod charli3;
pub mod charli3_aggregate;
pub mod intake;
pub mod kupo;
pub mod settlement;
pub mod ziranity_submit;
