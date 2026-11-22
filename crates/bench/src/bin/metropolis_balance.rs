//! Deterministic 1,000,000-resident operating-ledger probe.
//!
//! This complements the 20,000-resident Industrial probe and the 150,000-
//! resident Modern probe. It is **ledger-only**: the city is a fixed operating
//! fixture, not a simulated household/buildings/agent run. There is no
//! move-in, job assignment, construction, service-quality, congestion, traffic,
//! or render step. Population, employment, and completed buildings are pinned
//! constants so the probe isolates the treasury.
//!
//! Fixture arithmetic:
//!
//! - 250,000 buildings = **200,000 homes × 5 residents** (1,000,000 population)
//!   + **50,000 workplaces × 8 jobs** (400,000 job slots).
//! - 1,000 utility sites (500 water, 500 power) and 400 civic service sites
//!   (100 each school/clinic/fire/police) at the Advanced tier.
//! - Utility and service capacities and radii are scaled to the 1M fixture and
//!   documented as abstract; only their real per-kind/per-tier operating
//!   upkeep enters the ledger.
//!
//! An earlier brief described 5/6 homes × 16 capacity and 1/6 workplaces × 12
//! jobs. At 250,000 buildings those ratios are 3,333,333 residents and 500,000
//! jobs, so this probe uses the exact 1,000,000/400,000 fixture above instead.
//!
//! Three fixed tax policies run over the same city for 30 daily ticks:
//! low 8/4, default 20/4, and high 30/6 (resident/job per day). There are no
//! loans and no construction cost. Output is stable JSON with an FNV checksum.
//! `--smoke` runs the probe twice and fails unless the results match.

#![recursion_limit = "512"]

use core_city::{
    Age, AgeState, Economy, InfrastructureCosts, ServiceKind, ServiceStore, ServiceTier,
    UtilityKind, UtilityStore, DAY_SECONDS, STARTING_BALANCE, WAGE_PER_WORKER_PER_DAY,
};

const SEED: u64 = 2026;
const DEFAULT_DAYS: u64 = 30;

/// Fixed operating population and employment.
const POPULATION: u64 = 1_000_000;
const EMPLOYED: u64 = 400_000;
const BUILDINGS: u64 = 250_000;

/// Housing: 200,000 × 5 = 1,000,000 residents.
const HOMES: u64 = 200_000;
const HOME_CAPACITY: u64 = 5;
/// Workplaces: 50,000 × 8 = 400,000 jobs.
const WORKPLACES: u64 = 50_000;
const JOBS_PER_WORKPLACE: u64 = 8;
const JOBS: u64 = WORKPLACES * JOBS_PER_WORKPLACE;

/// Utility sites: one site per 1,000 residents, split evenly by kind.
const WATER_SITES: usize = 500;
const POWER_SITES: usize = 500;
const UTILITY_SITES: usize = WATER_SITES + POWER_SITES;
/// Capacity meeting the static fixture demand exactly:
/// water = 1,000,000 residents; power = 400,000 workers + 250,000 resident load.
const WATER_CAPACITY_PER_SITE: f64 = 2_000.0;
const POWER_CAPACITY_PER_SITE: f64 = 1_300.0;
const UTILITY_RADIUS_M: f32 = 400.0;

/// Civic sites: one site per 2,500 residents, split evenly by kind.
const SCHOOL_SITES: usize = 100;
const CLINIC_SITES: usize = 100;
const FIRE_SITES: usize = 100;
const POLICE_SITES: usize = 100;
const SERVICE_SITES: usize = SCHOOL_SITES + CLINIC_SITES + FIRE_SITES + POLICE_SITES;
/// Advanced-tier capacity meeting the static fixture demand exactly:
/// school = 250,000 residents; clinic/fire/police = 1,000,000 residents +
/// 0.5 × 400,000 workers = 1,200,000.
const SCHOOL_CAPACITY_PER_SITE: f64 = 2_500.0;
const CIVIC_CAPACITY_PER_SITE: f64 = 12_000.0;
const SERVICE_RADIUS_M: f32 = 600.0;

/// Building grid used only to spread abstract utility/service site centres.
const GRID_COLUMNS: u64 = 500;
const GRID_ROWS: u64 = 500;
const GRID_SPACING_M: f32 = 16.0;

/// The three fixed tax policies, in report order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaxPolicy {
    Low,
    Default,
    High,
}

impl TaxPolicy {
    const ALL: [Self; 3] = [Self::Low, Self::Default, Self::High];

    fn name(self) -> &'static str {
        match self {
            Self::Low => "low_tax",
            Self::Default => "default_tax",
            Self::High => "high_tax",
        }
    }

    fn rates(self) -> (f64, f64) {
        match self {
            Self::Low => (8.0, 4.0),
            Self::Default => (20.0, 4.0),
            Self::High => (30.0, 6.0),
        }
    }
}

fn main() {
    let days = arg_value("--days")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DAYS);
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    if days == 0 {
        eprintln!("usage: metropolis_balance [--days N] [--smoke]");
        std::process::exit(2);
    }

    if smoke {
        run_smoke(days);
        return;
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&report(days)).expect("pretty json")
    );
}

fn run_smoke(days: u64) {
    let first = report(days);
    let second = report(days);
    let deterministic = first == second;
    let policies = first["policies"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = policies
        .iter()
        .filter_map(|policy| policy["policy"].as_str())
        .collect();
    let order_ok = names == ["low_tax", "default_tax", "high_tax"];
    let solvent = policies
        .iter()
        .all(|policy| policy["bankrupt"].as_bool() == Some(false));
    let population = first["population"].as_u64().unwrap_or(0);
    let buildings = first["buildings"].as_u64().unwrap_or(0);
    let jobs = first["jobs"].as_u64().unwrap_or(0);
    let checksum = first["checksum"].as_str().unwrap_or("missing");
    let ok = deterministic
        && order_ok
        && solvent
        && population == POPULATION
        && buildings == BUILDINGS
        && jobs == JOBS;
    println!(
        "METROPOLIS_BALANCE_SMOKE population={population} buildings={buildings} jobs={jobs} \
         policies={} checksum={checksum} deterministic={deterministic} status={}",
        policies.len(),
        if ok { "ok" } else { "failed" }
    );
    if !ok {
        std::process::exit(1);
    }
}

fn report(days: u64) -> serde_json::Value {
    let utilities = build_utilities();
    let services = build_services();
    let costs = InfrastructureCosts::new(
        utilities.total_upkeep_per_day(),
        services.total_upkeep_per_day(),
    )
    .expect("utility and service upkeep are finite and non-negative");
    let age = AgeState::from_population(POPULATION);
    let policies: Vec<serde_json::Value> = TaxPolicy::ALL
        .iter()
        .enumerate()
        .map(|(index, policy)| simulate(*policy, index, days, costs))
        .collect();

    let wages_per_day = EMPLOYED as f64 * WAGE_PER_WORKER_PER_DAY;
    let building_upkeep_per_day = BUILDINGS as f64 * core_city::DEFAULT_UPKEEP_PER_BUILDING_PER_DAY;
    let infrastructure_per_day = costs.total_per_day();
    let mut result = serde_json::json!({
        "scene": "metropolis_balance",
        "format_version": 1,
        "kind": "operating_ledger",
        "render": false,
        "simulation": "static population and employment; no household assignment",
        "note": "Ledger-only operating probe: no construction, move-in, job assignment, service quality, congestion, traffic, or render. Utility/service capacities are abstract scaled fixture data.",
        "fixture_note": "250,000 buildings = 200,000 homes x 5 residents (1,000,000 population) + 50,000 workplaces x 8 jobs (400,000 jobs). An earlier 5/6 homes x 16 / 1/6 workplaces x 12 brief would be 3,333,333 residents and 500,000 jobs at this building count, so the exact 1M/400k fixture is used.",
        "seed": SEED,
        "days": days,
        "step_seconds": DAY_SECONDS,
        "population": POPULATION,
        "housing_capacity": POPULATION,
        "vacancies": 0,
        "homes": HOMES,
        "home_capacity": HOME_CAPACITY,
        "workplaces": WORKPLACES,
        "jobs_per_workplace": JOBS_PER_WORKPLACE,
        "jobs": JOBS,
        "employed": EMPLOYED,
        "unemployed": 0,
        "buildings": BUILDINGS,
        "age": age.age.name(),
        "age_index": Age::ALL
            .iter()
            .position(|candidate| *candidate == age.age)
            .unwrap_or(0),
        "next_age": age.next_age.map(Age::name),
        "next_threshold": age.next_threshold,
        "age_progress": age.progress,
        "utility_sites": UTILITY_SITES,
        "water_sites": WATER_SITES,
        "power_sites": POWER_SITES,
        "water_capacity": WATER_SITES as f64 * WATER_CAPACITY_PER_SITE,
        "power_capacity": POWER_SITES as f64 * POWER_CAPACITY_PER_SITE,
        "water_demand": POPULATION as f64,
        "power_demand": EMPLOYED as f64 + POPULATION as f64 * 0.25,
        "utility_radius_m": UTILITY_RADIUS_M,
        "service_sites": SERVICE_SITES,
        "service_tier": ServiceTier::Advanced.as_str(),
        "school_sites": SCHOOL_SITES,
        "clinic_sites": CLINIC_SITES,
        "fire_sites": FIRE_SITES,
        "police_sites": POLICE_SITES,
        "school_capacity": SCHOOL_SITES as f64 * SCHOOL_CAPACITY_PER_SITE,
        "clinic_capacity": CLINIC_SITES as f64 * CIVIC_CAPACITY_PER_SITE,
        "fire_capacity": FIRE_SITES as f64 * CIVIC_CAPACITY_PER_SITE,
        "police_capacity": POLICE_SITES as f64 * CIVIC_CAPACITY_PER_SITE,
        "school_demand": POPULATION as f64 * 0.25,
        "civic_demand": POPULATION as f64 + EMPLOYED as f64 * 0.5,
        "service_radius_m": SERVICE_RADIUS_M,
        "wages_per_day": wages_per_day,
        "building_upkeep_per_day": building_upkeep_per_day,
        "infrastructure_per_day": infrastructure_per_day,
        "operating_expenses_per_day": wages_per_day + building_upkeep_per_day + infrastructure_per_day,
        "wages_per_job_per_day": WAGE_PER_WORKER_PER_DAY,
        "starting_balance": STARTING_BALANCE,
        "loans": false,
        "policies": policies,
        "utility_checksum": format!("{:016x}", utilities.checksum()),
        "service_checksum": format!("{:016x}", services.checksum()),
        "status": "ok",
    });
    let checksum = checksum_json(&result);
    result["checksum"] = serde_json::Value::String(format!("{checksum:016x}"));
    result
}

fn simulate(
    policy: TaxPolicy,
    index: usize,
    days: u64,
    costs: InfrastructureCosts,
) -> serde_json::Value {
    let mut economy = Economy::new(SEED);
    let (resident_tax, job_tax) = policy.rates();
    economy.set_taxes(resident_tax, job_tax);

    let mut daily = Vec::with_capacity(days as usize);
    let mut min_balance = economy.balance;
    let mut min_balance_day = 0.0;
    let mut bankrupt_day: Option<f64> = None;
    let mut last_income = 0.0;
    let mut last_expenses = 0.0;

    for day in 1..=days {
        let tick = economy.tick_with_infrastructure_costs(
            DAY_SECONDS,
            POPULATION,
            EMPLOYED,
            BUILDINGS,
            costs,
        );
        last_income = tick.income;
        last_expenses = tick.expenses;
        if tick.balance < min_balance {
            min_balance = tick.balance;
            min_balance_day = tick.days_elapsed;
        }
        if tick.bankrupt && bankrupt_day.is_none() {
            bankrupt_day = Some(tick.days_elapsed);
        }
        daily.push(serde_json::json!({
            "day": day,
            "income": tick.income,
            "expenses": tick.expenses,
            "net": tick.income - tick.expenses,
            "balance": tick.balance,
            "infrastructure": tick.infrastructure,
            "bankrupt": tick.bankrupt,
        }));
    }

    let daily_net = last_income - last_expenses;
    let population = POPULATION as f64;
    let jobs = JOBS as f64;
    serde_json::json!({
        "policy": policy.name(),
        "policy_index": index,
        "resident_tax_per_resident_per_day": resident_tax,
        "business_tax_per_job_per_day": job_tax,
        "income_per_day": last_income,
        "expenses_per_day": last_expenses,
        "daily_net": daily_net,
        "balance": economy.balance,
        "min_balance": min_balance,
        "min_balance_day": min_balance_day,
        "days_elapsed": economy.days_elapsed,
        "debt": economy.debt(),
        "loan_service_per_day": economy.loan_interest_per_day() + economy.loan_payment_per_day(),
        "infrastructure_per_day": costs.total_per_day(),
        "bankrupt": economy.bankrupt,
        "bankrupt_day": bankrupt_day,
        "bankrupt_days": bankrupt_day.map_or(0.0, |day| (days as f64 - day).max(0.0)),
        "income_per_resident_per_day": last_income / population,
        "income_per_job_per_day": last_income / jobs,
        "expenses_per_resident_per_day": last_expenses / population,
        "expenses_per_job_per_day": last_expenses / jobs,
        "daily_net_per_resident_per_day": daily_net / population,
        "daily_net_per_job_per_day": daily_net / jobs,
        "balance_per_resident": economy.balance / population,
        "balance_per_job": economy.balance / jobs,
        "daily": daily,
        "economy_checksum": format!("{:016x}", economy.checksum()),
    })
}

fn build_utilities() -> UtilityStore {
    let bounds = city_bounds();
    let mut utilities = UtilityStore::new(SEED);
    for (kind, count, id_base, capacity) in [
        (
            UtilityKind::Water,
            WATER_SITES,
            1_000_000_u64,
            WATER_CAPACITY_PER_SITE,
        ),
        (
            UtilityKind::Power,
            POWER_SITES,
            1_100_000,
            POWER_CAPACITY_PER_SITE,
        ),
    ] {
        for (index, center) in grid_sites(count, bounds).into_iter().enumerate() {
            let id = id_base + index as u64;
            utilities
                .add_site(id, kind, [center[0], 0.0, center[1]])
                .expect("utility site");
            let site = match kind {
                UtilityKind::Water => utilities.water_sites.get_mut(&id),
                UtilityKind::Power => utilities.power_sites.get_mut(&id),
            }
            .expect("utility site was just added");
            site.capacity = capacity;
            site.radius_m = UTILITY_RADIUS_M;
        }
    }
    utilities
}

fn build_services() -> ServiceStore {
    let bounds = city_bounds();
    let mut services = ServiceStore::new(SEED);
    for (kind, count, id_base, capacity) in [
        (
            ServiceKind::School,
            SCHOOL_SITES,
            2_000_000_u64,
            SCHOOL_CAPACITY_PER_SITE,
        ),
        (
            ServiceKind::Clinic,
            CLINIC_SITES,
            2_100_000,
            CIVIC_CAPACITY_PER_SITE,
        ),
        (
            ServiceKind::Fire,
            FIRE_SITES,
            2_200_000,
            CIVIC_CAPACITY_PER_SITE,
        ),
        (
            ServiceKind::Police,
            POLICE_SITES,
            2_300_000,
            CIVIC_CAPACITY_PER_SITE,
        ),
    ] {
        for (index, center) in grid_sites(count, bounds).into_iter().enumerate() {
            let id = id_base + index as u64;
            services
                .add_site(id, kind, ServiceTier::Advanced, [center[0], 0.0, center[1]])
                .expect("service site");
            let site = services
                .sites
                .get_mut(&id)
                .expect("service site was just added");
            site.capacity = capacity;
            site.radius_m = SERVICE_RADIUS_M;
        }
    }
    services
}

/// Evenly spaced site positions spanning the city, one grid per site kind.
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
    fn fixture_counts_match_the_ledger() {
        assert_eq!(HOMES * HOME_CAPACITY, POPULATION);
        assert_eq!(WORKPLACES * JOBS_PER_WORKPLACE, JOBS);
        assert_eq!(HOMES + WORKPLACES, BUILDINGS);
        assert_eq!(WATER_SITES + POWER_SITES, UTILITY_SITES);
        assert_eq!(
            SCHOOL_SITES + CLINIC_SITES + FIRE_SITES + POLICE_SITES,
            SERVICE_SITES
        );

        let report = report(DEFAULT_DAYS);
        assert_eq!(report["scene"], "metropolis_balance");
        assert_eq!(report["status"], "ok");
        assert_eq!(report["population"], POPULATION);
        assert_eq!(report["buildings"], BUILDINGS);
        assert_eq!(report["jobs"], JOBS);
        assert_eq!(report["employed"], EMPLOYED);
        assert_eq!(report["homes"], HOMES);
        assert_eq!(report["workplaces"], WORKPLACES);
        assert_eq!(report["utility_sites"], UTILITY_SITES);
        assert_eq!(report["service_sites"], SERVICE_SITES);
        assert_eq!(report["infrastructure_per_day"], 4_900.0);
        assert_eq!(
            report["checksum"].as_str().map(str::len),
            Some(16),
            "checksum must be a stable 16-character hex string"
        );
    }

    #[test]
    fn report_checksum_is_stable() {
        let first = report(DEFAULT_DAYS);
        let second = report(DEFAULT_DAYS);
        assert_eq!(first, second);
        assert_eq!(first["checksum"], second["checksum"]);
        assert_eq!(first["checksum"].as_str(), Some("bebbaa2a56133b92"));
    }

    #[test]
    fn policy_order_is_low_default_high() {
        assert_eq!(
            TaxPolicy::ALL.map(TaxPolicy::name),
            ["low_tax", "default_tax", "high_tax"]
        );
        assert_eq!(TaxPolicy::Low.rates(), (8.0, 4.0));
        assert_eq!(TaxPolicy::Default.rates(), (20.0, 4.0));
        assert_eq!(TaxPolicy::High.rates(), (30.0, 6.0));

        let report = report(1);
        let policies = report["policies"].as_array().expect("policies");
        assert_eq!(policies.len(), 3);
        for (index, (name, resident, job)) in [
            ("low_tax", 8.0, 4.0),
            ("default_tax", 20.0, 4.0),
            ("high_tax", 30.0, 6.0),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(policies[index]["policy"], name);
            assert_eq!(policies[index]["policy_index"], index);
            assert_eq!(
                policies[index]["resident_tax_per_resident_per_day"],
                resident
            );
            assert_eq!(policies[index]["business_tax_per_job_per_day"], job);
        }
    }

    #[test]
    fn thirty_day_ledger_matches_the_operating_fixture() {
        let report = report(DEFAULT_DAYS);
        assert_eq!(report["days"], DEFAULT_DAYS);
        assert_eq!(report["step_seconds"], DAY_SECONDS);
        assert_eq!(report["operating_expenses_per_day"], 9_254_900.0);
        assert_eq!(report["wages_per_day"], 8_000_000.0);
        assert_eq!(report["building_upkeep_per_day"], 1_250_000.0);

        let policies = report["policies"].as_array().expect("policies");
        for policy in policies {
            let daily = policy["daily"].as_array().expect("daily series");
            assert_eq!(daily.len(), DEFAULT_DAYS as usize);
            assert_eq!(daily[0]["day"], 1);
            assert_eq!(daily[DEFAULT_DAYS as usize - 1]["day"], DEFAULT_DAYS);
            assert_eq!(policy["bankrupt"], false);
            assert_eq!(policy["bankrupt_day"], serde_json::Value::Null);
            assert_eq!(policy["debt"], 0.0);
        }

        let low = &policies[0];
        assert_eq!(low["daily_net"], 345_100.0);
        assert_eq!(low["balance"], 10_453_000.0);
        assert_eq!(low["daily_net_per_resident_per_day"], 0.3451);
        assert_eq!(low["daily_net_per_job_per_day"], 0.86275);

        let default = &policies[1];
        assert_eq!(default["daily_net"], 12_345_100.0);
        assert_eq!(default["balance"], 370_453_000.0);

        let high = &policies[2];
        assert_eq!(high["daily_net"], 23_145_100.0);
        assert_eq!(high["balance"], 694_453_000.0);
    }
}
