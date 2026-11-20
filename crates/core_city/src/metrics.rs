//! Derived city-wide metrics from completed buildings.
//!
//! Phase 3 starts with a ledger, not a full household simulation: completed
//! construction sites contribute their blueprint capacity and job slots, and
//! the ledger exposes occupancy, employment, and a first happiness signal.
//! This is deliberately derived state; it is not persisted separately.

use std::collections::BTreeSet;

use crate::construction::ConstructionStore;
use crate::placement::PlacementStore;

/// City-wide metrics derived from placements and construction.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CitySummary {
    /// Residents in completed housing.
    pub population: u64,
    /// Housing capacity in completed buildings.
    pub housing_capacity: u64,
    /// Job slots in completed buildings.
    pub jobs: u64,
    /// Employed residents, `min(population, jobs)`.
    pub employed: u64,
    /// Residents without a job.
    pub unemployed: u64,
    /// Population-weighted happiness signal, 0..=1.
    pub happiness: f32,
    /// Completed buildings represented in the ledger.
    pub completed_buildings: usize,
    /// Construction sites that are not complete.
    pub construction_sites: usize,
    /// Settlement records in the world.
    pub settlements: usize,
}

/// Summarize a world from completed construction sites.
///
/// Only placements with a `Complete` construction site count toward
/// population, housing, or jobs. A placement without a site (for example a
/// legacy save that has not been backfilled) is ignored.
#[must_use]
pub fn summarize(
    placements: &PlacementStore,
    construction: &ConstructionStore,
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

    let population = housing_capacity;
    let employed = population.min(jobs);
    let unemployed = population.saturating_sub(employed);
    let happiness = if population == 0 {
        0.0
    } else {
        let employment = employed as f32 / population as f32;
        let occupancy = if housing_capacity == 0 {
            0.0
        } else {
            population as f32 / housing_capacity as f32
        };
        (0.45 + 0.35 * employment + 0.20 * occupancy).clamp(0.0, 1.0)
    };
    let construction_sites = construction
        .sites
        .values()
        .filter(|site| !site.is_complete())
        .count();

    CitySummary {
        population,
        housing_capacity,
        jobs,
        employed,
        unemployed,
        happiness,
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

    #[test]
    fn empty_world_has_zero_metrics() {
        let summary = summarize(
            &PlacementStore::new(2026),
            &ConstructionStore::new(4, 1_000_000),
            0,
        );
        assert_eq!(summary, CitySummary::default());
    }

    #[test]
    fn completed_housing_and_jobs_are_counted() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 2, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        construction.add_site(1, 100).expect("site");
        construction.sites.get_mut(&1).expect("site").phase = ConstructionPhase::Complete;

        let summary = summarize(&placements, &construction, 1);
        assert_eq!(summary.population, 2);
        assert_eq!(summary.housing_capacity, 2);
        assert_eq!(summary.jobs, 2);
        assert_eq!(summary.employed, 2);
        assert_eq!(summary.unemployed, 0);
        assert_eq!(summary.completed_buildings, 1);
        assert_eq!(summary.construction_sites, 0);
        assert_eq!(summary.settlements, 1);
        assert!(summary.happiness > 0.99);
    }

    #[test]
    fn unfinished_sites_are_not_occupied() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 6, 3));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        construction.add_site(1, 100).expect("site");

        let summary = summarize(&placements, &construction, 0);
        assert_eq!(summary.population, 0);
        assert_eq!(summary.construction_sites, 1);
        assert_eq!(summary.completed_buildings, 0);
        assert_eq!(summary.happiness, 0.0);
    }

    #[test]
    fn a_job_shortage_reduces_employment_and_happiness() {
        let mut placements = PlacementStore::new(2026);
        placements.placements.push(placement(1, 10, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        construction.add_site(1, 100).expect("site");
        construction.sites.get_mut(&1).expect("site").phase = ConstructionPhase::Complete;

        let summary = summarize(&placements, &construction, 0);
        assert_eq!(summary.population, 10);
        assert_eq!(summary.employed, 2);
        assert_eq!(summary.unemployed, 8);
        assert!(summary.happiness < 0.75);
    }
}
