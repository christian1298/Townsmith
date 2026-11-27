//! Unwired command-journal record model for D-040.
//!
//! D-040 is **Proposed**. This module is a pure, in-memory model of the
//! proposed v1 journal entry, sequence, retention, and chain rules. It has no
//! disk, network, server, save, or GDExtension integration and is not wired
//! into the authoritative executor.
//!
//! # Chain
//!
//! The chain is the retained-window head: four FNV-1a-64 lanes over
//! `prev_head || canonical entry fields`, giving a 32-byte [`Journal::chain_hash`].
//! [`Journal::checksum`] folds that head with the epoch and entry count into a
//! `u64`. FNV is used because this crate has no hashing dependency; the chain
//! detects accidental corruption and naive edits, not a deliberate rewrite. It
//! is not a signature and is not authentication (D-004).
//!
//! Canonical fields are written in the order `seq`, `tick`, `player_id`,
//! `role`, `tag`, `command_checksum`, `accepted`, `result_code`. Integers are
//! little-endian; strings are a `u64` little-endian byte length followed by
//! their UTF-8 bytes.
//!
//! Retention trims the oldest entries and re-bases the chain over the retained
//! window, seeded by the epoch. This keeps the stored head verifiable from the
//! entries that remain.
//!
//! # Scope
//!
//! The model stores a 64-bit command fingerprint, not the proposed 64-hex
//! BLAKE3 command checksum. It documents the 64-byte limits for `tag` and
//! `result_code` but does not enforce them; a future writer must reject
//! oversized fields before append. Segments, payload capture, snapshots, and
//! rollback commands are out of scope.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Current journal model format.
pub const JOURNAL_FORMAT_VERSION: u16 = 1;

/// Longest protocol tag the proposed storage format accepts.
pub const MAX_JOURNAL_TAG_BYTES: usize = 64;

/// Longest stable result code the proposed storage format accepts.
pub const MAX_JOURNAL_RESULT_CODE_BYTES: usize = 64;

/// Provisional entry cap from D-040, pending Q-123.
pub const DEFAULT_MAX_JOURNAL_ENTRIES: usize = 100_000;

const HEADER_DOMAIN: &[u8] = b"townsmith.journal.header.v1";
const ENTRY_DOMAIN: &[u8] = b"townsmith.journal.entry.v1";

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One decided command record.
///
/// `seq` is assigned by [`Journal::append`] when it is `0`; otherwise the
/// caller must supply the next expected sequence. `player_id` is `0` for
/// server/system actions. `accepted` is `false` for journaled rejections,
/// which are retained for audit but never replayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// World-scoped monotonic sequence, starting at 1.
    pub seq: u64,
    /// Server tick when the command was decided.
    pub tick: u64,
    /// Stable save-local player id; `0` for server/system actions.
    pub player_id: u64,
    /// Role at decision time (`viewer`, `mayor`, `admin`, `system`).
    pub role: String,
    /// Protocol command tag, for example `place_blueprint`.
    pub tag: String,
    /// Opaque 64-bit fingerprint of the canonical command bytes.
    pub command_checksum: u64,
    /// Whether the command mutated state.
    pub accepted: bool,
    /// Stable result code, for example `ok` or `role.forbidden`.
    pub result_code: String,
}

impl JournalEntry {
    /// Build an entry with an unassigned sequence (`seq = 0`).
    #[must_use]
    pub fn new(
        tick: u64,
        player_id: u64,
        role: impl Into<String>,
        tag: impl Into<String>,
        command_checksum: u64,
        accepted: bool,
        result_code: impl Into<String>,
    ) -> Self {
        Self {
            seq: 0,
            tick,
            player_id,
            role: role.into(),
            tag: tag.into(),
            command_checksum,
            accepted,
            result_code: result_code.into(),
        }
    }
}

/// Bounded, append-only command journal for one epoch.
///
/// The model keeps the retained entries, the next sequence to assign, the
/// configured entry cap, and the retained-window chain head. It never updates
/// an entry in place during normal operation; [`Self::truncate_after`] is an
/// explicit tail-discard operation for tests and previews, not a committed
/// journal operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    epoch: u64,
    entries: Vec<JournalEntry>,
    max_entries: usize,
    next_seq: u64,
    chain: [u8; 32],
}

impl Journal {
    /// Create an empty journal.
    ///
    /// `epoch` must be at least 1 for [`Self::validate_loaded`] to accept the
    /// journal. `max_entries` of 0 is representable but cannot accept entries.
    #[must_use]
    pub fn new(epoch: u64, max_entries: usize) -> Self {
        let next_seq = 1;
        Self {
            epoch,
            entries: Vec::new(),
            max_entries,
            next_seq,
            chain: header_chain(epoch),
        }
    }

    /// Journal epoch; incremented by a proposed rollback.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Retained entries in `seq` order.
    #[must_use]
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// Configured maximum retained entry count.
    #[must_use]
    pub const fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Next sequence this journal will assign.
    #[must_use]
    pub const fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Append one decided command.
    ///
    /// An entry with `seq == 0` receives [`Self::next_seq`]. An entry with a
    /// nonzero `seq` must already equal the next expected sequence. The oldest
    /// entries are trimmed after the push when the cap is exceeded, and the
    /// retained-window chain is re-based.
    ///
    /// # Errors
    ///
    /// - [`JournalError::Capacity`] when `max_entries == 0`.
    /// - [`JournalError::Overflow`] when the sequence space is exhausted.
    /// - [`JournalError::InvalidSeq`] when a supplied `seq` is not next.
    ///
    /// A failed append leaves the entries, sequence, and chain unchanged.
    pub fn append(&mut self, mut entry: JournalEntry) -> Result<u64, JournalError> {
        if self.max_entries == 0 {
            return Err(JournalError::Capacity {
                max: 0,
                len: self.entries.len(),
            });
        }

        let expected = self.next_seq;
        let assigned_next = expected.checked_add(1).ok_or(JournalError::Overflow)?;

        if entry.seq == 0 {
            entry.seq = expected;
        } else if entry.seq != expected {
            return Err(JournalError::InvalidSeq {
                expected,
                found: entry.seq,
            });
        }

        self.chain = chain_entry(self.chain, &entry);
        let seq = entry.seq;
        self.entries.push(entry);
        self.next_seq = assigned_next;

        if self.entries.len() > self.max_entries {
            let excess = self.entries.len() - self.max_entries;
            self.entries.drain(0..excess);
            self.rebase_chain();
        }

        Ok(seq)
    }

    /// Compact checksum of the retained journal: chain head, epoch, and length.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = fnv_write(FNV_OFFSET_BASIS, &self.chain);
        hash = fnv_write_u64(hash, self.epoch);
        hash = fnv_write_u64(hash, self.entries.len() as u64);
        hash
    }

    /// Retained-window chain head.
    #[must_use]
    pub const fn chain_hash(&self) -> [u8; 32] {
        self.chain
    }

    /// Discard retained entries after `seq` and re-base the chain.
    ///
    /// This is a tail-discard helper for previews and tests. Sequences after
    /// `seq` become assignable again, so a committed writer must never call it.
    /// Returns the number of removed entries.
    pub fn truncate_after(&mut self, seq: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.seq <= seq);
        let removed = before - self.entries.len();
        if removed > 0 {
            self.next_seq = self
                .entries
                .last()
                .map_or(1, |entry| entry.seq.saturating_add(1));
            self.rebase_chain();
        }
        removed
    }

    /// Drop whole entries at or below `tick` and re-base the chain.
    ///
    /// The high-water sequence is preserved, so a fully drained window never
    /// reuses an assigned sequence. Returns the number of removed entries.
    pub fn retention_drop_through(&mut self, tick: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.tick > tick);
        let removed = before - self.entries.len();
        if removed > 0 {
            self.rebase_chain();
        }
        removed
    }

    /// Validate a freshly loaded journal.
    ///
    /// Checks that the epoch is nonzero, the retained entries fit the cap,
    /// sequences are contiguous and below the high-water mark, and the stored
    /// chain head matches the entries.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::InvalidEpoch`], [`JournalError::Capacity`],
    /// [`JournalError::Gap`], [`JournalError::InvalidSeq`],
    /// [`JournalError::Overflow`], or [`JournalError::ChainMismatch`].
    pub fn validate_loaded(&self) -> Result<(), JournalError> {
        if self.epoch == 0 {
            return Err(JournalError::InvalidEpoch { epoch: 0 });
        }
        if self.entries.len() > self.max_entries {
            return Err(JournalError::Capacity {
                max: self.max_entries,
                len: self.entries.len(),
            });
        }

        let mut expected: Option<u64> = None;
        for entry in &self.entries {
            if entry.seq == 0 {
                return Err(JournalError::InvalidSeq {
                    expected: 1,
                    found: 0,
                });
            }
            if let Some(expected) = expected {
                if entry.seq != expected {
                    return Err(JournalError::Gap {
                        expected,
                        found: entry.seq,
                    });
                }
            }
            expected = Some(entry.seq.checked_add(1).ok_or(JournalError::Overflow)?);
        }

        match (self.entries.last(), expected) {
            (Some(_), Some(expected_next)) => {
                if self.next_seq != expected_next {
                    return Err(JournalError::InvalidSeq {
                        expected: expected_next,
                        found: self.next_seq,
                    });
                }
            }
            (None, _) => {
                if self.next_seq == 0 {
                    return Err(JournalError::InvalidSeq {
                        expected: 1,
                        found: 0,
                    });
                }
            }
            (Some(_), None) => return Err(JournalError::Overflow),
        }

        if self.recomputed_chain() != self.chain {
            return Err(JournalError::ChainMismatch);
        }

        Ok(())
    }

    /// Retained entries after `seq`.
    ///
    /// The caller must check [`Self::gap_after`] first; this returns only what
    /// the retained window still holds.
    #[must_use]
    pub fn replay_after(&self, seq: u64) -> &[JournalEntry] {
        let start = self.entries.partition_point(|entry| entry.seq <= seq);
        &self.entries[start..]
    }

    /// First sequence missing after `seq`, or `None` when the retained window
    /// continues without a gap (or cannot be compared).
    #[must_use]
    pub fn gap_after(&self, seq: u64) -> Option<u64> {
        let next = seq.checked_add(1)?;
        let first = self.entries.first()?;
        if first.seq > next {
            Some(next)
        } else {
            None
        }
    }

    /// Number of retained entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the retained window is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn rebase_chain(&mut self) {
        let mut chain = header_chain(self.epoch);
        for entry in &self.entries {
            chain = chain_entry(chain, entry);
        }
        self.chain = chain;
    }

    fn recomputed_chain(&self) -> [u8; 32] {
        let mut chain = header_chain(self.epoch);
        for entry in &self.entries {
            chain = chain_entry(chain, entry);
        }
        chain
    }
}

/// Stable journal failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    /// A supplied sequence is not the next expected one.
    InvalidSeq {
        /// Sequence the journal expected.
        expected: u64,
        /// Sequence the caller supplied.
        found: u64,
    },
    /// The journal epoch is zero or otherwise unusable.
    InvalidEpoch {
        /// Rejected epoch value.
        epoch: u64,
    },
    /// The stored chain head does not match the retained entries.
    ChainMismatch,
    /// Retained sequences are not contiguous.
    Gap {
        /// First missing sequence.
        expected: u64,
        /// Sequence that was found instead.
        found: u64,
    },
    /// The sequence space is exhausted.
    Overflow,
    /// The entry cap cannot hold the record.
    Capacity {
        /// Configured cap.
        max: usize,
        /// Requested or retained count.
        len: usize,
    },
}

impl JournalError {
    /// Stable code for protocol, logs, and tests.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidSeq { .. } => "journal.invalid_seq",
            Self::InvalidEpoch { .. } => "journal.invalid_epoch",
            Self::ChainMismatch => "journal.chain_mismatch",
            Self::Gap { .. } => "journal.gap",
            Self::Overflow => "journal.overflow",
            Self::Capacity { .. } => "journal.capacity",
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSeq { expected, found } => {
                write!(f, "{}: expected seq {expected}, found {found}", self.code())
            }
            Self::InvalidEpoch { epoch } => {
                write!(f, "{}: epoch {epoch}", self.code())
            }
            Self::ChainMismatch => f.write_str(self.code()),
            Self::Gap { expected, found } => {
                write!(f, "{}: expected seq {expected}, found {found}", self.code())
            }
            Self::Overflow => f.write_str(self.code()),
            Self::Capacity { max, len } => {
                write!(f, "{}: max {max}, len {len}", self.code())
            }
        }
    }
}

impl Error for JournalError {}

fn header_chain(epoch: u64) -> [u8; 32] {
    lane_hash(|hash, lane| {
        let hash = fnv_write(hash, HEADER_DOMAIN);
        let hash = fnv_write_u64(hash, epoch);
        fnv_write(hash, &[lane])
    })
}

fn chain_entry(prev: [u8; 32], entry: &JournalEntry) -> [u8; 32] {
    lane_hash(|hash, lane| {
        let hash = fnv_write(hash, ENTRY_DOMAIN);
        let hash = fnv_write(hash, &prev);
        let hash = fnv_write(hash, &[lane]);
        let hash = fnv_write_u64(hash, entry.seq);
        let hash = fnv_write_u64(hash, entry.tick);
        let hash = fnv_write_u64(hash, entry.player_id);
        let hash = fnv_write_str(hash, &entry.role);
        let hash = fnv_write_str(hash, &entry.tag);
        let hash = fnv_write_u64(hash, entry.command_checksum);
        let hash = fnv_write(hash, &[u8::from(entry.accepted)]);
        fnv_write_str(hash, &entry.result_code)
    })
}

fn lane_hash(write_lane: impl Fn(u64, u8) -> u64) -> [u8; 32] {
    let mut out = [0_u8; 32];
    for lane in 0_u8..4 {
        let mut hash = FNV_OFFSET_BASIS ^ (u64::from(lane) + 1).wrapping_mul(FNV_PRIME);
        hash = write_lane(hash, lane);
        let start = usize::from(lane) * 8;
        out[start..start + 8].copy_from_slice(&hash.to_le_bytes());
    }
    out
}

fn fnv_write(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn fnv_write_u64(hash: u64, value: u64) -> u64 {
    fnv_write(hash, &value.to_le_bytes())
}

fn fnv_write_str(hash: u64, value: &str) -> u64 {
    let hash = fnv_write_u64(hash, value.len() as u64);
    fnv_write(hash, value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64, tick: u64) -> JournalEntry {
        JournalEntry {
            seq,
            tick,
            player_id: 7,
            role: "mayor".to_string(),
            tag: "place_blueprint".to_string(),
            command_checksum: 0x1234_5678,
            accepted: true,
            result_code: "ok".to_string(),
        }
    }

    #[test]
    fn first_append_starts_at_one() {
        let mut journal = Journal::new(3, 8);
        assert!(journal.is_empty());
        assert_eq!(journal.next_seq(), 1);

        let seq = journal.append(entry(0, 10)).unwrap();

        assert_eq!(seq, 1);
        assert_eq!(journal.len(), 1);
        assert_eq!(journal.entries()[0].seq, 1);
        assert_eq!(journal.next_seq(), 2);
        assert!(journal.validate_loaded().is_ok());
    }

    #[test]
    fn append_requires_monotonic_sequence() {
        let mut journal = Journal::new(1, 8);
        let empty_chain = journal.chain_hash();
        let empty_checksum = journal.checksum();

        let err = journal.append(entry(2, 1)).unwrap_err();
        assert_eq!(err.code(), "journal.invalid_seq");
        assert_eq!(
            err,
            JournalError::InvalidSeq {
                expected: 1,
                found: 2
            }
        );
        assert!(journal.is_empty());
        assert_eq!(journal.next_seq(), 1);
        assert_eq!(journal.chain_hash(), empty_chain);
        assert_eq!(journal.checksum(), empty_checksum);

        assert_eq!(journal.append(entry(1, 1)).unwrap(), 1);
        assert_eq!(journal.append(entry(0, 2)).unwrap(), 2);
        assert_eq!(journal.append(entry(3, 3)).unwrap(), 3);
        assert_eq!(journal.next_seq(), 4);
        assert!(journal.validate_loaded().is_ok());
    }

    #[test]
    fn sequence_overflow_is_rejected_without_mutation() {
        let mut journal = Journal::new(1, 8);
        journal.next_seq = u64::MAX;
        journal.entries.push(entry(u64::MAX - 1, 1));
        journal.rebase_chain();
        let before = journal.checksum();

        let err = journal.append(entry(0, 2)).unwrap_err();

        assert_eq!(err.code(), "journal.overflow");
        assert_eq!(journal.len(), 1);
        assert_eq!(journal.next_seq(), u64::MAX);
        assert_eq!(journal.checksum(), before);
    }

    #[test]
    fn capacity_trims_oldest_and_rebases_chain() {
        let mut journal = Journal::new(1, 2);

        assert_eq!(journal.append(entry(0, 10)).unwrap(), 1);
        assert_eq!(journal.append(entry(0, 20)).unwrap(), 2);
        assert_eq!(journal.append(entry(0, 30)).unwrap(), 3);

        assert_eq!(journal.len(), 2);
        assert_eq!(journal.entries()[0].seq, 2);
        assert_eq!(journal.entries()[1].seq, 3);
        assert_eq!(journal.next_seq(), 4);
        assert_eq!(journal.replay_after(1).len(), 2);
        assert_eq!(journal.gap_after(0), Some(1));
        assert_eq!(journal.gap_after(1), None);
        assert!(journal.validate_loaded().is_ok());
    }

    #[test]
    fn zero_capacity_rejects_append() {
        let mut journal = Journal::new(1, 0);
        let before = journal.chain_hash();

        let err = journal.append(entry(0, 1)).unwrap_err();

        assert_eq!(err.code(), "journal.capacity");
        assert_eq!(err, JournalError::Capacity { max: 0, len: 0 });
        assert!(journal.is_empty());
        assert_eq!(journal.next_seq(), 1);
        assert_eq!(journal.chain_hash(), before);
    }

    #[test]
    fn one_capacity_retains_latest() {
        let mut journal = Journal::new(1, 1);
        journal.append(entry(0, 1)).unwrap();
        journal.append(entry(0, 2)).unwrap();

        assert_eq!(journal.len(), 1);
        assert_eq!(journal.entries()[0].seq, 2);
        assert_eq!(journal.next_seq(), 3);
        assert_eq!(journal.replay_after(1).len(), 1);
        assert!(journal.replay_after(2).is_empty());
        assert!(journal.validate_loaded().is_ok());
    }

    #[test]
    fn retention_drops_whole_entries_and_preserves_high_water() {
        let mut journal = Journal::new(1, 8);
        journal.append(entry(0, 10)).unwrap();
        journal.append(entry(0, 10)).unwrap();
        journal.append(entry(0, 20)).unwrap();
        journal.append(entry(0, 30)).unwrap();

        assert_eq!(journal.retention_drop_through(10), 2);
        assert_eq!(journal.len(), 2);
        assert_eq!(journal.entries()[0].seq, 3);
        assert_eq!(journal.next_seq(), 5);
        assert_eq!(journal.gap_after(1), Some(2));
        assert_eq!(journal.gap_after(2), None);
        assert!(journal.validate_loaded().is_ok());

        assert_eq!(journal.retention_drop_through(30), 2);
        assert!(journal.is_empty());
        assert_eq!(journal.next_seq(), 5);
        assert!(journal.validate_loaded().is_ok());
        assert_eq!(journal.append(entry(0, 40)).unwrap(), 5);
    }

    #[test]
    fn truncate_after_keeps_prefix_and_allows_reuse() {
        let mut journal = Journal::new(1, 8);
        journal.append(entry(0, 1)).unwrap();
        journal.append(entry(0, 2)).unwrap();
        journal.append(entry(0, 3)).unwrap();

        assert_eq!(journal.truncate_after(1), 2);
        assert_eq!(journal.len(), 1);
        assert_eq!(journal.next_seq(), 2);
        assert!(journal.validate_loaded().is_ok());

        assert_eq!(journal.append(entry(0, 4)).unwrap(), 2);
        assert_eq!(journal.truncate_after(999), 0);
        assert_eq!(journal.truncate_after(u64::MAX), 0);
        assert!(journal.validate_loaded().is_ok());
    }

    #[test]
    fn replay_and_gap_track_the_retained_window() {
        let mut journal = Journal::new(1, 8);
        assert!(journal.replay_after(0).is_empty());
        assert_eq!(journal.gap_after(0), None);
        assert_eq!(journal.gap_after(u64::MAX), None);

        for tick in 1..=3 {
            journal.append(entry(0, tick)).unwrap();
        }
        assert_eq!(journal.replay_after(0).len(), 3);
        assert_eq!(journal.replay_after(1).len(), 2);
        assert_eq!(journal.replay_after(3).len(), 0);
        assert_eq!(journal.gap_after(0), None);

        journal.retention_drop_through(2);
        assert_eq!(journal.replay_after(0).len(), 1);
        assert_eq!(journal.gap_after(0), Some(1));
        assert_eq!(journal.gap_after(2), None);
    }

    #[test]
    fn empty_journal_validates_and_has_stable_checksum() {
        let journal = Journal::new(1, 8);
        assert!(journal.is_empty());
        assert_eq!(journal.len(), 0);
        assert!(journal.validate_loaded().is_ok());
        assert_eq!(journal.checksum(), Journal::new(1, 8).checksum());
        assert_ne!(journal.chain_hash(), [0_u8; 32]);
    }

    #[test]
    fn epoch_changes_the_chain_and_zero_epoch_is_invalid() {
        let mut first = Journal::new(1, 8);
        let mut second = Journal::new(2, 8);
        first.append(entry(0, 1)).unwrap();
        second.append(entry(0, 1)).unwrap();
        assert_ne!(first.chain_hash(), second.chain_hash());
        assert_ne!(first.checksum(), second.checksum());

        let journal = Journal::new(0, 8);
        assert_eq!(
            journal.validate_loaded().unwrap_err(),
            JournalError::InvalidEpoch { epoch: 0 }
        );
        assert_eq!(
            journal.validate_loaded().unwrap_err().code(),
            "journal.invalid_epoch"
        );
    }

    #[test]
    fn tampered_entry_fails_chain_validation() {
        let mut journal = Journal::new(1, 8);
        journal.append(entry(0, 1)).unwrap();
        journal.append(entry(0, 2)).unwrap();

        journal.entries[1].tick = 99;

        assert_eq!(
            journal.validate_loaded().unwrap_err(),
            JournalError::ChainMismatch
        );
    }

    #[test]
    fn validation_rejects_seq_gap_capacity_and_high_water_drift() {
        let mut gap = Journal::new(1, 8);
        gap.entries.push(entry(1, 1));
        gap.entries.push(entry(3, 2));
        gap.next_seq = 4;
        gap.rebase_chain();
        assert_eq!(
            gap.validate_loaded().unwrap_err(),
            JournalError::Gap {
                expected: 2,
                found: 3
            }
        );

        let mut over_capacity = Journal::new(1, 1);
        over_capacity.entries.push(entry(1, 1));
        over_capacity.entries.push(entry(2, 2));
        over_capacity.next_seq = 3;
        over_capacity.rebase_chain();
        assert_eq!(
            over_capacity.validate_loaded().unwrap_err(),
            JournalError::Capacity { max: 1, len: 2 }
        );

        let mut drift = Journal::new(1, 8);
        drift.entries.push(entry(1, 1));
        drift.next_seq = 3;
        drift.rebase_chain();
        assert_eq!(
            drift.validate_loaded().unwrap_err(),
            JournalError::InvalidSeq {
                expected: 2,
                found: 3
            }
        );
    }

    #[test]
    fn serde_roundtrip_preserves_chain_and_reopened_journal_validates() {
        let mut journal = Journal::new(4, 8);
        journal.append(entry(0, 10)).unwrap();
        journal.append(entry(0, 10)).unwrap();

        let json = serde_json::to_string(&journal).unwrap();
        let loaded: Journal = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, journal);
        assert_eq!(loaded.chain_hash(), journal.chain_hash());
        assert_eq!(loaded.checksum(), journal.checksum());
        assert!(loaded.validate_loaded().is_ok());
    }

    #[test]
    fn serde_edit_is_detected_as_tamper() {
        let mut journal = Journal::new(1, 8);
        journal.append(entry(0, 10)).unwrap();
        let json = serde_json::to_string(&journal).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["entries"][0]["accepted"] = serde_json::Value::Bool(false);

        let loaded: Journal = serde_json::from_value(value).unwrap();

        assert_eq!(
            loaded.validate_loaded().unwrap_err(),
            JournalError::ChainMismatch
        );
    }

    #[test]
    fn error_codes_are_stable_and_displayed() {
        let errors = [
            JournalError::InvalidSeq {
                expected: 1,
                found: 2,
            },
            JournalError::InvalidEpoch { epoch: 0 },
            JournalError::ChainMismatch,
            JournalError::Gap {
                expected: 2,
                found: 4,
            },
            JournalError::Overflow,
            JournalError::Capacity { max: 0, len: 1 },
        ];
        let codes = [
            "journal.invalid_seq",
            "journal.invalid_epoch",
            "journal.chain_mismatch",
            "journal.gap",
            "journal.overflow",
            "journal.capacity",
        ];

        for (error, code) in errors.iter().zip(codes) {
            assert_eq!(error.code(), code);
            assert!(error.to_string().starts_with(code));
        }
    }
}
