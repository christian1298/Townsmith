//! Cohort-scale city simulation.

use std::error::Error;
use std::fmt;

use crate::cohort::Cohort;
use crate::district::DistrictServices;

/// Simulation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    /// A cohort referenced a district that does not exist.
    UnknownDistrict(u32),
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownDistrict(id) => write!(f, "cohort references unknown district {id}"),
        }
    }
}

impl Error for SimError {}

/// A cohort simulation with per-district service inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct CohortSimulation {
    districts: Vec<DistrictServices>,
    cohorts: Vec<Cohort>,
}

impl CohortSimulation {
    /// Create a simulation with `district_count` districts using default services.
    #[must_use]
    pub fn new(district_count: usize) -> Self {
        Self {
            districts: vec![DistrictServices::default(); district_count],
            cohorts: Vec::new(),
        }
    }

    /// Add a cohort. Returns an error if its district does not exist.
    pub fn add_cohort(&mut self, cohort: Cohort) -> Result<(), SimError> {
        if cohort.district as usize >= self.districts.len() {
            return Err(SimError::UnknownDistrict(cohort.district));
        }
        self.cohorts.push(cohort);
        Ok(())
    }

    /// Total population across all cohorts.
    #[must_use]
    pub fn total_population(&self) -> u64 {
        self.cohorts
            .iter()
            .map(|cohort| u64::from(cohort.count))
            .sum()
    }

    /// Total employed citizens.
    #[must_use]
    pub fn employed_population(&self) -> u64 {
        self.cohorts
            .iter()
            .map(|cohort| u64::from(cohort.employed))
            .sum()
    }

    /// Population-weighted average happiness, 0..=1.
    #[must_use]
    pub fn average_happiness(&self) -> f32 {
        let total = self.total_population();
        if total == 0 {
            return 0.0;
        }
        let weighted: f32 = self
            .cohorts
            .iter()
            .map(|cohort| cohort.happiness * cohort.count as f32)
            .sum();
        weighted / total as f32
    }

    /// Population-weighted average need satisfaction, 0..=1.
    #[must_use]
    pub fn average_need(&self) -> f32 {
        let total = self.total_population();
        if total == 0 {
            return 0.0;
        }
        let weighted: f32 = self
            .cohorts
            .iter()
            .map(|cohort| cohort.average_need() * cohort.count as f32)
            .sum();
        weighted / total as f32
    }

    /// Advance every cohort by `dt_days`.
    pub fn tick(&mut self, dt_days: f32) {
        for cohort in &mut self.cohorts {
            let services = self.districts[cohort.district as usize];
            cohort.tick(&services, dt_days);
        }
    }

    /// Read-only cohorts.
    #[must_use]
    pub fn cohorts(&self) -> &[Cohort] {
        &self.cohorts
    }

    /// Read-only districts.
    #[must_use]
    pub fn districts(&self) -> &[DistrictServices] {
        &self.districts
    }

    /// Mutable districts, for changing services or policy.
    pub fn districts_mut(&mut self) -> &mut [DistrictServices] {
        &mut self.districts
    }

    /// Number of cohorts.
    #[must_use]
    pub fn cohort_count(&self) -> usize {
        self.cohorts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn city() -> CohortSimulation {
        let mut sim = CohortSimulation::new(4);
        for index in 0..100 {
            sim.add_cohort(Cohort::new((index % 4) as u32, 10_000, 0.8))
                .expect("district exists");
        }
        sim
    }

    #[test]
    fn builds_a_million_citizen_city() {
        let sim = city();
        assert_eq!(sim.total_population(), 1_000_000);
        assert_eq!(sim.cohort_count(), 100);
        assert_eq!(sim.employed_population(), 1_000_000);
    }

    #[test]
    fn ticking_preserves_population_and_updates_happiness() {
        let mut sim = city();
        sim.tick(1.0);
        assert_eq!(sim.total_population(), 1_000_000);
        assert!((0.0..=1.0).contains(&sim.average_happiness()));
        assert!((0.0..=1.0).contains(&sim.average_need()));
    }

    #[test]
    fn bad_services_lower_happiness() {
        let mut good = CohortSimulation::new(1);
        good.add_cohort(Cohort::new(0, 1_000, 0.9))
            .expect("district");
        let mut bad = CohortSimulation::new(1);
        bad.add_cohort(Cohort::new(0, 1_000, 0.9))
            .expect("district");
        bad.districts_mut()[0] = DistrictServices {
            food: 0.0,
            water: 0.0,
            jobs: 0.0,
            pollution: 1.0,
            ..DistrictServices::default()
        };

        for _ in 0..20 {
            good.tick(1.0);
            bad.tick(1.0);
        }
        assert!(good.average_happiness() > bad.average_happiness());
    }

    #[test]
    fn unknown_districts_are_rejected() {
        let mut sim = CohortSimulation::new(1);
        assert_eq!(
            sim.add_cohort(Cohort::new(4, 10, 0.5)),
            Err(SimError::UnknownDistrict(4))
        );
    }

    #[test]
    fn zero_dt_does_not_change_the_city() {
        let mut sim = city();
        let before = sim.clone();
        sim.tick(0.0);
        assert_eq!(sim, before);
    }
}
