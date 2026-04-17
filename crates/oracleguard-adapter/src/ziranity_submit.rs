//! Ziranity CLI submission shell.
//!
//! Owns: the imperative code that submits an authorized disbursement
//! intent to the Ziranity CLI and surfaces the response.
//!
//! Does NOT own: the canonical intent type (schemas), nor the
//! authorization decision that precedes submission (policy and private
//! Ziranity runtime).
