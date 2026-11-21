#![recursion_limit = "256"]

//! Deterministic 30-day city-loop benchmark.
//!
//! Builds a fixed 50-house / 500-resident city plus five workplaces and a road
//! grid, runs phased construction, move-in, road-distance job assignment,
//! needs, and wages at a fixed step, then prints a stable JSON summary and
//! checksum.

use std::collections::BTreeSet;

use core_city::{
    summarize, CityRuntime, ConstructionStore, Economy, Home, IndustryKind, IndustryStore,
    Placement, PlacementPlan, PlacementRequest, PlacementStore, RoadGraph, RoadSegment, RoadStore,
    UtilityConsumer, UtilityKind, UtilityReport, UtilityStore, Workplace, MAX_LOAN,
};

const DEFAULT_DAYS: u64 = 30;
const DEFAULT_HOUSES: u64 = 50;
const DEFAULT_STEP_SECONDS: f64 = 300.0;
const CAPACITY_PER_HOUSE: u32 = 10;
const JOBS_PER_HOUSE: u32 = 10;
const WORKPLACES: u64 = 5;
const JOBS_PER_WORKPLACE: u32 = 80;
const SOLID_VOXELS: u64 = 10_000;
const CREW_CAPACITY: u16 = 8;
const MATERIALS_STOCK: u64 = 10_000_000;
const CONSTRUCTION_COST_PER_BUILDING: f64 = 3_000.0;
const MAX_FINANCED_BUILDINGS: u64 = 116;
const RECOVERY_TAX_PER_RESIDENT: f64 = 20.0;
const RECOVERY_TAX_PER_JOB: f64 = 4.0;

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
    if houses + WORKPLACES > MAX_FINANCED_BUILDINGS {
        eprintln!(
            "city_loop: at most {MAX_FINANCED_BUILDINGS} buildings (including {WORKPLACES} workplaces) are supported by the starting balance and loan limit"
        );
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
            material_cost: CONSTRUCTION_COST_PER_BUILDING,
            resident_capacity: CAPACITY_PER_HOUSE,
            job_slots: if index < 10 { JOBS_PER_HOUSE } else { 0 },
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
    for index in 0..WORKPLACES {
        let id = houses + index + 1;
        let x = 116.0;
        let z = 128.0 + index as f32 * 20.0;
        let request = PlacementRequest {
            blueprint_id: "local:city_loop_workshop".to_string(),
            blueprint_hash: "blake3:city-loop".to_string(),
            blueprint_path: "/city_loop/workshop.tsbp".to_string(),
            name: "City Loop Workshop".to_string(),
            bounds_m: [12.0, 8.0, 12.0],
            center_x: x,
            center_z: z,
            rotation_deg: 0,
            material_cost: CONSTRUCTION_COST_PER_BUILDING,
            resident_capacity: 0,
            job_slots: JOBS_PER_WORKPLACE,
            solid_voxels: SOLID_VOXELS,
        };
        let plan = PlacementPlan {
            center: [x, 40.0, z],
            rotation_deg: 0,
            footprint_m: [12.0, 12.0],
            foundation_m: 0.0,
        };
        placements
            .placements
            .push(Placement::from_request(id, &request, plan));
        construction.add_site(id, SOLID_VOXELS).expect("site");
    }
    placements.next_id = houses + WORKPLACES + 1;

    let rows = houses.div_ceil(10);
    let mut road_store = RoadStore::new(seed);
    road_store.roads.push(RoadSegment {
        id: 1,
        start: [120, 118],
        end: [120, 128 + (rows as i32 - 1) * 20 + 10],
        width_m: 8,
    });
    for index in 0..rows {
        road_store.roads.push(RoadSegment {
            id: index + 2,
            start: [120, 118 + index as i32 * 20],
            end: [318, 118 + index as i32 * 20],
            width_m: 8,
        });
    }
    road_store.next_id = rows + 2;
    let traffic = RoadGraph::build(&road_store.roads).expect("traffic graph");

    let mut industry = IndustryStore::new(seed);
    for index in 0..houses.min(10) {
        let kind = match index {
            0..=3 => IndustryKind::Farm,
            4..=6 => IndustryKind::Lumberjack,
            _ => IndustryKind::Hunt,
        };
        industry.add_site(index + 1, kind).expect("industry site");
    }

    let utility_span = houses.min(10);
    let water_ids: BTreeSet<u64> = [1, utility_span].into_iter().collect();
    let power_ids: BTreeSet<u64> = [1, 5.min(utility_span), utility_span].into_iter().collect();
    let mut utilities = UtilityStore::new(seed);
    let mut added_utilities: BTreeSet<(u64, UtilityKind)> = BTreeSet::new();
    let mut utility_report = UtilityReport::default();

    let mut runtime = CityRuntime::new(seed);
    runtime.set_move_in_per_second(5.0);
    let mut economy = Economy::new(seed);
    let construction_cost = (houses + WORKPLACES) as f64 * CONSTRUCTION_COST_PER_BUILDING;
    let loan_amount = (construction_cost - 50_000.0).clamp(50_000.0, MAX_LOAN);
    economy
        .take_loan(
            loan_amount,
            core_city::DEFAULT_LOAN_RATE_PER_DAY,
            core_city::DEFAULT_LOAN_TERM_DAYS,
        )
        .expect("construction loan");
    economy.charge_construction(construction_cost);
    let recovery_day = days as f64 / 2.0;
    let mut recovered = false;
    let mut min_balance = economy.balance;
    let mut min_balance_day = 0.0;
    let step_count = (days as f64 * 86_400.0 / step).ceil() as u64;
    for _ in 0..step_count {
        construction.tick(step);
        if !recovered && economy.days_elapsed >= recovery_day {
            economy.set_taxes(RECOVERY_TAX_PER_RESIDENT, RECOVERY_TAX_PER_JOB);
            recovered = true;
        }
        for (&id, kind) in water_ids
            .iter()
            .map(|id| (id, UtilityKind::Water))
            .chain(power_ids.iter().map(|id| (id, UtilityKind::Power)))
        {
            if added_utilities.contains(&(id, kind)) {
                continue;
            }
            if !construction.site(id).is_some_and(|site| site.is_complete()) {
                continue;
            }
            let placement = placements
                .placements
                .iter()
                .find(|placement| placement.id == id)
                .expect("utility placement");
            utilities
                .add_site(id, kind, placement.center)
                .expect("utility site");
            added_utilities.insert((id, kind));
        }
        let homes = placements
            .placements
            .iter()
            .filter(|placement| {
                placement.resident_capacity > 0
                    && construction
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
                placement.job_slots > 0
                    && construction
                        .site(placement.id)
                        .is_some_and(|site| site.is_complete())
            })
            .map(|placement| Workplace {
                id: placement.id,
                slots: placement.job_slots,
                center: [placement.center[0], placement.center[2]],
            })
            .collect::<Vec<_>>();
        let consumers = placements
            .placements
            .iter()
            .filter(|placement| {
                construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
            })
            .map(|placement| UtilityConsumer {
                id: placement.id,
                center: [placement.center[0], placement.center[2]],
                residents: runtime
                    .households
                    .get(&placement.id)
                    .map_or(0, |household| household.residents),
                workers: runtime.staffing.get(&placement.id).copied().unwrap_or(0),
            })
            .collect::<Vec<_>>();
        utility_report = utilities.refresh(&consumers);
        industry.tick_with_power(
            step,
            &runtime.staffing,
            runtime.population(),
            utilities.power_supply,
        );
        runtime.tick_with_supplies_and_roads(
            step,
            &homes,
            &workplaces,
            industry.food_supply,
            utilities.water_supply,
            utilities.power_supply,
            Some(&traffic),
        );
        let completed = construction
            .sites
            .values()
            .filter(|site| site.is_complete())
            .count() as u64;
        let jobs = workplaces
            .iter()
            .map(|work| u64::from(work.slots))
            .sum::<u64>();
        economy.tick(
            step,
            runtime.population(),
            runtime.workers().min(jobs),
            completed,
        );
        if economy.balance < min_balance {
            min_balance = economy.balance;
            min_balance_day = economy.days_elapsed;
        }
    }

    let summary = summarize(&placements, &construction, &runtime, 0);
    let mut result = serde_json::json!({
        "scene": "city_loop",
        "seed": seed,
        "days": days,
        "houses": houses,
        "workplaces": WORKPLACES,
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
        "road_segments": road_store.roads.len(),
        "traffic_nodes": traffic.stats().nodes,
        "traffic_edges": traffic.stats().edges,
        "traffic_components": traffic.stats().components,
        "road_length_m": traffic.stats().total_length_m,
        "traffic_checksum": format!("{:016x}", traffic.checksum()),
        "population_checksum": format!("{:016x}", runtime.checksum()),
        "construction_checksum": format!("{:016x}", construction.checksum()),
        "industry_sites": industry.site_count(),
        "stock_wood": industry.stockpile.wood,
        "stock_food": industry.stockpile.food,
        "stock_hides": industry.stockpile.hides,
        "produced_wood": industry.produced_total.wood,
        "produced_food": industry.produced_total.food,
        "produced_hides": industry.produced_total.hides,
        "food_supply": industry.food_supply,
        "industry_checksum": format!("{:016x}", industry.checksum()),
        "utility_sites": utilities.site_count(),
        "water_sites": utilities.water_sites.len(),
        "power_sites": utilities.power_sites.len(),
        "water_supply": utilities.water_supply,
        "power_supply": utilities.power_supply,
        "water_demand": utility_report.water_demand,
        "power_demand": utility_report.power_demand,
        "water_capacity": utility_report.water_capacity,
        "power_capacity": utility_report.power_capacity,
        "utility_checksum": format!("{:016x}", utilities.checksum()),
        "balance": economy.balance,
        "min_balance": min_balance,
        "min_balance_day": min_balance_day,
        "daily_net": economy.daily_net(summary.population, summary.employed, summary.completed_buildings as u64),
        "construction_cost": construction_cost,
        "construction_spent": economy.construction_spent,
        "loan_amount": loan_amount,
        "loan_count": economy.loans.len(),
        "debt": economy.debt(),
        "loan_payment_per_day": economy.loan_payment_per_day(),
        "tax_per_resident_per_day": economy.tax_per_resident_per_day,
        "business_tax_per_job_per_day": economy.business_tax_per_job_per_day,
        "recovery_day": recovery_day,
        "bankrupt": economy.bankrupt,
        "economy_checksum": format!("{:016x}", economy.checksum()),
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
        assert_eq!(
            result["jobs"],
            5 * JOBS_PER_HOUSE as u64 + WORKPLACES * JOBS_PER_WORKPLACE as u64
        );
        assert_eq!(result["employed"], 50);
        assert_eq!(result["completed_buildings"], 5 + WORKPLACES);
        assert_eq!(result["construction_sites"], 0);
        assert!(result["construction_spent"].as_f64().expect("spent") > 0.0);
        assert!(result["debt"].as_f64().expect("debt") > 0.0);
        assert!(result["balance"].as_f64().expect("balance") > 0.0);
        assert_eq!(result["bankrupt"], false);
        assert!(result["construction_checksum"].as_str().is_some());
        assert!(result["economy_checksum"].as_str().is_some());
        assert!(result["utility_sites"].as_u64().expect("utilities") >= 2);
        assert!(result["water_supply"].as_f64().expect("water supply") > 0.99);
        assert!(result["power_supply"].as_f64().expect("power supply") > 0.99);
        assert!(result["water"].as_f64().expect("water") > 0.9);
        assert!(result["rest"].as_f64().expect("rest") > 0.9);
        assert!(result["happiness"].as_f64().expect("happiness") > 0.9);
        assert!(result["utility_checksum"].as_str().is_some());
        assert_eq!(result["industry_sites"], 5);
        assert!(result["stock_food"].as_f64().expect("stock") > 0.0);
        assert!(result["produced_wood"].as_f64().expect("wood") > 0.0);
        assert!(result["food_supply"].as_f64().expect("supply") > 0.99);
        assert!(result["happiness"].as_f64().expect("happiness") > 0.6);
        assert!(result["average_commute_m"].as_f64().expect("commute") >= 0.0);
        assert!(result["traffic_nodes"].as_u64().expect("nodes") >= 2);
        assert!(result["traffic_edges"].as_u64().expect("edges") >= 1);
        assert_eq!(result["checksum"], simulate(1, 5, 60.0)["checksum"]);
    }

    #[test]
    fn houses_beyond_the_first_row_use_road_commutes() {
        let result = simulate(1, 15, 60.0);
        assert_eq!(result["population"], 150);
        assert_eq!(result["jobs"], 500);
        assert!(result["average_commute_m"].as_f64().expect("commute") > 10.0);
        assert!(result["traffic_nodes"].as_u64().expect("nodes") > 2);
    }
}
