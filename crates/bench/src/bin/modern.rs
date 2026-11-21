#![recursion_limit = "512"]

//! Deterministic 150,000-resident Modern-age balance probe.
//!
//! The Industrial probe stops at 20,000 residents. This probe builds the next
//! milestone: 9,375 tenements (150,000 residents) and 9,375 offices (150,000
//! job slots), with a scaled primary-industry share, utilities, and Modern-age
//! civic services, then runs 30 game days at a fixed step.
//!
//! Construction is not phased. The city is imported as a completed build so the
//! probe measures operating rates (wages, taxes, upkeep, service capacity)
//! rather than a multi-year construction schedule that the current financing
//! model cannot fund anyway. The notional catalog cost is reported separately.
//!
//! Two tax policies run over the same city: the default 8/4 rates and the
//! runner's 20/4 recovery rates, labelled as the high-tax policy here. Both take
//! one maximum working-capital loan.
//!
//! Output is stable JSON with per-system checksums. `--smoke` re-runs the probe
//! twice and fails unless the results are identical and the city is Modern.

use core_city::{
    starter, summarize, Age, AgeState, CityRuntime, ConstructionPhase, ConstructionStore, Economy,
    Home, IndustryKind, IndustryStore, Placement, PlacementPlan, PlacementRequest, PlacementStore,
    ServiceConsumer, ServiceKind, ServiceStore, ServiceTier, UtilityConsumer, UtilityKind,
    UtilityStore, Workplace, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS, FLOOR_HEIGHT_M,
    MAX_LOAN, MAX_MOVE_IN_PER_SECOND, MAX_TOTAL_DEBT, STARTING_BALANCE, WAGE_PER_WORKER_PER_DAY,
};

const SEED: u64 = 2026;
const DEFAULT_DAYS: u64 = 30;
const DEFAULT_STEP_SECONDS: f64 = 3_600.0;
const SECONDS_PER_DAY: f64 = 86_400.0;

/// Tenements from the starter catalog, each with 16 resident slots.
const TENEMENTS: u64 = 9_375;
/// Offices from the starter catalog, each with 16 job slots.
const OFFICES: u64 = 9_375;
const TOTAL_BUILDINGS: u64 = TENEMENTS + OFFICES;
const SOLID_VOXELS: u64 = 10_000;
const MATERIALS_STOCK: u64 = 10_000_000;

/// Building grid. With exactly half homes and half workplaces, the arithmetic
/// progression alternates homes and offices, so every home has a workplace
/// next door and commutes stay local.
const GRID_COLUMNS: u64 = 150;
const GRID_ROWS: u64 = 125;
const GRID_SPACING_M: f32 = 16.0;

/// Primary industry sites, scaled from the Industrial probe. A farm office has
/// 16 workers and produces 12 food per worker per day; 1,875 farm sites and 375
/// hunting sites cover the 150,000/day resident demand with a 2.6x margin.
const FARM_SITES: usize = 1_875;
const LUMBERJACK_SITES: usize = 750;
const HUNT_SITES: usize = 375;

/// Utility sites sized from the 150,000-resident / 150,000-worker demand, with
/// a small headroom margin so floating-point division cannot report 0.999.
const WATER_SITES: usize = 510;
const POWER_SITES: usize = 640;

/// Modern/Advanced civic service sites. Counts are complete grids so every
/// building is inside each radius: 7x7 schools, 14x14 clinics, 12x12 fire and
/// police.
const SCHOOL_SITES: usize = 49;
const CLINIC_SITES: usize = 196;
const FIRE_SITES: usize = 144;
const POLICE_SITES: usize = 144;

/// Service coverage is infrastructure, not a per-tick quantity. Refreshing once
/// per game day keeps the probe under the query budget without changing the
/// steady-state result.
const SERVICE_REFRESH_STEPS: u64 = 24;

/// The recovery rate the existing runner uses after its construction dip.
const HIGH_TAX_PER_RESIDENT: f64 = 20.0;
const HIGH_TAX_PER_JOB: f64 = 4.0;

/// One maximum loan as working capital. This is 0.015% of the notional catalog
/// cost of the city and cannot finance construction.
const WORKING_CAPITAL_LOAN: f64 = MAX_LOAN;

/// Tax policy for one run of the same city.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaxPolicy {
    Default,
    High,
}

impl TaxPolicy {
    const ALL: [Self; 2] = [Self::Default, Self::High];

    fn name(self) -> &'static str {
        match self {
            Self::Default => "default_tax",
            Self::High => "high_tax",
        }
    }

    fn rates(self) -> (f64, f64) {
        match self {
            Self::Default => (8.0, 4.0),
            Self::High => (HIGH_TAX_PER_RESIDENT, HIGH_TAX_PER_JOB),
        }
    }

    fn recovery_day(self) -> Option<f64> {
        match self {
            Self::Default => None,
            // The high-tax policy is in force from the first tick.
            Self::High => Some(0.0),
        }
    }
}

fn main() {
    let days = arg_value("--days")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DAYS);
    let step = arg_value("--step")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(DEFAULT_STEP_SECONDS);
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    if days == 0 || !step.is_finite() || step <= 0.0 {
        eprintln!("usage: modern [--days N] [--step SECONDS] [--smoke]");
        std::process::exit(2);
    }
    if step > core_city::MAX_POPULATION_TICK_SECONDS {
        eprintln!(
            "modern: step must be at most {} seconds",
            core_city::MAX_POPULATION_TICK_SECONDS
        );
        std::process::exit(2);
    }

    if smoke {
        run_smoke(days, step);
        return;
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&report(days, step)).expect("pretty json")
    );
}

fn run_smoke(days: u64, step: f64) {
    let first = report(days, step);
    let second = report(days, step);
    let scenario = &first["scenarios"][1];
    let age = scenario["age"].as_str().unwrap_or("missing");
    let population = scenario["population"].as_u64().unwrap_or(0);
    let default_bankrupt = first["scenarios"][0]["bankrupt"].as_bool().unwrap_or(false);
    let high_bankrupt = scenario["bankrupt"].as_bool().unwrap_or(true);
    let checksum = scenario["checksum"].as_str().unwrap_or("missing");
    let deterministic = first == second;
    let ok = deterministic && age == "modern" && population == 150_000 && !high_bankrupt;
    println!(
        "MODERN_SMOKE age={age} population={population} \
         default_bankrupt={default_bankrupt} high_bankrupt={high_bankrupt} \
         deterministic={deterministic} checksum={checksum} status={}",
        if ok { "ok" } else { "failed" }
    );
    if !ok {
        std::process::exit(1);
    }
}

fn report(days: u64, step: f64) -> serde_json::Value {
    let scenarios: Vec<serde_json::Value> = TaxPolicy::ALL
        .iter()
        .map(|policy| simulate(*policy, days, step))
        .collect();
    let steps = step_count(days, step);
    serde_json::json!({
        "scene": "phase4_modern_balance",
        "format_version": 1,
        "seed": SEED,
        "days": days,
        "step_seconds": step,
        "steps": steps,
        "service_refresh_steps": SERVICE_REFRESH_STEPS,
        "city": city_meta(),
        "scenarios": scenarios,
    })
}

fn simulate(policy: TaxPolicy, days: u64, step: f64) -> serde_json::Value {
    let tenement = starter("base:tenement").expect("tenement starter");
    let office = starter("base:office").expect("office starter");

    let (placements, construction, office_ids) = build_city();
    debug_assert_eq!(
        placements.placements.len() as u64,
        TOTAL_BUILDINGS,
        "all buildings are placed"
    );
    debug_assert_eq!(office_ids.len() as u64, OFFICES, "office count");

    let homes: Vec<Home> = placements
        .placements
        .iter()
        .filter(|placement| placement.resident_capacity > 0)
        .map(|placement| Home {
            id: placement.id,
            capacity: placement.resident_capacity,
            center: [placement.center[0], placement.center[2]],
        })
        .collect();
    let workplaces: Vec<Workplace> = placements
        .placements
        .iter()
        .filter(|placement| placement.job_slots > 0)
        .map(|placement| Workplace {
            id: placement.id,
            slots: placement.job_slots,
            center: [placement.center[0], placement.center[2]],
        })
        .collect();
    let jobs: u64 = workplaces.iter().map(|work| u64::from(work.slots)).sum();
    let completed_buildings = placements.placements.len() as u64;

    let mut industry = IndustryStore::new(SEED);
    for (index, &id) in office_ids
        .iter()
        .take(FARM_SITES + LUMBERJACK_SITES + HUNT_SITES)
        .enumerate()
    {
        let kind = if index < FARM_SITES {
            IndustryKind::Farm
        } else if index < FARM_SITES + LUMBERJACK_SITES {
            IndustryKind::Lumberjack
        } else {
            IndustryKind::Hunt
        };
        industry.add_site(id, kind).expect("industry site");
    }

    let mut utilities = UtilityStore::new(SEED);
    add_utilities(&mut utilities);
    let mut services = ServiceStore::new(SEED);
    add_services(&mut services);

    let mut runtime = CityRuntime::new(SEED);
    runtime.set_move_in_per_second(MAX_MOVE_IN_PER_SECOND);

    let mut economy = Economy::new(SEED);
    economy
        .take_loan(
            WORKING_CAPITAL_LOAN,
            DEFAULT_LOAN_RATE_PER_DAY,
            DEFAULT_LOAN_TERM_DAYS,
        )
        .expect("working-capital loan");
    let (resident_tax, job_tax) = policy.rates();
    economy.set_taxes(resident_tax, job_tax);

    let mut utility_report = utilities.refresh(&utility_consumers(&placements, &runtime));
    let mut service_report = services.refresh(&service_consumers(&placements, &runtime));
    let mut min_balance = economy.balance;
    let mut min_balance_day = 0.0;
    let mut bankrupt_day: Option<f64> = None;
    let mut food_consumed_total = 0.0_f64;
    let steps = step_count(days, step);

    for step_index in 0..steps {
        if step_index % SERVICE_REFRESH_STEPS == 0 {
            utility_report = utilities.refresh(&utility_consumers(&placements, &runtime));
            service_report = services.refresh(&service_consumers(&placements, &runtime));
        }
        let industry_tick = industry.tick_with_power(
            step,
            &runtime.staffing,
            runtime.population(),
            utilities.power_supply,
        );
        food_consumed_total += industry_tick.food_consumed;
        runtime.tick_with_supplies(
            step,
            &homes,
            &workplaces,
            industry.food_supply,
            utilities.water_supply,
            utilities.power_supply,
        );
        economy.tick(
            step,
            runtime.population(),
            runtime.workers().min(jobs),
            completed_buildings,
        );
        if economy.balance < min_balance {
            min_balance = economy.balance;
            min_balance_day = economy.days_elapsed;
        }
        if economy.bankrupt && bankrupt_day.is_none() {
            bankrupt_day = Some(economy.days_elapsed);
        }
    }

    let summary = summarize(&placements, &construction, &runtime, 0);
    let age = AgeState::from_population(summary.population);
    let employed = summary.employed;
    let population = summary.population;
    let wages_per_day = employed as f64 * WAGE_PER_WORKER_PER_DAY;
    let upkeep_per_day = completed_buildings as f64 * economy.upkeep_per_building_per_day;
    let loan_service_per_day = economy.loan_interest_per_day() + economy.loan_payment_per_day();
    let daily_net_at = |resident: f64, job: f64| {
        population as f64 * resident + employed as f64 * job
            - wages_per_day
            - upkeep_per_day
            - loan_service_per_day
    };

    let notional_cost =
        TENEMENTS as f64 * tenement.material_cost + OFFICES as f64 * office.material_cost;
    let result = serde_json::json!({
        "scene": "modern_balance",
        "scenario": policy.name(),
        "tax_policy": policy.name(),
        "recovery_day": policy.recovery_day(),
        "seed": SEED,
        "days": days,
        "step_seconds": step,
        "steps": steps,
        "population": population,
        "population_target": TENEMENTS * u64::from(tenement.resident_capacity),
        "age": age.age.name(),
        "age_index": Age::ALL.iter().position(|candidate| *candidate == age.age).unwrap_or(0),
        "next_age": age.next_age.map(Age::name),
        "next_threshold": age.next_threshold,
        "age_progress": age.progress,
        "housing_capacity": summary.housing_capacity,
        "vacancies": summary.vacancies,
        "jobs": summary.jobs,
        "employed": employed,
        "unemployed": summary.unemployed,
        "happiness": summary.happiness,
        "food": summary.food,
        "water": summary.water,
        "rest": summary.rest,
        "average_commute_m": summary.average_commute_m,
        "daily_wages": summary.daily_wages,
        "completed_buildings": summary.completed_buildings,
        "construction_sites": summary.construction_sites,
        "construction_spent": economy.construction_spent,
        "notional_construction_cost": notional_cost,
        "tax_per_resident_per_day": economy.tax_per_resident_per_day,
        "business_tax_per_job_per_day": economy.business_tax_per_job_per_day,
        "upkeep_per_building_per_day": economy.upkeep_per_building_per_day,
        "resident_tax_per_day": population as f64 * economy.tax_per_resident_per_day,
        "business_tax_per_day": employed as f64 * economy.business_tax_per_job_per_day,
        "wages_per_day": wages_per_day,
        "upkeep_per_day": upkeep_per_day,
        "loan_service_per_day": loan_service_per_day,
        "daily_net": economy.daily_net(population, employed, completed_buildings),
        "daily_net_at_default_taxes": daily_net_at(8.0, 4.0),
        "daily_net_at_high_taxes": daily_net_at(HIGH_TAX_PER_RESIDENT, HIGH_TAX_PER_JOB),
        "balance": economy.balance,
        "min_balance": min_balance,
        "min_balance_day": min_balance_day,
        "debt": economy.debt(),
        "loan_count": economy.loans.len(),
        "loan_principal": WORKING_CAPITAL_LOAN,
        "loan_payment_per_day": economy.loan_payment_per_day(),
        "loan_interest_per_day": economy.loan_interest_per_day(),
        "bankrupt": economy.bankrupt,
        "bankrupt_day": bankrupt_day,
        "bankrupt_days": bankrupt_day.map_or(0.0, |day| (days as f64 - day).max(0.0)),
        "industry_sites": industry.site_count(),
        "farm_sites": FARM_SITES,
        "lumberjack_sites": LUMBERJACK_SITES,
        "hunt_sites": HUNT_SITES,
        "stock_wood": industry.stockpile.wood,
        "stock_food": industry.stockpile.food,
        "stock_hides": industry.stockpile.hides,
        "produced_wood": industry.produced_total.wood,
        "produced_food": industry.produced_total.food,
        "produced_hides": industry.produced_total.hides,
        "food_consumed": food_consumed_total,
        "food_supply": industry.food_supply,
        "water_sites": utilities.water_sites.len(),
        "power_sites": utilities.power_sites.len(),
        "water_supply": utilities.water_supply,
        "power_supply": utilities.power_supply,
        "water_demand": utility_report.water_demand,
        "water_capacity": utility_report.water_capacity,
        "water_served": utility_report.water_served,
        "water_coverage": utility_report.water_coverage,
        "power_demand": utility_report.power_demand,
        "power_capacity": utility_report.power_capacity,
        "power_served": utility_report.power_served,
        "power_coverage": utility_report.power_coverage,
        "service_sites": services.site_count(),
        "school_sites": services.count_of(ServiceKind::School),
        "clinic_sites": services.count_of(ServiceKind::Clinic),
        "fire_sites": services.count_of(ServiceKind::Fire),
        "police_sites": services.count_of(ServiceKind::Police),
        "school_demand": service_report.school_demand,
        "school_capacity": service_report.school_capacity,
        "school_supply": service_report.school_supply,
        "school_coverage": service_report.school_coverage,
        "clinic_demand": service_report.clinic_demand,
        "clinic_capacity": service_report.clinic_capacity,
        "clinic_supply": service_report.clinic_supply,
        "clinic_coverage": service_report.clinic_coverage,
        "fire_demand": service_report.fire_demand,
        "fire_capacity": service_report.fire_capacity,
        "fire_supply": service_report.fire_supply,
        "fire_coverage": service_report.fire_coverage,
        "police_demand": service_report.police_demand,
        "police_capacity": service_report.police_capacity,
        "police_supply": service_report.police_supply,
        "police_coverage": service_report.police_coverage,
        "research_per_day": service_report.research_per_day,
        "max_financing": STARTING_BALANCE + MAX_TOTAL_DEBT,
        "financing_gap": notional_cost - (STARTING_BALANCE + MAX_TOTAL_DEBT),
        "placements_checksum": format!("{:016x}", placements.checksum()),
        "population_checksum": format!("{:016x}", runtime.checksum()),
        "construction_checksum": format!("{:016x}", construction.checksum()),
        "industry_checksum": format!("{:016x}", industry.checksum()),
        "utility_checksum": format!("{:016x}", utilities.checksum()),
        "service_checksum": format!("{:016x}", services.checksum()),
        "economy_checksum": format!("{:016x}", economy.checksum()),
    });
    let checksum = checksum_json(&result);
    let mut result = result;
    result["checksum"] = serde_json::Value::String(format!("{checksum:016x}"));
    result
}

fn build_city() -> (PlacementStore, ConstructionStore, Vec<u64>) {
    let tenement = starter("base:tenement").expect("tenement starter");
    let office = starter("base:office").expect("office starter");
    let mut placements = PlacementStore::new(SEED);
    let mut construction = ConstructionStore::new(1, MATERIALS_STOCK);
    let mut office_ids = Vec::with_capacity(OFFICES as usize);
    let total = TOTAL_BUILDINGS;
    for cell in 0..total {
        let column = cell % GRID_COLUMNS;
        let row = cell / GRID_COLUMNS;
        let x = column as f32 * GRID_SPACING_M;
        let z = row as f32 * GRID_SPACING_M;
        // Exactly TENEMENTS of the TOTAL_BUILDINGS cells are homes. With the
        // two counts equal, homes and offices alternate across the grid.
        let is_home = (cell * TENEMENTS) % total < TENEMENTS;
        let id = cell + 1;
        let placement = if is_home {
            building_placement(id, tenement, x, z)
        } else {
            office_ids.push(id);
            building_placement(id, office, x, z)
        };
        placements.placements.push(placement);
        construction.add_site(id, SOLID_VOXELS).expect("site");
        construction.sites.get_mut(&id).expect("site").phase = ConstructionPhase::Complete;
    }
    placements.next_id = total + 1;
    debug_assert_eq!(
        placements
            .placements
            .iter()
            .filter(|placement| placement.resident_capacity > 0)
            .count() as u64,
        TENEMENTS
    );
    (placements, construction, office_ids)
}

fn building_placement(id: u64, starter: &core_city::StarterBuilding, x: f32, z: f32) -> Placement {
    let request = PlacementRequest {
        blueprint_id: starter.id.to_string(),
        blueprint_hash: format!("blake3:{}", starter.id),
        blueprint_path: format!("/starters/{}.tsbp", starter.id),
        name: starter.name.to_string(),
        bounds_m: [
            starter.footprint_m[0],
            f32::from(starter.stories) * FLOOR_HEIGHT_M,
            starter.footprint_m[1],
        ],
        center_x: x,
        center_z: z,
        rotation_deg: 0,
        material_cost: starter.material_cost,
        resident_capacity: starter.resident_capacity,
        job_slots: starter.job_slots,
        solid_voxels: SOLID_VOXELS,
        structure: None,
    };
    let plan = PlacementPlan {
        center: [x, 0.0, z],
        rotation_deg: 0,
        footprint_m: starter.footprint_m,
        foundation_m: 0.0,
    };
    Placement::from_request(id, &request, plan)
}

fn add_utilities(utilities: &mut UtilityStore) {
    let bounds = city_bounds();
    for (index, center) in grid_sites(WATER_SITES, bounds).into_iter().enumerate() {
        utilities
            .add_site(
                1_000_000 + index as u64,
                UtilityKind::Water,
                [center[0], 0.0, center[1]],
            )
            .expect("water site");
    }
    for (index, center) in grid_sites(POWER_SITES, bounds).into_iter().enumerate() {
        utilities
            .add_site(
                1_100_000 + index as u64,
                UtilityKind::Power,
                [center[0], 0.0, center[1]],
            )
            .expect("power site");
    }
}

fn add_services(services: &mut ServiceStore) {
    let bounds = city_bounds();
    let tier = ServiceTier::from_age(Age::Modern);
    for (kind, count, id_base) in [
        (ServiceKind::School, SCHOOL_SITES, 2_000_000_u64),
        (ServiceKind::Clinic, CLINIC_SITES, 2_100_000),
        (ServiceKind::Fire, FIRE_SITES, 2_200_000),
        (ServiceKind::Police, POLICE_SITES, 2_300_000),
    ] {
        for (index, center) in grid_sites(count, bounds).into_iter().enumerate() {
            services
                .add_site(
                    id_base + index as u64,
                    kind,
                    tier,
                    [center[0], 0.0, center[1]],
                )
                .expect("service site");
        }
    }
}

/// Evenly spaced site positions spanning the city, one grid per service kind.
fn grid_sites(count: usize, bounds: [f32; 4]) -> Vec<[f32; 2]> {
    if count == 0 {
        return Vec::new();
    }
    let columns = (count as f64).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let width = bounds[2] - bounds[0];
    let depth = bounds[3] - bounds[1];
    let mut positions = Vec::with_capacity(count);
    for index in 0..count {
        let column = index % columns;
        let row = index / columns;
        let x = bounds[0] + (column as f32 + 0.5) * width / columns as f32;
        let z = bounds[1] + (row as f32 + 0.5) * depth / rows as f32;
        positions.push([x, z]);
    }
    positions
}

fn city_bounds() -> [f32; 4] {
    [
        -GRID_SPACING_M,
        -GRID_SPACING_M,
        (GRID_COLUMNS - 1) as f32 * GRID_SPACING_M + GRID_SPACING_M,
        (GRID_ROWS - 1) as f32 * GRID_SPACING_M + GRID_SPACING_M,
    ]
}

fn utility_consumers(placements: &PlacementStore, runtime: &CityRuntime) -> Vec<UtilityConsumer> {
    placements
        .placements
        .iter()
        .map(|placement| UtilityConsumer {
            id: placement.id,
            center: [placement.center[0], placement.center[2]],
            residents: runtime
                .households
                .get(&placement.id)
                .map_or(0, |household| household.residents),
            workers: runtime.staffing.get(&placement.id).copied().unwrap_or(0),
        })
        .collect()
}

fn service_consumers(placements: &PlacementStore, runtime: &CityRuntime) -> Vec<ServiceConsumer> {
    placements
        .placements
        .iter()
        .map(|placement| ServiceConsumer {
            id: placement.id,
            center: [placement.center[0], placement.center[2]],
            residents: runtime
                .households
                .get(&placement.id)
                .map_or(0, |household| household.residents),
            workers: runtime.staffing.get(&placement.id).copied().unwrap_or(0),
        })
        .collect()
}

fn city_meta() -> serde_json::Value {
    let tenement = starter("base:tenement").expect("tenement starter");
    let office = starter("base:office").expect("office starter");
    let population_target = TENEMENTS * u64::from(tenement.resident_capacity);
    let jobs = OFFICES * u64::from(office.job_slots);
    let notional_cost =
        TENEMENTS as f64 * tenement.material_cost + OFFICES as f64 * office.material_cost;
    let max_financing = STARTING_BALANCE + MAX_TOTAL_DEBT;
    serde_json::json!({
        "population_target": population_target,
        "tenements": TENEMENTS,
        "tenement_capacity": tenement.resident_capacity,
        "tenement_material_cost": tenement.material_cost,
        "offices": OFFICES,
        "office_jobs": office.job_slots,
        "office_material_cost": office.material_cost,
        "jobs": jobs,
        "farm_sites": FARM_SITES,
        "lumberjack_sites": LUMBERJACK_SITES,
        "hunt_sites": HUNT_SITES,
        "water_sites": WATER_SITES,
        "power_sites": POWER_SITES,
        "school_sites": SCHOOL_SITES,
        "clinic_sites": CLINIC_SITES,
        "fire_sites": FIRE_SITES,
        "police_sites": POLICE_SITES,
        "grid_columns": GRID_COLUMNS,
        "grid_rows": GRID_ROWS,
        "grid_spacing_m": GRID_SPACING_M,
        "working_capital_loan": WORKING_CAPITAL_LOAN,
        "starting_balance": STARTING_BALANCE,
        "max_total_debt": MAX_TOTAL_DEBT,
        "max_financing": max_financing,
        "notional_construction_cost": notional_cost,
        "financing_gap": notional_cost - max_financing,
        "notional_cost_per_resident": notional_cost / population_target as f64,
        "max_financing_per_resident": max_financing / population_target as f64,
        "service_refresh_steps": SERVICE_REFRESH_STEPS,
        "roads": "omitted: per-workplace flow fields do not scale to 9,375 workplaces",
    })
}

fn step_count(days: u64, step: f64) -> u64 {
    (days as f64 * SECONDS_PER_DAY / step).ceil() as u64
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
    fn modern_city_is_deterministic_and_modern() {
        let first = simulate(TaxPolicy::High, 1, 3_600.0);
        let second = simulate(TaxPolicy::High, 1, 3_600.0);
        assert_eq!(first, second, "same input must produce the same report");
        assert_eq!(first["population"], 150_000);
        assert_eq!(first["housing_capacity"], 150_000);
        assert_eq!(first["jobs"], 150_000);
        assert_eq!(first["employed"], 150_000);
        assert_eq!(first["age"], "modern");
        assert_eq!(first["age_index"], 3);
        assert_eq!(first["next_age"], "information");
        assert_eq!(first["next_threshold"], 750_000);
        assert_eq!(first["bankrupt"], false);
        assert_eq!(first["food_supply"], 1.0);
        assert_eq!(first["water_supply"], 1.0);
        assert_eq!(first["power_supply"], 1.0);
        assert_eq!(first["school_supply"], 1.0);
        assert_eq!(first["clinic_supply"], 1.0);
        assert_eq!(first["fire_supply"], 1.0);
        assert_eq!(first["police_supply"], 1.0);
        assert!(first["checksum"].as_str().is_some());
    }

    #[test]
    fn default_taxes_run_a_structural_deficit() {
        let result = simulate(TaxPolicy::Default, 1, 3_600.0);
        assert_eq!(result["population"], 150_000);
        assert_eq!(result["age"], "modern");
        assert!(result["daily_net"].as_f64().expect("daily net") < -1_200_000.0);
        assert!(result["daily_wages"].as_f64().expect("wages") > 2_990_000.0);
        assert!(result["balance"].as_f64().expect("balance") < -900_000.0);
    }

    #[test]
    fn high_taxes_run_a_surplus() {
        let result = simulate(TaxPolicy::High, 1, 3_600.0);
        assert!(result["daily_net"].as_f64().expect("daily net") > 500_000.0);
        assert!(result["balance"].as_f64().expect("balance") > 800_000.0);
        assert_eq!(result["bankrupt"], false);
    }

    #[test]
    fn site_counts_cover_the_sized_demand() {
        let result = simulate(TaxPolicy::High, 1, 3_600.0);
        assert!(
            result["water_capacity"].as_f64().expect("water")
                >= result["water_demand"].as_f64().expect("water demand")
        );
        assert!(
            result["power_capacity"].as_f64().expect("power")
                >= result["power_demand"].as_f64().expect("power demand")
        );
        assert!(
            result["school_capacity"].as_f64().expect("school")
                >= result["school_demand"].as_f64().expect("school demand")
        );
        assert!(
            result["clinic_capacity"].as_f64().expect("clinic")
                >= result["clinic_demand"].as_f64().expect("clinic demand")
        );
        assert!(
            result["fire_capacity"].as_f64().expect("fire")
                >= result["fire_demand"].as_f64().expect("fire demand")
        );
        assert!(
            result["police_capacity"].as_f64().expect("police")
                >= result["police_demand"].as_f64().expect("police demand")
        );
    }

    #[test]
    fn report_has_a_stable_shape() {
        let first = report(1, 3_600.0);
        let second = report(1, 3_600.0);
        assert_eq!(first, second);
        assert_eq!(first["scene"], "phase4_modern_balance");
        assert_eq!(first["days"], 1);
        assert_eq!(first["step_seconds"], 3_600.0);
        assert_eq!(first["steps"], 24);
        assert_eq!(first["scenarios"].as_array().expect("scenarios").len(), 2);
        assert_eq!(first["city"]["population_target"], 150_000);
        assert!(first["city"]["financing_gap"].as_f64().expect("gap") > 0.0);
        assert!(first["scenarios"][0]["checksum"].is_string());
        assert!(first["scenarios"][1]["checksum"].is_string());
    }
}
