//! Deterministic 30-day city-loop benchmark.
//!
//! Builds a fixed 50-house / 500-resident city from placement records, runs
//! phased construction, move-in, job assignment, needs, and wages at a fixed
//! step, then prints a stable JSON summary and checksum.

use core_city::{
    summarize, CityRuntime, ConstructionStore, Home, Placement, PlacementPlan, PlacementRequest,
    PlacementStore, Workplace,
};

const DEFAULT_DAYS: u64 = 30;
const DEFAULT_HOUSES: u64 = 50;
const DEFAULT_STEP_SECONDS: f64 = 300.0;
const CAPACITY_PER_HOUSE: u32 = 10;
const JOBS_PER_HOUSE: u32 = 10;
const SOLID_VOXELS: u64 = 10_000;
const CREW_CAPACITY: u16 = 8;
const MATERIALS_STOCK: u64 = 10_000_000;

fn main() {
    let days = arg_value("--days")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DAYS);
    let houses = arg_value("--houses")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_HOUSES);
    let step = arg_value("--step")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(DEFAULT_STEP_SECONDS);
    if days == 0 || houses == 0 || !step.is_finite() || step <= 0.0 {
        eprintln!("usage: city_loop [--days N] [--houses N] [--step SECONDS]");
        std::process::exit(2);
    }

    let output = simulate(days, houses, step);
    println!(
        "{}",
        serde_json::to_string_pretty(&output).expect("pretty json")
    );
}

fn simulate(days: u64, houses: u64, step: f64) -> serde_json::Value {
    let seed = 2026;
    let mut placements = PlacementStore::new(seed);
    let mut construction = ConstructionStore::new(CREW_CAPACITY, MATERIALS_STOCK);
    for index in 0..houses {
        let id = index + 1;
        let x = 128.0 + (index % 10) as f32 * 20.0;
        let z = 128.0 + (index / 10) as f32 * 20.0;
        let request = PlacementRequest {
            blueprint_id: "local:city_loop_house".to_string(),
            blueprint_hash: "blake3:city-loop".to_string(),
            blueprint_path: "/city_loop/house.tsbp".to_string(),
            name: "City Loop House".to_string(),
            bounds_m: [8.0, 6.0, 8.0],
            center_x: x,
            center_z: z,
            rotation_deg: 0,
            material_cost: 0.0,
            resident_capacity: CAPACITY_PER_HOUSE,
            job_slots: JOBS_PER_HOUSE,
            solid_voxels: SOLID_VOXELS,
        };
        let plan = PlacementPlan {
            center: [x, 40.0, z],
            rotation_deg: 0,
            footprint_m: [8.0, 8.0],
            foundation_m: 0.0,
        };
        placements
            .placements
            .push(Placement::from_request(id, &request, plan));
        construction.add_site(id, SOLID_VOXELS).expect("site");
    }
    placements.next_id = houses + 1;

    let mut runtime = CityRuntime::new(seed);
    runtime.set_move_in_per_second(5.0);
    let step_count = (days as f64 * 86_400.0 / step).ceil() as u64;
    for _ in 0..step_count {
        construction.tick(step);
        let homes = placements
            .placements
            .iter()
            .filter(|placement| {
                construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
            })
            .map(|placement| Home {
                id: placement.id,
                capacity: placement.resident_capacity,
                center: [placement.center[0], placement.center[2]],
            })
            .collect::<Vec<_>>();
        let workplaces = placements
            .placements
            .iter()
            .filter(|placement| {
                construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
            })
            .map(|placement| Workplace {
                id: placement.id,
                slots: placement.job_slots,
                center: [placement.center[0], placement.center[2]],
            })
            .collect::<Vec<_>>();
        runtime.tick_with_workplaces(step, &homes, &workplaces);
    }

    let summary = summarize(&placements, &construction, &runtime, 0);
    let mut result = serde_json::json!({
        "scene": "city_loop",
        "seed": seed,
        "days": days,
        "houses": houses,
        "step_seconds": step,
        "steps": step_count,
        "population": summary.population,
        "housing_capacity": summary.housing_capacity,
        "vacancies": summary.vacancies,
        "jobs": summary.jobs,
        "employed": summary.employed,
        "unemployed": summary.unemployed,
        "happiness": summary.happiness,
        "food": summary.food,
        "water": summary.water,
        "rest": summary.rest,
        "average_commute_m": summary.average_commute_m,
        "daily_wages": summary.daily_wages,
        "completed_buildings": summary.completed_buildings,
        "construction_sites": summary.construction_sites,
        "population_checksum": format!("{:016x}", runtime.checksum()),
        "construction_checksum": format!("{:016x}", construction.checksum()),
    });
    let checksum = checksum_json(&result);
    result["checksum"] = serde_json::Value::String(format!("{checksum:016x}"));
    result
}

fn checksum_json(value: &serde_json::Value) -> u64 {
    let mut checksum = 0xcbf2_9ce4_8422_2325_u64;
    for byte in serde_json::to_vec(value).expect("json") {
        checksum ^= u64::from(byte);
        checksum = checksum.wrapping_mul(0x0000_0100_0000_01b3);
    }
    checksum
}

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for index in 0..args.len() {
        if args[index] == name {
            return args.get(index + 1).cloned();
        }
        let prefix = format!("{name}=");
        if let Some(value) = args[index].strip_prefix(&prefix) {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_five_house_city_completes_and_fills() {
        let result = simulate(1, 5, 60.0);
        assert_eq!(result["population"], 50);
        assert_eq!(result["jobs"], 50);
        assert_eq!(result["employed"], 50);
        assert_eq!(result["completed_buildings"], 5);
        assert_eq!(result["construction_sites"], 0);
        assert!(result["construction_checksum"].as_str().is_some());
        assert_eq!(result["checksum"], simulate(1, 5, 60.0)["checksum"]);
    }
}
