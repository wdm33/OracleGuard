//! Release-cap evaluator entry point.
//!
//! Owns: the deterministic function that maps canonical semantic inputs
//! (intent, oracle observation, policy parameters) to a decision and
//! reason code.
//!
//! Does NOT own: input collection, effect execution, or evidence
//! persistence. Those are shell responsibilities.
