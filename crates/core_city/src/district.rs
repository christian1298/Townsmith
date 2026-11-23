//! District-scale aggregation for Phase 5 dashboards.
//!
//! A district is an ad-hoc circular query over the world XZ plane, not a saved
//! region. The snapshot is derived on demand from placements, construction
//! sites, and the household runtime, exactly like [`crate::metrics::summarize`]
//! but restricted to one radius. It adds no save field and changes no existing
//! checksum.

use std::collections::{BTreeMap, BTreeSet};

use crate::construction::ConstructionStore;
use crate::placement::{PlacementStore, MAX_WORLD_COORD_M};
use crate::population::CityRuntime;
use crate::traffic::{fnv1a_feed, FNV_OFFSET_BASIS};

/// Smallest accepted heatmap cell edge, in metres.
pub const MIN_HEATMAP_CELL_M: f32 = 4.0;

/// Largest accepted heatmap cell edge, in metres.
pub const MAX_HEATMAP_CELL_M: f32 = 256.0;

/// Maximum number of cells in one district heatmap.
///
/// A 64 x 64 grid is the largest accepted query. The cap keeps one dashboard
/// rebuild bounded and makes the bridge payload size predictable.
pub const MAX_HEATMAP_CELLS: usize = 4_096;

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

/// One square cell in a [`DistrictHeatmap`].
///
/// `x` and `z` are the world-space centre of the cell, in metres. Counts and
/// needs use the same completed-only and resident-weighted rules as
/// [`DistrictSummary`]. A cell whose centre lies outside the query circle is
/// still present in the grid but reports zero for every metric.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DistrictCell {
    /// Cell centre on the world X axis, in metres.
    pub x: f32,
    /// Cell centre on the world Z axis, in metres.
    pub z: f32,
    /// Residents whose home placement centre is in this cell.
    pub population: u64,
    /// Housing capacity in completed buildings in this cell.
    pub housing_capacity: u64,
    /// Job slots in completed buildings in this cell.
    pub jobs: u64,
    /// Employed residents in this cell (sum of household workers).
    pub employed: u64,
    /// Resident-weighted happiness, 0..=1. Zero when the cell has no residents.
    pub happiness: f32,
    /// Resident-weighted food satisfaction.
    pub food: f32,
    /// Resident-weighted water satisfaction.
    pub water: f32,
    /// Resident-weighted rest satisfaction.
    pub rest: f32,
}

/// A dense, row-major heatmap grid for one circular district query.
///
/// The grid is a square bounding box around the circle: `cols == rows`, the
/// number of cells is `ceil(2 * radius_m / cell_m)` per axis (at least one),
/// and `cols * rows <= [`MAX_HEATMAP_CELLS`]`. The grid may extend up to one
/// cell beyond the circle diameter because the side count rounds up.
///
/// Cells are stored in row-major order: rows advance along `+z`, and each row
/// advances along `+x`. Cells whose centre lies outside the radius are included
/// with zero metrics rather than omitted, so `cells.len() == cols * rows`
/// always holds for a valid query. `DistrictCell::x` and `DistrictCell::z`
/// identify the world position; a renderer can sample the list as a texture
/// without a separate index.
///
/// The heatmap is derived state. It adds no save field and changes no existing
/// checksum. An invalid query returns [`DistrictHeatmap::default`]
/// (`cols == rows == 0`, empty cells, `checksum == 0`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DistrictHeatmap {
    /// Cell edge length in metres, copied from the query.
    pub cell_m: f32,
    /// Number of cell columns (X axis).
    pub cols: usize,
    /// Number of cell rows (Z axis).
    pub rows: usize,
    /// Query centre as `[x, z]` in metres.
    pub center: [f32; 2],
    /// Query radius in metres.
    pub radius_m: f32,
    /// Cells in row-major order; empty for an invalid query.
    pub cells: Vec<DistrictCell>,
    /// Stable FNV-1a checksum over the grid and every cell field.
    pub checksum: u64,
}

/// Number of square cells for a heatmap query, or `None` when the request is
/// outside the accepted bounds.
///
/// `radius_m` must be finite and within `0..=MAX_WORLD_COORD_M`; `cell_m` must
/// be finite and within [`MIN_HEATMAP_CELL_M`]`..=`[`MAX_HEATMAP_CELL_M`].
/// The per-axis count is `ceil(2 * radius_m / cell_m)`, at least one, and the
/// grid is rejected when `cols * rows` would exceed [`MAX_HEATMAP_CELLS`].
#[must_use]
pub fn heatmap_grid(radius_m: f32, cell_m: f32) -> Option<(usize, usize)> {
    if !radius_m.is_finite()
        || !(0.0..=MAX_WORLD_COORD_M).contains(&radius_m)
        || !cell_m.is_finite()
        || !(MIN_HEATMAP_CELL_M..=MAX_HEATMAP_CELL_M).contains(&cell_m)
    {
        return None;
    }
    let side = ((2.0 * f64::from(radius_m)) / f64::from(cell_m))
        .ceil()
        .max(1.0);
    if side * side > MAX_HEATMAP_CELLS as f64 {
        return None;
    }
    let side = side as usize;
    Some((side, side))
}

/// Build a dense district heatmap from completed placements and households.
///
/// The query radius and cell bounds are validated by [`heatmap_grid`]. The grid
/// is centred on `center` and covers the circle's bounding square. A placement
/// is attributed to the cell that contains its XZ centre when that centre is
/// inside the radius and its cell centre is also inside the radius; the same
/// filter applies to households. Edge points that map into a clipped corner
/// cell are therefore not represented; a smaller `cell_m` reduces that
/// quantization loss.
///
/// Completed metrics follow [`summarize_district`]: a placement without a
/// construction site is ignored, and an incomplete site contributes no
/// housing, jobs, population, or needs. Households are grouped by their home
/// placement, not by the cached `Household::home` field. Neighbours in the same
/// cell are resident-weighted for happiness, food, water, and rest.
#[must_use]
pub fn summarize_heatmap(
    placements: &PlacementStore,
    construction: &ConstructionStore,
    runtime: &CityRuntime,
    center: [f32; 2],
    radius_m: f32,
    cell_m: f32,
) -> DistrictHeatmap {
    let Some((cols, rows)) = heatmap_grid(radius_m, cell_m) else {
        return DistrictHeatmap::default();
    };
    if !center.iter().all(|value| value.is_finite()) {
        return DistrictHeatmap::default();
    }

    let half_width = cols as f32 * cell_m * 0.5;
    let half_depth = rows as f32 * cell_m * 0.5;
    let radius_sq = radius_m * radius_m;
    let mut cells = Vec::with_capacity(cols * rows);
    let mut cell_inside = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            // Compute the offset from the query centre first, so the middle
            // cell of an odd grid is exactly the centre even for radius zero.
            let offset_x = (col as f32 + 0.5) * cell_m - half_width;
            let offset_z = (row as f32 + 0.5) * cell_m - half_depth;
            let inside = offset_x * offset_x + offset_z * offset_z <= radius_sq;
            cell_inside.push(inside);
            cells.push(DistrictCell {
                x: center[0] + offset_x,
                z: center[1] + offset_z,
                ..DistrictCell::default()
            });
        }
    }

    // Map a world point that is already inside the circle to its cell. The
    // clamp keeps a placement exactly on the far grid edge in the last cell.
    let cell_index = |point: [f32; 3]| -> Option<usize> {
        let offset_x = point[0] - center[0] + half_width;
        let offset_z = point[2] - center[1] + half_depth;
        let col = (offset_x / cell_m).floor().clamp(0.0, (cols - 1) as f32) as usize;
        let row = (offset_z / cell_m).floor().clamp(0.0, (rows - 1) as f32) as usize;
        let index = row * cols + col;
        if cell_inside.get(index).copied().unwrap_or(false) {
            Some(index)
        } else {
            None
        }
    };
    let in_circle = |point: [f32; 3]| {
        let dx = point[0] - center[0];
        let dz = point[2] - center[1];
        dx * dx + dz * dz <= radius_sq
    };

    let mut home_cells = BTreeMap::new();
    for placement in &placements.placements {
        if !in_circle(placement.center) {
            continue;
        }
        let Some(index) = cell_index(placement.center) else {
            continue;
        };
        match construction.sites.get(&placement.id) {
            Some(site) if site.is_complete() => {
                home_cells.insert(placement.id, index);
                let cell = &mut cells[index];
                cell.housing_capacity += u64::from(placement.resident_capacity);
                cell.jobs += u64::from(placement.job_slots);
            }
            Some(_) | None => {}
        }
    }

    for (&id, household) in &runtime.households {
        let Some(&index) = home_cells.get(&id) else {
            continue;
        };
        let residents = u64::from(household.residents);
        if residents == 0 {
            continue;
        }
        let weight = household.residents as f32;
        let cell = &mut cells[index];
        cell.population += residents;
        cell.employed += u64::from(household.workers);
        cell.food += household.food * weight;
        cell.water += household.water * weight;
        cell.rest += household.rest * weight;
        cell.happiness += household.happiness() * weight;
    }

    for cell in &mut cells {
        if cell.population == 0 {
            cell.food = 0.0;
            cell.water = 0.0;
            cell.rest = 0.0;
            cell.happiness = 0.0;
        } else {
            let scale = 1.0 / cell.population as f32;
            cell.food *= scale;
            cell.water *= scale;
            cell.rest *= scale;
            cell.happiness *= scale;
        }
    }

    let mut hash = FNV_OFFSET_BASIS;
    fnv1a_feed(&mut hash, u64::from(cell_m.to_bits()));
    fnv1a_feed(&mut hash, cols as u64);
    fnv1a_feed(&mut hash, rows as u64);
    fnv1a_feed(&mut hash, u64::from(center[0].to_bits()));
    fnv1a_feed(&mut hash, u64::from(center[1].to_bits()));
    fnv1a_feed(&mut hash, u64::from(radius_m.to_bits()));
    fnv1a_feed(&mut hash, cells.len() as u64);
    for cell in &cells {
        fnv1a_feed(&mut hash, u64::from(cell.x.to_bits()));
        fnv1a_feed(&mut hash, u64::from(cell.z.to_bits()));
        fnv1a_feed(&mut hash, cell.population);
        fnv1a_feed(&mut hash, cell.housing_capacity);
        fnv1a_feed(&mut hash, cell.jobs);
        fnv1a_feed(&mut hash, cell.employed);
        fnv1a_feed(&mut hash, u64::from(cell.happiness.to_bits()));
        fnv1a_feed(&mut hash, u64::from(cell.food.to_bits()));
        fnv1a_feed(&mut hash, u64::from(cell.water.to_bits()));
        fnv1a_feed(&mut hash, u64::from(cell.rest.to_bits()));
    }

    DistrictHeatmap {
        cell_m,
        cols,
        rows,
        center,
        radius_m,
        cells,
        checksum: hash,
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
            blueprint_ref: None,
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
            workplace_id: None,
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

    fn cell_at(heatmap: &DistrictHeatmap, x: f32, z: f32) -> &DistrictCell {
        heatmap
            .cells
            .iter()
            .find(|cell| (cell.x - x).abs() < 1.0e-3 && (cell.z - z).abs() < 1.0e-3)
            .unwrap_or_else(|| panic!("no heatmap cell at ({x}, {z})"))
    }

    fn assert_cell_is_zero(cell: &DistrictCell) {
        assert_eq!(cell.population, 0);
        assert_eq!(cell.housing_capacity, 0);
        assert_eq!(cell.jobs, 0);
        assert_eq!(cell.employed, 0);
        assert_eq!(cell.happiness, 0.0);
        assert_eq!(cell.food, 0.0);
        assert_eq!(cell.water, 0.0);
        assert_eq!(cell.rest, 0.0);
    }

    #[test]
    fn empty_heatmap_is_a_zeroed_deterministic_grid() {
        let heatmap = summarize_heatmap(
            &PlacementStore::new(2026),
            &ConstructionStore::new(4, 1_000_000),
            &empty_runtime(2026),
            [10.0, 20.0],
            16.0,
            8.0,
        );
        assert_eq!((heatmap.cols, heatmap.rows), (4, 4));
        assert_eq!(heatmap.cells.len(), 16);
        assert_eq!(heatmap.cell_m, 8.0);
        assert_eq!(heatmap.center, [10.0, 20.0]);
        assert_eq!(heatmap.radius_m, 16.0);
        assert_ne!(heatmap.checksum, 0);
        assert_eq!(heatmap.cells[0].x, -2.0);
        assert_eq!(heatmap.cells[0].z, 8.0);
        assert_eq!(heatmap.cells[15].x, 22.0);
        assert_eq!(heatmap.cells[15].z, 32.0);
        for cell in &heatmap.cells {
            assert_cell_is_zero(cell);
        }

        let again = summarize_heatmap(
            &PlacementStore::new(2026),
            &ConstructionStore::new(4, 1_000_000),
            &empty_runtime(2026),
            [10.0, 20.0],
            16.0,
            8.0,
        );
        assert_eq!(heatmap, again);
    }

    #[test]
    fn single_cell_grid_aggregates_one_building_and_household() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [5.0, 0.0, 7.0], 4, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        let mut runtime = empty_runtime(2026);
        let mut home = household(4, 3, 2);
        home.food = 0.4;
        home.water = 0.5;
        home.rest = 0.6;
        runtime.households.insert(1, home);

        let heatmap = summarize_heatmap(&placements, &construction, &runtime, [5.0, 7.0], 0.0, 8.0);
        assert_eq!((heatmap.cols, heatmap.rows), (1, 1));
        let cell = &heatmap.cells[0];
        assert_eq!((cell.x, cell.z), (5.0, 7.0));
        assert_eq!(cell.population, 3);
        assert_eq!(cell.housing_capacity, 4);
        assert_eq!(cell.jobs, 2);
        assert_eq!(cell.employed, 2);
        assert!((cell.food - 0.4).abs() < 1.0e-6);
        assert!((cell.water - 0.5).abs() < 1.0e-6);
        assert!((cell.rest - 0.6).abs() < 1.0e-6);
        assert!((cell.happiness - runtime.households[&1].happiness()).abs() < 1.0e-6);
    }

    #[test]
    fn multi_cell_grid_attributes_buildings_and_households_by_home() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [-4.0, 0.0, -4.0], 2, 1));
        placements
            .placements
            .push(placement(2, [4.0, 0.0, 4.0], 6, 3));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        complete_site(&mut construction, 2);
        let mut runtime = empty_runtime(2026);
        let mut first = household(2, 2, 1);
        first.food = 0.2;
        let mut second = household(6, 4, 3);
        second.food = 0.8;
        runtime.households.insert(1, first);
        runtime.households.insert(2, second);

        let heatmap = summarize_heatmap(&placements, &construction, &runtime, [0.0, 0.0], 8.0, 8.0);
        assert_eq!((heatmap.cols, heatmap.rows), (2, 2));
        assert_eq!(heatmap.cells.len(), 4);

        let first_cell = cell_at(&heatmap, -4.0, -4.0);
        assert_eq!(first_cell.population, 2);
        assert_eq!(first_cell.housing_capacity, 2);
        assert_eq!(first_cell.jobs, 1);
        assert_eq!(first_cell.employed, 1);
        assert!((first_cell.food - 0.2).abs() < 1.0e-6);

        let second_cell = cell_at(&heatmap, 4.0, 4.0);
        assert_eq!(second_cell.population, 4);
        assert_eq!(second_cell.housing_capacity, 6);
        assert_eq!(second_cell.jobs, 3);
        assert_eq!(second_cell.employed, 3);
        assert!((second_cell.food - 0.8).abs() < 1.0e-6);

        assert_cell_is_zero(cell_at(&heatmap, 4.0, -4.0));
        assert_cell_is_zero(cell_at(&heatmap, -4.0, 4.0));
    }

    #[test]
    fn cells_outside_the_circle_are_zeroed_and_edge_points_are_kept() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [0.0, 0.0, 0.0], 1, 0));
        // Exactly on the radius; the last cell centre is inside it.
        placements
            .placements
            .push(placement(2, [10.0, 0.0, 0.0], 3, 0));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        complete_site(&mut construction, 2);

        let heatmap = summarize_heatmap(
            &placements,
            &construction,
            &empty_runtime(2026),
            [0.0, 0.0],
            10.0,
            4.0,
        );
        assert_eq!((heatmap.cols, heatmap.rows), (5, 5));
        assert_eq!(heatmap.cells.len(), 25);

        assert_eq!(cell_at(&heatmap, 0.0, 0.0).housing_capacity, 1);
        assert_eq!(cell_at(&heatmap, 8.0, 0.0).housing_capacity, 3);

        for cell in &heatmap.cells {
            let outside = cell.x * cell.x + cell.z * cell.z > 100.0;
            if outside {
                assert_cell_is_zero(cell);
            }
        }
        // The 5 x 5 grid has four corners at (+/-8, +/-8), outside radius 10.
        for (x, z) in [(-8.0, -8.0), (8.0, -8.0), (-8.0, 8.0), (8.0, 8.0)] {
            assert_cell_is_zero(cell_at(&heatmap, x, z));
        }
    }

    #[test]
    fn heatmap_grid_accepts_only_bounded_cells_and_radii() {
        assert_eq!(heatmap_grid(0.0, 4.0), Some((1, 1)));
        assert_eq!(heatmap_grid(8.0, 4.0), Some((4, 4)));
        assert_eq!(heatmap_grid(256.0, 16.0), Some((32, 32)));
        assert_eq!(heatmap_grid(256.0, 4.0), None);

        for radius in [-1.0, f32::NAN, f32::INFINITY, MAX_WORLD_COORD_M + 1.0] {
            assert_eq!(heatmap_grid(radius, 16.0), None, "radius {radius}");
        }
        for cell_m in [
            3.99,
            0.0,
            -4.0,
            f32::NAN,
            f32::INFINITY,
            MAX_HEATMAP_CELL_M + 0.01,
        ] {
            assert_eq!(heatmap_grid(64.0, cell_m), None, "cell {cell_m}");
        }
    }

    #[test]
    fn invalid_heatmap_inputs_return_the_default() {
        let placements = PlacementStore::new(2026);
        let construction = ConstructionStore::new(4, 1_000_000);
        let runtime = empty_runtime(2026);
        for (center, radius, cell_m) in [
            ([f32::NAN, 0.0], 64.0, 16.0),
            ([0.0, f32::INFINITY], 64.0, 16.0),
            ([0.0, 0.0], -1.0, 16.0),
            ([0.0, 0.0], MAX_WORLD_COORD_M + 1.0, 16.0),
            ([0.0, 0.0], 64.0, 2.0),
            ([0.0, 0.0], 64.0, f32::NAN),
        ] {
            let heatmap =
                summarize_heatmap(&placements, &construction, &runtime, center, radius, cell_m);
            assert_eq!(
                heatmap,
                DistrictHeatmap::default(),
                "center {center:?}, radius {radius}, cell {cell_m}"
            );
            assert_eq!(heatmap.checksum, 0);
        }
    }

    #[test]
    fn household_needs_are_resident_weighted_within_one_cell() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [-4.0, 0.0, -4.0], 2, 0));
        placements
            .placements
            .push(placement(2, [-4.0, 0.0, -3.0], 8, 0));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        complete_site(&mut construction, 2);
        let mut runtime = empty_runtime(2026);
        let mut first = household(2, 2, 1);
        first.food = 0.4;
        first.water = 0.4;
        first.rest = 0.4;
        let mut second = household(8, 8, 4);
        second.food = 0.9;
        second.water = 0.9;
        second.rest = 0.9;
        runtime.households.insert(1, first);
        runtime.households.insert(2, second);

        let heatmap = summarize_heatmap(&placements, &construction, &runtime, [0.0, 0.0], 8.0, 8.0);
        let cell = cell_at(&heatmap, -4.0, -4.0);
        assert_eq!(cell.population, 10);
        assert_eq!(cell.housing_capacity, 10);
        assert_eq!(cell.employed, 5);
        assert!((cell.food - 0.8).abs() < 1.0e-6);
        assert!((cell.water - 0.8).abs() < 1.0e-6);
        assert!((cell.rest - 0.8).abs() < 1.0e-6);
    }

    #[test]
    fn incomplete_and_unbackfilled_placements_are_ignored() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [-4.0, 0.0, -4.0], 4, 2));
        placements
            .placements
            .push(placement(2, [4.0, 0.0, -4.0], 6, 3));
        placements
            .placements
            .push(placement(3, [-4.0, 0.0, 4.0], 8, 4));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        construction.add_site(2, 100).expect("site");
        let mut runtime = empty_runtime(2026);
        runtime.households.insert(1, household(4, 2, 1));
        runtime.households.insert(2, household(6, 5, 3));
        runtime.households.insert(3, household(8, 7, 4));

        let heatmap = summarize_heatmap(&placements, &construction, &runtime, [0.0, 0.0], 8.0, 8.0);
        let completed = cell_at(&heatmap, -4.0, -4.0);
        assert_eq!(completed.population, 2);
        assert_eq!(completed.housing_capacity, 4);
        assert_eq!(completed.jobs, 2);
        assert_eq!(completed.employed, 1);
        assert_cell_is_zero(cell_at(&heatmap, 4.0, -4.0));
        assert_cell_is_zero(cell_at(&heatmap, -4.0, 4.0));
    }

    #[test]
    fn heatmap_checksum_is_deterministic_and_data_sensitive() {
        let mut placements = PlacementStore::new(2026);
        placements
            .placements
            .push(placement(1, [-4.0, 0.0, -4.0], 4, 2));
        let mut construction = ConstructionStore::new(4, 1_000_000);
        complete_site(&mut construction, 1);
        let mut runtime = empty_runtime(2026);
        runtime.households.insert(1, household(4, 3, 2));

        let first = summarize_heatmap(&placements, &construction, &runtime, [0.0, 0.0], 8.0, 8.0);
        let second = summarize_heatmap(&placements, &construction, &runtime, [0.0, 0.0], 8.0, 8.0);
        assert_eq!(first, second);
        assert_eq!(first.checksum, second.checksum);
        assert_ne!(first.checksum, 0);

        let mut changed = runtime.clone();
        changed.households.get_mut(&1).expect("household").food = 0.25;
        let changed = summarize_heatmap(&placements, &construction, &changed, [0.0, 0.0], 8.0, 8.0);
        assert_ne!(first.checksum, changed.checksum);

        let resized =
            summarize_heatmap(&placements, &construction, &runtime, [0.0, 0.0], 16.0, 16.0);
        assert_ne!(first.checksum, resized.checksum);
    }
}
