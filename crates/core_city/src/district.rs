//! District-scale aggregation for Phase 5 dashboards.
//!
//! A district is an ad-hoc circular query over the world XZ plane, not a saved
//! region. The snapshot is derived on demand from placements, construction
//! sites, and the household runtime, exactly like [`crate::metrics::summarize`]
//! but restricted to one radius. It adds no save field and changes no existing
//! checksum.

use std::collections::BTreeSet;

use crate::construction::ConstructionStore;
use crate::placement::{PlacementStore, MAX_WORLD_COORD_M};
use crate::population::CityRuntime;
use crate::traffic::{fnv1a_feed, FNV_OFFSET_BASIS};

/// Metrics for one circular district query.
///
/// Completed-only metrics ignore placements whose construction site is missing
/// or incomplete. Occupied households are counted once per household record
/// with at least one resident.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DistrictSummary {
    /// Residents currently living in completed housing inside the radius.
    pub population: u64,
    /// Housing capacity in completed buildings inside the radius.
    pub housing_capacity: u64,
    /// Vacant housing slots: `housing_capacity - population`, clamped at zero.
    pub vacancies: u64,
    /// Occupied household records inside the radius.
    pub households: u64,
    /// Job slots in completed buildings inside the radius.
    pub jobs: u64,
    /// Employed residents: the sum of household workers in the district.
    pub employed: u64,
    /// Residents without a job: `population - employed`, clamped at zero.
    pub unemployed: u64,
    /// Resident-weighted happiness, 0..=1. Zero when there are no residents.
    pub happiness: f32,
    /// Resident-weighted food satisfaction.
    pub food: f32,
    /// Resident-weighted water satisfaction.
    pub water: f32,
    /// Resident-weighted rest satisfaction.
    pub rest: f32,
    /// Resident-weighted average commute in metres.
    pub average_commute_m: f32,
    /// Completed buildings inside the radius.
    pub completed_buildings: usize,
    /// Incomplete construction sites inside the radius.
    pub construction_sites: usize,
}

impl DistrictSummary {
    /// Stable FNV-1a checksum over every summary field.
    ///
    /// The checksum is intended for deterministic bridge payloads and
    /// regression comparisons within one build, not for cryptographic use.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        fnv1a_feed(&mut hash, self.population);
        fnv1a_feed(&mut hash, self.housing_capacity);
        fnv1a_feed(&mut hash, self.vacancies);
        fnv1a_feed(&mut hash, self.households);
        fnv1a_feed(&mut hash, self.jobs);
        fnv1a_feed(&mut hash, self.employed);
        fnv1a_feed(&mut hash, self.unemployed);
        fnv1a_feed(&mut hash, u64::from(self.happiness.to_bits()));
        fnv1a_feed(&mut hash, u64::from(self.food.to_bits()));
        fnv1a_feed(&mut hash, u64::from(self.water.to_bits()));
        fnv1a_feed(&mut hash, u64::from(self.rest.to_bits()));
        fnv1a_feed(&mut hash, u64::from(self.average_commute_m.to_bits()));
        fnv1a_feed(&mut hash, self.completed_buildings as u64);
        fnv1a_feed(&mut hash, self.construction_sites as u64);
        hash
    }
}

/// Summarize the completed buildings and households whose XZ centre lies within
/// `radius_m` of `center`.
///
/// The radius is a 2D Euclidean distance on the world XZ plane and is
/// inclusive. A placement without a construction site (for example a legacy
/// save that has not been backfilled) is ignored. Incomplete sites inside the
/// radius are counted in [`DistrictSummary::construction_sites`] but never
/// contribute housing, jobs, population, or needs.
///
/// Household needs, happiness, and commute are resident-weighted exactly like
/// [`crate::metrics::summarize`]. Employment is the sum of household workers
/// for homes in the district, so it is intentionally not clamped to local job
/// slots: residents may commute to jobs outside the radius.
///
/// A non-finite center, a non-finite or negative radius, or a radius above
/// [`MAX_WORLD_COORD_M`] returns [`DistrictSummary::default`]. The bridge
/// rejects those inputs before calling this function.
#[must_use]
pub fn summarize_district(
    placements: &PlacementStore,
    construction: &ConstructionStore,
    runtime: &CityRuntime,
    center: [f32; 2],
    radius_m: f32,
) -> DistrictSummary {
    if !center.iter().all(|value| value.is_finite())
        || !radius_m.is_finite()
        || !(0.0..=MAX_WORLD_COORD_M).contains(&radius_m)
    {
        return DistrictSummary::default();
    }

    let radius_sq = radius_m * radius_m;
    let in_radius = |placement_center: [f32; 3]| {
        let dx = placement_center[0] - center[0];
        let dz = placement_center[2] - center[1];
        dx * dx + dz * dz <= radius_sq
    };

    let mut complete = BTreeSet::new();
    let mut housing_capacity = 0_u64;
    let mut jobs = 0_u64;
    let mut completed_buildings = 0_usize;
    let mut construction_sites = 0_usize;
    for placement in &placements.placements {
        if !in_radius(placement.center) {
            continue;
        }
        match construction.sites.get(&placement.id) {
            Some(site) if site.is_complete() => {
                complete.insert(placement.id);
                housing_capacity += u64::from(placement.resident_capacity);
                jobs += u64::from(placement.job_slots);
                completed_buildings += 1;
            }
            Some(_) => construction_sites += 1,
            None => {}
        }
    }

    let mut population = 0_u64;
    let mut households = 0_u64;
    let mut workers = 0_u64;
    let mut food = 0.0;
    let mut water = 0.0;
    let mut rest = 0.0;
    let mut commute = 0.0;
    let mut happiness = 0.0;
    for (&id, household) in &runtime.households {
        if !complete.contains(&id) {
            continue;
        }
        let residents = u64::from(household.residents);
        if residents == 0 {
            continue;
        }
        population += residents;
        households += 1;
        workers += u64::from(household.workers);
        let weight = household.residents as f32;
        food += household.food * weight;
        water += household.water * weight;
        rest += household.rest * weight;
        commute += household.commute_m * weight;
        happiness += household.happiness() * weight;
    }
    if population > 0 {
        let scale = 1.0 / population as f32;
        food *= scale;
        water *= scale;
        rest *= scale;
        commute *= scale;
        happiness *= scale;
    } else {
        food = 0.0;
        water = 0.0;
        rest = 0.0;
        commute = 0.0;
        happiness = 0.0;
    }

    DistrictSummary {
        population,
        housing_capacity,
        vacancies: housing_capacity.saturating_sub(population),
        households,
        jobs,
        employed: workers,
        unemployed: population.saturating_sub(workers),
        happiness,
        food,
        water,
        rest,
        average_commute_m: commute,
        completed_buildings,
        construction_sites,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construction::ConstructionPhase;
    use crate::placement::Placement;
    use crate::population::Household;

    fn placement(id: u64, center: [f32; 3], capacity: u32, jobs: u32) -> Placement {
        Placement {
            id,
            blueprint_id: format!("local:test_{id}"),
            blueprint_hash: "blake3:test".to_string(),
            blueprint_path: format!("/tmp/test_{id}.tsbp"),
            name: format!("Test {id}"),
            center,
            rotation_deg: 0,
            footprint_m: [8.0, 8.0],
            bounds_m: [8.0, 6.0, 8.0],
            foundation_m: 0.0,
            material_cost: 1.0,
            resident_capacity: capacity,
            job_slots: jobs,
            solid_voxels: 100,
            structure: None,
        }
    }

    fn complete_site(construction: &mut ConstructionStore, id: u64) {
        construction.add_site(id, 100).expect("site");
        construction.sites.get_mut(&id).expect("site").phase = ConstructionPhase::Complete;
    }

    fn household(capacity: u32, residents: u32, workers: u32) -> Household {
        Household {
            capacity,
            residents,
            workers,
            food: 1.0,
            water: 1.0,
            rest: 1.0,
            commute_m: 0.0,
            commute_time_s: 0.0,
            income_per_day: 0.0,
            home: [0.0, 0.0],
        }
    }

    fn empty_runtime(seed: u64) -> CityRuntime {
        CityRuntime::new(seed)
    }

    #[test]
    fn empty_world_has_zero_metrics() {
        let summary = summarize_district(
            &PlacementStore::new(2026),
            &ConstructionStore::new(4, 1_000_000),
            &empty_runtime(2026),
            [0.0, 0.0],
            512.0,
        );
        assert_eq!(summary, DistrictSummary::default());
        assert_eq!(summary.checksum(), DistrictSummary::default().checksum());
    }

    #[test]
    fn radius_is_inclusive_and_two_dimensional() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [0.0, 0.0, 0.0], 1, 0));
        // Exactly 5 m away on the XZ plane: included at radius 5.
        placements
            .placements
            .push(placement(2, [3.0, 50.0, 4.0], 1, 0));
        // Height does not count; this one is 9 m away.
        placements
            .placements
            .push(placement(3, [0.0, 999.0, 9.0], 1, 0));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        for id in 1..=3 {
            complete_site(&mut construction, id);
        }

        let inside = summarize_district(
            &placements,
            &construction,
            &empty_runtime(2026),
            [0.0, 0.0],
            5.0,
        );
        assert_eq!(inside.completed_buildings, 2);
        assert_eq!(inside.housing_capacity, 2);

        let tighter = summarize_district(
            &placements,
            &construction,
            &empty_runtime(2026),
            [0.0, 0.0],
            4.999,
        );
        assert_eq!(tighter.completed_buildings, 1);
        assert_eq!(tighter.housing_capacity, 1);
    }

    #[test]
    fn complete_and_incomplete_sites_are_separated() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [10.0, 0.0, 0.0], 4, 2));
        placements
            .placements
            .push(placement(2, [20.0, 0.0, 0.0], 8, 5));
        // No site record: a legacy placement that is ignored entirely.
        placements
            .placements
            .push(placement(3, [30.0, 0.0, 0.0], 6, 3));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        construction.add_site(2, 100).expect("site");

        let summary = summarize_district(
            &placements,
            &construction,
            &empty_runtime(2026),
            [0.0, 0.0],
            100.0,
        );
        assert_eq!(summary.completed_buildings, 1);
        assert_eq!(summary.construction_sites, 1);
        assert_eq!(summary.housing_capacity, 4);
        assert_eq!(summary.jobs, 2);
    }

    #[test]
    fn needs_and_happiness_are_resident_weighted() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [0.0, 0.0, 0.0], 2, 0));
        placements
            .placements
            .push(placement(2, [20.0, 0.0, 0.0], 8, 0));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        complete_site(&mut construction, 2);

        let mut runtime = empty_runtime(2026);
        let mut first = household(2, 2, 1);
        first.food = 0.4;
        first.water = 0.6;
        first.rest = 0.8;
        first.commute_m = 10.0;
        let mut second = household(8, 8, 4);
        second.food = 0.9;
        second.water = 0.9;
        second.rest = 0.9;
        second.commute_m = 30.0;
        runtime.households.insert(1, first);
        runtime.households.insert(2, second);

        let summary = summarize_district(&placements, &construction, &runtime, [0.0, 0.0], 100.0);
        assert_eq!(summary.population, 10);
        assert_eq!(summary.households, 2);
        assert_eq!(summary.housing_capacity, 10);
        assert_eq!(summary.vacancies, 0);
        assert_eq!(summary.employed, 5);
        assert_eq!(summary.unemployed, 5);
        assert!((summary.food - 0.8).abs() < 1.0e-6);
        assert!((summary.water - 0.84).abs() < 1.0e-6);
        assert!((summary.rest - 0.88).abs() < 1.0e-6);
        assert!((summary.average_commute_m - 26.0).abs() < 1.0e-6);
        // 0.2 * happiness(1) + 0.8 * happiness(2).
        let first = &runtime.households[&1];
        let second = &runtime.households[&2];
        let first_happiness = 0.5 * first.needs()
            + 0.3 * (first.workers as f32 / first.residents as f32)
            + 0.2 * (1.0 - first.commute_m / crate::population::MAX_COMMUTE_M);
        let second_happiness = 0.5 * second.needs()
            + 0.3 * (second.workers as f32 / second.residents as f32)
            + 0.2 * (1.0 - second.commute_m / crate::population::MAX_COMMUTE_M);
        assert!(
            (summary.happiness - (0.2 * first_happiness + 0.8 * second_happiness)).abs() < 1.0e-6
        );
    }

    #[test]
    fn jobs_and_employment_are_local_home_workers() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [0.0, 0.0, 0.0], 6, 1));
        // Jobs outside the district must not change local employment.
        placements
            .placements
            .push(placement(2, [900.0, 0.0, 0.0], 0, 40));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        complete_site(&mut construction, 2);

        let mut runtime = empty_runtime(2026);
        runtime.households.insert(1, household(6, 6, 3));

        let summary = summarize_district(&placements, &construction, &runtime, [0.0, 0.0], 100.0);
        assert_eq!(summary.jobs, 1);
        assert_eq!(summary.employed, 3);
        assert_eq!(summary.unemployed, 3);
    }

    #[test]
    fn vacancies_are_clamped_capacity_minus_residents() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [0.0, 0.0, 0.0], 6, 0));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        let summary = summarize_district(
            &placements,
            &construction,
            &empty_runtime(2026),
            [0.0, 0.0],
            10.0,
        );
        assert_eq!(summary.housing_capacity, 6);
        assert_eq!(summary.population, 0);
        assert_eq!(summary.vacancies, 6);
        assert_eq!(summary.households, 0);
        assert_eq!(summary.happiness, 0.0);
    }

    #[test]
    fn invalid_inputs_return_an_empty_summary() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [0.0, 0.0, 0.0], 6, 0));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        let runtime = empty_runtime(2026);

        for (center, radius) in [
            ([f32::NAN, 0.0], 10.0),
            ([0.0, f32::INFINITY], 10.0),
            ([0.0, 0.0], f32::NAN),
            ([0.0, 0.0], f32::INFINITY),
            ([0.0, 0.0], -1.0),
            ([0.0, 0.0], MAX_WORLD_COORD_M + 1.0),
        ] {
            assert_eq!(
                summarize_district(&placements, &construction, &runtime, center, radius),
                DistrictSummary::default(),
                "center {center:?}, radius {radius}"
            );
        }
    }

    #[test]
    fn summary_and_checksum_are_deterministic() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [10.0, 0.0, 10.0], 4, 2));
        placements
            .placements
            .push(placement(2, [20.0, 0.0, 20.0], 4, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        complete_site(&mut construction, 2);
        let mut runtime = empty_runtime(2026);
        runtime.households.insert(1, household(4, 3, 2));
        runtime.households.insert(2, household(4, 1, 1));

        let first = summarize_district(&placements, &construction, &runtime, [10.0, 10.0], 10.0);
        let second = summarize_district(&placements, &construction, &runtime, [10.0, 10.0], 10.0);
        assert_eq!(first, second);
        assert_eq!(first.checksum(), second.checksum());

        let wider = summarize_district(&placements, &construction, &runtime, [10.0, 10.0], 15.0);
        assert_ne!(first, wider);
        assert_ne!(first.checksum(), wider.checksum());
    }
}
