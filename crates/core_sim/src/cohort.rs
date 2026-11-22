//! A statistical cohort of citizens.
//!
//! Cohorts are the unit of 1M-scale population simulation. A cohort stores a
//! count and averages for needs, happiness, employment, and income. Individual
//! citizens are materialized near the camera; they are not stored here.

use crate::district::{finite_or_zero, DistrictServices};

/// Number of tracked needs.
pub const NEED_COUNT: usize = 6;

/// Longest `dt` one [`Cohort::tick`] call advances, in days (ten years).
///
/// [`Cohort::tick`] splits a multi-day `dt` into whole 1-day steps plus a
/// bounded remainder, so this cap is only a safety valve against a corrupt
/// caller (`dt = 1e30` would otherwise loop for a very long time). Normal
/// callers pass at most one day.
pub const MAX_TICK_DAYS: f32 = 3_650.0;

/// Per-day rate at which employment moves toward the desired count.
const EMPLOYMENT_RESPONSIVENESS: f32 = 0.10;

/// Upper bound on [`Cohort::income`].
const MAX_INCOME: f32 = 10.0;

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
    /// Sub-citizen employment remainder, in `-0.5..=0.5`.
    ///
    /// The smoothed employment position is `employed + employment_carry`; the
    /// carry lets a fractional daily employment delta accumulate instead of
    /// rounding back to the current integer every tick (which froze small
    /// cohorts). It is transient: `core_sim` has no serializer, and a reload
    /// that loses it costs at most one tick of smoothing, never correctness.
    /// It is deliberately outside the pinned state checksums, which cover the
    /// integer employment mirror and the reported aggregates.
    employment_carry: f32,
}

impl Cohort {
    /// Create a cohort with a uniform needs state.
    ///
    /// A non-finite `needs` becomes `0.0` and is then clamped to `0..=1`, so a
    /// `NaN` from a bad load cannot enter the simulation through this
    /// constructor.
    #[must_use]
    pub fn new(district: u32, count: u32, needs: f32) -> Self {
        let needs = finite_or_zero(needs).clamp(0.0, 1.0);
        Self {
            district,
            count,
            employed: count,
            needs: [needs; NEED_COUNT],
            happiness: needs,
            income: 1.0,
            employment_carry: 0.0,
        }
    }

    /// Return a repaired copy of this cohort.
    ///
    /// This is the validation boundary for cohort state that arrives outside a
    /// constructor (a save, a bridge, or a mod): non-finite needs, happiness,
    /// and income become `0.0`, every bounded field is clamped, and `employed`
    /// cannot exceed `count`. [`Self::tick`] applies the same repair before it
    /// integrates, so a poisoned cohort recovers on the next step.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let mut cohort = self.clone();
        cohort.repair();
        cohort
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
    ///
    /// A `dt_days` above one day is split into whole 1-day steps plus a bounded
    /// fractional remainder, so `tick(7.0)` advances seven days instead of
    /// silently truncating to one. Whole steps use `dt = 1.0` exactly, which
    /// makes `tick(7.0)` bit-identical to seven `tick(1.0)` calls. A
    /// non-finite or non-positive `dt_days` is a no-op.
    pub fn tick(&mut self, services: &DistrictServices, dt_days: f32) {
        self.repair();
        if self.count == 0 || !dt_days.is_finite() || dt_days <= 0.0 {
            return;
        }
        debug_assert!(
            dt_days <= MAX_TICK_DAYS,
            "cohort tick dt_days {dt_days} exceeds the {MAX_TICK_DAYS}-day cap"
        );
        let days = dt_days.min(MAX_TICK_DAYS);
        let services = services.sanitized();

        // Whole days are stepped with an exact integer count and the remainder
        // is derived once, so there is no iterative float subtraction.
        let whole_days = days.floor() as u32;
        for _ in 0..whole_days {
            self.step(&services, 1.0);
        }
        let remainder = days - whole_days as f32;
        if remainder > 0.0 {
            self.step(&services, remainder);
        }
    }

    /// Repair state that may have arrived from outside a constructor.
    fn repair(&mut self) {
        for need in &mut self.needs {
            *need = finite_or_zero(*need).clamp(0.0, 1.0);
        }
        self.happiness = finite_or_zero(self.happiness).clamp(0.0, 1.0);
        self.income = finite_or_zero(self.income).clamp(0.0, MAX_INCOME);
        self.employment_carry = finite_or_zero(self.employment_carry).clamp(-0.5, 0.5);
        self.employed = self.employed.min(self.count);
    }

    /// One integration step with `0 < dt <= 1` days.
    fn step(&mut self, services: &DistrictServices, dt: f32) {
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

        self.update_employment(services.jobs, dt);

        self.income = (services.wages * self.employed_fraction()).clamp(0.0, MAX_INCOME);
    }

    /// Move the integer employment mirror toward `jobs` coverage.
    ///
    /// The smoothed position is kept as `employed + carry`, so a fractional
    /// step (`|desired - employed| * 0.10 * dt < 0.5`) accumulates in the carry
    /// instead of rounding back every tick. The integer mirror is the rounded
    /// position and is always clamped to `0..=count`.
    fn update_employment(&mut self, jobs: f32, dt: f32) {
        let count = self.count as f32;
        let desired = (count * jobs).round();
        let mut position = self.employed as f32 + self.employment_carry;
        position += (desired - position) * EMPLOYMENT_RESPONSIVENESS * dt;
        position = position.clamp(0.0, count);
        let employed = position.round().clamp(0.0, count);
        self.employed = employed as u32;
        self.employment_carry = (position - employed).clamp(-0.5, 0.5);
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

    #[test]
    fn non_finite_dt_is_a_no_op() {
        for dt in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut cohort = Cohort::new(0, 100, 0.7);
            let before = cohort.clone();
            cohort.tick(&DistrictServices::default(), dt);
            assert_eq!(cohort, before, "dt {dt}");
        }
    }

    #[test]
    fn non_finite_services_keep_state_finite() {
        let matrix = [
            (f32::NAN, f32::NAN, f32::NAN),
            (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY),
            (f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY),
        ];
        for (food, jobs, wages) in matrix {
            let services = DistrictServices {
                food,
                jobs,
                wages,
                water: jobs,
                rest: wages,
                pollution: food,
                ..DistrictServices::default()
            };
            let mut cohort = Cohort::new(0, 100, 0.8);
            cohort.tick(&services, 1.0);
            assert!(
                cohort.needs.iter().all(|need| need.is_finite()),
                "needs {:?}",
                cohort.needs
            );
            assert!(
                cohort.needs.iter().all(|need| (0.0..=1.0).contains(need)),
                "needs {:?}",
                cohort.needs
            );
            assert!(cohort.happiness.is_finite());
            assert!((0.0..=1.0).contains(&cohort.happiness));
            assert!(cohort.income.is_finite());
            assert!((0.0..=MAX_INCOME).contains(&cohort.income));
            assert!(cohort.employed <= cohort.count);
        }
    }

    #[test]
    fn non_finite_cohort_state_is_repaired() {
        let mut cohort = Cohort::new(0, 100, 0.7);
        cohort.needs = [f32::NAN; NEED_COUNT];
        cohort.needs[0] = f32::INFINITY;
        cohort.needs[1] = f32::NEG_INFINITY;
        cohort.happiness = f32::NAN;
        cohort.income = f32::INFINITY;
        cohort.employed = 500;

        let repaired = cohort.sanitized();
        assert!(repaired.needs.iter().all(|need| need.is_finite()));
        assert!(repaired.needs.iter().all(|need| (0.0..=1.0).contains(need)));
        assert_eq!(repaired.happiness, 0.0);
        assert_eq!(repaired.income, 0.0);
        assert_eq!(repaired.employed, 100);

        // The same repair runs before a tick, so the cohort recovers in place.
        cohort.tick(&DistrictServices::default(), 1.0);
        assert!(cohort.needs.iter().all(|need| need.is_finite()));
        assert!(cohort.happiness.is_finite());
        assert!(cohort.income.is_finite());
        assert!(cohort.employed <= cohort.count);
    }

    #[test]
    fn new_repairs_non_finite_needs() {
        for poison in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let cohort = Cohort::new(0, 10, poison);
            assert_eq!(cohort.average_need(), 0.0);
            assert_eq!(cohort.happiness, 0.0);
        }
    }

    #[test]
    fn multi_day_tick_equals_repeated_single_day_ticks() {
        let services = DistrictServices {
            jobs: 0.9,
            ..DistrictServices::default()
        };
        let mut split = Cohort::new(0, 100, 0.4);
        let mut repeated = split.clone();

        split.tick(&services, 7.0);
        for _ in 0..7 {
            repeated.tick(&services, 1.0);
        }

        assert_eq!(split, repeated);
    }

    #[test]
    fn fractional_multi_day_tick_equals_split_ticks() {
        let services = DistrictServices::default();
        let mut split = Cohort::new(0, 100, 0.4);
        let mut piecewise = split.clone();

        split.tick(&services, 2.5);
        piecewise.tick(&services, 1.0);
        piecewise.tick(&services, 1.0);
        piecewise.tick(&services, 0.5);

        assert_eq!(split, piecewise);
    }

    #[test]
    fn multi_day_tick_advances_every_day() {
        let services = DistrictServices::default();
        let mut one_day = Cohort::new(0, 100, 0.2);
        let mut seven_days = one_day.clone();

        one_day.tick(&services, 1.0);
        seven_days.tick(&services, 7.0);

        assert!(
            seven_days.average_need() > one_day.average_need(),
            "multi-day tick must not truncate to one day"
        );
        assert!(seven_days.happiness > one_day.happiness);
    }

    #[test]
    fn small_cohort_employment_converges_instead_of_freezing() {
        let services = DistrictServices {
            jobs: 0.9,
            ..DistrictServices::default()
        };
        let mut cohort = Cohort::new(0, 10, 0.8);
        assert_eq!(cohort.employed, 10);

        for _ in 0..30 {
            cohort.tick(&services, 1.0);
        }

        assert_eq!(cohort.employed, 9, "employment must not freeze at 10");
        assert_eq!(cohort.employed_fraction(), 0.9);
        assert!(cohort.employment_carry.abs() <= 0.5);
    }

    #[test]
    fn employment_carry_stays_in_bounds_and_never_exceeds_count() {
        for count in [1u32, 2, 3, 5, 10, 25, 100] {
            for jobs in [0.0, 0.25, 0.5, 0.75, 0.9, 1.0] {
                for dt in [0.25f32, 1.0] {
                    let services = DistrictServices {
                        jobs,
                        ..DistrictServices::default()
                    };
                    let mut cohort = Cohort::new(0, count, 0.5);
                    for tick in 0..2_000 {
                        cohort.tick(&services, dt);
                        assert!(cohort.employed <= cohort.count);
                        assert!(cohort.employment_carry.is_finite());
                        assert!(
                            cohort.employment_carry.abs() <= 0.5,
                            "count {count} jobs {jobs} dt {dt} tick {tick}: carry {}",
                            cohort.employment_carry
                        );
                    }
                    assert_eq!(
                        cohort.employed,
                        (count as f32 * jobs).round() as u32,
                        "count {count} jobs {jobs} dt {dt} must converge"
                    );
                }
            }
        }
    }

    #[test]
    fn employment_carry_does_not_overshoot_toward_full_employment() {
        let services = DistrictServices {
            jobs: 1.0,
            ..DistrictServices::default()
        };
        let mut cohort = Cohort::new(0, 3, 0.5);
        cohort.employed = 0;
        for _ in 0..100 {
            cohort.tick(&services, 1.0);
            assert!(cohort.employed <= cohort.count);
        }
        assert_eq!(cohort.employed, 3);
        assert_eq!(cohort.employed_fraction(), 1.0);
    }
}
