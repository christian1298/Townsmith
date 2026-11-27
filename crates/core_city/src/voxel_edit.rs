//! Placed-blueprint voxel edit log (D-038, proposed).
//!
//! A placed blueprint is immutable: its package hash identifies the source
//! volume. Live edits are therefore stored as an append-only delta log scoped
//! to one placement, not by rewriting the package. This module is the pure,
//! engine-agnostic core model from [[79 - Voxel Edit Sync]] §3–§6:
//!
//! - [`VoxelEdit`] is one local voxel write; material `0` removes the voxel.
//! - [`PlacedBlueprintEdit`] holds the placement identity, the package base
//!   revision, the accepted-batch revision, and the ordered delta log.
//! - [`validate_batch`] applies the authoritative batch checks without state.
//! - [`PlacedBlueprintEdit::apply_batch`] appends an accepted batch, bumps the
//!   revision by exactly one, and never mutates on rejection.
//!
//! The delta order is the application order. Duplicate positions are rejected
//! inside one batch, but the same position may appear in later batches, where
//! the last accepted delta wins. Compaction is a storage concern; this type
//! keeps the exact accepted sequence so the checksum is stable.
//!
//! There is **no** protocol, save, server, or rendering wiring here. Materials
//! are raw registry ids; the core model only reserves [`MATERIAL_RESERVED`] and
//! treats [`MATERIAL_EMPTY`] as "remove". A caller with a live registry is
//! expected to check unknown ids before calling [`PlacedBlueprintEdit::apply_batch`].
//!
//! # Coordinates
//!
//! `pos` is a local voxel coordinate inside the package volume. Volume bounds
//! passed to [`validate_batch`] are exclusive extents: `pos[i] < bounds[i]` for
//! every axis.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Maximum edits accepted in one batch (D-038, [[79 - Voxel Edit Sync]] §4).
pub const MAX_EDITS_PER_BATCH: usize = 1_024;

/// Maximum retained edits for one placement.
///
/// This is a storage guard for the pure model, not a wire limit. A caller that
/// grows past it must compact or materialise the volume before appending.
pub const MAX_EDIT_LOG: usize = 1 << 18;

/// Material id that means "empty": the voxel is removed.
pub const MATERIAL_EMPTY: u16 = 0;

/// Reserved material id. It is never a valid registry material.
pub const MATERIAL_RESERVED: u16 = u16::MAX;

/// Prefix of a content-addressed blueprint hash.
pub const BLUEPRINT_HASH_PREFIX: &str = "blake3:";

/// Number of lowercase hex digits after [`BLUEPRINT_HASH_PREFIX`].
pub const BLUEPRINT_HASH_HEX_LEN: usize = 64;

/// One voxel write on a placed blueprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoxelEdit {
    /// Local voxel coordinate inside the package volume.
    pub pos: [u16; 3],
    /// Registry material id; [`MATERIAL_EMPTY`] removes the voxel.
    pub material: u16,
}

/// Append-only edit log for one placed blueprint instance.
///
/// The struct is deliberately plain: [`Self::checksum`] is derived, not stored,
/// so a deserialised record cannot carry a stale checksum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacedBlueprintEdit {
    /// Stable placement id within the world save.
    pub placement_id: u64,
    /// Content-addressed hash of the source `.tsbp` package.
    pub blueprint_hash: String,
    /// Package/library revision at placement time.
    pub base_revision: u32,
    /// Accepted-batch revision; starts at 0 and increases by one per batch.
    pub edit_revision: u32,
    /// Compact deltas in application order.
    pub edits: Vec<VoxelEdit>,
}

impl PlacedBlueprintEdit {
    /// Create an empty edit log for a placement.
    ///
    /// The hash is not validated here; [`Self::validate_loaded`] does that when
    /// a record enters the process from persistence or the wire.
    #[must_use]
    pub fn new(placement_id: u64, blueprint_hash: impl Into<String>, base_revision: u32) -> Self {
        Self {
            placement_id,
            blueprint_hash: blueprint_hash.into(),
            base_revision,
            edit_revision: 0,
            edits: Vec::new(),
        }
    }

    /// Number of retained deltas.
    #[must_use]
    pub fn delta_count(&self) -> usize {
        self.edits.len()
    }

    /// Whether this placement has no accepted edits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Stable FNV-1a checksum over identity, revisions, and deltas.
    ///
    /// Field order is fixed: placement id, hash length and bytes,
    /// `base_revision`, `edit_revision`, edit count, then each edit in stored
    /// order as `pos[0], pos[1], pos[2], material`. Delta order is therefore
    /// part of the checksum.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = Fnv1a::new();
        hash.write_u64(self.placement_id);
        hash.write_u32(u32::try_from(self.blueprint_hash.len()).unwrap_or(u32::MAX));
        hash.write(self.blueprint_hash.as_bytes());
        hash.write_u32(self.base_revision);
        hash.write_u32(self.edit_revision);
        hash.write_u64(u64::try_from(self.edits.len()).unwrap_or(u64::MAX));
        for edit in &self.edits {
            for component in edit.pos {
                hash.write_u16(component);
            }
            hash.write_u16(edit.material);
        }
        hash.finish()
    }

    /// Last delta at `pos`, in application order.
    ///
    /// Returns `None` when no accepted edit touches the cell; `Some(0)` means
    /// the last edit removed it.
    #[must_use]
    pub fn delta_at(&self, pos: [u16; 3]) -> Option<u16> {
        self.edits
            .iter()
            .rev()
            .find(|edit| edit.pos == pos)
            .map(|edit| edit.material)
    }

    /// Effective delta overlay at `pos`, with the last accepted edit winning.
    ///
    /// The result starts from empty, so an untouched cell and a removed cell
    /// both return [`MATERIAL_EMPTY`]. Use [`Self::delta_at`] when the caller
    /// must distinguish them, for example when applying the log over a package
    /// volume.
    #[must_use]
    pub fn effective_voxel(&self, pos: [u16; 3]) -> u16 {
        self.delta_at(pos).unwrap_or(MATERIAL_EMPTY)
    }

    /// Validate and append one batch.
    ///
    /// `expected_revision` must equal [`Self::edit_revision`] exactly; there is
    /// no merge and no automatic retry. On acceptance the edits are appended in
    /// the supplied order and the revision increases by one, saturating at
    /// `u32::MAX`. On rejection nothing changes.
    ///
    /// # Errors
    ///
    /// Returns [`EditBatchError::StaleRevision`] when the revision does not
    /// match, [`EditBatchError::Overflow`] when the retained log would exceed
    /// [`MAX_EDIT_LOG`], or any [`validate_batch`] error.
    pub fn apply_batch(
        &mut self,
        expected_revision: u32,
        edits: &[VoxelEdit],
        volume_bounds: [u16; 3],
    ) -> Result<(), EditBatchError> {
        if expected_revision != self.edit_revision {
            return Err(EditBatchError::StaleRevision {
                expected: expected_revision,
                current: self.edit_revision,
            });
        }
        validate_batch(edits, volume_bounds)?;

        let total = self.edits.len().saturating_add(edits.len());
        if total > MAX_EDIT_LOG {
            return Err(EditBatchError::Overflow {
                count: total,
                max: MAX_EDIT_LOG,
            });
        }

        self.edits.extend_from_slice(edits);
        self.edit_revision = self.edit_revision.saturating_add(1);
        Ok(())
    }

    /// Validate a record that arrived from persistence or the wire.
    ///
    /// Checks the package hash shape, the retained-log cap, reserved materials,
    /// and the empty-log/revision invariant. Bounds, registry membership,
    /// placement existence, and hash equality with the placement record stay
    /// with the caller because this module has no package or world context.
    ///
    /// # Errors
    ///
    /// Returns a stable [`EditBatchError`] describing the first violation. All
    /// violations are reported without mutating the record.
    pub fn validate_loaded(&self) -> Result<(), EditBatchError> {
        if !is_canonical_blueprint_hash(&self.blueprint_hash) {
            return Err(EditBatchError::InvalidBlueprintHash);
        }
        if self.edits.len() > MAX_EDIT_LOG {
            return Err(EditBatchError::Overflow {
                count: self.edits.len(),
                max: MAX_EDIT_LOG,
            });
        }
        if self.edits.is_empty() && self.edit_revision != 0 {
            return Err(EditBatchError::RevisionMismatch {
                edit_revision: self.edit_revision,
            });
        }
        for edit in &self.edits {
            validate_material(edit.material)?;
        }
        Ok(())
    }
}

/// Validate one batch without touching placement state.
///
/// Checks, in order: non-empty, at most [`MAX_EDITS_PER_BATCH`], every
/// coordinate inside the exclusive `volume_bounds`, every material not
/// [`MATERIAL_RESERVED`], and no duplicate positions within the batch.
///
/// # Errors
///
/// Returns the first violated rule as an [`EditBatchError`].
pub fn validate_batch(edits: &[VoxelEdit], volume_bounds: [u16; 3]) -> Result<(), EditBatchError> {
    if edits.is_empty() {
        return Err(EditBatchError::EmptyBatch);
    }
    if edits.len() > MAX_EDITS_PER_BATCH {
        return Err(EditBatchError::TooManyEdits {
            count: edits.len(),
            max: MAX_EDITS_PER_BATCH,
        });
    }

    let mut seen = HashSet::with_capacity(edits.len());
    for edit in edits {
        if !in_bounds(edit.pos, volume_bounds) {
            return Err(EditBatchError::InvalidEdit(
                VoxelEditError::InvalidPosition {
                    pos: edit.pos,
                    bounds: volume_bounds,
                },
            ));
        }
        validate_material(edit.material)?;
        if !seen.insert(edit.pos) {
            return Err(EditBatchError::DuplicatePosition { pos: edit.pos });
        }
    }
    Ok(())
}

/// Whether a local voxel coordinate is inside the exclusive volume extents.
#[must_use]
pub fn in_bounds(pos: [u16; 3], volume_bounds: [u16; 3]) -> bool {
    pos[0] < volume_bounds[0] && pos[1] < volume_bounds[1] && pos[2] < volume_bounds[2]
}

/// Whether `value` is a canonical `blake3:<64 lowercase hex>` hash.
#[must_use]
pub fn is_canonical_blueprint_hash(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(BLUEPRINT_HASH_PREFIX) else {
        return false;
    };
    hex.len() == BLUEPRINT_HASH_HEX_LEN
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_material(material: u16) -> Result<(), EditBatchError> {
    if material == MATERIAL_RESERVED {
        Err(EditBatchError::InvalidEdit(
            VoxelEditError::InvalidMaterial { material },
        ))
    } else {
        Ok(())
    }
}

/// Per-edit validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxelEditError {
    /// Coordinate is outside the package volume.
    InvalidPosition {
        /// Rejected coordinate.
        pos: [u16; 3],
        /// Exclusive extents supplied by the caller.
        bounds: [u16; 3],
    },
    /// Material id is reserved and can never be a registry material.
    InvalidMaterial {
        /// Rejected material id.
        material: u16,
    },
}

impl VoxelEditError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidPosition { .. } => "edit.invalid_position",
            Self::InvalidMaterial { .. } => "edit.invalid_material",
        }
    }
}

impl fmt::Display for VoxelEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPosition { pos, bounds } => write!(
                f,
                "voxel position {pos:?} is outside the package volume {bounds:?}"
            ),
            Self::InvalidMaterial { material } => {
                write!(f, "material {material} is reserved and cannot be edited")
            }
        }
    }
}

impl Error for VoxelEditError {}

/// Batch-level failure for [`PlacedBlueprintEdit::apply_batch`] and
/// [`validate_batch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditBatchError {
    /// No edits supplied.
    EmptyBatch,
    /// More than [`MAX_EDITS_PER_BATCH`] edits supplied.
    TooManyEdits {
        /// Supplied count.
        count: usize,
        /// Accepted maximum.
        max: usize,
    },
    /// `expected_revision` is not the current revision.
    StaleRevision {
        /// Revision supplied by the caller.
        expected: u32,
        /// Current authoritative revision.
        current: u32,
    },
    /// The same cell is edited twice in one batch.
    DuplicatePosition {
        /// Repeated coordinate.
        pos: [u16; 3],
    },
    /// Appending the batch would exceed [`MAX_EDIT_LOG`].
    Overflow {
        /// Resulting retained count.
        count: usize,
        /// Accepted maximum.
        max: usize,
    },
    /// A single edit failed validation.
    InvalidEdit(VoxelEditError),
    /// A loaded record has a malformed package hash.
    InvalidBlueprintHash,
    /// A loaded record has no edits but a non-zero revision.
    RevisionMismatch {
        /// Revision found on the record.
        edit_revision: u32,
    },
}

impl EditBatchError {
    /// Stable machine-readable code.
    ///
    /// Single-edit failures delegate to [`VoxelEditError::code`].
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EmptyBatch => "edit.empty_batch",
            Self::TooManyEdits { .. } => "edit.too_many",
            Self::StaleRevision { .. } => "edit.stale_revision",
            Self::DuplicatePosition { .. } => "edit.duplicate_position",
            Self::Overflow { .. } => "edit.overflow",
            Self::InvalidEdit(error) => error.code(),
            Self::InvalidBlueprintHash => "edit.invalid_hash",
            Self::RevisionMismatch { .. } => "edit.invalid_revision",
        }
    }
}

impl fmt::Display for EditBatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBatch => f.write_str("edit batch is empty"),
            Self::TooManyEdits { count, max } => {
                write!(f, "edit batch has {count} edits, maximum is {max}")
            }
            Self::StaleRevision { expected, current } => write!(
                f,
                "edit batch expected revision {expected}, current revision is {current}"
            ),
            Self::DuplicatePosition { pos } => {
                write!(f, "edit batch writes voxel {pos:?} more than once")
            }
            Self::Overflow { count, max } => {
                write!(f, "edit log would hold {count} edits, maximum is {max}")
            }
            Self::InvalidEdit(error) => error.fmt(f),
            Self::InvalidBlueprintHash => f.write_str("blueprint hash is not canonical"),
            Self::RevisionMismatch { edit_revision } => {
                write!(f, "edit log has no deltas but revision {edit_revision}")
            }
        }
    }
}

impl Error for EditBatchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidEdit(error) => Some(error),
            _ => None,
        }
    }
}

/// FNV-1a 64-bit hasher, matching the pinning style used by
/// `CityLayout::checksum`.
struct Fnv1a(u64);

impl Fnv1a {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn write_u16(&mut self, value: u16) {
        self.write(&value.to_le_bytes());
    }

    fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }

    fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_canonical_blueprint_hash, validate_batch, EditBatchError, PlacedBlueprintEdit,
        VoxelEdit, VoxelEditError, BLUEPRINT_HASH_PREFIX, MATERIAL_EMPTY, MATERIAL_RESERVED,
        MAX_EDITS_PER_BATCH, MAX_EDIT_LOG,
    };

    const HASH_A: &str = "blake3:abababababababababababababababababababababababababababababababab";
    const HASH_B: &str = "blake3:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
    const BOUNDS: [u16; 3] = [8, 8, 8];

    fn edit(x: u16, y: u16, z: u16, material: u16) -> VoxelEdit {
        VoxelEdit {
            pos: [x, y, z],
            material,
        }
    }

    fn record() -> PlacedBlueprintEdit {
        PlacedBlueprintEdit::new(42, HASH_A, 3)
    }

    fn filled_log(len: usize) -> PlacedBlueprintEdit {
        PlacedBlueprintEdit {
            placement_id: 1,
            blueprint_hash: HASH_A.to_owned(),
            base_revision: 0,
            edit_revision: 0,
            edits: vec![edit(0, 0, 0, 1); len],
        }
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(EditBatchError::EmptyBatch.code(), "edit.empty_batch");
        assert_eq!(
            EditBatchError::TooManyEdits { count: 2, max: 1 }.code(),
            "edit.too_many"
        );
        assert_eq!(
            EditBatchError::StaleRevision {
                expected: 0,
                current: 1
            }
            .code(),
            "edit.stale_revision"
        );
        assert_eq!(
            EditBatchError::DuplicatePosition { pos: [1, 2, 3] }.code(),
            "edit.duplicate_position"
        );
        assert_eq!(
            EditBatchError::Overflow { count: 2, max: 1 }.code(),
            "edit.overflow"
        );
        assert_eq!(
            VoxelEditError::InvalidPosition {
                pos: [0, 0, 0],
                bounds: [0, 0, 0]
            }
            .code(),
            "edit.invalid_position"
        );
        assert_eq!(
            VoxelEditError::InvalidMaterial {
                material: MATERIAL_RESERVED
            }
            .code(),
            "edit.invalid_material"
        );
        assert_eq!(
            EditBatchError::InvalidBlueprintHash.code(),
            "edit.invalid_hash"
        );
        assert_eq!(
            EditBatchError::RevisionMismatch { edit_revision: 1 }.code(),
            "edit.invalid_revision"
        );
    }

    #[test]
    fn empty_batch_is_rejected_without_mutation() {
        let mut log = record();
        assert_eq!(validate_batch(&[], BOUNDS), Err(EditBatchError::EmptyBatch));
        assert_eq!(
            log.apply_batch(0, &[], BOUNDS),
            Err(EditBatchError::EmptyBatch)
        );
        assert_eq!(log.edit_revision, 0);
        assert!(log.is_empty());
    }

    #[test]
    fn batch_cap_boundary_is_1024() {
        let bounds = [1_024, 1, 1];
        let at_cap: Vec<_> = (0..MAX_EDITS_PER_BATCH)
            .map(|x| edit(u16::try_from(x).expect("fits"), 0, 0, 1))
            .collect();
        assert_eq!(at_cap.len(), 1_024);
        assert!(validate_batch(&at_cap, bounds).is_ok());

        let mut over = at_cap.clone();
        over.push(edit(1_024, 0, 0, 1));
        assert_eq!(over.len(), 1_025);
        assert_eq!(
            validate_batch(&over, bounds),
            Err(EditBatchError::TooManyEdits {
                count: 1_025,
                max: MAX_EDITS_PER_BATCH
            })
        );

        let mut log = record();
        log.apply_batch(0, &at_cap, bounds)
            .expect("cap batch applies");
        assert_eq!(log.delta_count(), MAX_EDITS_PER_BATCH);
        assert_eq!(log.edit_revision, 1);
    }

    #[test]
    fn stale_revision_is_rejected_and_state_is_unchanged() {
        let mut log = record();
        log.apply_batch(0, &[edit(1, 1, 1, 5)], BOUNDS)
            .expect("first batch");
        let before = log.clone();

        let error = log
            .apply_batch(0, &[edit(2, 2, 2, 7)], BOUNDS)
            .expect_err("stale revision");
        assert_eq!(
            error,
            EditBatchError::StaleRevision {
                expected: 0,
                current: 1
            }
        );
        assert_eq!(log, before);
        assert_eq!(log.checksum(), before.checksum());
    }

    #[test]
    fn out_of_bounds_position_is_rejected() {
        let bounds = [4, 4, 4];
        assert!(validate_batch(&[edit(3, 3, 3, 1)], bounds).is_ok());
        for pos in [[4, 0, 0], [0, 4, 0], [0, 0, 4], [9, 9, 9]] {
            let error = validate_batch(&[edit(pos[0], pos[1], pos[2], 1)], bounds)
                .expect_err("out of bounds");
            assert_eq!(
                error,
                EditBatchError::InvalidEdit(VoxelEditError::InvalidPosition { pos, bounds })
            );
        }
    }

    #[test]
    fn material_zero_removes_and_reserved_material_is_rejected() {
        assert!(validate_batch(&[edit(0, 0, 0, MATERIAL_EMPTY)], BOUNDS).is_ok());
        assert!(validate_batch(&[edit(0, 0, 0, MATERIAL_RESERVED - 1)], BOUNDS).is_ok());
        assert_eq!(
            validate_batch(&[edit(0, 0, 0, MATERIAL_RESERVED)], BOUNDS),
            Err(EditBatchError::InvalidEdit(
                VoxelEditError::InvalidMaterial {
                    material: MATERIAL_RESERVED
                }
            ))
        );

        let mut log = record();
        log.apply_batch(0, &[edit(1, 1, 1, 7)], BOUNDS)
            .expect("set voxel");
        log.apply_batch(1, &[edit(1, 1, 1, MATERIAL_EMPTY)], BOUNDS)
            .expect("remove voxel");
        assert_eq!(log.effective_voxel([1, 1, 1]), MATERIAL_EMPTY);
        assert_eq!(log.delta_at([1, 1, 1]), Some(MATERIAL_EMPTY));
    }

    #[test]
    fn duplicate_positions_in_one_batch_are_rejected() {
        let edits = [edit(2, 3, 4, 1), edit(2, 3, 4, 2)];
        assert_eq!(
            validate_batch(&edits, BOUNDS),
            Err(EditBatchError::DuplicatePosition { pos: [2, 3, 4] })
        );

        let mut log = record();
        assert!(log.apply_batch(0, &edits, BOUNDS).is_err());
        assert!(log.is_empty());
        assert_eq!(log.edit_revision, 0);
    }

    #[test]
    fn revision_saturates_at_u32_max() {
        let mut log = record();
        log.edit_revision = u32::MAX;
        log.apply_batch(u32::MAX, &[edit(0, 0, 0, 1)], BOUNDS)
            .expect("saturated revision still applies");
        assert_eq!(log.edit_revision, u32::MAX);
        assert_eq!(log.delta_count(), 1);
    }

    #[test]
    fn edit_log_overflow_is_rejected_without_mutation() {
        let mut log = filled_log(MAX_EDIT_LOG);
        log.edit_revision = 7;
        let before = log.clone();

        let error = log
            .apply_batch(7, &[edit(5, 5, 5, 1)], BOUNDS)
            .expect_err("log is full");
        assert_eq!(
            error,
            EditBatchError::Overflow {
                count: MAX_EDIT_LOG + 1,
                max: MAX_EDIT_LOG
            }
        );
        assert_eq!(log, before);
    }

    #[test]
    fn last_edit_wins_across_batches() {
        let mut log = record();
        log.apply_batch(0, &[edit(1, 2, 3, 5)], BOUNDS)
            .expect("first write");
        assert_eq!(log.effective_voxel([1, 2, 3]), 5);

        log.apply_batch(1, &[edit(1, 2, 3, 9)], BOUNDS)
            .expect("second write");
        assert_eq!(log.effective_voxel([1, 2, 3]), 9);
        assert_eq!(log.delta_at([1, 2, 3]), Some(9));
        assert_eq!(log.delta_count(), 2);

        log.apply_batch(2, &[edit(1, 2, 3, MATERIAL_EMPTY)], BOUNDS)
            .expect("remove");
        assert_eq!(log.effective_voxel([1, 2, 3]), MATERIAL_EMPTY);
    }

    #[test]
    fn effective_voxel_defaults_to_empty_for_untouched_cells() {
        let mut log = record();
        log.apply_batch(0, &[edit(1, 1, 1, 4)], BOUNDS)
            .expect("write");
        assert_eq!(log.effective_voxel([0, 0, 0]), MATERIAL_EMPTY);
        assert_eq!(log.delta_at([0, 0, 0]), None);
        assert_eq!(log.effective_voxel([1, 1, 1]), 4);
    }

    #[test]
    fn checksum_is_stable_and_order_sensitive() {
        let mut first = record();
        first
            .apply_batch(0, &[edit(0, 0, 0, 1), edit(1, 0, 0, 2)], BOUNDS)
            .expect("first order");

        let mut second = record();
        second
            .apply_batch(0, &[edit(1, 0, 0, 2), edit(0, 0, 0, 1)], BOUNDS)
            .expect("second order");

        assert_ne!(first.checksum(), second.checksum());

        let mut same = first.clone();
        same.apply_batch(1, &[edit(2, 0, 0, 3)], BOUNDS)
            .expect("append");
        let checksum = same.checksum();
        assert_eq!(same.checksum(), checksum, "repeated calls are stable");
        assert_ne!(first.checksum(), checksum);

        let roundtrip: PlacedBlueprintEdit =
            serde_json::from_str(&serde_json::to_string(&same).expect("serialise"))
                .expect("deserialise");
        assert_eq!(roundtrip, same);
        assert_eq!(roundtrip.checksum(), checksum);
    }

    #[test]
    fn checksum_covers_identity_and_both_revisions() {
        let base = record();
        let checksum = base.checksum();

        let mut other_id = base.clone();
        other_id.placement_id += 1;
        assert_ne!(other_id.checksum(), checksum);

        let mut other_hash = base.clone();
        other_hash.blueprint_hash = HASH_B.to_owned();
        assert_ne!(other_hash.checksum(), checksum);

        let mut other_base = base.clone();
        other_base.base_revision += 1;
        assert_ne!(other_base.checksum(), checksum);

        let mut other_edit = base.clone();
        other_edit.edit_revision += 1;
        assert_ne!(other_edit.checksum(), checksum);
    }

    #[test]
    fn golden_checksum_pins_the_fixture_record() {
        let mut log = record();
        log.apply_batch(
            0,
            &[edit(1, 2, 3, 5), edit(4, 5, 6, MATERIAL_EMPTY)],
            BOUNDS,
        )
        .expect("fixture batch");
        assert_eq!(log.checksum(), 542_860_170_560_474_146);
    }

    #[test]
    fn serde_roundtrip_preserves_the_record() {
        let mut log = record();
        log.apply_batch(0, &[edit(1, 2, 3, 5), edit(4, 5, 6, 0)], BOUNDS)
            .expect("batch");
        let json = serde_json::to_string(&log).expect("serialise");
        let decoded: PlacedBlueprintEdit = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(decoded, log);
        assert_eq!(decoded.checksum(), log.checksum());
        assert_eq!(decoded.effective_voxel([1, 2, 3]), 5);
        assert_eq!(decoded.edit_revision, 1);
    }

    #[test]
    fn validate_loaded_accepts_well_formed_records() {
        let mut log = record();
        log.apply_batch(0, &[edit(0, 0, 0, 1)], BOUNDS)
            .expect("batch");
        assert!(log.validate_loaded().is_ok());
        assert!(record().validate_loaded().is_ok());
    }

    #[test]
    fn validate_loaded_rejects_malformed_records() {
        let mut bad_hash = record();
        bad_hash.blueprint_hash = format!("{BLUEPRINT_HASH_PREFIX}ABCDEF");
        assert_eq!(
            bad_hash.validate_loaded(),
            Err(EditBatchError::InvalidBlueprintHash)
        );
        assert!(!is_canonical_blueprint_hash(HASH_A.to_uppercase().as_str()));

        let mut bad_material = record();
        bad_material.edits = vec![edit(0, 0, 0, MATERIAL_RESERVED)];
        bad_material.edit_revision = 1;
        assert_eq!(
            bad_material.validate_loaded(),
            Err(EditBatchError::InvalidEdit(
                VoxelEditError::InvalidMaterial {
                    material: MATERIAL_RESERVED
                }
            ))
        );

        let mut bad_revision = record();
        bad_revision.edit_revision = 2;
        assert_eq!(
            bad_revision.validate_loaded(),
            Err(EditBatchError::RevisionMismatch { edit_revision: 2 })
        );

        let oversized = filled_log(MAX_EDIT_LOG + 1);
        assert_eq!(
            oversized.validate_loaded(),
            Err(EditBatchError::Overflow {
                count: MAX_EDIT_LOG + 1,
                max: MAX_EDIT_LOG
            })
        );
    }

    #[test]
    fn canonical_hash_shape_is_blake3_and_lowercase_hex() {
        assert!(is_canonical_blueprint_hash(HASH_A));
        assert!(!is_canonical_blueprint_hash(""));
        assert!(!is_canonical_blueprint_hash("blake3:"));
        assert!(!is_canonical_blueprint_hash(&format!(
            "{BLUEPRINT_HASH_PREFIX}{}",
            "a".repeat(63)
        )));
        assert!(!is_canonical_blueprint_hash(&format!(
            "{BLUEPRINT_HASH_PREFIX}{}",
            "a".repeat(65)
        )));
        assert!(!is_canonical_blueprint_hash(&format!(
            "{BLUEPRINT_HASH_PREFIX}{}",
            "g".repeat(64)
        )));
    }
}
