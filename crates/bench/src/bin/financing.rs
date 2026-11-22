//! Deterministic phased-construction financing projection.
//!
//! The Industrial and Modern balance probes import a finished city and measure
//! operating rates. That is deliberate: the current financing model cannot fund
//! either city, so a multi-year construction schedule would stop at the first
//! batch. This harness isolates that constraint.
//!
//! It models a fixed-cadence build in 50-building batches. Each batch is paid
//! for up front from cash plus a working-capital loan, then earns taxes for the
//! rest of its life. Operating costs (wages, building upkeep, infrastructure
//! upkeep) accrue every day from the buildings that are already complete.
//!
//! Two runs per scenario:
//!
//! - **Capped:** the shipping rules. 100,000 starting balance plus the 500,000
//!   total-debt cap, so 600,000 of available financing.
//! - **Hypothetical uncapped:** an imaginary zero-interest construction
//!   facility with no debt ceiling. It is not a proposal and not a game rule;
//!   it measures how much financing the phased build would actually need.
//!
//! The model is an accounting projection, not a simulation of the game. It
//! intentionally assumes no inflation, a fixed 20/4 tax policy, full occupancy
//! of completed capacity, no service/utility construction cost, no loan
//! interest (the game rate only makes the uncapped case worse), and a fixed
//! construction cadence. Output is stable JSON with a checksum.
//!
//! `--smoke` re-runs the projection twice and fails unless the results are
//! identical and the headline invariants hold.

use core_city::{
    ConstructionStore, ConstructionTimeScale, Economy, InfrastructureCosts, BANKRUPTCY_THRESHOLD,
    CAMPAIGN_DEFAULT_SECONDS_PER_GAME_DAY, DAY_SECONDS, DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY,
    DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS, DEFAULT_SERVICE_UPKEEP_PER_SITE_PER_DAY,
    DEFAULT_UPKEEP_PER_BUILDING_PER_DAY, DEFAULT_UTILITY_UPKEEP_PER_SITE_PER_DAY, MAX_LOAN,
    MAX_TICK_SECONDS, MAX_TOTAL_DEBT, STARTING_BALANCE, WAGE_PER_WORKER_PER_DAY,
};

/// The runner's recovery tax policy. Fixed for the whole projection.
const TAX_PER_RESIDENT_PER_DAY: f64 = 20.0;

/// Fifty buildings is the smallest batch that still looks like a city block and
/// keeps the projection to a few hundred steps.
const BATCH_SIZE: u64 = 50;

/// Days of construction per batch (about 1.7 buildings per day). This is an
/// assumption, not a measured crew rate; the probes import construction as
/// complete, so the game has no build-rate number yet. A faster build raises
/// peak debt because revenue arrives later relative to spending.
const DAYS_PER_BATCH: u64 = 30;

/// Stop the hypothetical facility after 200 game years. No scenario should get
/// near it; the bound keeps the harness deterministic if a future tweak makes
/// the model insolvent.
const MAX_SIM_DAYS: u64 = 73_050;

const DAYS_PER_YEAR: f64 = 365.25;
const DAYS_PER_MONTH: f64 = DAYS_PER_YEAR / 12.0;

/// Live model: a four-slot crew, matching the developer default. Structure and
/// utilities need all four slots, so the live build is effectively serial.
const LIVE_CREW_CAPACITY: u16 = 4;
/// Live model: every tenement, factory, and office carries 10,000 material
/// litres, the same reference used by the Industrial and Modern probes.
const LIVE_SOLID_VOXELS: u64 = 10_000;
/// `ConstructionStore::tick` clamps at 3,600 simulated seconds, so one game
/// day has to be fed in hourly steps.
const LIVE_HOURS_PER_DAY: u64 = 24;
/// Timeline sampling interval.
const LIVE_TIMELINE_DAYS: u64 = 30;
/// Live model seed. The projection has no seed, so this only affects loan ids.
const LIVE_SEED: u64 = 2026;

/// Tenement cost from `base:tenement` in the starter catalog.
const TENEMENT_COST: f64 = 60_000.0;
const TENEMENT_CAPACITY: u64 = 16;
/// Factory cost and jobs from `base:factory`.
const FACTORY_COST: f64 = 72_000.0;
const FACTORY_JOBS: u64 = 12;
/// Office cost and jobs from `base:office`.
const OFFICE_COST: f64 = 120_000.0;
const OFFICE_JOBS: u64 = 16;

/// Number of daily operating-net figures reported by the two balance probes,
/// including their 800/day working-capital loan service.
const INDUSTRIAL_PROBE_NET: f64 = 63_618.0;
const MODERN_PROBE_NET: f64 = 500_485.0;

struct Scenario {
    name: &'static str,
    homes: u64,
    home_capacity: u64,
    home_cost: f64,
    workplaces: u64,
    workplace_capacity: u64,
    workplace_cost: f64,
    utility_sites: u64,
    service_sites: u64,
    probe_daily_net: f64,
}

impl Scenario {
    fn industrial() -> Self {
        Self {
            name: "industrial",
            homes: 1_250,
            home_capacity: TENEMENT_CAPACITY,
            home_cost: TENEMENT_COST,
            workplaces: 1_667,
            workplace_capacity: FACTORY_JOBS,
            workplace_cost: FACTORY_COST,
            utility_sites: 67 + 84,
            service_sites: 13 + 50 + 38 + 38,
            probe_daily_net: INDUSTRIAL_PROBE_NET,
        }
    }

    fn modern() -> Self {
        Self {
            name: "modern",
            homes: 9_375,
            home_capacity: TENEMENT_CAPACITY,
            home_cost: TENEMENT_COST,
            workplaces: 9_375,
            workplace_capacity: OFFICE_JOBS,
            workplace_cost: OFFICE_COST,
            utility_sites: 510 + 640,
            service_sites: 49 + 196 + 144 + 144,
            probe_daily_net: MODERN_PROBE_NET,
        }
    }

    fn buildings(&self) -> u64 {
        self.homes + self.workplaces
    }

    fn population_target(&self) -> u64 {
        self.homes * self.home_capacity
    }

    fn jobs(&self) -> u64 {
        self.workplaces * self.workplace_capacity
    }

    fn notional_cost(&self) -> f64 {
        self.homes as f64 * self.home_cost + self.workplaces as f64 * self.workplace_cost
    }

    fn infrastructure_per_day(&self) -> f64 {
        self.utility_sites as f64 * DEFAULT_UTILITY_UPKEEP_PER_SITE_PER_DAY
            + self.service_sites as f64 * DEFAULT_SERVICE_UPKEEP_PER_SITE_PER_DAY
    }

    fn operating_net_per_day(&self) -> f64 {
        let residents = self.population_target();
        let employed = residents.min(self.jobs());
        residents as f64 * TAX_PER_RESIDENT_PER_DAY
            + employed as f64 * DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY
            - employed as f64 * WAGE_PER_WORKER_PER_DAY
            - self.buildings() as f64 * DEFAULT_UPKEEP_PER_BUILDING_PER_DAY
            - self.infrastructure_per_day()
    }

    /// Batches in build order. Homes and workplaces are interleaved
    /// proportionally with a Bresenham-style split, so every partial city keeps
    /// roughly the final housing/jobs ratio.
    fn batches(&self) -> Vec<Batch> {
        let total = self.buildings();
        let mut batches = Vec::with_capacity(total.div_ceil(BATCH_SIZE) as usize);
        let mut slot = 0_u64;
        while slot < total {
            let end = (slot + BATCH_SIZE).min(total);
            let homes_before = slot * self.homes / total;
            let homes_after = end * self.homes / total;
            let homes = homes_after - homes_before;
            let workplaces = (end - slot) - homes;
            batches.push(Batch {
                homes,
                workplaces,
                cost: homes as f64 * self.home_cost + workplaces as f64 * self.workplace_cost,
            });
            slot = end;
        }
        batches
    }
    /// Per-building sequence for the live model. Homes and workplaces are
    /// interleaved proportionally with a Bresenham-style split, so every
    /// partial city keeps the final housing/jobs ratio.
    fn live_order(&self) -> Vec<LiveBuilding> {
        let total = self.buildings();
        let mut order = Vec::with_capacity(total as usize);
        for slot in 0..total {
            let homes_before = slot * self.homes / total;
            let homes_after = (slot + 1) * self.homes / total;
            if homes_after > homes_before {
                order.push(LiveBuilding::Home);
            } else {
                order.push(LiveBuilding::Workplace);
            }
        }
        order
    }
}

struct Batch {
    homes: u64,
    workplaces: u64,
    cost: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveBuilding {
    Home,
    Workplace,
}

#[derive(Debug, Clone, PartialEq)]
struct Plan {
    feasible: bool,
    batches_completed: u64,
    construction_days: Option<u64>,
    peak_debt: f64,
    peak_debt_day: u64,
    debt_free_day: Option<u64>,
    final_cash: f64,
    final_debt: f64,
    failed_batch: Option<u64>,
    final_batches_funded: u64,
}

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    if smoke {
        run_smoke();
        return;
    }
    let live = std::env::args().any(|arg| arg == "--live");
    if live {
        println!(
            "{}",
            serde_json::to_string_pretty(&live_report()).expect("pretty json")
        );
        return;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report()).expect("pretty json")
    );
}

fn report() -> serde_json::Value {
    let industrial = Scenario::industrial();
    let modern = Scenario::modern();
    let max_financing = STARTING_BALANCE + MAX_TOTAL_DEBT;
    let scenarios = [&industrial, &modern]
        .into_iter()
        .map(|scenario| scenario_report(scenario, max_financing))
        .collect::<Vec<_>>();
    let mut result = serde_json::json!({
        "scene": "phased_financing",
        "model": "deterministic phased construction financing projection",
        "batch_size": BATCH_SIZE,
        "batch_days": DAYS_PER_BATCH,
        "tax_per_resident_per_day": TAX_PER_RESIDENT_PER_DAY,
        "business_tax_per_job_per_day": DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY,
        "wage_per_worker_per_day": WAGE_PER_WORKER_PER_DAY,
        "upkeep_per_building_per_day": DEFAULT_UPKEEP_PER_BUILDING_PER_DAY,
        "utility_upkeep_per_site_per_day": DEFAULT_UTILITY_UPKEEP_PER_SITE_PER_DAY,
        "service_upkeep_per_site_per_day": DEFAULT_SERVICE_UPKEEP_PER_SITE_PER_DAY,
        "starting_balance": STARTING_BALANCE,
        "max_total_debt": MAX_TOTAL_DEBT,
        "max_financing": max_financing,
        "construction_loan_rate_per_day": 0.0,
        "game_loan_rate_per_day": DEFAULT_LOAN_RATE_PER_DAY,
        "assumptions": [
            "fixed 20/4 tax policy; no inflation or policy change",
            "full occupancy of completed housing and workplaces; move-in is instantaneous",
            "no construction loan market beyond the 500,000 debt cap; the uncapped plan is hypothetical and is not a game rule",
            "no service or utility construction cost; only their operating upkeep is charged",
            "zero interest on the hypothetical construction facility; the game rate only increases the gap",
            "construction cadence is an assumption (30 days per 50-building batch), not a measured crew rate",
            "infrastructure upkeep is phased in linearly with completed batches",
        ],
        "scenarios": scenarios,
    });
    let checksum = checksum_json(&result);
    result["checksum"] = serde_json::Value::String(format!("{checksum:016x}"));
    result
}

fn scenario_report(scenario: &Scenario, max_financing: f64) -> serde_json::Value {
    let batches = scenario.batches();
    let notional = scenario.notional_cost();
    let max_financing_share = max_financing / notional;
    let average_cost = notional / scenario.buildings() as f64;
    let first_batch_cost = batches.first().map_or(0.0, |batch| batch.cost);
    let capped = run_plan(scenario, &batches, true);
    let uncapped = run_plan(scenario, &batches, false);
    let operating_net = scenario.operating_net_per_day();
    let payback_days = notional / operating_net;
    let completion_batch_days = batches.len() as u64 * DAYS_PER_BATCH;

    serde_json::json!({
        "scenario": scenario.name,
        "population_target": scenario.population_target(),
        "jobs": scenario.jobs(),
        "buildings": scenario.buildings(),
        "notional_construction_cost": notional,
        "notional_cost_per_resident": notional / scenario.population_target() as f64,
        "current_max_financing": max_financing,
        "financing_gap": notional - max_financing,
        "max_financing_pct_of_notional": max_financing_share * 100.0,
        "buildings_fundable_by_current_financing": (max_financing / average_cost).floor() as u64,
        "average_cost_per_building": average_cost,
        "batches": batches.len() as u64,
        "first_batch_buildings": batches.first().map_or(0, |batch| batch.homes + batch.workplaces),
        "first_batch_cost": first_batch_cost,
        "first_batch_shortfall": (first_batch_cost - max_financing).max(0.0),
        "infrastructure_per_day": scenario.infrastructure_per_day(),
        "operating_net_per_day": operating_net,
        "probe_daily_net": scenario.probe_daily_net,
        "probe_loan_service_per_day": operating_net - scenario.probe_daily_net,
        "payback_days_at_completion_net": payback_days,
        "payback_months_at_completion_net": payback_days / DAYS_PER_MONTH,
        "payback_years_at_completion_net": payback_days / DAYS_PER_YEAR,
        "assumed_construction_days": completion_batch_days,
        "assumed_construction_years": completion_batch_days as f64 / DAYS_PER_YEAR,
        "capped_plan": plan_json(&capped),
        "hypothetical_uncapped_plan": plan_json(&uncapped),
    })
}

fn plan_json(plan: &Plan) -> serde_json::Value {
    serde_json::json!({
        "feasible": plan.feasible,
        "batches_completed": plan.batches_completed,
        "batches_funded": plan.final_batches_funded,
        "failed_batch": plan.failed_batch,
        "construction_days": plan.construction_days,
        "construction_months": plan.construction_days.map(|days| days as f64 / DAYS_PER_MONTH),
        "construction_years": plan.construction_days.map(|days| days as f64 / DAYS_PER_YEAR),
        "peak_debt": plan.peak_debt,
        "peak_debt_day": plan.peak_debt_day,
        "debt_free_day": plan.debt_free_day,
        "months_to_debt_free": plan.debt_free_day.map(|days| days as f64 / DAYS_PER_MONTH),
        "years_to_debt_free": plan.debt_free_day.map(|days| days as f64 / DAYS_PER_YEAR),
        "final_cash": plan.final_cash,
        "final_debt": plan.final_debt,
    })
}

/// Day-stepped financing projection for one scenario.
///
/// A batch is paid for when it starts, completes `DAYS_PER_BATCH` days later,
/// and then contributes taxes and upkeep. Negative cash is covered by debt up
/// to the cap; positive cash first repays debt. With `capped` the 500,000 debt
/// ceiling is enforced; otherwise the facility is unlimited and interest-free.
fn run_plan(scenario: &Scenario, batches: &[Batch], capped: bool) -> Plan {
    let total_batches = batches.len() as u64;
    let mut cash = STARTING_BALANCE;
    let mut debt = 0.0_f64;
    let mut peak_debt = 0.0_f64;
    let mut peak_debt_day = 0_u64;
    let mut built = 0_u64;
    let mut started = 0_u64;
    let mut homes_built = 0_u64;
    let mut workplaces_built = 0_u64;
    let mut construction_days = None;
    let mut debt_free_day = None;
    let mut feasible = true;
    let mut failed_batch = None;

    for day in 0..=MAX_SIM_DAYS {
        if day % DAYS_PER_BATCH == 0 {
            // The batch started one period ago is now complete.
            if started > built {
                let batch = &batches[built as usize];
                homes_built += batch.homes;
                workplaces_built += batch.workplaces;
                built += 1;
                if built == total_batches {
                    construction_days = Some(day);
                }
            }

            // Start the next batch if the previous one is complete.
            if built == started && started < total_batches {
                let cost = batches[started as usize].cost;
                if cash < cost {
                    let need = cost - cash;
                    let headroom = if capped {
                        (MAX_TOTAL_DEBT - debt).max(0.0)
                    } else {
                        f64::INFINITY
                    };
                    if need > headroom + 1e-9 {
                        feasible = false;
                        failed_batch = Some(started);
                        break;
                    }
                    debt += need;
                    cash += need;
                    if debt > peak_debt {
                        peak_debt = debt;
                        peak_debt_day = day;
                    }
                }
                cash -= cost;
                started += 1;
            }
        }

        let residents = homes_built * scenario.home_capacity;
        let jobs = workplaces_built * scenario.workplace_capacity;
        let employed = residents.min(jobs);
        let revenue = residents as f64 * TAX_PER_RESIDENT_PER_DAY
            + employed as f64 * DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY;
        let wages = employed as f64 * WAGE_PER_WORKER_PER_DAY;
        let upkeep = (homes_built + workplaces_built) as f64 * DEFAULT_UPKEEP_PER_BUILDING_PER_DAY;
        let infrastructure =
            scenario.infrastructure_per_day() * built as f64 / total_batches as f64;
        cash += revenue - wages - upkeep - infrastructure;

        if cash < 0.0 {
            let need = -cash;
            let headroom = if capped {
                (MAX_TOTAL_DEBT - debt).max(0.0)
            } else {
                f64::INFINITY
            };
            if need > headroom + 1e-9 {
                feasible = false;
                break;
            }
            debt += need;
            cash = 0.0;
            if debt > peak_debt {
                peak_debt = debt;
                peak_debt_day = day;
            }
        } else if debt > 0.0 {
            let repay = cash.min(debt);
            cash -= repay;
            debt -= repay;
        }

        // Record the final day construction debt is cleared. Requiring enough
        // cash for the next batch proves the city will not need to borrow
        // again; after completion there is no next batch.
        if debt <= 1e-9
            && peak_debt > 0.0
            && debt_free_day.is_none()
            && (built == total_batches
                || (started < total_batches && cash >= batches[started as usize].cost))
        {
            debt_free_day = Some(day);
        }

        if built == total_batches && debt <= 1e-9 {
            break;
        }
    }

    Plan {
        feasible,
        batches_completed: built,
        construction_days,
        peak_debt,
        peak_debt_day,
        debt_free_day,
        final_cash: cash,
        final_debt: debt,
        failed_batch,
        final_batches_funded: started,
    }
}

// --- Live phased construction (D-029 campaign clock) -----------------------

/// One settled construction day's payment result.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct LiveSettlement {
    paid_total: f64,
    paid_sites: u32,
    stalled_sites: u32,
    borrowed: f64,
}

/// Outcome of one live scenario.
#[derive(Debug, Clone, PartialEq)]
struct LiveResult {
    final_day: u64,
    completion_day: Option<u64>,
    peak_debt: f64,
    peak_debt_day: u64,
    max_financing_required: f64,
    total_interest: f64,
    insolvent_days: u64,
    bankrupt_day: Option<u64>,
    final_balance: f64,
    final_debt: f64,
    loan_count: usize,
    completed: u64,
    completed_homes: u64,
    completed_workplaces: u64,
    status: &'static str,
    timeline: Vec<serde_json::Value>,
}

/// Charge every due installment in ascending placement-id order.
///
/// This mirrors the bridge `settle_construction_payments`: an unpaid
/// installment stalls the site at its next phase boundary and stays due for a
/// later retry. The live model has to borrow before charging when cash is
/// short, because `try_charge_construction` refuses to overdraw.
fn settle_live_construction(
    store: &mut ConstructionStore,
    economy: &mut Economy,
) -> LiveSettlement {
    let mut report = LiveSettlement::default();
    let ids: Vec<u64> = store.sites.keys().copied().collect();
    for id in ids {
        let due = store.payment_due(id);
        if due <= 0.0 {
            continue;
        }
        while economy.balance < due {
            match economy.take_loan(MAX_LOAN, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS) {
                Ok(()) => report.borrowed += MAX_LOAN,
                Err(_) => break,
            }
        }
        if economy.try_charge_construction(due) {
            if let Some(site) = store.site_mut(id) {
                if site.mark_paid(due).is_ok() {
                    site.resume_payment();
                    report.paid_total += due;
                    report.paid_sites += 1;
                }
            }
        } else if let Some(site) = store.site_mut(id) {
            site.stall_for_payment();
            report.stalled_sites += 1;
        }
    }
    report
}

fn add_next_live_site(
    store: &mut ConstructionStore,
    scenario: &Scenario,
    order: &[LiveBuilding],
    next_building: &mut usize,
    next_placement_id: &mut u64,
) {
    if *next_building >= order.len() {
        return;
    }
    let cost = match order[*next_building] {
        LiveBuilding::Home => scenario.home_cost,
        LiveBuilding::Workplace => scenario.workplace_cost,
    };
    let id = *next_placement_id;
    store
        .add_priced_site(id, LIVE_SOLID_VOXELS, cost)
        .expect("placement ids are unique and sequential");
    *next_building += 1;
    *next_placement_id += 1;
}

fn ensure_live_site(
    store: &mut ConstructionStore,
    scenario: &Scenario,
    order: &[LiveBuilding],
    next_building: &mut usize,
    next_placement_id: &mut u64,
) {
    if store.site_count() == 0 {
        add_next_live_site(store, scenario, order, next_building, next_placement_id);
    }
}

/// Drop finished sites once their final installment is paid.
///
/// A completed site can still owe the last 3% inspection installment; the
/// bridge keeps charging it, so the live model keeps the site (and its debt)
/// until the treasury clears it.
fn remove_paid_complete_sites(store: &mut ConstructionStore) {
    let ids: Vec<u64> = store
        .sites
        .values()
        .filter(|site| site.is_complete() && site.payment_due() <= 1e-9)
        .map(|site| site.placement_id)
        .collect();
    for id in ids {
        store.remove_site(id);
    }
}

fn run_live_plan(scenario: &Scenario) -> LiveResult {
    let order = scenario.live_order();
    let total_buildings = u64::try_from(order.len()).expect("building count fits u64");
    let mut store = ConstructionStore::new(
        LIVE_CREW_CAPACITY,
        total_buildings.saturating_mul(LIVE_SOLID_VOXELS),
    );
    store
        .set_time_scale(ConstructionTimeScale::campaign_default())
        .expect("campaign default pacing is valid");
    let mut economy = Economy::new(LIVE_SEED);
    economy.set_taxes(
        TAX_PER_RESIDENT_PER_DAY,
        DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY,
    );

    let mut next_building = 0_usize;
    let mut next_placement_id = 1_u64;
    let mut completed = 0_u64;
    let mut completed_homes = 0_u64;
    let mut completed_workplaces = 0_u64;
    let mut completion_day = None;
    let mut bankrupt_day = None;
    let mut peak_debt = 0.0_f64;
    let mut peak_debt_day = 0_u64;
    let mut max_financing_required = 0.0_f64;
    let mut total_interest = 0.0_f64;
    let mut insolvent_days = 0_u64;
    let mut timeline = Vec::new();
    let mut final_day = 0_u64;
    let mut status = "year_cap";

    // Starting cash plus operating net minus paid installments. Loan proceeds
    // and debt service are excluded: this is the working-capital line the
    // build needed, not the capped loan book.
    let mut shadow_balance = STARTING_BALANCE;

    for day in 0..=MAX_SIM_DAYS {
        final_day = day;
        let mut paid_today = 0.0_f64;
        let mut insolvent_today = false;

        ensure_live_site(
            &mut store,
            scenario,
            &order,
            &mut next_building,
            &mut next_placement_id,
        );
        let opening = settle_live_construction(&mut store, &mut economy);
        paid_today += opening.paid_total;
        if store.total_payment_due() > 1e-9 {
            insolvent_today = true;
        }
        if economy.debt() > peak_debt {
            peak_debt = economy.debt();
            peak_debt_day = day;
        }

        for _hour in 0..LIVE_HOURS_PER_DAY {
            let tick = store.tick(MAX_TICK_SECONDS);
            for id in tick.completed {
                let index = usize::try_from(id - 1).expect("placement id fits usize");
                match order[index] {
                    LiveBuilding::Home => completed_homes += 1,
                    LiveBuilding::Workplace => completed_workplaces += 1,
                }
                completed += 1;
                if completed == total_buildings {
                    completion_day = Some(day);
                }
            }

            let settlement = settle_live_construction(&mut store, &mut economy);
            paid_today += settlement.paid_total;
            if store.total_payment_due() > 1e-9 {
                insolvent_today = true;
            }
            if economy.debt() > peak_debt {
                peak_debt = economy.debt();
                peak_debt_day = day;
            }

            remove_paid_complete_sites(&mut store);
            ensure_live_site(
                &mut store,
                scenario,
                &order,
                &mut next_building,
                &mut next_placement_id,
            );
            let resumed = settle_live_construction(&mut store, &mut economy);
            paid_today += resumed.paid_total;
            if store.total_payment_due() > 1e-9 {
                insolvent_today = true;
            }
            if economy.debt() > peak_debt {
                peak_debt = economy.debt();
                peak_debt_day = day;
            }
        }

        let population = completed_homes * scenario.home_capacity;
        let jobs = completed_workplaces * scenario.workplace_capacity;
        let employed = population.min(jobs);
        let fraction = if total_buildings == 0 {
            0.0
        } else {
            completed as f64 / total_buildings as f64
        };
        let costs = InfrastructureCosts::new(
            scenario.utility_sites as f64 * DEFAULT_UTILITY_UPKEEP_PER_SITE_PER_DAY * fraction,
            scenario.service_sites as f64 * DEFAULT_SERVICE_UPKEEP_PER_SITE_PER_DAY * fraction,
        )
        .expect("phased infrastructure upkeep is finite and non-negative");
        let tick = economy.tick_with_infrastructure_costs(
            DAY_SECONDS,
            population,
            employed,
            completed,
            costs,
        );
        total_interest += tick.loan_interest;

        let operating_net =
            tick.income - (tick.expenses - tick.loan_interest - tick.loan_principal);
        shadow_balance += operating_net - paid_today;
        let financing_now = store.total_payment_due() - shadow_balance;
        if financing_now > max_financing_required {
            max_financing_required = financing_now;
        }

        if insolvent_today {
            insolvent_days += 1;
        }
        if tick.bankrupt {
            bankrupt_day = Some(day);
        }

        if day % LIVE_TIMELINE_DAYS == 0 || day == MAX_SIM_DAYS {
            timeline.push(live_timeline_sample(
                day,
                completed,
                completed_homes,
                completed_workplaces,
                population,
                jobs,
                employed,
                economy.balance,
                economy.debt(),
                insolvent_days,
            ));
        }

        if completed == total_buildings && store.site_count() == 0 {
            status = "complete";
            break;
        }
        if tick.bankrupt {
            status = "bankrupt";
            break;
        }
    }

    let last_sample_day = timeline
        .last()
        .and_then(|sample| sample["day"].as_u64())
        .unwrap_or(0);
    if last_sample_day != final_day {
        timeline.push(live_timeline_sample(
            final_day,
            completed,
            completed_homes,
            completed_workplaces,
            completed_homes * scenario.home_capacity,
            completed_workplaces * scenario.workplace_capacity,
            (completed_homes * scenario.home_capacity)
                .min(completed_workplaces * scenario.workplace_capacity),
            economy.balance,
            economy.debt(),
            insolvent_days,
        ));
    }

    LiveResult {
        final_day,
        completion_day,
        peak_debt,
        peak_debt_day,
        max_financing_required,
        total_interest,
        insolvent_days,
        bankrupt_day,
        final_balance: economy.balance,
        final_debt: economy.debt(),
        loan_count: economy.loans.len(),
        completed,
        completed_homes,
        completed_workplaces,
        status,
        timeline,
    }
}

#[allow(clippy::too_many_arguments)]
fn live_timeline_sample(
    day: u64,
    completed: u64,
    homes: u64,
    workplaces: u64,
    population: u64,
    jobs: u64,
    employed: u64,
    balance: f64,
    debt: f64,
    insolvent_days: u64,
) -> serde_json::Value {
    serde_json::json!({
        "day": day,
        "completed": completed,
        "homes": homes,
        "workplaces": workplaces,
        "population": population,
        "jobs": jobs,
        "employed": employed,
        "balance": balance,
        "debt": debt,
        "insolvent_days": insolvent_days,
    })
}

fn live_report() -> serde_json::Value {
    let scenarios = [Scenario::industrial(), Scenario::modern()]
        .into_iter()
        .map(|scenario| live_scenario_json(&scenario))
        .collect::<Vec<_>>();
    let mut result = serde_json::json!({
        "scene": "live_phased_financing",
        "model": "live_phased_construction",
        "campaign_seconds_per_game_day": CAMPAIGN_DEFAULT_SECONDS_PER_GAME_DAY,
        "hours_per_game_day": LIVE_HOURS_PER_DAY,
        "crew_capacity": LIVE_CREW_CAPACITY,
        "solid_voxels_per_building": LIVE_SOLID_VOXELS,
        "tax_per_resident_per_day": TAX_PER_RESIDENT_PER_DAY,
        "business_tax_per_job_per_day": DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY,
        "starting_balance": STARTING_BALANCE,
        "max_loan": MAX_LOAN,
        "max_total_debt": MAX_TOTAL_DEBT,
        "loan_rate_per_day": DEFAULT_LOAN_RATE_PER_DAY,
        "loan_term_days": DEFAULT_LOAN_TERM_DAYS,
        "bankruptcy_threshold": BANKRUPTCY_THRESHOLD,
        "max_sim_days": MAX_SIM_DAYS,
        "timeline_interval_days": LIVE_TIMELINE_DAYS,
        "assumptions": [
            "one building is placed at a time in a proportional home/workplace order; placement charges its Survey installment immediately",
            "campaign clock: 60 real seconds per game day, fed to construction as 24 hourly 3,600-second ticks",
            "four-slot crew; Structure and Utilities need all four slots, so the build is effectively serial",
            "fixed 20/4 tax policy, full occupancy of completed housing and workplaces, and no inflation",
            "no service or utility construction cost; their operating upkeep is phased in with completed buildings",
            "loans use the shipping 250,000 principal, 0.05%/day rate, 365-day term, eight-loan book, and 500,000 total-debt cap",
            "max_financing_required is peak (unpaid installments due minus shadow cash); shadow cash is the 100,000 starting balance plus operating net minus installments paid, excluding loan proceeds and debt service",
            "an installment that cannot be paid after borrowing stalls the site and counts an insolvent day; the run stops at bankruptcy, completion, or the 200-year cap"
        ],
        "scenarios": scenarios,
    });
    let checksum = checksum_json(&result);
    result["checksum"] = serde_json::Value::String(format!("{checksum:016x}"));
    result
}

fn live_scenario_json(scenario: &Scenario) -> serde_json::Value {
    let result = run_live_plan(scenario);
    serde_json::json!({
        "scenario": scenario.name,
        "buildings": scenario.buildings(),
        "homes": scenario.homes,
        "workplaces": scenario.workplaces,
        "population_target": scenario.population_target(),
        "jobs": scenario.jobs(),
        "notional_construction_cost": scenario.notional_cost(),
        "final_day": result.final_day,
        "completion_day": result.completion_day,
        "completion_years": result.completion_day.map(|days| days as f64 / DAYS_PER_YEAR),
        "peak_debt": result.peak_debt,
        "peak_debt_day": result.peak_debt_day,
        "max_financing_required": result.max_financing_required,
        "total_interest": result.total_interest,
        "insolvent_days": result.insolvent_days,
        "bankrupt_day": result.bankrupt_day,
        "final_balance": result.final_balance,
        "final_debt": result.final_debt,
        "loan_count": result.loan_count,
        "completed": result.completed,
        "completed_homes": result.completed_homes,
        "completed_workplaces": result.completed_workplaces,
        "buildings_remaining": scenario.buildings() - result.completed,
        "status": result.status,
        "timeline": result.timeline,
    })
}

fn run_smoke() {
    let first = report();
    let second = report();
    let deterministic = first == second;
    let industrial = &first["scenarios"][0];
    let modern = &first["scenarios"][1];
    let checksum = first["checksum"].as_str().unwrap_or("missing");
    let invariants = industrial["notional_construction_cost"] == 195_024_000.0
        && modern["notional_construction_cost"] == 1_687_500_000.0
        && industrial["capped_plan"]["feasible"] == false
        && modern["capped_plan"]["feasible"] == false
        && industrial["hypothetical_uncapped_plan"]["feasible"] == true
        && modern["hypothetical_uncapped_plan"]["feasible"] == true
        && industrial["hypothetical_uncapped_plan"]["peak_debt"]
            .as_f64()
            .is_some_and(|debt| debt > MAX_TOTAL_DEBT)
        && modern["hypothetical_uncapped_plan"]["peak_debt"]
            .as_f64()
            .is_some_and(|debt| debt > MAX_TOTAL_DEBT);

    let live_first = live_report();
    let live_second = live_report();
    let live_deterministic = live_first == live_second;
    let live_checksum = live_first["checksum"].as_str().unwrap_or("missing");
    let live_invariants = live_first["model"] == "live_phased_construction"
        && live_first["scenarios"]
            .as_array()
            .is_some_and(|scenarios| scenarios.len() == 2)
        && live_first["scenarios"].as_array().is_some_and(|scenarios| {
            scenarios.iter().all(|scenario| {
                scenario["timeline"]
                    .as_array()
                    .is_some_and(|timeline| !timeline.is_empty())
                    && scenario["final_day"].as_u64().is_some()
            })
        });

    let status = if deterministic && invariants && live_deterministic && live_invariants {
        "ok"
    } else {
        "fail"
    };
    println!(
        "financing: deterministic={deterministic} invariants={invariants} checksum={checksum} live_deterministic={live_deterministic} live_invariants={live_invariants} live_checksum={live_checksum} status={status}"
    );
    if !deterministic || !invariants || !live_deterministic || !live_invariants {
        std::process::exit(1);
    }
}

fn checksum_json(value: &serde_json::Value) -> u64 {
    let mut checksum = 0xcbf2_9ce4_8422_2325_u64;
    for byte in serde_json::to_vec(value).expect("json") {
        checksum ^= u64::from(byte);
        checksum = checksum.wrapping_mul(0x0000_0100_0000_01b3);
    }
    checksum
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_city::ConstructionPhase;

    #[test]
    fn projection_is_deterministic() {
        let first = report();
        let second = report();
        assert_eq!(first, second, "same inputs must produce the same report");
        assert!(first["checksum"].as_str().is_some());
        assert_eq!(first["batch_size"], BATCH_SIZE);
        assert_eq!(first["batch_days"], DAYS_PER_BATCH);
    }

    #[test]
    fn notional_costs_match_the_balance_probes() {
        let report = report();
        assert_eq!(
            report["scenarios"][0]["notional_construction_cost"],
            195_024_000.0
        );
        assert_eq!(
            report["scenarios"][1]["notional_construction_cost"],
            1_687_500_000.0
        );
        assert_eq!(report["scenarios"][0]["buildings"], 2_917);
        assert_eq!(report["scenarios"][1]["buildings"], 18_750);
    }

    #[test]
    fn current_cap_cannot_fund_the_first_batch() {
        let report = report();
        for scenario in report["scenarios"]
            .as_array()
            .expect("scenario array")
            .iter()
        {
            let capped = &scenario["capped_plan"];
            assert_eq!(capped["feasible"], false);
            assert_eq!(capped["batches_completed"], 0);
            assert_eq!(capped["failed_batch"], 0);
            assert_eq!(capped["peak_debt"], 0.0);
            assert!(
                scenario["first_batch_shortfall"]
                    .as_f64()
                    .expect("shortfall")
                    > 0.0
            );
        }
    }

    #[test]
    fn uncapped_plan_completes_and_exceeds_the_cap() {
        let report = report();
        for scenario in report["scenarios"]
            .as_array()
            .expect("scenario array")
            .iter()
        {
            let plan = &scenario["hypothetical_uncapped_plan"];
            assert_eq!(plan["feasible"], true);
            assert_eq!(plan["batches_completed"], scenario["batches"]);
            assert!(
                plan["peak_debt"].as_f64().expect("peak debt") > MAX_TOTAL_DEBT,
                "peak debt must be far above the shipping cap"
            );
            assert_eq!(
                plan["construction_days"],
                scenario["batches"].as_u64().expect("batches") * DAYS_PER_BATCH
            );
        }
    }

    #[test]
    fn completion_net_matches_the_probe_figures() {
        let report = report();
        assert_eq!(report["scenarios"][0]["operating_net_per_day"], 64_418.0);
        assert_eq!(report["scenarios"][1]["operating_net_per_day"], 501_285.0);
        assert_eq!(report["scenarios"][0]["probe_daily_net"], 63_618.0);
        assert_eq!(report["scenarios"][1]["probe_daily_net"], 500_485.0);
        assert_eq!(report["scenarios"][0]["probe_loan_service_per_day"], 800.0);
        assert_eq!(report["scenarios"][1]["probe_loan_service_per_day"], 800.0);
    }

    #[test]
    fn batches_sum_to_the_city() {
        for scenario in [Scenario::industrial(), Scenario::modern()] {
            let batches = scenario.batches();
            let homes: u64 = batches.iter().map(|batch| batch.homes).sum();
            let workplaces: u64 = batches.iter().map(|batch| batch.workplaces).sum();
            let cost: f64 = batches.iter().map(|batch| batch.cost).sum();
            assert_eq!(homes, scenario.homes);
            assert_eq!(workplaces, scenario.workplaces);
            assert!((cost - scenario.notional_cost()).abs() < 0.001);
        }
    }

    #[test]
    fn live_settlement_stalls_without_debt_headroom_and_resumes() {
        let mut store = ConstructionStore::new(LIVE_CREW_CAPACITY, 1_000_000);
        store
            .add_priced_site(1, LIVE_SOLID_VOXELS, 10_000.0)
            .expect("priced site");
        let mut economy = Economy::new(LIVE_SEED);
        economy
            .take_loan(MAX_LOAN, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS)
            .expect("first loan");
        economy
            .take_loan(MAX_LOAN, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS)
            .expect("second loan");
        economy.balance = 0.0;

        let stalled = settle_live_construction(&mut store, &mut economy);
        assert_eq!(stalled.paid_sites, 0);
        assert_eq!(stalled.stalled_sites, 1);
        assert_eq!(stalled.borrowed, 0.0);
        assert!(store.sites[&1].payment_stalled);
        assert!((store.payment_due(1) - 300.0).abs() < 1e-9);

        economy.balance = 1_000.0;
        let resumed = settle_live_construction(&mut store, &mut economy);
        assert_eq!(resumed.paid_sites, 1);
        assert!(!store.sites[&1].payment_stalled);
        assert!(store.payment_due(1).abs() < 1e-9);
    }

    #[test]
    fn live_one_game_day_is_twenty_four_hourly_ticks() {
        let mut store = ConstructionStore::new(LIVE_CREW_CAPACITY, LIVE_SOLID_VOXELS);
        store
            .set_time_scale(ConstructionTimeScale::campaign_default())
            .expect("campaign clock");
        store
            .add_priced_site(1, LIVE_SOLID_VOXELS, 60_000.0)
            .expect("priced site");
        for _ in 0..LIVE_HOURS_PER_DAY {
            store.tick(MAX_TICK_SECONDS);
        }
        let site = store.site(1).expect("site");
        assert_eq!(site.phase, ConstructionPhase::Structure);
        assert!((site.phase_progress - 0.380_952_4).abs() < 1e-3);
        assert!((site.elapsed_s - 86_400.0).abs() < 0.01);
    }
}
