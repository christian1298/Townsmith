//! Derived city-wide metrics from completed buildings and household runtime.
//!
//! The ledger only counts placements whose construction site is complete.
//! Households provide the independent population, vacancy, need, and happiness
//! values; job slots still come from the completed blueprint metadata.

use std::collections::BTreeSet;

use crate::construction::ConstructionStore;
use crate::placement::PlacementStore;
use crate::population::CityRuntime;

/// City-wide metrics derived from placements, construction, and households.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CitySummary {
    /// Residents currently living in completed housing.
    pub population: u64,
    /// Housing capacity in completed buildings.
    pub housing_capacity: u64,
    /// Vacant housing slots.
    pub vacancies: u64,
    /// Job slots in completed buildings.
    pub jobs: u64,
    /// Employed residents.
    pub employed: u64,
    /// Residents without a job.
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
    /// Gross wages paid per game day.
    pub daily_wages: f64,
    /// Completed buildings represented in the ledger.
    pub completed_buildings: usize,
    /// Construction sites that are not complete.
    pub construction_sites: usize,
    /// Settlement records in the world.
    pub settlements: usize,
}

/// Summarize a world from completed construction sites and household runtime.
///
/// A placement without a site (for example a legacy save that has not been
/// backfilled) is ignored. Households for incomplete or removed placements are
/// filtered out even if the runtime is stale.
#[must_use]
pub fn summarize(
    placements: &PlacementStore,
    construction: &ConstructionStore,
    runtime: &CityRuntime,
    settlements: usize,
) -> CitySummary {
    let complete: BTreeSet<u64> = construction
        .sites
        .iter()
        .filter(|(_, site)| site.is_complete())
        .map(|(id, _)| *id)
        .collect();

    let mut housing_capacity = 0_u64;
    let mut jobs = 0_u64;
    let mut completed_buildings = 0_usize;
    for placement in &placements.placements {
        if !complete.contains(&placement.id) {
            continue;
        }
        housing_capacity += u64::from(placement.resident_capacity);
        jobs += u64::from(placement.job_slots);
        completed_buildings += 1;
    }

    let mut population = 0_u64;
    let mut workers = 0_u64;
    let mut food = 0.0;
    let mut water = 0.0;
    let mut rest = 0.0;
    let mut commune = 0.0;
    let mut daily_wages = 0.0;
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
        workers += u64::from(household.workers);
        let weight = household.residents as f32;
        food += household.food * weight;
        water += household.water * weight;
        rest += household.rest * weight;
        commune += household.commute_m * weight;
        daily_wages += household.income_per_day;
        happiness += household.happiness() * weight;
    }
    if population > 0 {
        let scale = 1.0 / population as f32;
        food *= scale;
        water *= scale;
        rest *= scale;
        commune *= scale;
        happiness *= scale;
    } else {
        food = 0.0;
        water = 0.0;
        rest = 0.0;
        commune = 0.0;
        daily_wages = 0.0;
        happiness = 0.0;
    }

    let employed = workers.min(jobs);
    let unemployed = population.saturating_sub(employed);
    let vacancies = housing_capacity.saturating_sub(population);
    let construction_sites = construction
        .sites
        .values()
        .filter(|site| !site.is_complete())
        .count();

    CitySummary {
        population,
        housing_capacity,
        vacancies,
        jobs,
        employed,
        unemployed,
        happiness,
        food,
        water,
        rest,
        average_commute_m: commune,
        daily_wages,
        completed_buildings,
        construction_sites,
        settlements,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construction::ConstructionPhase;
    use crate::placement::Placement;

    fn placement(id: u64, capacity: u32, jobs: u32) -> Placement {
        Placement {
            id,
            blueprint_id: format!("local:test_{id}"),
            blueprint_hash: "blake3:test".to_string(),
            blueprint_path: format!("/tmp/test_{id}.tsbp"),
            name: format!("Test {id}"),
            center: [0.0, 0.0, 0.0],
            rotation_deg: 0,
            footprint_m: [8.0, 8.0],
            bounds_m: [8.0, 6.0, 8.0],
            foundation_m: 0.0,
            material_cost: 1.0,
            resident_capacity: capacity,
            job_slots: jobs,
            solid_voxels: 100,
        }
    }

    fn complete_site(construction: &mut ConstructionStore, id: u64) {
        construction.add_site(id, 100).expect("site");
        construction.sites.get_mut(&id).expect("site").phase = ConstructionPhase::Complete;
    }

    fn slots(placements: &PlacementStore, construction: &ConstructionStore) -> Vec<(u64, u32)> {
        placements
            .placements
            .iter()
            .filter(|placement| {
                construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
                    && placement.resident_capacity > 0
            })
            .map(|placement| (placement.id, placement.resident_capacity))
            .collect()
    }

    #[test]
    fn empty_world_has_zero_metrics() {
        let summary = summarize(
            &PlacementStore::new(2026),
            &ConstructionStore::new(4, 1_000_000),
            &CityRuntime::new(2026),
            0,
        );
        assert_eq!(summary, CitySummary::default());
    }

    #[test]
    fn completed_housing_and_jobs_are_counted_after_move_in() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 2, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        let mut runtime = CityRuntime::new(2026);
        let slots = slots(&placements, &construction);
        runtime.tick(10.0, 2, &slots);

        let summary = summarize(&placements, &construction, &runtime, 1);
        assert_eq!(summary.population, 2);
        assert_eq!(summary.housing_capacity, 2);
        assert_eq!(summary.vacancies, 0);
        assert_eq!(summary.jobs, 2);
        assert_eq!(summary.employed, 2);
        assert_eq!(summary.unemployed, 0);
        assert_eq!(summary.average_commute_m, 0.0);
        assert!((summary.daily_wages - 40.0).abs() < 0.001);
        assert_eq!(summary.completed_buildings, 1);
        assert_eq!(summary.construction_sites, 0);
        assert_eq!(summary.settlements, 1);
        assert!(summary.happiness > 0.9);
        assert!(summary.food > 0.9);
    }

    #[test]
    fn finished_housing_starts_vacant() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 2, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        let summary = summarize(&placements, &construction, &CityRuntime::new(2026), 0);
        assert_eq!(summary.population, 0);
        assert_eq!(summary.vacancies, 2);
        assert_eq!(summary.happiness, 0.0);
    }

    #[test]
    fn unfinished_sites_are_not_occupied() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 6, 3));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        construction.add_site(1, 100).expect("site");

        let summary = summarize(&placements, &construction, &CityRuntime::new(2026), 0);
        assert_eq!(summary.population, 0);
        assert_eq!(summary.construction_sites, 1);
        assert_eq!(summary.completed_buildings, 0);
        assert_eq!(summary.happiness, 0.0);
    }

    #[test]
    fn a_placement_without_a_site_is_ignored() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 4, 4));
        let summary = summarize(
            &placements,
            &ConstructionStore::new(4, 1_000_000),
            &CityRuntime::new(2026),
            0,
        );
        assert_eq!(summary, CitySummary::default());
    }

    #[test]
    fn a_jobs_only_city_has_no_population_or_happiness() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 0, 5));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);

        let summary = summarize(&placements, &construction, &CityRuntime::new(2026), 0);
        assert_eq!(summary.population, 0);
        assert_eq!(summary.jobs, 5);
        assert_eq!(summary.employed, 0);
        assert_eq!(summary.happiness, 0.0);
        assert_eq!(summary.completed_buildings, 1);
    }

    #[test]
    fn a_job_shortage_reduces_employment_and_happiness() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 10, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        let mut runtime = CityRuntime::new(2026);
        let slots = slots(&placements, &construction);
        runtime.tick(10.0, 2, &slots);

        let summary = summarize(&placements, &construction, &runtime, 0);
        assert_eq!(summary.population, 10);
        assert_eq!(summary.employed, 2);
        assert_eq!(summary.unemployed, 8);
        assert!(summary.happiness < 0.8);
    }
}
