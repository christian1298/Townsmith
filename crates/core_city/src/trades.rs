//! Age-aware construction trades.
//!
//! Phase 3 made construction a phased site with a shared crew pool. This module
//! adds the trade vocabulary and a small age-derived roster: a Wood-age town
//! has laborers, carpenters, and a mason; later ages add engineers, steel
//! workers, and electricians. The roster caps the aggregate crew pool and
//! labels each phase with its preferred trade. Per-trade crew queues and
//! training are later work.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::progression::Age;

/// One construction trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trade {
    /// General site labor and excavation.
    Laborer,
    /// Framing, roofs, and interior finish.
    Carpenter,
    /// Foundations, stone, brick, and masonry.
    Mason,
    /// Steel erection and heavy structure.
    Steelworker,
    /// Utility rough-in and electrical systems.
    Electrician,
    /// Survey, inspection, and structural engineering.
    Engineer,
}

impl Trade {
    /// Every trade in display order.
    pub const ALL: [Self; 6] = [
        Self::Laborer,
        Self::Carpenter,
        Self::Mason,
        Self::Steelworker,
        Self::Electrician,
        Self::Engineer,
    ];

    /// Stable save/HUD code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Laborer => "laborer",
            Self::Carpenter => "carpenter",
            Self::Mason => "mason",
            Self::Steelworker => "steelworker",
            Self::Electrician => "electrician",
            Self::Engineer => "engineer",
        }
    }

    /// Human-readable label.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Laborer => "Laborer",
            Self::Carpenter => "Carpenter",
            Self::Mason => "Mason",
            Self::Steelworker => "Steelworker",
            Self::Electrician => "Electrician",
            Self::Engineer => "Engineer",
        }
    }

    /// Parse a stable code or common alias.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "laborer" | "labourer" | "worker" | "general_labor" => Some(Self::Laborer),
            "carpenter" | "framer" | "roofer" => Some(Self::Carpenter),
            "mason" | "bricklayer" | "stonecutter" => Some(Self::Mason),
            "steelworker" | "steel_worker" | "ironworker" => Some(Self::Steelworker),
            "electrician" | "electrical" => Some(Self::Electrician),
            "engineer" | "surveyor" | "inspector" | "civil_engineer" => Some(Self::Engineer),
            _ => None,
        }
    }
}

/// A per-trade crew roster.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradePool {
    /// Crew slots by trade. Missing trades are zero.
    pub capacity: BTreeMap<Trade, u16>,
}

impl TradePool {
    /// An empty roster.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The default roster for an age.
    #[must_use]
    pub fn for_age(age: Age) -> Self {
        let mut capacity = BTreeMap::new();
        let counts: &[(Trade, u16)] = match age {
            Age::Wood => &[
                (Trade::Laborer, 4),
                (Trade::Carpenter, 2),
                (Trade::Mason, 1),
            ],
            Age::Masonry => &[
                (Trade::Laborer, 4),
                (Trade::Carpenter, 3),
                (Trade::Mason, 3),
                (Trade::Engineer, 1),
            ],
            Age::Industrial => &[
                (Trade::Laborer, 4),
                (Trade::Carpenter, 3),
                (Trade::Mason, 3),
                (Trade::Steelworker, 2),
                (Trade::Electrician, 2),
                (Trade::Engineer, 1),
            ],
            Age::Modern => &[
                (Trade::Laborer, 4),
                (Trade::Carpenter, 3),
                (Trade::Mason, 3),
                (Trade::Steelworker, 3),
                (Trade::Electrician, 3),
                (Trade::Engineer, 2),
            ],
            Age::Information => &[
                (Trade::Laborer, 4),
                (Trade::Carpenter, 4),
                (Trade::Mason, 4),
                (Trade::Steelworker, 4),
                (Trade::Electrician, 4),
                (Trade::Engineer, 4),
            ],
        };
        for &(trade, count) in counts {
            capacity.insert(trade, count);
        }
        Self { capacity }
    }

    /// Crew slots for one trade.
    #[must_use]
    pub fn get(&self, trade: Trade) -> u16 {
        self.capacity.get(&trade).copied().unwrap_or(0)
    }

    /// Set crew slots for one trade. Values are clamped to 64.
    pub fn set(&mut self, trade: Trade, count: u16) {
        if count == 0 {
            self.capacity.remove(&trade);
        } else {
            self.capacity.insert(trade, count.min(64));
        }
    }

    /// Total crew slots.
    #[must_use]
    pub fn total(&self) -> u16 {
        self.capacity.values().copied().sum()
    }

    /// Whether the roster has at least one crew slot for a trade.
    #[must_use]
    pub fn has(&self, trade: Trade) -> bool {
        self.get(trade) > 0
    }

    /// Validate counts and the aggregate cap.
    pub fn validate(&self) -> Result<(), TradeError> {
        let mut total = 0_u32;
        for (&trade, &count) in &self.capacity {
            if count == 0 || count > 64 {
                return Err(TradeError::Invalid(trade.as_str()));
            }
            total += u32::from(count);
        }
        if total > 256 {
            return Err(TradeError::Invalid("total crew"));
        }
        Ok(())
    }
}

/// Errors returned by the trade roster.
#[derive(Debug)]
pub enum TradeError {
    /// A trade count or the aggregate total is out of range.
    Invalid(&'static str),
}

impl std::fmt::Display for TradeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(name) => write!(formatter, "invalid trade crew: {name}"),
        }
    }
}

impl std::error::Error for TradeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wood_has_the_smallest_roster() {
        let wood = TradePool::for_age(Age::Wood);
        assert_eq!(wood.total(), 7);
        assert_eq!(wood.get(Trade::Laborer), 4);
        assert_eq!(wood.get(Trade::Carpenter), 2);
        assert_eq!(wood.get(Trade::Mason), 1);
        assert_eq!(wood.get(Trade::Electrician), 0);
        assert!(!wood.has(Trade::Engineer));
    }

    #[test]
    fn later_ages_add_specialists_and_capacity() {
        let masonry = TradePool::for_age(Age::Masonry);
        let industrial = TradePool::for_age(Age::Industrial);
        let modern = TradePool::for_age(Age::Modern);
        let information = TradePool::for_age(Age::Information);
        assert!(masonry.has(Trade::Engineer));
        assert!(!masonry.has(Trade::Electrician));
        assert!(industrial.has(Trade::Electrician));
        assert!(industrial.has(Trade::Steelworker));
        assert!(modern.total() > industrial.total());
        assert!(information.total() > modern.total());
    }

    #[test]
    fn parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(Trade::parse("labourer"), Some(Trade::Laborer));
        assert_eq!(Trade::parse(" steel-worker "), Some(Trade::Steelworker));
        assert_eq!(Trade::parse("inspector"), Some(Trade::Engineer));
        assert_eq!(Trade::parse("wizard"), None);
    }

    #[test]
    fn set_and_validate_clamp_and_reject_invalid() {
        let mut pool = TradePool::new();
        pool.set(Trade::Laborer, 70);
        assert_eq!(pool.get(Trade::Laborer), 64);
        pool.set(Trade::Laborer, 0);
        assert_eq!(pool.get(Trade::Laborer), 0);
        assert!(pool.validate().is_ok());
        pool.capacity.insert(Trade::Mason, 0);
        assert!(pool.validate().is_err());
    }
}
