//! Deterministic save/load stress harness for the core city layers.
//!
//! Builds a fixed city (120 placements by default), composes every core store
//! into one JSON object, and round-trips it repeatedly. This is a core harness,
//! not the Godot world save: it catches layer-local serialization/validation
//! drift without needing a Godot runtime.
//!
//! `--terrain` adds a deterministic [`DeltaStore`] to the same envelope using
//! the bridge convention: lowercase hex under `terrain_deltas` plus a
//! `terrain_checksum` field.

use std::collections::BTreeSet;
use std::time::Instant;

use core_city::{
    Age, CityRuntime, ConstructionStore, Economy, Home, IndustryKind, IndustryStore, LotRequest,
    LotStore, NamedCitizens, PlacementRequest, PlacementStore, ResearchStore, RoadRequest,
    RoadStore, ServiceKind, ServiceStore, ServiceTier, SettlementRequest, SettlementStore,
    TechTree, UtilityKind, UtilityStore, Workplace, Zone, DAY_SECONDS, DEFAULT_LOAN_RATE_PER_DAY,
    DEFAULT_LOAN_TERM_DAYS,
};
use core_world::{DeltaStore, WorldChunkCoord, MAX_HEIGHT_M, WATER_LEVEL_M};

const SEED: u64 = 2026;
const DEFAULT_CYCLES: usize = 60;
/// `--smoke` uses a short cycle count; the fixture checksum is cycle-invariant.
const SMOKE_CYCLES: usize = 2;
const DEFAULT_PLACEMENTS: usize = 120;
const MIN_PLACEMENTS: usize = 6;
/// The fixture stockpiles 10,000,000 litres and every site needs 10,000, so
/// 1,000 placements is also the material ceiling for a fully completed city.
const MAX_PLACEMENTS: usize = 1_000;
/// Pinned combined checksum for the default 120-placement fixture. A change is
/// either an intentional fixture change or a regression; update this pin and
/// `docs/75` together when it is intentional.
const DEFAULT_COMBINED_CHECKSUM: u64 = 0x0ce6_1188_7161_ff62;
const HOUSES_PER_SIX: usize = 5;
const RESIDENTS_PER_HOUSE: u32 = 10;
const JOBS_PER_WORKPLACE: u32 = 10;
const SOLID_VOXELS: u64 = 10_000;
const HEIGHT: u16 = 40;
const MOVE_IN_TICK_SECONDS: f64 = 60.0;
const MOVE_IN_BASE_RATE: f32 = 100.0;
const MAX_MOVE_IN_TICKS: usize = 16;
const MAX_CYCLES: usize = 10_000;
const DEFAULT_TERRAIN_EDITS: usize = 2_000;
const MAX_TERRAIN_EDITS: usize = 100_000;
/// Terrain edits are spread over an 8×8 block of 256 m chunks, mirroring the
/// multi-chunk streaming scenes.
const TERRAIN_CHUNKS_PER_SIDE: usize = 8;
const TERRAIN_CELLS_PER_SIDE: usize = 256;
const TERRAIN_CHUNKS: usize = TERRAIN_CHUNKS_PER_SIDE * TERRAIN_CHUNKS_PER_SIDE;

/// Per-cycle timing samples, only populated when `--timing` is set.
///
/// `cycle_ms` is the save round-trip only: compose (`to_value`), serialize, and
/// parse. In-memory validation and the post-parse verification are timed and
/// reported separately so they cannot overstate the round-trip cost.
#[derive(Default)]
struct CycleTiming {
    cycle_ms: Vec<f64>,
    compose_ms: f64,
    serialize_ms: f64,
    parse_ms: f64,
    validate_ms: f64,
    verify_ms: f64,
    loop_ms_total: f64,
    combined_json_bytes: usize,
}

/// Parsed `save_stress` arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cli {
    cycles: usize,
    placements: usize,
    timing: bool,
    terrain: bool,
    terrain_edits: usize,
    smoke: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            cycles: DEFAULT_CYCLES,
            placements: DEFAULT_PLACEMENTS,
            timing: false,
            terrain: false,
            terrain_edits: DEFAULT_TERRAIN_EDITS,
            smoke: false,
        }
    }
}

impl Cli {
    fn terrain_edits(self) -> Option<usize> {
        self.terrain.then_some(self.terrain_edits)
    }

    fn validate(self) -> Result<Self, String> {
        if self.cycles == 0 || self.cycles > MAX_CYCLES {
            return Err(format!(
                "cycles must be 1..={MAX_CYCLES}, got {}",
                self.cycles
            ));
        }
        if !(MIN_PLACEMENTS..=MAX_PLACEMENTS).contains(&self.placements) {
            return Err(format!(
                "placements must be {MIN_PLACEMENTS}..={MAX_PLACEMENTS}, got {}",
                self.placements
            ));
        }
        if !(1..=MAX_TERRAIN_EDITS).contains(&self.terrain_edits) {
            return Err(format!(
                "terrain edits must be 1..={MAX_TERRAIN_EDITS}, got {}",
                self.terrain_edits
            ));
        }
        Ok(self)
    }

    fn is_default_fixture(self) -> bool {
        self.placements == DEFAULT_PLACEMENTS && !self.terrain
    }
}

fn usage() -> String {
    format!(
        "usage: save_stress [--cycles 1..={MAX_CYCLES}] \
         [--placements {MIN_PLACEMENTS}..={MAX_PLACEMENTS}] \
         [--terrain [--terrain-edits 1..={MAX_TERRAIN_EDITS}]] \
         [--timing|--bench] [--smoke]\n\
         --smoke runs {SMOKE_CYCLES} cycles and gates on the pinned default checksum."
    )
}

/// Parse the CLI, rejecting unknown flags and malformed values with exit 2.
/// Supports both `--flag value` and `--flag=value`.
fn parse_args(arguments: &[String]) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut cycles_given = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        index += 1;
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (argument.as_str(), None),
        };
        match flag {
            "--cycles" => {
                let raw = take_value(arguments, &mut index, flag, inline)?;
                cli.cycles = parse_usize(flag, &raw)?;
                cycles_given = true;
            }
            "--placements" => {
                let raw = take_value(arguments, &mut index, flag, inline)?;
                cli.placements = parse_usize(flag, &raw)?;
            }
            "--terrain" => {
                if inline.is_some() {
                    return Err("--terrain does not take a value".to_string());
                }
                cli.terrain = true;
            }
            "--terrain-edits" => {
                let raw = take_value(arguments, &mut index, flag, inline)?;
                cli.terrain_edits = parse_usize(flag, &raw)?;
            }
            "--timing" | "--bench" => {
                if inline.is_some() {
                    return Err(format!("{flag} does not take a value"));
                }
                cli.timing = true;
            }
            "--smoke" => {
                if inline.is_some() {
                    return Err("--smoke does not take a value".to_string());
                }
                cli.smoke = true;
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    if cli.smoke && !cycles_given {
        cli.cycles = SMOKE_CYCLES;
    }
    cli.validate()
}

fn take_value(
    arguments: &[String],
    index: &mut usize,
    flag: &str,
    inline: Option<&str>,
) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value.to_string());
    }
    let value = arguments
        .get(*index)
        .ok_or_else(|| format!("{flag} requires a value"))?;
    *index += 1;
    Ok(value.clone())
}

fn parse_usize(flag: &str, raw: &str) -> Result<usize, String> {
    raw.parse::<usize>()
        .map_err(|_| format!("{flag} expects a non-negative integer, got `{raw}`"))
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(&arguments) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("{error}\n{}", usage());
            std::process::exit(2);
        }
    };
    match run(cli) {
        Ok(summary) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&summary).expect("summary json")
            );
            if cli.smoke {
                println!(
                    "SAVE_STRESS_SMOKE cycles={} placements={} checksum={} status=ok",
                    cli.cycles,
                    summary["placements"],
                    summary["combined_checksum"].as_str().unwrap_or("")
                );
            }
        }
        Err(error) => {
            eprintln!("SAVE_STRESS status=failed error={error}");
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<serde_json::Value, String> {
    let run_start = Instant::now();
    let build_start = Instant::now();
    let city = City::build(cli.placements, cli.terrain_edits())?;
    let build_ms = elapsed_ms(build_start);
    let mut checksum: Option<u64> = None;
    let mut timings = CycleTiming::default();
    for _ in 0..cli.cycles {
        let loop_start = Instant::now();
        let validate_start = Instant::now();
        city.validate()?;
        let validate_ms = elapsed_ms(validate_start);

        let compose_start = Instant::now();
        let value = city.to_value();
        let compose_ms = elapsed_ms(compose_start);

        let serialize_start = Instant::now();
        let encoded = serde_json::to_string(&value).map_err(|error| error.to_string())?;
        let serialize_ms = elapsed_ms(serialize_start);

        let parse_start = Instant::now();
        let decoded: serde_json::Value =
            serde_json::from_str(&encoded).map_err(|error| error.to_string())?;
        let parse_ms = elapsed_ms(parse_start);

        let verify_start = Instant::now();
        city.check_value(&decoded)?;
        let verify_ms = elapsed_ms(verify_start);

        let cycle_checksum = combined_checksum(&city);
        if let Some(previous) = checksum {
            if previous != cycle_checksum {
                return Err("combined checksum drifted across cycles".to_string());
            }
        }
        checksum = Some(cycle_checksum);
        if cli.timing {
            timings.cycle_ms.push(compose_ms + serialize_ms + parse_ms);
            timings.compose_ms += compose_ms;
            timings.serialize_ms += serialize_ms;
            timings.parse_ms += parse_ms;
            timings.validate_ms += validate_ms;
            timings.verify_ms += verify_ms;
            timings.loop_ms_total += elapsed_ms(loop_start);
            timings.combined_json_bytes = encoded.len();
        }
    }
    let checksum = checksum.ok_or_else(|| "no cycles".to_string())?;
    if cli.smoke && cli.is_default_fixture() && checksum != DEFAULT_COMBINED_CHECKSUM {
        return Err(format!(
            "smoke checksum {checksum:016x} moved from pinned {DEFAULT_COMBINED_CHECKSUM:016x}"
        ));
    }
    reject_bad_inputs(&city)?;
    let total_ms = elapsed_ms(run_start);
    let (houses, workplaces) = placement_split(cli.placements);
    let mut summary = serde_json::json!({
        "scene": "save_stress",
        "seed": SEED,
        "cycles": cli.cycles,
        "placements": city.placements.placements.len(),
        "construction_sites": city.construction.site_count(),
        "households": city.runtime.households.len(),
        "population": city.runtime.population(),
        "named": city.named.len(),
        "research_completed": city.research.completed_count(),
        "combined_checksum": format!("{checksum:016x}"),
        "status": "ok",
    });
    if let Some(terrain) = &city.terrain {
        summary["terrain_edits"] = serde_json::json!(terrain.edit_count());
        summary["terrain_chunks"] = serde_json::json!(terrain.chunk_count());
        summary["terrain_bytes"] = serde_json::json!(terrain.encode().len());
        summary["terrain_checksum"] = serde_json::json!(format!("{:016x}", terrain.checksum()));
    }
    if cli.timing {
        summary["timing"] = timing_report(
            &timings,
            &city,
            houses,
            workplaces,
            build_ms,
            total_ms,
            cli.placements,
        );
    }
    Ok(summary)
}

fn timing_report(
    timings: &CycleTiming,
    city: &City,
    houses: usize,
    workplaces: usize,
    build_ms: f64,
    total_ms: f64,
    placements: usize,
) -> serde_json::Value {
    let cycle_ms: Vec<f64> = timings.cycle_ms.iter().copied().map(round_ms).collect();
    let cycle_total: f64 = timings.cycle_ms.iter().sum();
    let cycle_min = timings
        .cycle_ms
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let cycle_max = timings
        .cycle_ms
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let cycle_mean = if timings.cycle_ms.is_empty() {
        0.0
    } else {
        cycle_total / timings.cycle_ms.len() as f64
    };
    serde_json::json!({
        "placements": placements,
        "houses": houses,
        "workplaces": workplaces,
        "build_ms": round_ms(build_ms),
        "total_ms": round_ms(total_ms),
        "cycle_ms_total": round_ms(cycle_total),
        "cycle_ms_mean": round_ms(cycle_mean),
        "cycle_ms_min": round_ms(cycle_min),
        "cycle_ms_max": round_ms(cycle_max),
        "cycle_ms": cycle_ms,
        "compose_ms": round_ms(timings.compose_ms),
        "serialize_ms": round_ms(timings.serialize_ms),
        "parse_ms": round_ms(timings.parse_ms),
        "validate_ms": round_ms(timings.validate_ms),
        "verify_ms": round_ms(timings.verify_ms),
        "loop_ms_total": round_ms(timings.loop_ms_total),
        "combined_json_bytes": timings.combined_json_bytes,
        "store_checksums": city.checksum_map(),
    })
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn round_ms(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

/// Split a placement budget into houses and workplaces: five sixths houses,
/// one sixth workplaces, with at least one of each.
fn placement_split(placements: usize) -> (usize, usize) {
    let houses = placements * HOUSES_PER_SIX / 6;
    (houses, placements - houses)
}

struct City {
    placements: PlacementStore,
    construction: ConstructionStore,
    runtime: CityRuntime,
    economy: Economy,
    industry: IndustryStore,
    utilities: UtilityStore,
    services: ServiceStore,
    research: ResearchStore,
    named: NamedCitizens,
    settlements: SettlementStore,
    roads: RoadStore,
    lots: LotStore,
    terrain: Option<DeltaStore>,
}

impl City {
    fn build(placement_count: usize, terrain_edits: Option<usize>) -> Result<Self, String> {
        if !(MIN_PLACEMENTS..=MAX_PLACEMENTS).contains(&placement_count) {
            return Err(format!(
                "placements must be {MIN_PLACEMENTS}..={MAX_PLACEMENTS}, got {placement_count}"
            ));
        }
        let (houses, _workplaces) = placement_split(placement_count);
        let tree = TechTree::default_tree();
        let mut placements = PlacementStore::new(SEED);
        let mut construction = ConstructionStore::new(8, 10_000_000);
        for index in 0..placement_count {
            let workplace = index >= houses;
            let (x, z) = if workplace {
                // Workplaces sit east of the house grid in a five-column block.
                let local = index - houses;
                (
                    328.0 + (local % 5) as f32 * 20.0,
                    128.0 + (local / 5) as f32 * 20.0,
                )
            } else {
                // Houses use a ten-column grid, growing south with the count.
                (
                    128.0 + (index % 10) as f32 * 20.0,
                    128.0 + (index / 10) as f32 * 20.0,
                )
            };
            let request = PlacementRequest {
                blueprint_id: if workplace {
                    "local:save_stress_workshop".to_string()
                } else {
                    "local:save_stress_house".to_string()
                },
                blueprint_hash: "blake3:save-stress".to_string(),
                blueprint_path: "/save_stress/building.tsbp".to_string(),
                name: if workplace { "Workshop" } else { "House" }.to_string(),
                bounds_m: [8.0, 6.0, 8.0],
                center_x: x,
                center_z: z,
                rotation_deg: 0,
                material_cost: 3_000.0,
                resident_capacity: if workplace { 0 } else { RESIDENTS_PER_HOUSE },
                job_slots: if workplace { JOBS_PER_WORKPLACE } else { 0 },
                solid_voxels: SOLID_VOXELS,
                structure: None,
            };
            let placement = placements
                .place(&request, |_, _| HEIGHT)
                .map_err(|error| error.to_string())?;
            construction
                .add_site(placement.id, SOLID_VOXELS)
                .map_err(|error| error.to_string())?;
        }
        for _ in 0..20 {
            construction.tick(60.0);
            if construction.sites.values().all(|site| site.is_complete()) {
                break;
            }
        }
        if !construction.sites.values().all(|site| site.is_complete()) {
            return Err("construction did not complete".to_string());
        }

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
        let mut runtime = CityRuntime::new(SEED);
        runtime.set_move_in_per_second(MOVE_IN_BASE_RATE);
        // The move-in cap is 100 residents per simulated second, so a large
        // fixture needs more than one 60 s step to fill every home. Small
        // fixtures stop after the first step, as before.
        for _ in 0..MAX_MOVE_IN_TICKS {
            runtime.tick_with_workplaces(MOVE_IN_TICK_SECONDS, &homes, &workplaces);
            let vacant: u32 = runtime
                .households
                .values()
                .map(|household| household.capacity - household.residents)
                .sum();
            if vacant == 0 {
                break;
            }
        }
        let capacity: u32 = homes.iter().map(|home| home.capacity).sum();
        if runtime.population() != u64::from(capacity) {
            return Err(format!(
                "move-in did not fill the city: {} of {capacity}",
                runtime.population()
            ));
        }

        let mut economy = Economy::new(SEED);
        economy
            .take_loan(40_000.0, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS)
            .map_err(|error| error.to_string())?;
        economy.tick(
            DAY_SECONDS,
            runtime.population(),
            runtime.workers(),
            placements.placements.len() as u64,
        );

        let mut industry = IndustryStore::new(SEED);
        for (index, workplace) in workplaces.iter().take(10).enumerate() {
            let kind = match index % 3 {
                0 => IndustryKind::Farm,
                1 => IndustryKind::Lumberjack,
                _ => IndustryKind::Hunt,
            };
            industry
                .add_site(workplace.id, kind)
                .map_err(|error| error.to_string())?;
        }
        industry.tick_with_power(DAY_SECONDS, &runtime.staffing, runtime.population(), 1.0);

        let mut utilities = UtilityStore::new(SEED);
        for (index, workplace) in workplaces.iter().enumerate() {
            let kind = if index % 2 == 0 {
                UtilityKind::Water
            } else {
                UtilityKind::Power
            };
            utilities
                .add_site(
                    workplace.id,
                    kind,
                    [workplace.center[0], 40.0, workplace.center[1]],
                )
                .map_err(|error| error.to_string())?;
        }
        let consumers: Vec<core_city::UtilityConsumer> = placements
            .placements
            .iter()
            .filter(|placement| {
                construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
            })
            .map(|placement| core_city::UtilityConsumer {
                id: placement.id,
                center: [placement.center[0], placement.center[2]],
                residents: runtime
                    .households
                    .get(&placement.id)
                    .map_or(0, |household| household.residents),
                workers: runtime
                    .households
                    .get(&placement.id)
                    .map_or(0, |household| household.workers),
            })
            .collect();
        utilities.refresh(&consumers);

        let mut services = ServiceStore::new(SEED);
        for (index, workplace) in workplaces.iter().take(8).enumerate() {
            let kind = match index % 4 {
                0 => ServiceKind::School,
                1 => ServiceKind::Clinic,
                2 => ServiceKind::Fire,
                _ => ServiceKind::Police,
            };
            services
                .add_site(
                    workplace.id,
                    kind,
                    ServiceTier::Basic,
                    [workplace.center[0], 40.0, workplace.center[1]],
                )
                .map_err(|error| error.to_string())?;
        }
        let service_consumers: Vec<core_city::ServiceConsumer> = consumers
            .iter()
            .map(|consumer| core_city::ServiceConsumer {
                id: consumer.id,
                center: consumer.center,
                residents: consumer.residents,
                workers: consumer.workers,
            })
            .collect();
        services.refresh(&service_consumers);

        let mut research = ResearchStore::new(SEED);
        research.tick(DAY_SECONDS * 10.0, 5.0);
        research
            .unlock("construction.timber_frame", &tree, Age::Wood)
            .map_err(|error| error.to_string())?;

        let mut named = NamedCitizens::new(SEED);
        named.sync(&runtime.households);

        let mut settlements = SettlementStore::new(SEED);
        settlements
            .found(&SettlementRequest {
                name: "Stressford".to_string(),
                owner: "player".to_string(),
                center: [128, 128],
                radius_m: 300,
            })
            .map_err(|error| error.to_string())?;

        let mut roads = RoadStore::new(SEED);
        roads
            .add(
                &RoadRequest {
                    start: [90, 118],
                    end: [130, 118],
                    width_m: 6,
                },
                |_, _| HEIGHT,
            )
            .map_err(|error| error.to_string())?;
        let mut lots = LotStore::new(SEED);
        lots.add(
            &LotRequest {
                bounds: [96, 120, 106, 130],
                zone: Zone::Residential,
            },
            |_, _| HEIGHT,
            &roads,
        )
        .map_err(|error| error.to_string())?;

        let terrain = terrain_edits.map(terrain_fixture).transpose()?;

        let city = Self {
            placements,
            construction,
            runtime,
            economy,
            industry,
            utilities,
            services,
            research,
            named,
            settlements,
            roads,
            lots,
            terrain,
        };
        city.validate()?;
        Ok(city)
    }

    fn to_value(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "placements": self.placements,
            "construction": self.construction,
            "population": self.runtime,
            "economy": self.economy,
            "industry": self.industry,
            "utilities": self.utilities,
            "services": self.services,
            "research": self.research,
            "named": self.named.to_save(),
            "settlements": self.settlements,
            "roads": self.roads,
            "lots": self.lots,
        });
        if let Some(terrain) = &self.terrain {
            // Same envelope convention as the Godot bridge: the encoded
            // binary delta blob as lowercase hex plus its FNV-1a checksum.
            value["terrain_deltas"] = serde_json::Value::String(hex_encode(&terrain.encode()));
            value["terrain_checksum"] =
                serde_json::Value::String(format!("{:016x}", terrain.checksum()));
        }
        value
    }

    fn check_value(&self, value: &serde_json::Value) -> Result<(), String> {
        let placements: PlacementStore = decode(value, "placements")?;
        let construction: ConstructionStore = decode(value, "construction")?;
        let runtime: CityRuntime = decode(value, "population")?;
        let economy: Economy = decode(value, "economy")?;
        let industry: IndustryStore = decode(value, "industry")?;
        let utilities: UtilityStore = decode(value, "utilities")?;
        let services: ServiceStore = decode(value, "services")?;
        let research: ResearchStore = decode(value, "research")?;
        let named: core_city::NamedSave = decode(value, "named")?;
        let settlements: SettlementStore = decode(value, "settlements")?;
        let roads: RoadStore = decode(value, "roads")?;
        let lots: LotStore = decode(value, "lots")?;
        let expected = self.checksums();
        let actual = [
            placements.checksum(),
            construction.checksum(),
            runtime.checksum(),
            economy.checksum(),
            industry.checksum(),
            utilities.checksum(),
            services.checksum(),
            research.checksum(),
            NamedCitizens::from_save(named)
                .map_err(|error| error.to_string())?
                .checksum(),
            settlements.checksum(),
            roads.checksum(),
            lots.checksum(),
        ];
        // The combined Value roundtrip can normalize f64 text (for example
        // 109.58904109589041 to 109.5890410958904). Compare the economy
        // semantically; the other stores' checksums are byte-stable.
        let mut stable = true;
        for index in [0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11] {
            if actual[index] != expected[index] {
                stable = false;
            }
        }
        let economy_matches = (economy.balance - self.economy.balance).abs() < 1e-6
            && (economy.days_elapsed - self.economy.days_elapsed).abs() < 1e-9
            && economy.loans.len() == self.economy.loans.len();
        if !stable || !economy_matches {
            return Err(format!(
                "per-store state changed during roundtrip: {actual:?} != {expected:?}"
            ));
        }
        if let Some(expected) = &self.terrain {
            let encoded = value
                .get("terrain_deltas")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing terrain_deltas".to_string())?;
            let checksum_hex = value
                .get("terrain_checksum")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing terrain_checksum".to_string())?;
            let expected_checksum = u64::from_str_radix(checksum_hex, 16)
                .map_err(|error| format!("terrain_checksum: {error}"))?;
            let decoded = terrain_from_hex(encoded, expected_checksum)?;
            if decoded.checksum() != expected.checksum() {
                return Err("terrain checksum changed during roundtrip".to_string());
            }
            if decoded.edit_count() != expected.edit_count() {
                return Err("terrain edit count changed during roundtrip".to_string());
            }
            if hex_encode(&decoded.encode()) != encoded {
                return Err("terrain deltas are not byte-stable".to_string());
            }
        } else if value.get("terrain_deltas").is_some() || value.get("terrain_checksum").is_some() {
            return Err("unexpected terrain deltas in the combined value".to_string());
        }
        Ok(())
    }

    fn checksum_map(&self) -> serde_json::Value {
        const NAMES: [&str; 12] = [
            "placements",
            "construction",
            "population",
            "economy",
            "industry",
            "utilities",
            "services",
            "research",
            "named",
            "settlements",
            "roads",
            "lots",
        ];
        let mut map = serde_json::Map::new();
        for (name, value) in NAMES.iter().zip(self.checksums()) {
            map.insert(
                (*name).to_string(),
                serde_json::Value::String(format!("{value:016x}")),
            );
        }
        serde_json::Value::Object(map)
    }

    fn checksums(&self) -> [u64; 12] {
        [
            self.placements.checksum(),
            self.construction.checksum(),
            self.runtime.checksum(),
            self.economy.checksum(),
            self.industry.checksum(),
            self.utilities.checksum(),
            self.services.checksum(),
            self.research.checksum(),
            self.named.checksum(),
            self.settlements.checksum(),
            self.roads.checksum(),
            self.lots.checksum(),
        ]
    }

    fn validate(&self) -> Result<(), String> {
        let placement_json = self
            .placements
            .to_json_pretty()
            .map_err(|error| error.to_string())?;
        PlacementStore::from_json(&placement_json).map_err(|error| error.to_string())?;
        self.construction
            .validate()
            .map_err(|error| error.to_string())?;
        self.runtime.validate().map_err(|error| error.to_string())?;
        self.economy.validate().map_err(|error| error.to_string())?;
        self.industry
            .validate()
            .map_err(|error| error.to_string())?;
        self.utilities
            .validate()
            .map_err(|error| error.to_string())?;
        self.services
            .validate()
            .map_err(|error| error.to_string())?;
        self.research
            .validate(&TechTree::default_tree())
            .map_err(|error| error.to_string())?;
        self.named.validate().map_err(|error| error.to_string())?;
        self.settlements
            .validate_loaded()
            .map_err(|error| error.to_string())?;
        self.roads
            .validate_loaded()
            .map_err(|error| error.to_string())?;
        self.lots
            .validate_loaded()
            .map_err(|error| error.to_string())?;

        let completed: BTreeSet<u64> = self
            .construction
            .sites
            .iter()
            .filter(|(_, site)| site.is_complete())
            .map(|(&id, _)| id)
            .collect();
        for placement in &self.placements.placements {
            if !completed.contains(&placement.id) {
                return Err(format!("placement {} has no completed site", placement.id));
            }
        }
        for &id in self.runtime.households.keys() {
            if !completed.contains(&id) {
                return Err(format!("household {id} has no completed home"));
            }
        }
        for &id in self.industry.sites.keys() {
            if !completed.contains(&id) {
                return Err(format!("industry site {id} is not completed"));
            }
        }
        for id in self
            .utilities
            .water_sites
            .keys()
            .chain(self.utilities.power_sites.keys())
        {
            if !completed.contains(id) {
                return Err(format!("utility site {id} is not completed"));
            }
        }
        for &id in self.services.sites.keys() {
            if !completed.contains(&id) {
                return Err(format!("service site {id} is not completed"));
            }
        }
        for ((placement_id, slot), citizen) in &self.named.citizens {
            let household = self
                .runtime
                .households
                .get(placement_id)
                .ok_or_else(|| format!("named citizen {placement_id} has no household"))?;
            if *slot >= household.residents || citizen.id == 0 {
                return Err("named citizen slot is invalid".to_string());
            }
        }
        if let Some(terrain) = &self.terrain {
            let encoded = terrain.encode();
            let decoded =
                DeltaStore::decode(&encoded).map_err(|error| format!("terrain deltas: {error}"))?;
            if decoded != *terrain {
                return Err("terrain deltas changed during roundtrip".to_string());
            }
        }
        Ok(())
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    key: &str,
) -> Result<T, String> {
    serde_json::from_value(
        value
            .get(key)
            .ok_or_else(|| format!("missing {key}"))?
            .clone(),
    )
    .map_err(|error| format!("{key}: {error}"))
}

fn reject_bad_inputs(city: &City) -> Result<(), String> {
    if PlacementStore::from_json("{").is_ok() {
        return Err("malformed placement JSON accepted".to_string());
    }
    let mut construction = city.construction.clone();
    construction.format_version = 99;
    if construction.validate().is_ok() {
        return Err("unsupported construction version accepted".to_string());
    }
    let mut population = city.runtime.clone();
    population.format_version = 99;
    if population.validate().is_ok() {
        return Err("unsupported population version accepted".to_string());
    }
    let mut economy = city.economy.clone();
    economy.tax_per_resident_per_day = 500.0;
    if economy.validate().is_ok() {
        return Err("invalid economy accepted".to_string());
    }
    let mut named = city.named.to_save();
    if let Some(first) = named.citizens.first().cloned() {
        named.citizens.push(first);
    }
    if named.validate().is_ok() {
        return Err("duplicate named identity accepted".to_string());
    }
    Ok(())
}

fn combined_checksum(city: &City) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in city.checksums() {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    if let Some(terrain) = &city.terrain {
        for byte in terrain.checksum().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Build the deterministic terrain fixture: edits are dealt round-robin across
/// an 8×8 chunk block, cells never repeat within a chunk, and heights come from
/// a seeded splitmix64 stream.
fn terrain_fixture(edit_count: usize) -> Result<DeltaStore, String> {
    if !(1..=MAX_TERRAIN_EDITS).contains(&edit_count) {
        return Err(format!(
            "terrain edits must be 1..={MAX_TERRAIN_EDITS}, got {edit_count}"
        ));
    }
    if edit_count > TERRAIN_CHUNKS * TERRAIN_CELLS_PER_SIDE * TERRAIN_CELLS_PER_SIDE {
        return Err("terrain fixture cannot place unique cells".to_string());
    }
    let mut store = DeltaStore::new();
    for index in 0..edit_count {
        let chunk_index = index % TERRAIN_CHUNKS;
        let column = index / TERRAIN_CHUNKS;
        let coord = WorldChunkCoord::new(
            (chunk_index % TERRAIN_CHUNKS_PER_SIDE) as i64,
            (chunk_index / TERRAIN_CHUNKS_PER_SIDE) as i64,
        );
        // Multiplication by 37 is a bijection modulo 256, so the x offset
        // scatters cells without ever repeating a (z, x) pair in a chunk.
        let x = ((column % TERRAIN_CELLS_PER_SIDE) as u8)
            .wrapping_mul(37)
            .wrapping_add(chunk_index as u8);
        let z = (column / TERRAIN_CELLS_PER_SIDE) as u8;
        let height = WATER_LEVEL_M
            + (splitmix64(SEED ^ (index as u64)) % u64::from(MAX_HEIGHT_M - WATER_LEVEL_M + 1))
                as u16;
        store
            .set_height(coord, x, z, height)
            .map_err(|error| error.to_string())?;
    }
    Ok(store)
}

/// splitmix64 finalizer, used only to derive fixture heights.
fn splitmix64(value: u64) -> u64 {
    let mut value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_decode(encoded: &str) -> Result<Vec<u8>, String> {
    let bytes = encoded.as_bytes();
    if !bytes.len().is_multiple_of(2) || !encoded.is_ascii() {
        return Err("terrain delta hex string has an invalid length or character".to_string());
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        decoded.push((hex_value(pair[0])? << 4) | hex_value(pair[1])?);
    }
    Ok(decoded)
}

fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("terrain delta hex string has an invalid character".to_string()),
    }
}

/// Decode the bridge hex envelope, check its checksum, then decode the blob.
fn terrain_from_hex(encoded: &str, expected_checksum: u64) -> Result<DeltaStore, String> {
    let bytes = hex_decode(encoded)?;
    let actual = DeltaStore::checksum_bytes(&bytes);
    if actual != expected_checksum {
        return Err(format!(
            "terrain checksum mismatch: expected {expected_checksum:016x}, deltas are {actual:016x}"
        ));
    }
    DeltaStore::decode(&bytes).map_err(|error| format!("terrain deltas: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(cycles: usize, placements: usize, terrain_edits: Option<usize>) -> Cli {
        Cli {
            cycles,
            placements,
            terrain: terrain_edits.is_some(),
            terrain_edits: terrain_edits.unwrap_or(DEFAULT_TERRAIN_EDITS),
            ..Cli::default()
        }
    }

    fn args(values: impl IntoIterator<Item = &'static str>) -> Vec<String> {
        values.into_iter().map(str::to_string).collect()
    }

    #[test]
    fn a_small_city_round_trips() {
        let summary = run(cli(2, 120, None)).expect("stress");
        assert_eq!(summary["status"], "ok");
        assert_eq!(summary["placements"], 120);
        assert!(summary["population"].as_u64().expect("population") > 0);
    }

    #[test]
    fn default_fixture_checksum_is_unchanged() {
        let expected = format!("{DEFAULT_COMBINED_CHECKSUM:016x}");
        let summary = run(cli(1, 120, None)).expect("stress");
        assert_eq!(summary["combined_checksum"], expected);
        assert!(summary.get("terrain_edits").is_none());
        let city = City::build(120, None).expect("city");
        let value = city.to_value();
        assert!(value.get("terrain_deltas").is_none());
        assert!(value.get("terrain_checksum").is_none());
    }

    #[test]
    fn smoke_runs_a_short_gate_and_pins_the_default_checksum() {
        let parsed = parse_args(&args(["--smoke"])).expect("parse");
        assert!(parsed.smoke);
        assert_eq!(parsed.cycles, SMOKE_CYCLES);
        assert_eq!(parsed.placements, DEFAULT_PLACEMENTS);
        let summary = run(parsed).expect("smoke");
        assert_eq!(summary["status"], "ok");
        assert_eq!(
            summary["combined_checksum"],
            format!("{DEFAULT_COMBINED_CHECKSUM:016x}")
        );
        assert_eq!(summary["cycles"], SMOKE_CYCLES);
    }

    #[test]
    fn explicit_cycles_override_the_smoke_default() {
        let parsed = parse_args(&args(["--smoke", "--cycles", "7"])).expect("parse");
        assert_eq!(parsed.cycles, 7);
        let parsed = parse_args(&args(["--cycles=7", "--smoke"])).expect("parse");
        assert_eq!(parsed.cycles, 7);
    }

    #[test]
    fn unknown_and_malformed_flags_are_rejected() {
        assert!(parse_args(&args(["--cycle", "5"])).is_err());
        assert!(parse_args(&args(["--terrain-edits"])).is_err());
        assert!(parse_args(&args(["--cycles", "abc"])).is_err());
        assert!(parse_args(&args(["--cycles=0"])).is_err());
        assert!(parse_args(&args(["--terrain=1"])).is_err());
        assert!(parse_args(&args(["60"])).is_err());
        assert!(parse_args(&args(["--cycles", "1", "extra"])).is_err());
    }

    #[test]
    fn explicit_flags_parse() {
        let parsed = parse_args(&args([
            "--cycles=3",
            "--placements",
            "56",
            "--terrain",
            "--terrain-edits=500",
            "--timing",
        ]))
        .expect("parse");
        assert_eq!(parsed.cycles, 3);
        assert_eq!(parsed.placements, 56);
        assert!(parsed.terrain);
        assert_eq!(parsed.terrain_edits(), Some(500));
        assert!(parsed.timing);
        assert!(!parsed.smoke);
    }

    #[test]
    fn timing_cycle_excludes_validation_and_verification() {
        let summary = run(Cli {
            cycles: 2,
            timing: true,
            ..cli(2, 120, None)
        })
        .expect("timing stress");
        let timing = &summary["timing"];
        let cycle_total = timing["cycle_ms_total"].as_f64().expect("cycle total");
        let compose = timing["compose_ms"].as_f64().expect("compose");
        let serialize = timing["serialize_ms"].as_f64().expect("serialize");
        let parse = timing["parse_ms"].as_f64().expect("parse");
        let loop_total = timing["loop_ms_total"].as_f64().expect("loop total");
        assert!((cycle_total - (compose + serialize + parse)).abs() <= 0.01);
        assert!(timing["validate_ms"].as_f64().expect("validate") > 0.0);
        assert!(timing["verify_ms"].as_f64().expect("verify") > 0.0);
        assert!(loop_total >= cycle_total);
    }

    #[test]
    fn terrain_fixture_is_deterministic() {
        let first = terrain_fixture(DEFAULT_TERRAIN_EDITS).expect("terrain");
        let second = terrain_fixture(DEFAULT_TERRAIN_EDITS).expect("terrain");
        assert_eq!(first, second);
        assert_eq!(first.edit_count(), DEFAULT_TERRAIN_EDITS);
        assert_eq!(first.chunk_count(), TERRAIN_CHUNKS);
        assert_eq!(first.checksum(), second.checksum());
        assert_eq!(first.encode(), second.encode());
    }

    #[test]
    fn terrain_hex_round_trips() {
        let store = terrain_fixture(500).expect("terrain");
        let encoded = hex_encode(&store.encode());
        let decoded = terrain_from_hex(&encoded, store.checksum()).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(hex_encode(&decoded.encode()), encoded);
    }

    #[test]
    fn invalid_terrain_hex_is_rejected() {
        let store = terrain_fixture(100).expect("terrain");
        let checksum = store.checksum();
        assert!(terrain_from_hex("zz", checksum).is_err());
        assert!(terrain_from_hex("abc", checksum).is_err());
        assert!(terrain_from_hex(&hex_encode(&store.encode()), checksum ^ 1).is_err());
        let mut corrupt = store.encode();
        corrupt[0] = b'X';
        let encoded = hex_encode(&corrupt);
        assert!(terrain_from_hex(&encoded, DeltaStore::checksum_bytes(&corrupt)).is_err());
    }

    #[test]
    fn terrain_enabled_round_trips_in_the_combined_value() {
        let summary = run(cli(1, 120, Some(256))).expect("stress");
        assert_eq!(summary["status"], "ok");
        assert_eq!(summary["terrain_edits"], 256);
        assert_eq!(summary["terrain_chunks"], TERRAIN_CHUNKS);
        assert!(summary["terrain_bytes"].as_u64().expect("bytes") > 0);
        assert_ne!(summary["combined_checksum"], "0ce611887161ff62");
    }
}
