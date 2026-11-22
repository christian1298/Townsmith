//! Derived per-district cohort summaries for the district dashboard.
//!
//! Summaries are computed on demand from the current cohorts. They store no
//! state, are not part of the save, and never mutate the simulation.

use crate::sim::CohortSimulation;

/// Average need at or above this value is [`NeedBands::content`].
pub const NEED_CONTENT_THRESHOLD: f32 = 0.75;

/// Average need below this value is [`NeedBands::critical`].
pub const NEED_CRITICAL_THRESHOLD: f32 = 0.4;

/// Citizen counts by average-need band.
///
/// Bands are assigned per cohort from [`Cohort::average_need`](crate::cohort::Cohort::average_need)
/// and counted by citizen population, so the three counts sum to the district
/// population.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NeedBands {
    /// Citizens in cohorts with average need at or above
    /// [`NEED_CONTENT_THRESHOLD`].
    pub content: u64,
    /// Citizens in cohorts with average need at or above
    /// [`NEED_CRITICAL_THRESHOLD`] and below [`NEED_CONTENT_THRESHOLD`].
    pub stressed: u64,
    /// Citizens in cohorts with average need below [`NEED_CRITICAL_THRESHOLD`].
    pub critical: u64,
}

impl NeedBands {
    /// Classify one cohort average and add its citizens to a band.
    fn record(&mut self, average_need: f32, count: u32) {
        let count = u64::from(count);
        if average_need >= NEED_CONTENT_THRESHOLD {
            self.content += count;
        } else if average_need < NEED_CRITICAL_THRESHOLD {
            self.critical += count;
        } else {
            self.stressed += count;
        }
    }

    /// Total citizens across all bands.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.content + self.stressed + self.critical
    }
}

/// A population-weighted snapshot of one district's cohorts.
///
/// Empty districts still produce a summary: every count is `0`, every average
/// is `0.0`, and the band counts are empty.
#[derive(Debug, Clone, PartialEq)]
pub struct DistrictCohortSummary {
    /// District index.
    pub district: u32,
    /// Citizens in the district.
    pub population: u64,
    /// Cohort records assigned to the district.
    pub cohort_count: u32,
    /// Employed citizens.
    pub employed: u64,
    /// Employed citizens divided by population, `0..=1`; `0.0` when empty.
    pub employed_fraction: f32,
    /// Population-weighted average happiness, `0..=1`.
    pub average_happiness: f32,
    /// Population-weighted average need satisfaction, `0..=1`.
    pub average_need: f32,
    /// Population-weighted average income in abstract units.
    pub average_income: f32,
    /// Population counts by average-need band.
    pub need_bands: NeedBands,
}

impl CohortSimulation {
    /// One summary per district, in district-index order.
    ///
    /// Empty districts are included with zeroed values. The result is derived
    /// deterministically from the current cohorts and does not mutate the
    /// simulation.
    #[must_use]
    pub fn district_summaries(&self) -> Vec<DistrictCohortSummary> {
        (0..self.districts().len())
            .map(|district| self.summarize_district(district as u32))
            .collect()
    }

    /// Summary for a single district, or `None` if the index does not exist.
    #[must_use]
    pub fn district_summary(&self, district: u32) -> Option<DistrictCohortSummary> {
        if district as usize >= self.districts().len() {
            return None;
        }
        Some(self.summarize_district(district))
    }

    /// Aggregate all cohorts assigned to `district`.
    fn summarize_district(&self, district: u32) -> DistrictCohortSummary {
        let mut population = 0u64;
        let mut cohort_count = 0u32;
        let mut employed = 0u64;
        let mut happiness = 0f64;
        let mut need = 0f64;
        let mut income = 0f64;
        let mut need_bands = NeedBands::default();

        for cohort in self
            .cohorts()
            .iter()
            .filter(|cohort| cohort.district == district)
        {
            cohort_count += 1;
            population += u64::from(cohort.count);
            employed += u64::from(cohort.employed);

            let weight = f64::from(cohort.count);
            let average_need = cohort.average_need();
            happiness += f64::from(cohort.happiness) * weight;
            need += f64::from(average_need) * weight;
            income += f64::from(cohort.income) * weight;
            need_bands.record(average_need, cohort.count);
        }

        let population_weight = population as f64;
        let average = |weighted: f64| {
            if population == 0 {
                0.0
            } else {
                (weighted / population_weight) as f32
            }
        };

        DistrictCohortSummary {
            district,
            population,
            cohort_count,
            employed,
            employed_fraction: if population == 0 {
                0.0
            } else {
                employed as f32 / population as f32
            },
            average_happiness: average(happiness),
            average_need: average(need),
            average_income: average(income),
            need_bands,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cohort::Cohort;

    fn cohort(
        district: u32,
        count: u32,
        need: f32,
        happiness: f32,
        income: f32,
        employed: u32,
    ) -> Cohort {
        let mut cohort = Cohort::new(district, count, need);
        cohort.employed = employed;
        cohort.happiness = happiness;
        cohort.income = income;
        cohort
    }

    #[test]
    fn empty_simulation_has_no_summaries() {
        let sim = CohortSimulation::new(0);
        assert!(sim.district_summaries().is_empty());
        assert_eq!(sim.district_summary(0), None);
    }

    #[test]
    fn one_district_weights_cohorts_by_population() {
        let mut sim = CohortSimulation::new(1);
        sim.add_cohort(cohort(0, 100, 0.75, 0.5, 1.0, 50))
            .expect("district exists");
        sim.add_cohort(cohort(0, 300, 0.5, 1.0, 2.0, 300))
            .expect("district exists");

        let summary = sim.district_summary(0).expect("district exists");
        assert_eq!(summary.district, 0);
        assert_eq!(summary.population, 400);
        assert_eq!(summary.cohort_count, 2);
        assert_eq!(summary.employed, 350);
        assert_eq!(summary.employed_fraction, 0.875);
        assert_eq!(summary.average_happiness, 0.875);
        assert_eq!(summary.average_need, 0.5625);
        assert_eq!(summary.average_income, 1.75);
        assert_eq!(
            summary.need_bands,
            NeedBands {
                content: 100,
                stressed: 300,
                critical: 0,
            }
        );
        assert_eq!(summary.need_bands.total(), summary.population);
    }

    #[test]
    fn need_bands_use_exact_thresholds() {
        let mut bands = NeedBands::default();
        bands.record(0.75, 10);
        bands.record(0.4, 20);
        bands.record(0.399_999_97, 30);

        assert_eq!(
            bands,
            NeedBands {
                content: 10,
                stressed: 20,
                critical: 30,
            }
        );
        assert_eq!(bands.total(), 60);
    }

    #[test]
    fn district_bands_place_threshold_cohorts_in_the_higher_band() {
        let mut sim = CohortSimulation::new(1);
        sim.add_cohort(Cohort::new(0, 10, 0.75))
            .expect("district exists");
        sim.add_cohort(Cohort::new(0, 20, 0.4))
            .expect("district exists");
        sim.add_cohort(Cohort::new(0, 30, 0.399_999_97))
            .expect("district exists");

        let summary = sim.district_summary(0).expect("district exists");
        assert_eq!(summary.population, 60);
        assert_eq!(
            summary.need_bands,
            NeedBands {
                content: 10,
                stressed: 20,
                critical: 30,
            }
        );
        assert!(summary.average_need > 0.4);
        assert!(summary.average_need < 0.5);
    }

    #[test]
    fn multiple_districts_are_summarized_independently() {
        let mut sim = CohortSimulation::new(3);
        sim.add_cohort(cohort(0, 100, 0.875, 0.75, 1.5, 90))
            .expect("district exists");
        sim.add_cohort(cohort(0, 100, 0.625, 0.5, 0.5, 70))
            .expect("district exists");
        sim.add_cohort(cohort(2, 50, 0.2, 0.25, 0.5, 10))
            .expect("district exists");

        let summaries = sim.district_summaries();
        assert_eq!(summaries.len(), 3);
        assert_eq!(
            summaries.iter().map(|s| s.district).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        assert_eq!(summaries[0].population, 200);
        assert_eq!(summaries[0].cohort_count, 2);
        assert_eq!(summaries[0].employed, 160);
        assert_eq!(summaries[0].employed_fraction, 0.8);
        assert_eq!(summaries[0].average_happiness, 0.625);
        assert_eq!(summaries[0].average_need, 0.75);
        assert_eq!(summaries[0].average_income, 1.0);
        assert_eq!(
            summaries[0].need_bands,
            NeedBands {
                content: 100,
                stressed: 100,
                critical: 0,
            }
        );

        assert_eq!(
            summaries[1],
            DistrictCohortSummary {
                district: 1,
                population: 0,
                cohort_count: 0,
                employed: 0,
                employed_fraction: 0.0,
                average_happiness: 0.0,
                average_need: 0.0,
                average_income: 0.0,
                need_bands: NeedBands::default(),
            }
        );

        assert_eq!(summaries[2].population, 50);
        assert_eq!(summaries[2].cohort_count, 1);
        assert_eq!(summaries[2].employed, 10);
        assert_eq!(summaries[2].employed_fraction, 0.2);
        assert_eq!(
            summaries[2].need_bands,
            NeedBands {
                content: 0,
                stressed: 0,
                critical: 50,
            }
        );
    }

    #[test]
    fn empty_districts_are_included_and_unknown_districts_are_none() {
        let mut sim = CohortSimulation::new(2);
        sim.add_cohort(cohort(1, 10, 0.5, 0.5, 1.0, 5))
            .expect("district exists");

        let summaries = sim.district_summaries();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].district, 0);
        assert_eq!(summaries[0].population, 0);
        assert_eq!(summaries[0].need_bands.total(), 0);
        assert_eq!(summaries[1].population, 10);

        let empty = sim.district_summary(0).expect("district exists");
        assert_eq!(empty.population, 0);
        assert_eq!(empty.cohort_count, 0);
        assert_eq!(empty.employed_fraction, 0.0);
        assert_eq!(sim.district_summary(2), None);
    }

    #[test]
    fn summaries_are_deterministic_and_read_only() {
        let mut sim = CohortSimulation::new(2);
        sim.add_cohort(cohort(0, 100, 0.9, 0.8, 1.5, 90))
            .expect("district exists");
        sim.add_cohort(cohort(1, 50, 0.3, 0.4, 0.75, 20))
            .expect("district exists");
        let before = sim.clone();

        let first = sim.district_summaries();
        let second = sim.district_summaries();
        assert_eq!(first, second);
        assert_eq!(sim, before);
    }
}
