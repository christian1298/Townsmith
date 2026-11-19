//! A statistical cohort of citizens.
//!
//! Cohorts are the unit of 1M-scale population simulation. A cohort stores a
//! count and averages for needs, happiness, employment, and income. Individual
//! citizens are materialized near the camera; they are not stored here.

use crate::district::DistrictServices;

/// Number of tracked needs.
pub const NEED_COUNT: usize = 6;

/// Need indices in [`Cohort::needs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Food.
    Food = 0,
    /// Water.
    Water = 1,
    /// Rest.
    Rest = 2,
    /// Hygiene.
    Hygiene = 3,
    /// Social contact.
    Social = 4,
    /// Safety.
    Safety = 5,
}

impl Need {
    /// All needs in index order.
    pub const ALL: [Self; NEED_COUNT] = [
        Self::Food,
        Self::Water,
        Self::Rest,
        Self::Hygiene,
        Self::Social,
        Self::Safety,
    ];

    /// Stable index into [`Cohort::needs`].
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// A group of citizens with the same district and needs state.
#[derive(Debug, Clone, PartialEq)]
pub struct Cohort {
    /// District index.
    pub district: u32,
    /// Number of citizens in the cohort.
    pub count: u32,
    /// Number of employed citizens.
    pub employed: u32,
    /// Need satisfaction, 0..=1, indexed by [`Need`].
    pub needs: [f32; NEED_COUNT],
    /// Aggregate happiness, 0..=1.
    pub happiness: f32,
    /// Average income in abstract units.
    pub income: f32,
}

impl Cohort {
    /// Create a cohort with a uniform needs state.
    #[must_use]
    pub fn new(district: u32, count: u32, needs: f32) -> Self {
        let needs = needs.clamp(0.0, 1.0);
        Self {
            district,
            count,
            employed: count,
            needs: [needs; NEED_COUNT],
            happiness: needs,
            income: 1.0,
        }
    }

    /// Employed fraction, 0..=1.
    #[must_use]
    pub fn employed_fraction(&self) -> f32 {
        if self.count == 0 {
            0.0
        } else {
            self.employed as f32 / self.count as f32
        }
    }

    /// Average need satisfaction.
    #[must_use]
    pub fn average_need(&self) -> f32 {
        self.needs.iter().sum::<f32>() / NEED_COUNT as f32
    }

    /// Advance the cohort by `dt_days`.
    pub fn tick(&mut self, services: &DistrictServices, dt_days: f32) {
        if self.count == 0 || dt_days <= 0.0 {
            return;
        }
        let services = services.sanitized();
        let dt = dt_days.clamp(0.0, 1.0);

        let targets = [
            services.food,
            services.water,
            services.rest * services.housing,
            services.hygiene,
            services.social,
            services.safety,
        ];
        let responsiveness = 0.25 * dt;
        for (need, target) in self.needs.iter_mut().zip(targets) {
            *need += (target - *need) * responsiveness;
            *need = need.clamp(0.0, 1.0);
        }

        let need_score = self.needs[Need::Food.index()] * 0.25
            + self.needs[Need::Water.index()] * 0.20
            + self.needs[Need::Rest.index()] * 0.15
            + self.needs[Need::Hygiene.index()] * 0.10
            + self.needs[Need::Social.index()] * 0.10
            + self.needs[Need::Safety.index()] * 0.20;

        let liveability = (need_score
            - services.pollution * 0.15
            - services.noise * 0.10
            - services.commute * 0.10
            + self.income.clamp(0.0, 2.0) * 0.05)
            .clamp(0.0, 1.0);
        self.happiness += (liveability - self.happiness) * 0.20 * dt;
        self.happiness = self.happiness.clamp(0.0, 1.0);

        let desired_employed = (self.count as f32 * services.jobs).round();
        let employment_step = (desired_employed - self.employed as f32) * 0.10 * dt;
        self.employed = (self.employed as f32 + employment_step)
            .round()
            .clamp(0.0, self.count as f32) as u32;

        self.income = (services.wages * self.employed_fraction()).clamp(0.0, 10.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_cohort_starts_uniform() {
        let cohort = Cohort::new(3, 100, 0.8);
        assert_eq!(cohort.count, 100);
        assert_eq!(cohort.employed, 100);
        assert_eq!(cohort.average_need(), 0.8);
        assert_eq!(cohort.employed_fraction(), 1.0);
    }

    #[test]
    fn ticking_moves_needs_toward_services() {
        let mut cohort = Cohort::new(0, 100, 0.2);
        cohort.tick(&DistrictServices::default(), 1.0);
        assert!(cohort.average_need() > 0.2);
        assert!(cohort.happiness > 0.2);
        assert!(cohort.happiness <= 1.0);
    }

    #[test]
    fn poor_services_reduce_happiness() {
        let mut good = Cohort::new(0, 100, 0.9);
        let mut bad = Cohort::new(0, 100, 0.9);
        good.tick(&DistrictServices::default(), 1.0);
        bad.tick(
            &DistrictServices {
                food: 0.1,
                water: 0.1,
                jobs: 0.2,
                pollution: 0.9,
                ..DistrictServices::default()
            },
            1.0,
        );
        assert!(good.happiness > bad.happiness);
    }

    #[test]
    fn employment_follows_jobs_and_stays_in_bounds() {
        let mut cohort = Cohort::new(0, 100, 0.8);
        cohort.tick(
            &DistrictServices {
                jobs: 0.0,
                ..DistrictServices::default()
            },
            1.0,
        );
        assert!(cohort.employed < 100);
        assert!(cohort.employed <= cohort.count);
        assert!(cohort.employed_fraction() < 1.0);
    }

    #[test]
    fn zero_dt_is_a_no_op() {
        let mut cohort = Cohort::new(0, 100, 0.7);
        let before = cohort.clone();
        cohort.tick(&DistrictServices::default(), 0.0);
        assert_eq!(cohort, before);
    }
}
