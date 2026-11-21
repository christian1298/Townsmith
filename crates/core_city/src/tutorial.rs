//! A small derived tutorial checklist for the first town.
//!
//! The tutorial is intentionally not a saved campaign state. It reads the
//! current city and reports which starter milestones exist, so a sandbox save
//! can be used as a tutorial and vice versa. Steps are independent; the first
//! unfinished step is highlighted but later steps still count.

/// One tutorial milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TutorialStep {
    /// A lumberjack industry site exists.
    LumberCamp,
    /// A farm industry site exists.
    Farm,
    /// A water utility site exists.
    Well,
    /// A completed residential building exists.
    House,
    /// A school service site exists.
    School,
}

impl TutorialStep {
    /// Every step, in suggested order.
    pub const ALL: [Self; 5] = [
        Self::LumberCamp,
        Self::Farm,
        Self::Well,
        Self::House,
        Self::School,
    ];

    /// Stable identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LumberCamp => "lumber_camp",
            Self::Farm => "farm",
            Self::Well => "well",
            Self::House => "house",
            Self::School => "school",
        }
    }

    /// Short player-facing title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::LumberCamp => "Build a lumber camp",
            Self::Farm => "Build a farm",
            Self::Well => "Build a well",
            Self::House => "Finish a house",
            Self::School => "Build a school",
        }
    }

    /// One-line hint.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::LumberCamp => "Select a job building and press I to add a lumberjack.",
            Self::Farm => "Select a job building and press I to add a farm.",
            Self::Well => "Select a completed job building and press U to add water.",
            Self::House => "Place a starter home and wait for construction to finish.",
            Self::School => "Select a completed job building and press K to add a school.",
        }
    }
}

/// Counts read from the current city.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TutorialInput {
    /// Lumberjack industry sites.
    pub lumberjacks: u64,
    /// Farm industry sites.
    pub farms: u64,
    /// Water utility sites.
    pub wells: u64,
    /// Completed residential buildings.
    pub houses: u64,
    /// School service sites.
    pub schools: u64,
}

impl TutorialInput {
    fn count(self, step: TutorialStep) -> u64 {
        match step {
            TutorialStep::LumberCamp => self.lumberjacks,
            TutorialStep::Farm => self.farms,
            TutorialStep::Well => self.wells,
            TutorialStep::House => self.houses,
            TutorialStep::School => self.schools,
        }
    }
}

/// One step with its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TutorialStepState {
    /// Step definition.
    pub step: TutorialStep,
    /// Whether the milestone exists.
    pub done: bool,
    /// Whether this is the first unfinished step.
    pub current: bool,
}

/// Derived tutorial report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TutorialReport {
    /// One entry per step, in suggested order.
    pub steps: Vec<TutorialStepState>,
    /// Number of completed steps.
    pub completed: usize,
    /// Total steps.
    pub total: usize,
    /// First unfinished step, if any.
    pub current: Option<TutorialStep>,
    /// Whether every step is complete.
    pub complete: bool,
}

/// Evaluate the tutorial from current city counts.
#[must_use]
pub fn evaluate(input: TutorialInput) -> TutorialReport {
    let completed = TutorialStep::ALL
        .iter()
        .filter(|step| input.count(**step) > 0)
        .count();
    let current = TutorialStep::ALL
        .iter()
        .copied()
        .find(|step| input.count(*step) == 0);
    let steps = TutorialStep::ALL
        .iter()
        .copied()
        .map(|step| TutorialStepState {
            step,
            done: input.count(step) > 0,
            current: current == Some(step),
        })
        .collect();
    TutorialReport {
        steps,
        completed,
        total: TutorialStep::ALL.len(),
        current,
        complete: completed == TutorialStep::ALL.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_city_starts_at_the_lumber_camp() {
        let report = evaluate(TutorialInput::default());
        assert_eq!(report.completed, 0);
        assert_eq!(report.current, Some(TutorialStep::LumberCamp));
        assert!(!report.complete);
        assert!(report.steps[0].current);
    }

    #[test]
    fn steps_are_independent_and_out_of_order() {
        let report = evaluate(TutorialInput {
            houses: 1,
            schools: 1,
            ..TutorialInput::default()
        });
        assert_eq!(report.completed, 2);
        assert_eq!(report.current, Some(TutorialStep::LumberCamp));
        assert!(report.steps[3].done);
        assert!(report.steps[4].done);
    }

    #[test]
    fn a_complete_town_has_no_current_step() {
        let report = evaluate(TutorialInput {
            lumberjacks: 1,
            farms: 1,
            wells: 1,
            houses: 2,
            schools: 1,
        });
        assert_eq!(report.completed, 5);
        assert_eq!(report.total, 5);
        assert!(report.complete);
        assert_eq!(report.current, None);
    }

    #[test]
    fn the_first_missing_step_is_current() {
        let report = evaluate(TutorialInput {
            lumberjacks: 1,
            farms: 1,
            ..TutorialInput::default()
        });
        assert_eq!(report.current, Some(TutorialStep::Well));
        assert!(!report.steps[2].done);
    }

    #[test]
    fn step_metadata_is_unique_and_stable() {
        let mut ids = std::collections::BTreeSet::new();
        let mut titles = std::collections::BTreeSet::new();
        for step in TutorialStep::ALL {
            assert!(ids.insert(step.as_str()));
            assert!(titles.insert(step.title()));
            assert!(!step.hint().is_empty());
        }
        assert_eq!(ids.len(), 5);
        assert_eq!(titles.len(), 5);
    }
}
