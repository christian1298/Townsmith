//! Population-driven age progression.
//!
//! Phase 4 starts with the long arc: a settlement moves from wood through
//! masonry, industrial, modern, and information ages. Research and explicit
//! tech unlocks arrive later; this module is the deterministic population
//! milestone spine that the HUD, advisor, and content library can share.

use serde::{Deserialize, Serialize};

/// The current age of a settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Age {
    /// Timber, thatch, stone, clay, and lime.
    Wood,
    /// Brick, masonry, glass, and mortar.
    Masonry,
    /// Steel, concrete, coal, and iron.
    Industrial,
    /// Reinforced concrete, steel, glass, and aluminum.
    Modern,
    /// Composites, electronics, and solar.
    Information,
}

impl Age {
    /// All ages in progression order.
    pub const ALL: [Self; 5] = [
        Self::Wood,
        Self::Masonry,
        Self::Industrial,
        Self::Modern,
        Self::Information,
    ];

    /// Stable save/UI name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Wood => "wood",
            Self::Masonry => "masonry",
            Self::Industrial => "industrial",
            Self::Modern => "modern",
            Self::Information => "information",
        }
    }

    /// Minimum population that unlocks this age.
    #[must_use]
    pub const fn min_population(self) -> u64 {
        match self {
            Self::Wood => 0,
            Self::Masonry => 250,
            Self::Industrial => 20_000,
            Self::Modern => 150_000,
            Self::Information => 750_000,
        }
    }

    /// Next age, if this is not the final age.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Wood => Some(Self::Masonry),
            Self::Masonry => Some(Self::Industrial),
            Self::Industrial => Some(Self::Modern),
            Self::Modern => Some(Self::Information),
            Self::Information => None,
        }
    }

    /// Materials associated with the age, for the HUD and content filters.
    #[must_use]
    pub const fn materials(self) -> &'static [&'static str] {
        match self {
            Self::Wood => &["timber", "thatch", "stone", "clay", "lime", "glass"],
            Self::Masonry => &["brick", "stone", "mortar", "glass", "lime"],
            Self::Industrial => &["steel", "concrete", "coal", "iron"],
            Self::Modern => &["reinforced_concrete", "steel", "glass", "aluminum"],
            Self::Information => &["composites", "electronics", "solar", "glass"],
        }
    }

    /// Parse an age id or alias.
    ///
    /// Matching trims, lowercases, and treats spaces and hyphens as
    /// underscores.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let normalized: String = value
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|ch| {
                if ch.is_whitespace() || ch == '-' {
                    '_'
                } else {
                    ch
                }
            })
            .collect();
        match normalized.as_str() {
            "wood" | "timber" => Some(Self::Wood),
            "masonry" | "stone" => Some(Self::Masonry),
            "industrial" | "industry" => Some(Self::Industrial),
            "modern" => Some(Self::Modern),
            "information" | "info" => Some(Self::Information),
            _ => None,
        }
    }

    /// The age for a population, using the highest threshold reached.
    #[must_use]
    pub fn from_population(population: u64) -> Self {
        Self::ALL
            .into_iter()
            .rev()
            .find(|age| population >= age.min_population())
            .unwrap_or(Self::Wood)
    }
}

/// Derived age state for the current population.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgeState {
    /// Current age.
    pub age: Age,
    /// Population used to derive the age.
    pub population: u64,
    /// Next age, or `None` at the final age.
    pub next_age: Option<Age>,
    /// Population needed for the next age, or `None` at the final age.
    pub next_threshold: Option<u64>,
    /// Progress from the current threshold to the next threshold, 0..=1.
    pub progress: f32,
}

impl AgeState {
    /// Build the derived state for a population.
    #[must_use]
    pub fn from_population(population: u64) -> Self {
        let age = Age::from_population(population);
        let next_age = age.next();
        let next_threshold = next_age.map(Age::min_population);
        let progress = match (next_age, next_threshold) {
            (Some(_), Some(next)) => {
                let current = age.min_population();
                let span = next.saturating_sub(current).max(1);
                ((population.saturating_sub(current)) as f32 / span as f32).clamp(0.0, 1.0)
            }
            _ => 1.0,
        };
        Self {
            age,
            population,
            next_age,
            next_threshold,
            progress,
        }
    }

    /// Whether the final age has been reached.
    #[must_use]
    pub const fn is_final(&self) -> bool {
        self.next_age.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_unlock_the_expected_age() {
        assert_eq!(Age::from_population(0), Age::Wood);
        assert_eq!(Age::from_population(249), Age::Wood);
        assert_eq!(Age::from_population(250), Age::Masonry);
        assert_eq!(Age::from_population(19_999), Age::Masonry);
        assert_eq!(Age::from_population(20_000), Age::Industrial);
        assert_eq!(Age::from_population(149_999), Age::Industrial);
        assert_eq!(Age::from_population(150_000), Age::Modern);
        assert_eq!(Age::from_population(749_999), Age::Modern);
        assert_eq!(Age::from_population(750_000), Age::Information);
        assert_eq!(Age::from_population(u64::MAX), Age::Information);
    }

    #[test]
    fn state_reports_next_threshold_and_progress() {
        let wood = AgeState::from_population(100);
        assert_eq!(wood.age, Age::Wood);
        assert_eq!(wood.next_age, Some(Age::Masonry));
        assert_eq!(wood.next_threshold, Some(250));
        assert!((wood.progress - 0.4).abs() < 0.001);
        assert!(!wood.is_final());

        let masonry = AgeState::from_population(250);
        assert_eq!(masonry.age, Age::Masonry);
        assert_eq!(masonry.next_age, Some(Age::Industrial));
        assert_eq!(masonry.progress, 0.0);
    }

    #[test]
    fn final_age_is_complete() {
        let state = AgeState::from_population(1_000_000);
        assert_eq!(state.age, Age::Information);
        assert_eq!(state.next_age, None);
        assert_eq!(state.next_threshold, None);
        assert_eq!(state.progress, 1.0);
        assert!(state.is_final());
    }

    #[test]
    fn ages_have_stable_names_and_materials() {
        for (index, age) in Age::ALL.into_iter().enumerate() {
            assert!(!age.name().is_empty());
            assert!(!age.materials().is_empty());
            assert_eq!(
                age.min_population(),
                AgeState::from_population(age.min_population())
                    .age
                    .min_population()
            );
            if let Some(next) = age.next() {
                assert_eq!(next, Age::ALL[index + 1]);
                assert!(next.min_population() > age.min_population());
            }
        }
    }

    #[test]
    fn age_ids_and_aliases_parse() {
        assert_eq!(Age::parse("wood"), Some(Age::Wood));
        assert_eq!(Age::parse(" Timber "), Some(Age::Wood));
        assert_eq!(Age::parse("MASONRY"), Some(Age::Masonry));
        assert_eq!(Age::parse("stone"), Some(Age::Masonry));
        assert_eq!(Age::parse("information"), Some(Age::Information));
        assert_eq!(Age::parse("future"), None);
    }

    #[test]
    fn state_is_deterministic() {
        assert_eq!(
            AgeState::from_population(500),
            AgeState::from_population(500)
        );
        assert_eq!(
            AgeState::from_population(750_000),
            AgeState::from_population(750_000)
        );
    }
}
