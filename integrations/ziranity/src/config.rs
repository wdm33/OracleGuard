//! Config-backed implementations of `PolicyAnchorView` and
//! `AllocationRegistryView`.
//!
//! The node operator supplies a TOML file describing:
//!
//! - the set of `policy_ref`s the anchor gate accepts, and
//! - the allocation table the registry gate consults (per-allocation
//!   basis, authorized subjects, approved asset, approved destinations).
//!
//! At node startup, [`build_disbursement_handler`] reads this file
//! exactly once, freezes it into a [`ConfigSnapshot`], and wraps it
//! in a [`ConfigView`] newtype that implements both trait surfaces.
//! The resulting `DisbursementHandler` holds `Arc`-shared views, so
//! registering the handler into the substrate-dispatch registry is
//! O(1) and no I/O runs on the dispatch hot path.
//!
//! ## Hex-only boundary
//!
//! Every byte-array field is accepted as lowercase hex (matching the
//! shape of `fixtures/routing/disbursement_ocu_id.hex` and the
//! canonical `tx_hash_sample.hex`). There is deliberately no bech32
//! or base58 decoding here: Cardano-bech32 round-trip needs a real
//! dependency, and operator tooling can convert bech32 to raw bytes
//! once (`cardano-cli address info --address addr_test1... | jq -r
//! .base16` or equivalent). Keeping the config format byte-exact
//! against our canonical types removes an encoding-layer risk.
//!
//! ## Asset shape
//!
//! v1 of the config format supports ADA only — `approved_asset = "ADA"`.
//! Non-ADA support is a future schema-version change; today the
//! fulfillment boundary's ADA-only guard (in `oracleguard-adapter`)
//! would refuse a non-ADA authorized effect anyway. Accepting only
//! the literal string `"ADA"` here pins operator expectations against
//! reality.

use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use oracleguard_policy::authorize::{AllocationFacts, AllocationRegistryView, PolicyAnchorView};
use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};

use serde::Deserialize;

/// Top-level TOML shape, deserialized directly from the config file.
#[derive(Debug, Deserialize)]
struct ConfigFile {
    anchor: AnchorSection,
    registry: RegistrySection,
}

#[derive(Debug, Deserialize)]
struct AnchorSection {
    /// 64-char lowercase hex strings, each a 32-byte `policy_ref`.
    registered_policy_refs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RegistrySection {
    allocations: Vec<AllocationFile>,
}

#[derive(Debug, Deserialize)]
struct AllocationFile {
    /// 64-char lowercase hex — 32-byte allocation id.
    id: String,
    /// Approved allocation size in lovelace.
    basis_lovelace: u64,
    /// 64-char lowercase hex strings, each a 32-byte subject id.
    authorized_subjects: Vec<String>,
    /// Literal string `"ADA"` for v1.
    approved_asset: String,
    /// List of approved destinations.
    approved_destinations: Vec<DestinationFile>,
}

#[derive(Debug, Deserialize)]
struct DestinationFile {
    /// Up to 114-char lowercase hex — raw Cardano address bytes.
    bytes_hex: String,
    /// Semantic address length in bytes (1..=57).
    length: u8,
}

/// Immutable runtime-shape config snapshot.
///
/// Populated once at node startup via [`ConfigSnapshot::from_toml_path`]
/// or [`ConfigSnapshot::from_toml_str`]. The handler holds an `Arc`
/// to this type and every trait-method call reads from its pre-built
/// Vecs. No I/O, no allocation, no wall-clock reads on the hot path.
#[derive(Debug)]
pub struct ConfigSnapshot {
    registered_policy_refs: Vec<[u8; 32]>,
    allocations: Vec<AllocationEntry>,
}

#[derive(Debug)]
struct AllocationEntry {
    id: [u8; 32],
    basis_lovelace: u64,
    authorized_subjects: Vec<[u8; 32]>,
    approved_asset: AssetIdV1,
    approved_destinations: Vec<CardanoAddressV1>,
}

/// Errors returned by the config loader.
#[derive(Debug)]
pub enum BuildError {
    /// Filesystem read failed.
    Io(std::io::Error),
    /// TOML parse failed.
    Parse(toml::de::Error),
    /// A 32-byte hex field was not exactly 64 lowercase-hex characters.
    HexByteArrayLength {
        field: &'static str,
        found: usize,
        expected: usize,
    },
    /// A hex string contained a character outside `0-9a-f`.
    HexInvalidChar {
        field: &'static str,
    },
    /// `approved_asset` was a string other than the accepted v1 aliases.
    UnsupportedAsset {
        found: String,
    },
    /// `length` on a destination was outside `1..=57`.
    DestinationLength {
        found: u8,
    },
    /// A destination's `bytes_hex` had an odd character count or too
    /// many bytes for `CardanoAddressV1.bytes`.
    DestinationBytesShape {
        found_bytes: usize,
    },
    /// Two allocations shared the same 32-byte id.
    DuplicateAllocationId,
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "config: io error: {e}"),
            Self::Parse(e) => write!(f, "config: toml parse error: {e}"),
            Self::HexByteArrayLength {
                field,
                found,
                expected,
            } => write!(
                f,
                "config: field {field} expected {expected} hex chars, found {found}"
            ),
            Self::HexInvalidChar { field } => {
                write!(f, "config: field {field} contained non-hex characters")
            }
            Self::UnsupportedAsset { found } => write!(
                f,
                "config: approved_asset = {found:?}, only \"ADA\" is supported in v1"
            ),
            Self::DestinationLength { found } => write!(
                f,
                "config: destination length {found} out of range 1..=57"
            ),
            Self::DestinationBytesShape { found_bytes } => write!(
                f,
                "config: destination bytes had {found_bytes} bytes, must be 1..=57"
            ),
            Self::DuplicateAllocationId => {
                write!(f, "config: two allocations share the same id")
            }
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BuildError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<toml::de::Error> for BuildError {
    fn from(err: toml::de::Error) -> Self {
        Self::Parse(err)
    }
}

impl ConfigSnapshot {
    /// Load a `ConfigSnapshot` from a TOML file on disk.
    pub fn from_toml_path(path: &Path) -> Result<Self, BuildError> {
        let text = fs::read_to_string(path)?;
        Self::from_toml_str(&text)
    }

    /// Parse a `ConfigSnapshot` from TOML text. Exposed for tests and
    /// for callers that source config from somewhere other than disk.
    pub fn from_toml_str(text: &str) -> Result<Self, BuildError> {
        let file: ConfigFile = toml::from_str(text)?;
        Self::from_file(file)
    }

    fn from_file(file: ConfigFile) -> Result<Self, BuildError> {
        let registered_policy_refs = file
            .anchor
            .registered_policy_refs
            .iter()
            .map(|s| decode_32_hex(s, "anchor.registered_policy_refs"))
            .collect::<Result<Vec<[u8; 32]>, _>>()?;

        let mut allocations: Vec<AllocationEntry> =
            Vec::with_capacity(file.registry.allocations.len());
        for a in file.registry.allocations {
            let id = decode_32_hex(&a.id, "registry.allocations.id")?;
            if allocations.iter().any(|existing| existing.id == id) {
                return Err(BuildError::DuplicateAllocationId);
            }
            let authorized_subjects = a
                .authorized_subjects
                .iter()
                .map(|s| decode_32_hex(s, "registry.allocations.authorized_subjects"))
                .collect::<Result<Vec<[u8; 32]>, _>>()?;
            let approved_asset = decode_asset(&a.approved_asset)?;
            let approved_destinations = a
                .approved_destinations
                .iter()
                .map(decode_destination)
                .collect::<Result<Vec<CardanoAddressV1>, _>>()?;
            allocations.push(AllocationEntry {
                id,
                basis_lovelace: a.basis_lovelace,
                authorized_subjects,
                approved_asset,
                approved_destinations,
            });
        }

        Ok(Self {
            registered_policy_refs,
            allocations,
        })
    }
}

fn decode_32_hex(s: &str, field: &'static str) -> Result<[u8; 32], BuildError> {
    if s.len() != 64 {
        return Err(BuildError::HexByteArrayLength {
            field,
            found: s.len(),
            expected: 64,
        });
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
        let hi = decode_nibble(chunk[0], field)?;
        let lo = decode_nibble(chunk[1], field)?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn decode_asset(s: &str) -> Result<AssetIdV1, BuildError> {
    match s {
        "ADA" => Ok(AssetIdV1::ADA),
        _ => Err(BuildError::UnsupportedAsset {
            found: String::from(s),
        }),
    }
}

fn decode_destination(d: &DestinationFile) -> Result<CardanoAddressV1, BuildError> {
    if d.length == 0 || d.length > 57 {
        return Err(BuildError::DestinationLength { found: d.length });
    }
    if d.bytes_hex.len() % 2 != 0 {
        return Err(BuildError::DestinationBytesShape {
            found_bytes: d.bytes_hex.len() / 2,
        });
    }
    let byte_count = d.bytes_hex.len() / 2;
    if byte_count == 0 || byte_count > 57 {
        return Err(BuildError::DestinationBytesShape {
            found_bytes: byte_count,
        });
    }
    let mut bytes = [0u8; 57];
    for (i, chunk) in d.bytes_hex.as_bytes().chunks_exact(2).enumerate() {
        let hi = decode_nibble(chunk[0], "registry.allocations.approved_destinations.bytes_hex")?;
        let lo = decode_nibble(chunk[1], "registry.allocations.approved_destinations.bytes_hex")?;
        bytes[i] = (hi << 4) | lo;
    }
    Ok(CardanoAddressV1 {
        bytes,
        length: d.length,
    })
}

fn decode_nibble(b: u8, field: &'static str) -> Result<u8, BuildError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(BuildError::HexInvalidChar { field }),
    }
}

impl PolicyAnchorView for ConfigSnapshot {
    fn is_policy_registered(&self, policy_ref: &[u8; 32]) -> bool {
        self.registered_policy_refs.iter().any(|r| r == policy_ref)
    }
}

impl AllocationRegistryView for ConfigSnapshot {
    fn lookup(&self, allocation_id: &[u8; 32]) -> Option<AllocationFacts<'_>> {
        let entry = self
            .allocations
            .iter()
            .find(|a| &a.id == allocation_id)?;
        Some(AllocationFacts {
            basis_lovelace: entry.basis_lovelace,
            authorized_subjects: &entry.authorized_subjects,
            approved_asset: entry.approved_asset,
            approved_destinations: &entry.approved_destinations,
        })
    }
}

/// `Arc`-shared view over a [`ConfigSnapshot`] that implements both
/// [`PolicyAnchorView`] and [`AllocationRegistryView`].
///
/// The two trait impls delegate to the underlying `ConfigSnapshot`.
/// Cloning a `ConfigView` is an `Arc::clone` (one atomic increment),
/// so the two-arg `DisbursementHandler::new(view.clone(), view)`
/// pattern is O(1) and allocation-free.
#[derive(Clone, Debug)]
pub struct ConfigView(Arc<ConfigSnapshot>);

impl ConfigView {
    /// Wrap a `ConfigSnapshot` in an `Arc`.
    #[must_use]
    pub fn new(snapshot: ConfigSnapshot) -> Self {
        Self(Arc::new(snapshot))
    }

    /// Borrow the underlying snapshot. Exposed for tests and for
    /// operator tooling that wants to introspect the loaded config.
    #[must_use]
    pub fn snapshot(&self) -> &ConfigSnapshot {
        &self.0
    }
}

impl PolicyAnchorView for ConfigView {
    fn is_policy_registered(&self, policy_ref: &[u8; 32]) -> bool {
        self.0.is_policy_registered(policy_ref)
    }
}

impl AllocationRegistryView for ConfigView {
    fn lookup(&self, allocation_id: &[u8; 32]) -> Option<AllocationFacts<'_>> {
        self.0.lookup(allocation_id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const SAMPLE_CONFIG: &str = r#"
[anchor]
registered_policy_refs = [
  "56a7bb9793e40aa54402ce67fcbce17dee93b6713c76ccba6c02c11f749968c2",
]

[[registry.allocations]]
id = "2222222222222222222222222222222222222222222222222222222222222222"
basis_lovelace = 1_000_000_000
authorized_subjects = [
  "3333333333333333333333333333333333333333333333333333333333333333",
]
approved_asset = "ADA"
approved_destinations = [
  { bytes_hex = "555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555", length = 57 },
]
"#;

    fn load_sample() -> ConfigSnapshot {
        match ConfigSnapshot::from_toml_str(SAMPLE_CONFIG) {
            Ok(s) => s,
            Err(e) => panic!("sample config failed to parse: {e}"),
        }
    }

    #[test]
    fn sample_config_parses_one_anchor_one_allocation() {
        let snap = load_sample();
        assert_eq!(snap.registered_policy_refs.len(), 1);
        assert_eq!(snap.allocations.len(), 1);
    }

    #[test]
    fn anchor_view_matches_registered_policy() {
        let view = ConfigView::new(load_sample());
        let expected: [u8; 32] = {
            let mut out = [0u8; 32];
            for (i, chunk) in
                b"56a7bb9793e40aa54402ce67fcbce17dee93b6713c76ccba6c02c11f749968c2"
                    .chunks_exact(2)
                    .enumerate()
            {
                let hi = match chunk[0] {
                    b'0'..=b'9' => chunk[0] - b'0',
                    b'a'..=b'f' => chunk[0] - b'a' + 10,
                    _ => panic!(),
                };
                let lo = match chunk[1] {
                    b'0'..=b'9' => chunk[1] - b'0',
                    b'a'..=b'f' => chunk[1] - b'a' + 10,
                    _ => panic!(),
                };
                out[i] = (hi << 4) | lo;
            }
            out
        };
        assert!(view.is_policy_registered(&expected));
        assert!(!view.is_policy_registered(&[0x00; 32]));
    }

    #[test]
    fn registry_view_looks_up_allocation_by_id() {
        let view = ConfigView::new(load_sample());
        let facts = view.lookup(&[0x22; 32]).expect("allocation present");
        assert_eq!(facts.basis_lovelace, 1_000_000_000);
        assert_eq!(facts.authorized_subjects, &[[0x33; 32]]);
        assert_eq!(facts.approved_asset, AssetIdV1::ADA);
        assert_eq!(facts.approved_destinations.len(), 1);
        assert_eq!(facts.approved_destinations[0].length, 57);
        assert_eq!(facts.approved_destinations[0].bytes, [0x55; 57]);
    }

    #[test]
    fn registry_view_returns_none_for_unknown_allocation() {
        let view = ConfigView::new(load_sample());
        assert!(view.lookup(&[0xFF; 32]).is_none());
    }

    #[test]
    fn config_view_clone_is_shallow() {
        let view = ConfigView::new(load_sample());
        let clone = view.clone();
        // Both views see the same snapshot by Arc identity.
        assert!(std::ptr::eq(view.snapshot(), clone.snapshot()));
    }

    #[test]
    fn duplicate_allocation_id_is_rejected() {
        let config = r#"
[anchor]
registered_policy_refs = []

[[registry.allocations]]
id = "2222222222222222222222222222222222222222222222222222222222222222"
basis_lovelace = 1
authorized_subjects = []
approved_asset = "ADA"
approved_destinations = []

[[registry.allocations]]
id = "2222222222222222222222222222222222222222222222222222222222222222"
basis_lovelace = 2
authorized_subjects = []
approved_asset = "ADA"
approved_destinations = []
"#;
        let err = ConfigSnapshot::from_toml_str(config).expect_err("must reject duplicate");
        assert!(matches!(err, BuildError::DuplicateAllocationId));
    }

    #[test]
    fn non_ada_asset_is_rejected_with_explicit_error() {
        let config = r#"
[anchor]
registered_policy_refs = []

[[registry.allocations]]
id = "2222222222222222222222222222222222222222222222222222222222222222"
basis_lovelace = 1
authorized_subjects = []
approved_asset = "SOMETHING_ELSE"
approved_destinations = []
"#;
        let err = ConfigSnapshot::from_toml_str(config).expect_err("must reject non-ADA");
        assert!(matches!(err, BuildError::UnsupportedAsset { .. }));
    }

    #[test]
    fn short_hex_policy_ref_is_rejected() {
        let config = r#"
[anchor]
registered_policy_refs = [ "56a7bb" ]

[registry]
allocations = []
"#;
        let err = ConfigSnapshot::from_toml_str(config).expect_err("must reject short hex");
        assert!(matches!(err, BuildError::HexByteArrayLength { .. }));
    }

    #[test]
    fn non_hex_char_in_policy_ref_is_rejected() {
        let config = r#"
[anchor]
registered_policy_refs = [ "XX00000000000000000000000000000000000000000000000000000000000000" ]

[registry]
allocations = []
"#;
        let err = ConfigSnapshot::from_toml_str(config).expect_err("must reject non-hex");
        assert!(matches!(err, BuildError::HexInvalidChar { .. }));
    }

    #[test]
    fn destination_length_zero_is_rejected() {
        let config = r#"
[anchor]
registered_policy_refs = []

[[registry.allocations]]
id = "2222222222222222222222222222222222222222222222222222222222222222"
basis_lovelace = 1
authorized_subjects = []
approved_asset = "ADA"
approved_destinations = [ { bytes_hex = "55", length = 0 } ]
"#;
        let err = ConfigSnapshot::from_toml_str(config).expect_err("must reject length 0");
        assert!(matches!(err, BuildError::DestinationLength { found: 0 }));
    }

    #[test]
    fn destination_length_over_57_is_rejected() {
        let config = r#"
[anchor]
registered_policy_refs = []

[[registry.allocations]]
id = "2222222222222222222222222222222222222222222222222222222222222222"
basis_lovelace = 1
authorized_subjects = []
approved_asset = "ADA"
approved_destinations = [ { bytes_hex = "55", length = 58 } ]
"#;
        let err = ConfigSnapshot::from_toml_str(config).expect_err("must reject length 58");
        assert!(matches!(err, BuildError::DestinationLength { found: 58 }));
    }

    #[test]
    fn parse_error_surfaces_toml_error() {
        let err = ConfigSnapshot::from_toml_str("not = valid = toml")
            .expect_err("malformed toml must reject");
        assert!(matches!(err, BuildError::Parse(_)));
    }
}
