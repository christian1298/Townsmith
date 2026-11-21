#![recursion_limit = "256"]

//! Deterministic 30-day city-loop benchmark.
//!
//! Builds a fixed 50-house / 500-resident city plus five workplaces and a road
//! grid, runs phased construction, move-in, road-distance job assignment,
//! needs, and wages at a fixed step, then prints a stable JSON summary and
//! checksum.

use std::collections::BTreeSet;

use core_city::{
    summarize, Age, CityRuntime, ConstructionStore, Economy, Home, IndustryKind, IndustryStore,
    Placement, PlacementPlan, PlacementRequest, PlacementStore, RoadGraph, RoadSegment, RoadStore,
    UtilityConsumer, UtilityKind, UtilityReport, UtilityStore, Workplace, MAX_LOAN,
};

const DEFAULT_DAYS: u64 = 30;
const DEFAULT_STEP_SECONDS: f64 = 300.0;
const CAPACITY_PER_HOUSE: u32 = 10;
const JOBS_PER_HOUSE: u32 = 10;
const JOBS_PER_WORKPLACE: u32 = 80;
const SOLID_VOXELS: u64 = 10_000;
const CREW_CAPACITY: u16 = 8;
const MATERIALS_STOCK: u64 = 10_000_000;
const CONSTRUCTION_COST_PER_BUILDING: f64 = 3_000.0;
const MAX_FINANCED_BUILDINGS: u64 = 116;
const RECOVERY_TAX_PER_RESIDENT: f64 = 20.0;
const RECOVERY_TAX_PER_JOB: f64 = 4.0;

/// One named city-loop scenario.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Scenario {
    name: &'static str,
    houses: u64,
    workplaces: u64,
    industry_count: u64,
    recovery_day: f64,
    water_ids: &'static [u64],
    power_ids: &'static [u64],
}

const SCENARIOS: [Scenario; 3] = [
    Scenario {
        name: "early",
        houses: 10,
        workplaces: 1,
        industry_count: 3,
        recovery_day: 15.0,
        water_ids: &[1, 10],
        power_ids: &[1, 5, 10],
    },
    Scenario {
        name: "mid",
        houses: 50,
        workplaces: 5,
        industry_count: 10,
        recovery_day: 15.0,
        water_ids: &[1, 10],
        power_ids: &[1, 5, 10],
    },
    Scenario {
        name: "late",
        houses: 100,
        workplaces: 10,
        industry_count: 20,
        recovery_day: 5.0,
        water_ids: &[1, 10, 91, 100],
        power_ids: &[1, 10, 46, 55, 91, 100],
    },
];

/// One deliberately broken service in the mid-city degraded matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Degraded {
    Water,
    Power,
    Industry,
    Roads,
    Utilities,
}

impl Degraded {
    /// Every degraded variant, in matrix order.
    const ALL: [Self; 5] = [
        Self::Water,
        Self::Power,
        Self::Industry,
        Self::Roads,
        Self::Utilities,
    ];

    /// Stable lowercase identifier.
    fn name(self) -> &'static str {
        match self {
            Self::Water => "no_water",
            Self::Power => "no_power",
            Self::Industry => "no_industry",
            Self::Roads => "no_roads",
            Self::Utilities => "no_utilities",
        }
    }

    /// Parse a stable identifier, accepting a few short aliases.
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "no_water" | "water" => Some(Self::Water),
            "no_power" | "power" => Some(Self::Power),
            "no_industry" | "industry" => Some(Self::Industry),
            "no_roads" | "roads" => Some(Self::Roads),
            "no_utilities" | "utilities" => Some(Self::Utilities),
            _ => None,
        }
    }

    fn skips_water(self) -> bool {
        matches!(self, Self::Water | Self::Utilities)
    }

    fn skips_power(self) -> bool {
        matches!(self, Self::Power | Self::Utilities)
    }

    fn skips_industry(self) -> bool {
        matches!(self, Self::Industry)
    }

    fn skips_roads(self) -> bool {
        matches!(self, Self::Roads)
    }
}

fn main() {
    let days = arg_value("--days")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DAYS);
    let step = arg_value("--step")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(DEFAULT_STEP_SECONDS);
    let matrix = std::env::args().any(|arg| arg == "--matrix");
    let balance = std::env::args().any(|arg| arg == "--balance");
    let degraded_matrix = std::env::args().any(|arg| arg == "--degraded-matrix");
    let degraded_name = arg_value("--degraded");
    if days == 0 || !step.is_finite() || step <= 0.0 {
        eprintln!(
            "usage: city_loop [--scenario early|mid|late] [--days N] [--houses N] [--workplaces N] [--step SECONDS] [--matrix] [--balance] [--degraded no_water|no_power|no_industry|no_roads|no_utilities] [--degraded-matrix]"
        );
        std::process::exit(2);
    }
    let modes = [matrix, balance, degraded_matrix, degraded_name.is_some()]
        .into_iter()
        .filter(|enabled| *enabled)
        .count();
    if modes > 1 {
        eprintln!(
            "city_loop: choose only one of --matrix, --balance, --degraded, or --degraded-matrix"
        );
        std::process::exit(2);
    }

    if degraded_matrix {
        println!(
            "{}",
            serde_json::to_string_pretty(&degraded_matrix_report(days, step)).expect("pretty json")
        );
        return;
    }

    if let Some(name) = degraded_name {
        let Some(degraded) = Degraded::parse(&name) else {
            eprintln!(
                "city_loop: unknown degraded variant '{name}' (expected no_water, no_power, no_industry, no_roads, or no_utilities)"
            );
            std::process::exit(2);
        };
        let output = simulate_with(&SCENARIOS[1], days, step, Some(degraded));
        println!(
            "{}",
            serde_json::to_string_pretty(&output).expect("pretty json")
        );
        return;
    }

    if matrix {
        let scenarios: Vec<serde_json::Value> = SCENARIOS
            .iter()
            .map(|scenario| simulate(scenario, days, step))
            .collect();
        let output = serde_json::json!({
            "scene": "city_loop_matrix",
            "days": days,
            "step_seconds": step,
            "scenarios": scenarios,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).expect("pretty json")
        );
        return;
    }

    if balance {
        println!(
            "{}",
            serde_json::to_string_pretty(&balance_report(&SCENARIOS, days, step))
                .expect("pretty json")
        );
        return;
    }

    let mut scenario = match arg_value("--scenario") {
        Some(name) => match SCENARIOS.iter().find(|scenario| scenario.name == name) {
            Some(scenario) => *scenario,
            None => {
                eprintln!("city_loop: unknown scenario '{name}' (expected early, mid, or late)");
                std::process::exit(2);
            }
        },
        None => SCENARIOS[1],
    };
    if let Some(houses) = arg_value("--houses").and_then(|value| value.parse::<u64>().ok()) {
        scenario.houses = houses;
    }
    if let Some(workplaces) = arg_value("--workplaces").and_then(|value| value.parse::<u64>().ok())
    {
        scenario.workplaces = workplaces;
    }
    if scenario.houses == 0 || scenario.workplaces == 0 {
        eprintln!("city_loop: houses and workplaces must be greater than zero");
        std::process::exit(2);
    }
    if scenario.houses + scenario.workplaces > MAX_FINANCED_BUILDINGS {
        eprintln!(
            "city_loop: at most {MAX_FINANCED_BUILDINGS} buildings (including {} workplaces) are supported by the starting balance and loan limit",
            scenario.workplaces
        );
        std::process::exit(2);
    }

    let output = simulate(&scenario, days, step);
    println!(
        "{}",
        serde_json::to_string_pretty(&output).expect("pretty json")
    );
}

fn simulate(scenario: &Scenario, days: u64, step: f64) -> serde_json::Value {
    simulate_with(scenario, days, step, None)
}

fn simulate_with(
    scenario: &Scenario,
    days: u64,
    step: f64,
    degraded: Option<Degraded>,
) -> serde_json::Value {
    let houses = scenario.houses;
    let workplaces_count = scenario.workplaces;
    let skips_water = degraded.is_some_and(Degraded::skips_water);
    let skips_power = degraded.is_some_and(Degraded::skips_power);
    let skips_industry = degraded.is_some_and(Degraded::skips_industry);
    let skips_roads = degraded.is_some_and(Degraded::skips_roads);
    let planned_industry_count =
        usize::try_from(scenario.industry_count.min(houses)).expect("industry count fits usize");
    let industry_count = if skips_industry {
        0
    } else {
        planned_industry_count
    };
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
            job_slots: if (index as usize) < industry_count {
                JOBS_PER_HOUSE
            } else {
                0
            },
            solid_voxels: SOLID_VOXELS,
            structure: None,
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
    for index in 0..workplaces_count {
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
            structure: None,
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
    placements.next_id = houses + workplaces_count + 1;

    let rows = houses.div_ceil(10);
    let mut road_store = RoadStore::new(seed);
    if !skips_roads {
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
    }
    let traffic = RoadGraph::build(&road_store.roads).expect("traffic graph");

    let mut industry = IndustryStore::new(seed);
    for index in 0..industry_count {
        let kind = match index % 10 {
            0..=3 => IndustryKind::Farm,
            4..=6 => IndustryKind::Lumberjack,
            _ => IndustryKind::Hunt,
        };
        let id = u64::try_from(index).expect("industry id") + 1;
        industry.add_site(id, kind).expect("industry site");
    }

    let utility_span = houses;
    let water_count = (houses / 25).max(2);
    let power_count = (houses / 20).max(3);
    let water_ids = if skips_water {
        BTreeSet::new()
    } else {
        even_ids(utility_span, water_count)
    };
    let power_ids = if skips_power {
        BTreeSet::new()
    } else {
        even_ids(utility_span, power_count)
    };
    let mut utilities = UtilityStore::new(seed);
    let mut added_utilities: BTreeSet<(u64, UtilityKind)> = BTreeSet::new();
    let mut utility_report = UtilityReport::default();

    let mut runtime = CityRuntime::new(seed);
    runtime.set_move_in_per_second(5.0);
    let mut economy = Economy::new(seed);
    let construction_cost = (houses + workplaces_count) as f64 * CONSTRUCTION_COST_PER_BUILDING;
    let loan_amount = (construction_cost - 50_000.0).clamp(50_000.0, MAX_LOAN);
    economy
        .take_loan(
            loan_amount,
            core_city::DEFAULT_LOAN_RATE_PER_DAY,
            core_city::DEFAULT_LOAN_TERM_DAYS,
        )
        .expect("construction loan");
    economy.charge_construction(construction_cost);
    let recovery_day = scenario.recovery_day.min(days as f64);
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
            if skips_roads { None } else { Some(&traffic) },
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
        "scenario": scenario.name,
        "seed": seed,
        "days": days,
        "houses": houses,
        "workplaces": workplaces_count,
        "house_jobs": industry_count * usize::try_from(JOBS_PER_HOUSE).expect("jobs"),
        "industry_sites_planned": planned_industry_count,
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
    if let Some(degraded) = degraded {
        result["scene"] = serde_json::Value::String("city_loop_degraded".to_string());
        result["degraded"] = serde_json::Value::String(degraded.name().to_string());
        result["age"] =
            serde_json::Value::String(Age::from_population(summary.population).name().to_string());
    }
    let checksum = checksum_json(&result);
    result["checksum"] = serde_json::Value::String(format!("{checksum:016x}"));
    result
}

fn degraded_matrix_report(days: u64, step: f64) -> serde_json::Value {
    let control = simulate(&SCENARIOS[1], days, step);
    let variants: Vec<serde_json::Value> = Degraded::ALL
        .iter()
        .map(|degraded| simulate_with(&SCENARIOS[1], days, step, Some(*degraded)))
        .collect();
    serde_json::json!({
        "scene": "city_loop_degraded_matrix",
        "scenario": "mid",
        "days": days,
        "step_seconds": step,
        "control": control,
        "variants": variants,
    })
}

fn balance_report(scenarios: &[Scenario], days: u64, step: f64) -> serde_json::Value {
    let scenarios: Vec<serde_json::Value> = scenarios
        .iter()
        .map(|scenario| balance_entry(scenario, days, step))
        .collect();
    serde_json::json!({
        "scene": "city_loop_balance",
        "days": days,
        "step_seconds": step,
        "scenarios": scenarios,
    })
}

fn balance_entry(scenario: &Scenario, days: u64, step: f64) -> serde_json::Value {
    let result = simulate(scenario, days, step);
    let population = result["population"].as_u64().expect("population");
    let final_balance = result["balance"].as_f64().expect("balance");
    let bankrupt = result["bankrupt"].as_bool().expect("bankrupt");
    serde_json::json!({
        "scenario": scenario.name,
        "population": population,
        "jobs": result["jobs"],
        "age": Age::from_population(population).name(),
        "daily_net": result["daily_net"],
        "min_balance": result["min_balance"],
        "min_balance_day": result["min_balance_day"],
        "final_balance": final_balance,
        "debt": result["debt"],
        "recovery_day": result["recovery_day"],
        "bankrupt": bankrupt,
        "sustainable": final_balance >= 0.0 && !bankrupt,
        "checksum": result["checksum"],
    })
}

fn even_ids(span: u64, count: u64) -> BTreeSet<u64> {
    if span == 0 || count == 0 {
        return BTreeSet::new();
    }
    if count == 1 {
        return [1].into_iter().collect();
    }
    (0..count)
        .map(|index| 1 + index * (span - 1) / (count - 1))
        .collect()
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

    fn test_scenario(houses: u64, workplaces: u64) -> Scenario {
        Scenario {
            name: "test",
            houses,
            workplaces,
            industry_count: houses.min(10),
            recovery_day: 15.0,
            water_ids: &[],
            power_ids: &[],
        }
    }

    #[test]
    fn a_five_house_city_completes_and_fills() {
        let result = simulate(&test_scenario(5, 5), 1, 60.0);
        assert_eq!(result["population"], 50);
        assert_eq!(
            result["jobs"],
            5 * JOBS_PER_HOUSE as u64 + 5 * JOBS_PER_WORKPLACE as u64
        );
        assert_eq!(result["employed"], 50);
        assert_eq!(result["completed_buildings"], 5 + 5);
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
        assert_eq!(
            result["checksum"],
            simulate(&test_scenario(5, 5), 1, 60.0)["checksum"]
        );
    }

    #[test]
    fn houses_beyond_the_first_row_use_road_commutes() {
        let result = simulate(&test_scenario(15, 5), 1, 60.0);
        assert_eq!(result["population"], 150);
        assert_eq!(result["jobs"], 500);
        assert!(result["average_commute_m"].as_f64().expect("commute") > 10.0);
        assert!(result["traffic_nodes"].as_u64().expect("nodes") > 2);
    }

    #[test]
    fn degraded_variants_are_deterministic_and_short() {
        let no_water = simulate_with(&SCENARIOS[1], 10, 300.0, Some(Degraded::Water));
        let no_water_again = simulate_with(&SCENARIOS[1], 10, 300.0, Some(Degraded::Water));
        assert_eq!(no_water["checksum"], no_water_again["checksum"]);
        assert_eq!(no_water["degraded"], "no_water");
        assert!(no_water["water_supply"].as_f64().expect("water") < 0.01);

        let no_power = simulate_with(&SCENARIOS[1], 10, 300.0, Some(Degraded::Power));
        assert!(no_power["power_supply"].as_f64().expect("power") < 0.01);
        assert!(no_power["food_supply"].as_f64().expect("food") < 0.01);

        let no_industry = simulate_with(&SCENARIOS[1], 10, 300.0, Some(Degraded::Industry));
        assert_eq!(no_industry["industry_sites"], 0);
        assert!(no_industry["food_supply"].as_f64().expect("food") < 0.01);

        let no_roads = simulate_with(&SCENARIOS[1], 10, 300.0, Some(Degraded::Roads));
        assert_eq!(no_roads["traffic_nodes"], 0);
        assert_eq!(no_roads["road_segments"], 0);
        assert!(no_roads["average_commute_m"].as_f64().expect("commute") > 0.0);

        let no_utilities = simulate_with(&SCENARIOS[1], 10, 300.0, Some(Degraded::Utilities));
        assert!(no_utilities["water_supply"].as_f64().expect("water") < 0.01);
        assert!(no_utilities["power_supply"].as_f64().expect("power") < 0.01);
    }

    #[test]
    fn degraded_matrix_has_one_entry_per_variant() {
        let matrix = degraded_matrix_report(2, 3_600.0);
        assert_eq!(matrix["scene"], "city_loop_degraded_matrix");
        assert_eq!(matrix["variants"].as_array().expect("variants").len(), 5);
        let again = degraded_matrix_report(2, 3_600.0);
        assert_eq!(matrix, again);
    }

    #[test]
    fn scenario_presets_have_expected_sizes() {
        let early = SCENARIOS[0];
        let mid = SCENARIOS[1];
        let late = SCENARIOS[2];
        assert_eq!((early.houses, early.workplaces), (10, 1));
        assert_eq!((mid.houses, mid.workplaces), (50, 5));
        assert_eq!((late.houses, late.workplaces), (100, 10));
        assert!(early.recovery_day >= late.recovery_day);
    }

    #[test]
    fn early_scenario_completes_and_fills() {
        let early = SCENARIOS[0];
        let result = simulate(&early, 30, 300.0);
        assert_eq!(result["scenario"], "early");
        assert_eq!(result["population"], 100);
        assert_eq!(result["jobs"], 110);
        assert_eq!(result["employed"], 100);
        assert_eq!(result["completed_buildings"], 11);
        assert_eq!(result["construction_sites"], 0);
        assert_eq!(result["bankrupt"], false);
        assert!(result["balance"].as_f64().expect("balance") > 0.0);
        assert!(result["water_supply"].as_f64().expect("water") > 0.99);
        assert!(result["power_supply"].as_f64().expect("power") > 0.99);
        assert!(result["food_supply"].as_f64().expect("food") > 0.99);
        assert_eq!(result["checksum"], simulate(&early, 30, 300.0)["checksum"]);
    }

    #[test]
    fn balance_report_has_a_stable_shape() {
        let scenarios = [test_scenario(5, 5)];
        let first = balance_report(&scenarios, 1, 60.0);
        let second = balance_report(&scenarios, 1, 60.0);
        assert_eq!(first, second);
        assert_eq!(first["scene"], "city_loop_balance");
        assert_eq!(first["days"], 1);
        assert_eq!(first["step_seconds"], 60.0);

        let entry = &first["scenarios"][0];
        assert_eq!(entry["scenario"], "test");
        assert_eq!(entry["population"], 50);
        assert_eq!(entry["jobs"], 450);
        assert_eq!(entry["age"], "wood");
        assert!(entry["daily_net"].is_f64());
        assert!(entry["min_balance"].is_f64());
        assert!(entry["min_balance_day"].is_f64());
        assert!(entry["final_balance"].is_f64());
        assert!(entry["debt"].is_f64());
        assert_eq!(entry["recovery_day"], 1.0);
        assert_eq!(entry["bankrupt"], false);
        assert_eq!(
            entry["sustainable"],
            entry["final_balance"].as_f64().expect("balance") >= 0.0
                && !entry["bankrupt"].as_bool().expect("bankrupt")
        );
        assert!(entry["checksum"].is_string());
    }
}
