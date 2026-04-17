//! Authorized effect types.
//!
//! Owns: canonical representations of effects the evaluator is allowed
//! to authorize (e.g. release of a capped amount to a named payee).
//!
//! Does NOT own: execution of effects (that is the adapter), nor the
//! decision of whether to authorize an effect (that is the policy
//! crate). This module defines the *vocabulary* of authorized effects.
