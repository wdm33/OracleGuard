//! Authorization-gate taxonomy.
//!
//! Owns: the closed, typed enumeration of authorization gates in
//! OracleGuard's three-gate model. Every
//! [`crate::reason::DisbursementReasonCode`] is produced by exactly
//! one gate; this type encodes that partition so downstream orchestration
//! (Cluster 5 Slice 02 onward) can reason about gate-level precedence
//! without re-deriving the mapping.
//!
//! Does NOT own: gate sequencing, per-gate logic, or deny/effect closure.
//! Those live in `oracleguard-policy::authorize` (Cluster 5 Slices 02+).

use serde::{Deserialize, Serialize};

/// The three authorization gates in OracleGuard's three-gate model.
///
/// Gates are evaluated in the fixed order
/// [`AuthorizationGate::Anchor`] → [`AuthorizationGate::Registry`] →
/// [`AuthorizationGate::Grant`]. The first failing gate produces the
/// canonical denial reason; later gates are not consulted.
///
/// ## Variant partition
///
/// Each [`crate::reason::DisbursementReasonCode`] is produced by exactly
/// one gate; see [`crate::reason::DisbursementReasonCode::gate`] for the
/// machine-readable partition.
///
/// The variant set is frozen at v1. Adding, removing, or reordering
/// variants is a canonical-byte change and therefore a schema-version
/// boundary crossing — see `docs/canonical-encoding.md` §Version boundary.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthorizationGate {
    /// First gate: validates the intent schema version and confirms the
    /// referenced policy identity is registered.
    Anchor = 0,
    /// Second gate: validates allocation identity, subject authorization,
    /// asset match, and destination authorization against the
    /// allocation's approved-destination set.
    Registry = 1,
    /// Third gate: validates oracle freshness and delegates the
    /// release-cap decision to the pure evaluator in
    /// `oracleguard-policy`.
    Grant = 2,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // Destructuring guard: if a variant is added, removed, or reordered
    // this match fails to compile, forcing the schema-version boundary
    // conversation rather than a silent canonical-byte change.
    #[test]
    fn variant_list_is_frozen_at_three_gates() {
        let gates = [
            AuthorizationGate::Anchor,
            AuthorizationGate::Registry,
            AuthorizationGate::Grant,
        ];
        for g in gates {
            let _: u8 = match g {
                AuthorizationGate::Anchor => 0,
                AuthorizationGate::Registry => 1,
                AuthorizationGate::Grant => 2,
            };
        }
    }

    #[test]
    fn repr_u8_discriminants_are_stable() {
        assert_eq!(AuthorizationGate::Anchor as u8, 0);
        assert_eq!(AuthorizationGate::Registry as u8, 1);
        assert_eq!(AuthorizationGate::Grant as u8, 2);
    }

    #[test]
    fn authorization_gate_postcard_round_trips() {
        for g in [
            AuthorizationGate::Anchor,
            AuthorizationGate::Registry,
            AuthorizationGate::Grant,
        ] {
            let bytes = postcard::to_allocvec(&g).expect("encode");
            let (decoded, rest) =
                postcard::take_from_bytes::<AuthorizationGate>(&bytes).expect("decode");
            assert!(rest.is_empty());
            assert_eq!(decoded, g);
        }
    }
}
