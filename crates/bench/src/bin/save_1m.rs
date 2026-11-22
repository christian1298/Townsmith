//! 1M-population / 250k-placement full-world save-load data harness.
//!
//! Builds the Phase 5 save shape — 250,000 placements, 250,000 four-resident
//! households (1,000,000 population), 2,000 named citizens, a completed
//! construction site per placement, 30 days of economy and industry history,
//! industry/utility/service sites, research, a 64×64-block road grid, zoned
//! lots, settlements, and a deterministic terrain delta set — composes every
//! core store into one JSON envelope, writes it to a real file, reads it back,
//! and re-parses and validates every store against pinned checksums.
//!
//! This measures the core-store save/load path. It is not the Godot
//! `world_save` wrapper and does not assert disk atomicity or durability. The
//! 30,000 ms save and load targets are the Phase 5 gate values from
//! [[68 - Metropolis Gate]] §5.4.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use core_city::{
    Age, CityRuntime, ConstructionPhase, ConstructionStore, Economy, Home, IndustryKind,
    IndustryStore, InfrastructureCosts, LotRequest, LotStore, NamedCitizens, NamedSave,
    PlacementIndex, PlacementRequest, PlacementStore, ResearchStore, RoadRequest, RoadStore,
    ServiceConsumer, ServiceKind, ServiceStore, ServiceTier, SettlementRequest, SettlementStore,
    TechTree, UtilityConsumer, UtilityKind, UtilityStore, Zone, DAY_SECONDS,
    DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS, MAX_HOUSEHOLDS, MAX_MOVE_IN_PER_SECOND,
    MAX_NAMED_CITIZENS, MAX_ROAD_LENGTH_M, MIN_LOT_SIDE_M,
};
use core_world::{DeltaStore, WorldChunkCoord, MAX_HEIGHT_M, WATER_LEVEL_M};
use serde::{Deserialize, Serialize};

const SEED: u64 = 2026;
const DEFAULT_PLACEMENTS: usize = 250_000;
const SMOKE_PLACEMENTS: usize = 25_000;
const MIN_PLACEMENTS: usize = 1_000;
/// Upper bound for `--placements`. The gate shape is 250,000; the larger cap
/// leaves headroom for a stress run without letting a typo exhaust memory.
const MAX_PLACEMENTS: usize = 500_000;
const RESIDENTS_PER_HOUSEHOLD: u32 = 4;
const NAMED_CITIZENS: usize = 2_000;
/// Phase 5 save/load target, each direction.
const TARGET_MS: f64 = 30_000.0;
const SPACING_M: f32 = 10.0;
const BOUNDS_M: [f32; 3] = [8.0, 6.0, 8.0];
const GROUND_M: u16 = 40;

/// Simulated operating history for the economy and industry stores.
const HISTORY_DAYS: usize = 30;
/// One road block per side of the city; 64×64 blocks, 65 centre lines.
const ROAD_BLOCKS: usize = 64;
const ROAD_WIDTH_M: u8 = 6;
/// Lot edge inset from the nearest road centreline. A 6 m road has a 3 m
/// half-width; the extra 2 m matches the lot frontage gap.
const LOT_INSET_M: i32 = 5;
const SETTLEMENT_COUNT: usize = 4;
const INDUSTRY_SITES_PER_1K: usize = 1;
const UTILITY_SITES_PER_1K: usize = 1;
const SERVICE_SITES_PER_KIND_PER_1K: usize = 1;
const JOBS_PER_INDUSTRY_SITE: u32 = 10;

const TERRAIN_EDITS: usize = 2_000;
const TERRAIN_CHUNKS_PER_SIDE: usize = 8;
const TERRAIN_CELLS_PER_SIDE: usize = 256;
const TERRAIN_CHUNKS: usize = TERRAIN_CHUNKS_PER_SIDE * TERRAIN_CHUNKS_PER_SIDE;

const STORE_NAMES: [&str; 12] = [
    "placements",
    "construction",
    "population",
    "economy",
    "industry",
    "utilities",
    "services",
    "research",
    "named_citizens",
    "settlements",
    "roads",
    "lots",
];

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    let strict = std::env::args().any(|arg| arg == "--strict");
    let placements = if smoke {
        SMOKE_PLACEMENTS
    } else {
        arg_value("--placements")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PLACEMENTS)
    };
    if !(MIN_PLACEMENTS..=MAX_PLACEMENTS).contains(&placements) {
        eprintln!(
            "save_1m: --placements must be {MIN_PLACEMENTS}..={MAX_PLACEMENTS}, got {placements}"
        );
        std::process::exit(2);
    }
    if placements > MAX_HOUSEHOLDS {
        eprintln!("save_1m: {placements} households exceeds MAX_HOUSEHOLDS {MAX_HOUSEHOLDS}");
        std::process::exit(2);
    }
    let json_path = arg_value("--json").map(PathBuf::from);

    let result = if smoke {
        run_smoke(json_path.as_deref())
    } else {
        run_case(placements, json_path.as_deref())
    };
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("report json")
            );
            if report["status"] != "ok" {
                eprintln!(
                    "SAVE_1M target miss: {}",
                    report["miss"]
                        .as_array()
                        .map(|misses| misses
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                );
                if strict {
                    std::process::exit(1);
                }
            }
        }
        Err(error) => {
            eprintln!("SAVE_1M status=failed error={error}");
            std::process::exit(1);
        }
    }
}

/// Run the smoke fixture twice and require identical counts and checksums.
fn run_smoke(path: Option<&Path>) -> Result<serde_json::Value, String> {
    let mut first = run_case(SMOKE_PLACEMENTS, path)?;
    let second = run_case(SMOKE_PLACEMENTS, path)?;
    for field in [
        "placements",
        "construction_sites",
        "households",
        "population",
        "named",
        "industry_sites",
        "utility_sites",
        "service_sites",
        "research_completed",
        "settlements",
        "roads",
        "lots",
        "terrain_edits",
        "terrain_chunks",
    ] {
        if first[field] != second[field] {
            return Err(format!(
                "smoke drift in {field}: {} vs {}",
                first[field], second[field]
            ));
        }
    }
    if first["checksums"] != second["checksums"]
        || first["combined_checksum"] != second["combined_checksum"]
        || first["terrain_checksum"] != second["terrain_checksum"]
    {
        return Err(format!(
            "smoke checksum drift: {} vs {}",
            first["combined_checksum"], second["combined_checksum"]
        ));
    }
    first["deterministic"] = serde_json::Value::Bool(true);
    Ok(first)
}

fn arg_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

struct Fixture {
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
    terrain: DeltaStore,
    build_ms: f64,
    build_stages_ms: serde_json::Value,
}

impl Fixture {
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

    fn encode(&self) -> Result<String, String> {
        let terrain_hex = hex_encode(&self.terrain.encode());
        let envelope = EnvelopeRef {
            placements: &self.placements,
            construction: &self.construction,
            population: &self.runtime,
            economy: &self.economy,
            industry: &self.industry,
            utilities: &self.utilities,
            services: &self.services,
            research: &self.research,
            named_citizens: self.named.to_save(),
            settlements: &self.settlements,
            roads: &self.roads,
            lots: &self.lots,
            terrain_deltas: terrain_hex,
            terrain_checksum: format!("{:016x}", self.terrain.checksum()),
        };
        serde_json::to_string(&envelope).map_err(|error| error.to_string())
    }
}

/// Build the deterministic full-world fixture: one 8×6×8 house per 10 m grid
/// cell, four residents per household, the first 2,000 household slots named,
/// a completed construction site per placement, and deterministic
/// infrastructure scaled to the city.
fn build_fixture(placements: usize) -> Result<Fixture, String> {
    if !(MIN_PLACEMENTS..=MAX_PLACEMENTS).contains(&placements) {
        return Err(format!(
            "placements must be {MIN_PLACEMENTS}..={MAX_PLACEMENTS}, got {placements}"
        ));
    }
    let total_start = Instant::now();
    let build_start = Instant::now();
    let side = (placements as f64).sqrt().ceil() as i64;
    let offset = -((side - 1) as f32 * SPACING_M) * 0.5;
    let mut store = PlacementStore::new(SEED);
    let mut index = PlacementIndex::new();
    for slot in 0..placements {
        let column = (slot as i64) % side;
        let row = (slot as i64) / side;
        let request = PlacementRequest {
            blueprint_id: "base:tenement".to_string(),
            blueprint_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            blueprint_path: "res://content/base/tenement.tsbp".to_string(),
            name: "Tenement".to_string(),
            bounds_m: BOUNDS_M,
            center_x: offset + column as f32 * SPACING_M,
            center_z: offset + row as f32 * SPACING_M,
            rotation_deg: 0,
            material_cost: 60_000.0,
            resident_capacity: RESIDENTS_PER_HOUSEHOLD,
            job_slots: 0,
            solid_voxels: 4_800,
            structure: None,
        };
        store
            .place_indexed(&mut index, &request, |_, _| GROUND_M)
            .map_err(|error| error.to_string())?;
    }
    if index.len() != placements {
        return Err(format!(
            "placement index has {} entries, expected {placements}",
            index.len()
        ));
    }
    let placements_ms = elapsed_ms(build_start);

    // Every building already finished construction in this save shape. Sites
    // are marked complete directly: the fixture is a save, not a build
    // simulation, and ticking 250k sites would need a city-sized crew and
    // material pool to finish.
    let construction_start = Instant::now();
    let mut construction = ConstructionStore::new(64, 0);
    for placement in &store.placements {
        construction
            .add_site(placement.id, placement.solid_voxels)
            .map_err(|error| error.to_string())?;
    }
    for site in construction.sites.values_mut() {
        site.phase = ConstructionPhase::Complete;
        site.phase_progress = 1.0;
        site.elapsed_s = site.total_s;
        site.materials_delivered = site.materials_required;
        site.crew_required = 0;
        site.crew_assigned = 0;
        site.stall_reason = None;
    }
    construction.validate().map_err(|error| error.to_string())?;
    let construction_ms = elapsed_ms(construction_start);

    let population_start = Instant::now();
    let mut runtime = CityRuntime::new(SEED);
    runtime.set_move_in_per_second(MAX_MOVE_IN_PER_SECOND);
    let homes: Vec<Home> = store
        .placements
        .iter()
        .map(|placement| Home {
            id: placement.id,
            capacity: placement.resident_capacity,
            center: [placement.center[0], placement.center[2]],
        })
        .collect();
    runtime.sync_homes(&homes);
    // Fill every home directly: the fixture is a save shape, not a move-in
    // simulation. Needs stay at 1.0 so the saved floats are representative.
    for household in runtime.households.values_mut() {
        household.residents = household.capacity;
    }
    runtime.validate().map_err(|error| error.to_string())?;

    let mut named = NamedCitizens::new(SEED);
    named.max = NAMED_CITIZENS.min(MAX_NAMED_CITIZENS);
    named.sync(&runtime.households);
    if named.len() != NAMED_CITIZENS {
        return Err(format!(
            "named pool has {} citizens, expected {NAMED_CITIZENS}",
            named.len()
        ));
    }
    named.validate().map_err(|error| error.to_string())?;
    let population_ms = elapsed_ms(population_start);

    let infrastructure_start = Instant::now();
    let consumers: Vec<UtilityConsumer> = store
        .placements
        .iter()
        .map(|placement| UtilityConsumer {
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

    let industry_sites = scaled_sites(placements, INDUSTRY_SITES_PER_1K);
    let mut industry = IndustryStore::new(SEED);
    let mut staffing = BTreeMap::new();
    for site in 0..industry_sites {
        let placement = &store.placements[site_index(site, industry_sites, placements)];
        let kind = match site % 3 {
            0 => IndustryKind::Farm,
            1 => IndustryKind::Lumberjack,
            _ => IndustryKind::Hunt,
        };
        industry
            .add_site(placement.id, kind)
            .map_err(|error| error.to_string())?;
        staffing.insert(placement.id, JOBS_PER_INDUSTRY_SITE);
    }
    for _ in 0..HISTORY_DAYS {
        industry.tick_with_power(DAY_SECONDS, &staffing, runtime.population(), 1.0);
    }
    industry.validate().map_err(|error| error.to_string())?;

    let utility_sites = scaled_sites(placements, UTILITY_SITES_PER_1K);
    let mut utilities = UtilityStore::new(SEED);
    for site in 0..utility_sites {
        let placement = &store.placements[site_index(site, utility_sites, placements)];
        let kind = if site % 2 == 0 {
            UtilityKind::Water
        } else {
            UtilityKind::Power
        };
        utilities
            .add_site(
                placement.id,
                kind,
                [
                    placement.center[0],
                    f32::from(GROUND_M),
                    placement.center[2],
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    utilities.refresh(&consumers);
    utilities.validate().map_err(|error| error.to_string())?;

    let service_sites = scaled_sites(placements, SERVICE_SITES_PER_KIND_PER_1K);
    let service_kinds = [
        ServiceKind::School,
        ServiceKind::Clinic,
        ServiceKind::Fire,
        ServiceKind::Police,
    ];
    let mut services = ServiceStore::new(SEED);
    let total_service_sites = service_sites * service_kinds.len();
    for (kind_index, kind) in service_kinds.iter().enumerate() {
        for site in 0..service_sites {
            let index = kind_index * service_sites + site;
            let placement = &store.placements[site_index(index, total_service_sites, placements)];
            services
                .add_site(
                    placement.id,
                    *kind,
                    ServiceTier::Basic,
                    [
                        placement.center[0],
                        f32::from(GROUND_M),
                        placement.center[2],
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    let service_consumers: Vec<ServiceConsumer> = consumers
        .iter()
        .map(|consumer| ServiceConsumer {
            id: consumer.id,
            center: consumer.center,
            residents: consumer.residents,
            workers: consumer.workers,
        })
        .collect();
    services.refresh(&service_consumers);
    services.validate().map_err(|error| error.to_string())?;
    let infrastructure_ms = elapsed_ms(infrastructure_start);

    let research_start = Instant::now();
    let tree = TechTree::default_tree();
    let mut research = ResearchStore::new(SEED);
    for _ in 0..10 {
        research.tick(DAY_SECONDS, 5.0);
    }
    for node in [
        "construction.timber_frame",
        "industry.farming",
        "utilities.well",
    ] {
        research
            .unlock(node, &tree, Age::Wood)
            .map_err(|error| error.to_string())?;
    }
    research
        .validate(&tree)
        .map_err(|error| error.to_string())?;
    let research_ms = elapsed_ms(research_start);

    let economy_start = Instant::now();
    let mut economy = Economy::new(SEED);
    economy
        .take_loan(250_000.0, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS)
        .map_err(|error| error.to_string())?;
    let costs = InfrastructureCosts::new(
        utilities.total_upkeep_per_day(),
        services.total_upkeep_per_day(),
    )
    .map_err(|error| error.to_string())?;
    for _ in 0..HISTORY_DAYS {
        economy.tick_with_infrastructure_costs(
            DAY_SECONDS,
            runtime.population(),
            0,
            placements as u64,
            costs,
        );
    }
    economy.validate().map_err(|error| error.to_string())?;
    let economy_ms = elapsed_ms(economy_start);

    let networks_start = Instant::now();
    let (_, city_min_m, city_max_m) = city_bounds(placements);
    let lines = grid_lines(city_min_m, city_max_m, ROAD_BLOCKS);
    let mut roads = RoadStore::new(SEED);
    for &z in &lines {
        add_road_run(&mut roads, [city_min_m, z], [city_max_m, z])?;
    }
    for &x in &lines {
        add_road_run(&mut roads, [x, city_min_m], [x, city_max_m])?;
    }
    roads.validate_loaded().map_err(|error| error.to_string())?;

    let mut lots = LotStore::new(SEED);
    for block_x in 0..ROAD_BLOCKS {
        for block_z in 0..ROAD_BLOCKS {
            let bounds = [
                lines[block_x] + LOT_INSET_M,
                lines[block_z] + LOT_INSET_M,
                lines[block_x + 1] - LOT_INSET_M,
                lines[block_z + 1] - LOT_INSET_M,
            ];
            if bounds[2] - bounds[0] < MIN_LOT_SIDE_M || bounds[3] - bounds[1] < MIN_LOT_SIDE_M {
                // Very small `--placements` runs have blocks too narrow for a
                // valid parcel; the envelope still carries an empty lot store.
                continue;
            }
            let zone = match (block_x + block_z) % Zone::ALL.len() {
                0 => Zone::Residential,
                1 => Zone::Commercial,
                _ => Zone::Industrial,
            };
            lots.add(&LotRequest { bounds, zone }, |_, _| GROUND_M, &roads)
                .map_err(|error| error.to_string())?;
        }
    }
    lots.validate_loaded().map_err(|error| error.to_string())?;

    let radius = u32::try_from((city_max_m - city_min_m) / 2)
        .unwrap_or(0)
        .clamp(
            core_city::MIN_SETTLEMENT_RADIUS_M,
            core_city::MAX_SETTLEMENT_RADIUS_M,
        );
    let quarter = (city_max_m - city_min_m) / 4;
    let settlement_centers = [
        [city_min_m + quarter, city_min_m + quarter],
        [city_max_m - quarter, city_min_m + quarter],
        [city_min_m + quarter, city_max_m - quarter],
        [city_max_m - quarter, city_max_m - quarter],
    ];
    let mut settlements = SettlementStore::new(SEED);
    for (number, center) in settlement_centers.iter().take(SETTLEMENT_COUNT).enumerate() {
        settlements
            .found(&SettlementRequest {
                name: format!("Townsmith {}", number + 1),
                owner: "player".to_string(),
                center: *center,
                radius_m: radius,
            })
            .map_err(|error| error.to_string())?;
    }
    settlements
        .validate_loaded()
        .map_err(|error| error.to_string())?;
    let networks_ms = elapsed_ms(networks_start);

    let terrain_start = Instant::now();
    let terrain = terrain_fixture(TERRAIN_EDITS)?;
    let terrain_ms = elapsed_ms(terrain_start);

    let build_ms = elapsed_ms(total_start);
    let city = Fixture {
        placements: store,
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
        build_ms,
        build_stages_ms: serde_json::json!({
            "placements": round_ms(placements_ms),
            "construction": round_ms(construction_ms),
            "population": round_ms(population_ms),
            "infrastructure": round_ms(infrastructure_ms),
            "research": round_ms(research_ms),
            "economy": round_ms(economy_ms),
            "networks": round_ms(networks_ms),
            "terrain": round_ms(terrain_ms),
        }),
    };
    city.validate()?;
    Ok(city)
}

impl Fixture {
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

        if self.construction.sites.len() != self.placements.placements.len() {
            return Err(format!(
                "construction has {} sites for {} placements",
                self.construction.sites.len(),
                self.placements.placements.len()
            ));
        }
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
        let encoded = self.terrain.encode();
        let decoded =
            DeltaStore::decode(&encoded).map_err(|error| format!("terrain deltas: {error}"))?;
        if decoded != self.terrain {
            return Err("terrain deltas changed during roundtrip".to_string());
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct EnvelopeRef<'a> {
    placements: &'a PlacementStore,
    construction: &'a ConstructionStore,
    population: &'a CityRuntime,
    economy: &'a Economy,
    industry: &'a IndustryStore,
    utilities: &'a UtilityStore,
    services: &'a ServiceStore,
    research: &'a ResearchStore,
    named_citizens: NamedSave,
    settlements: &'a SettlementStore,
    roads: &'a RoadStore,
    lots: &'a LotStore,
    terrain_deltas: String,
    terrain_checksum: String,
}

#[derive(Deserialize)]
struct EnvelopeSave {
    placements: PlacementStore,
    construction: ConstructionStore,
    population: CityRuntime,
    economy: Economy,
    industry: IndustryStore,
    utilities: UtilityStore,
    services: ServiceStore,
    research: ResearchStore,
    named_citizens: NamedSave,
    settlements: SettlementStore,
    roads: RoadStore,
    lots: LotStore,
    terrain_deltas: String,
    terrain_checksum: String,
}

struct Loaded {
    serialize_ms: f64,
    file_write_ms: f64,
    file_read_ms: f64,
    parse_ms: f64,
    verify_ms: f64,
    json_bytes: u64,
    checksums: [u64; 12],
    terrain_checksum: u64,
    economy_checksum_exact: bool,
    combined_checksum: u64,
}

fn run_case(placements: usize, path: Option<&Path>) -> Result<serde_json::Value, String> {
    let fixture = build_fixture(placements)?;
    let expected = fixture.checksums();
    let expected_terrain = fixture.terrain.checksum();
    let expected_combined = combined_checksum(&expected, expected_terrain);
    let json_path = path.map_or_else(|| temp_save_path(placements), Path::to_path_buf);
    let temporary = path.is_none();

    let result = round_trip(
        &fixture,
        expected,
        expected_terrain,
        expected_combined,
        &json_path,
    );
    if temporary {
        let _ = fs::remove_file(&json_path);
    }
    let loaded = result?;

    let households = fixture.runtime.households.len();
    let population = fixture.runtime.population();
    let named = fixture.named.len();
    let total_save_ms = loaded.serialize_ms + loaded.file_write_ms;
    let total_load_ms = loaded.file_read_ms + loaded.parse_ms + loaded.verify_ms;
    let save_within_target = total_save_ms <= TARGET_MS;
    let load_within_target = total_load_ms <= TARGET_MS;
    let mut misses = Vec::new();
    if !save_within_target {
        misses.push(format!(
            "save {:.3} ms exceeds {:.0} ms",
            total_save_ms, TARGET_MS
        ));
    }
    if !load_within_target {
        misses.push(format!(
            "load {:.3} ms exceeds {:.0} ms",
            total_load_ms, TARGET_MS
        ));
    }

    let mut report = serde_json::json!({
        "scene": "save_1m",
        "seed": SEED,
        "placements": fixture.placements.placements.len(),
        "construction_sites": fixture.construction.site_count(),
        "households": households,
        "population": population,
        "named": named,
        "residents_per_household": RESIDENTS_PER_HOUSEHOLD,
        "industry_sites": fixture.industry.site_count(),
        "utility_sites": fixture.utilities.site_count(),
        "service_sites": fixture.services.site_count(),
        "research_completed": fixture.research.completed_count(),
        "settlements": fixture.settlements.settlements.len(),
        "roads": fixture.roads.roads.len(),
        "lots": fixture.lots.lots.len(),
        "terrain_edits": fixture.terrain.edit_count(),
        "terrain_chunks": fixture.terrain.chunk_count(),
        "terrain_bytes": fixture.terrain.encode().len(),
        "economy_days": fixture.economy.days_elapsed,
        "grid_side": (placements as f64).sqrt().ceil() as i64,
        "spacing_m": SPACING_M,
        "bounds_m": BOUNDS_M,
        "build_ms": round_ms(fixture.build_ms),
        "build_stages_ms": fixture.build_stages_ms,
        "serialize_ms": round_ms(loaded.serialize_ms),
        "file_write_ms": round_ms(loaded.file_write_ms),
        "file_read_ms": round_ms(loaded.file_read_ms),
        "parse_ms": round_ms(loaded.parse_ms),
        "verify_ms": round_ms(loaded.verify_ms),
        "total_save_ms": round_ms(total_save_ms),
        "total_load_ms": round_ms(total_load_ms),
        "target_ms": TARGET_MS,
        "save_within_target": save_within_target,
        "load_within_target": load_within_target,
        "json_bytes": loaded.json_bytes,
        "json_path": json_path.to_string_lossy(),
        "checksums": checksum_map(&loaded.checksums, loaded.terrain_checksum),
        "terrain_checksum": format!("{:016x}", loaded.terrain_checksum),
        "economy_checksum_exact": loaded.economy_checksum_exact,
        "combined_checksum": format!("{:016x}", loaded.combined_checksum),
        "status": if misses.is_empty() { "ok" } else { "miss" },
    });
    if !misses.is_empty() {
        report["miss"] = serde_json::json!(misses);
    }
    if let Some(mb) = peak_rss_mb() {
        report["peak_rss_mb"] = serde_json::json!(mb);
    }
    Ok(report)
}

fn round_trip(
    fixture: &Fixture,
    expected: [u64; 12],
    expected_terrain: u64,
    expected_combined: u64,
    json_path: &Path,
) -> Result<Loaded, String> {
    let serialize_start = Instant::now();
    let json = fixture.encode()?;
    let serialize_ms = elapsed_ms(serialize_start);
    let json_bytes = json.len() as u64;

    let write_start = Instant::now();
    {
        let mut file = File::create(json_path)
            .map_err(|error| format!("create {}: {error}", json_path.display()))?;
        file.write_all(json.as_bytes())
            .map_err(|error| format!("write {}: {error}", json_path.display()))?;
        file.flush()
            .map_err(|error| format!("flush {}: {error}", json_path.display()))?;
    }
    let file_write_ms = elapsed_ms(write_start);
    // Release the serialized string before the load allocates its copy.
    drop(json);

    let read_start = Instant::now();
    let encoded = fs::read_to_string(json_path)
        .map_err(|error| format!("read {}: {error}", json_path.display()))?;
    let file_read_ms = elapsed_ms(read_start);

    let parse_start = Instant::now();
    let parsed: EnvelopeSave =
        serde_json::from_str(&encoded).map_err(|error| format!("parse combined json: {error}"))?;
    let parse_ms = elapsed_ms(parse_start);
    drop(encoded);

    let verify_start = Instant::now();
    verify_loaded(fixture, &parsed, expected, expected_terrain)?;
    let declared_terrain = parse_checksum(&parsed.terrain_checksum)?;
    let terrain = terrain_from_hex(&parsed.terrain_deltas, declared_terrain)?;

    let named =
        NamedCitizens::from_save(parsed.named_citizens).map_err(|error| error.to_string())?;
    let actual = [
        parsed.placements.checksum(),
        parsed.construction.checksum(),
        parsed.population.checksum(),
        parsed.economy.checksum(),
        parsed.industry.checksum(),
        parsed.utilities.checksum(),
        parsed.services.checksum(),
        parsed.research.checksum(),
        named.checksum(),
        parsed.settlements.checksum(),
        parsed.roads.checksum(),
        parsed.lots.checksum(),
    ];
    let economy_checksum_exact = actual[3] == expected[3];
    let mut stable = true;
    for (index, name) in STORE_NAMES.iter().enumerate() {
        if index == 3 {
            continue;
        }
        if actual[index] != expected[index] {
            stable = false;
            eprintln!(
                "save_1m: {name} checksum changed: {:016x} != {:016x}",
                actual[index], expected[index]
            );
        }
    }
    // The combined JSON path can normalize f64 text. Compare the economy
    // semantically when its byte checksum changes and report the exactness
    // flag instead of failing a semantically identical save.
    if !economy_checksum_exact && !economy_matches(&parsed.economy, &fixture.economy) {
        stable = false;
        eprintln!("save_1m: economy state changed");
    }
    if !stable {
        return Err(format!(
            "per-store checksum changed: {actual:?} != {expected:?}"
        ));
    }
    let actual_terrain = terrain.checksum();
    if actual_terrain != expected_terrain {
        return Err(format!(
            "terrain checksum changed: {actual_terrain:016x} != {expected_terrain:016x}"
        ));
    }
    if terrain.edit_count() != fixture.terrain.edit_count() {
        return Err(format!(
            "terrain edit count changed: {} != {}",
            terrain.edit_count(),
            fixture.terrain.edit_count()
        ));
    }
    if terrain.chunk_count() != fixture.terrain.chunk_count() {
        return Err(format!(
            "terrain chunk count changed: {} != {}",
            terrain.chunk_count(),
            fixture.terrain.chunk_count()
        ));
    }
    let combined = combined_checksum(&actual, actual_terrain);
    if economy_checksum_exact && combined != expected_combined {
        return Err(format!(
            "combined checksum changed: {combined:016x} != {expected_combined:016x}"
        ));
    }
    let verify_ms = elapsed_ms(verify_start);

    Ok(Loaded {
        serialize_ms,
        file_write_ms,
        file_read_ms,
        parse_ms,
        verify_ms,
        json_bytes,
        checksums: actual,
        terrain_checksum: actual_terrain,
        economy_checksum_exact,
        combined_checksum: combined,
    })
}

fn verify_loaded(
    fixture: &Fixture,
    parsed: &EnvelopeSave,
    expected: [u64; 12],
    _expected_terrain: u64,
) -> Result<(), String> {
    parsed
        .placements
        .validate_loaded()
        .map_err(|error| format!("placements: {error}"))?;
    parsed
        .construction
        .validate()
        .map_err(|error| format!("construction: {error}"))?;
    parsed
        .population
        .validate()
        .map_err(|error| format!("population: {error}"))?;
    parsed
        .economy
        .validate()
        .map_err(|error| format!("economy: {error}"))?;
    parsed
        .industry
        .validate()
        .map_err(|error| format!("industry: {error}"))?;
    parsed
        .utilities
        .validate()
        .map_err(|error| format!("utilities: {error}"))?;
    parsed
        .services
        .validate()
        .map_err(|error| format!("services: {error}"))?;
    parsed
        .research
        .validate(&TechTree::default_tree())
        .map_err(|error| format!("research: {error}"))?;
    parsed
        .settlements
        .validate_loaded()
        .map_err(|error| format!("settlements: {error}"))?;
    parsed
        .roads
        .validate_loaded()
        .map_err(|error| format!("roads: {error}"))?;
    parsed
        .lots
        .validate_loaded()
        .map_err(|error| format!("lots: {error}"))?;
    let named = NamedCitizens::from_save(parsed.named_citizens.clone())
        .map_err(|error| error.to_string())?;
    named
        .validate()
        .map_err(|error| format!("named_citizens: {error}"))?;

    if parsed.placements.placements.len() != fixture.placements.placements.len() {
        return Err(format!(
            "placement count changed: {} != {}",
            parsed.placements.placements.len(),
            fixture.placements.placements.len()
        ));
    }
    if parsed.construction.site_count() != fixture.construction.site_count() {
        return Err(format!(
            "construction site count changed: {} != {}",
            parsed.construction.site_count(),
            fixture.construction.site_count()
        ));
    }
    if parsed.population.households.len() != fixture.runtime.households.len() {
        return Err(format!(
            "household count changed: {} != {}",
            parsed.population.households.len(),
            fixture.runtime.households.len()
        ));
    }
    if parsed.population.population() != fixture.runtime.population() {
        return Err(format!(
            "population changed: {} != {}",
            parsed.population.population(),
            fixture.runtime.population()
        ));
    }
    if named.len() != fixture.named.len() {
        return Err(format!(
            "named count changed: {} != {}",
            named.len(),
            fixture.named.len()
        ));
    }
    if parsed.industry.site_count() != fixture.industry.site_count()
        || parsed.utilities.site_count() != fixture.utilities.site_count()
        || parsed.services.site_count() != fixture.services.site_count()
    {
        return Err("infrastructure site counts changed".to_string());
    }
    if parsed.roads.roads.len() != fixture.roads.roads.len()
        || parsed.lots.lots.len() != fixture.lots.lots.len()
        || parsed.settlements.settlements.len() != fixture.settlements.settlements.len()
    {
        return Err("network counts changed".to_string());
    }

    let completed: BTreeSet<u64> = parsed
        .construction
        .sites
        .iter()
        .filter(|(_, site)| site.is_complete())
        .map(|(&id, _)| id)
        .collect();
    if completed.len() != parsed.placements.placements.len() {
        return Err("completed site count does not match placements".to_string());
    }
    for placement in &parsed.placements.placements {
        if !completed.contains(&placement.id) {
            return Err(format!("placement {} has no completed site", placement.id));
        }
    }
    for &id in parsed.population.households.keys() {
        if !completed.contains(&id) {
            return Err(format!("household {id} has no completed home"));
        }
    }
    for id in parsed
        .industry
        .sites
        .keys()
        .chain(parsed.utilities.water_sites.keys())
        .chain(parsed.utilities.power_sites.keys())
        .chain(parsed.services.sites.keys())
    {
        if !completed.contains(id) {
            return Err(format!("infrastructure site {id} is not completed"));
        }
    }
    for ((placement_id, slot), citizen) in &named.citizens {
        let household = parsed
            .population
            .households
            .get(placement_id)
            .ok_or_else(|| format!("named citizen {placement_id} has no household"))?;
        if *slot >= household.residents || citizen.id == 0 {
            return Err("named citizen slot is invalid".to_string());
        }
    }

    let declared_terrain = parse_checksum(&parsed.terrain_checksum)?;
    let decoded = terrain_from_hex(&parsed.terrain_deltas, declared_terrain)?;
    if decoded.edit_count() != fixture.terrain.edit_count() {
        return Err("terrain edit count changed".to_string());
    }
    if hex_encode(&decoded.encode()) != parsed.terrain_deltas {
        return Err("terrain deltas are not byte-stable".to_string());
    }
    let _ = expected;
    Ok(())
}

fn economy_matches(actual: &Economy, expected: &Economy) -> bool {
    (actual.balance - expected.balance).abs() < 1e-6
        && (actual.days_elapsed - expected.days_elapsed).abs() < 1e-9
        && (actual.construction_spent - expected.construction_spent).abs() < 1e-6
        && actual.loans.len() == expected.loans.len()
}

fn checksum_map(checksums: &[u64; 12], terrain_checksum: u64) -> serde_json::Value {
    let mut map = serde_json::Map::with_capacity(13);
    for (name, value) in STORE_NAMES.iter().zip(checksums) {
        map.insert(
            (*name).to_string(),
            serde_json::Value::String(format!("{value:016x}")),
        );
    }
    map.insert(
        "terrain_deltas".to_string(),
        serde_json::Value::String(format!("{terrain_checksum:016x}")),
    );
    serde_json::Value::Object(map)
}

fn combined_checksum(checksums: &[u64; 12], terrain_checksum: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in checksums
        .iter()
        .copied()
        .chain(std::iter::once(terrain_checksum))
    {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// A scaled deterministic site budget: one per `per_1k` units of 1,000
/// placements, at least one.
fn scaled_sites(placements: usize, per_1k: usize) -> usize {
    (placements / 1_000).max(1) * per_1k
}

/// Evenly spread a site over the placement list so infrastructure is not
/// clustered in the first rows.
fn site_index(site: usize, count: usize, placements: usize) -> usize {
    if count == 0 {
        return 0;
    }
    (site * placements / count).min(placements - 1)
}

fn city_bounds(placements: usize) -> (i64, i32, i32) {
    let side = (placements as f64).sqrt().ceil() as i64;
    let offset = -((side - 1) as f32 * SPACING_M) * 0.5;
    (
        side,
        offset.round() as i32,
        (offset + (side - 1) as f32 * SPACING_M).round() as i32,
    )
}

fn grid_lines(min: i32, max: i32, blocks: usize) -> Vec<i32> {
    let span = i64::from(max - min);
    (0..=blocks)
        .map(|index| min + (index as i64 * span / blocks as i64) as i32)
        .collect()
}

/// Add one road run as consecutive segments no longer than the store limit.
fn add_road_run(roads: &mut RoadStore, start: [i32; 2], end: [i32; 2]) -> Result<(), String> {
    let dx = i64::from(end[0]) - i64::from(start[0]);
    let dz = i64::from(end[1]) - i64::from(start[1]);
    let length = dx.abs() + dz.abs();
    if length == 0 {
        return Err("zero-length road run".to_string());
    }
    let limit = i64::from(MAX_ROAD_LENGTH_M);
    let pieces = ((length + limit - 1) / limit).max(1);
    for piece in 0..pieces {
        let from = [
            start[0] + (dx * piece / pieces) as i32,
            start[1] + (dz * piece / pieces) as i32,
        ];
        let to = [
            start[0] + (dx * (piece + 1) / pieces) as i32,
            start[1] + (dz * (piece + 1) / pieces) as i32,
        ];
        roads
            .add(
                &RoadRequest {
                    start: from,
                    end: to,
                    width_m: ROAD_WIDTH_M,
                },
                |_, _| GROUND_M,
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Build the deterministic terrain fixture: edits are dealt round-robin across
/// an 8×8 chunk block, cells never repeat within a chunk, and heights come from
/// a seeded splitmix64 stream.
fn terrain_fixture(edit_count: usize) -> Result<DeltaStore, String> {
    if edit_count == 0
        || edit_count > TERRAIN_CHUNKS * TERRAIN_CELLS_PER_SIDE * TERRAIN_CELLS_PER_SIDE
    {
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

fn parse_checksum(value: &str) -> Result<u64, String> {
    u64::from_str_radix(value, 16).map_err(|error| format!("terrain_checksum: {error}"))
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

fn temp_save_path(placements: usize) -> PathBuf {
    let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "townsmith_save_1m_{}_{placements}_{serial}.json",
        std::process::id()
    ))
}

/// Peak resident set in MiB on Linux, best effort.
fn peak_rss_mb() -> Option<f64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some((kb / 1024.0 * 100.0).round() / 100.0)
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

fn round_ms(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_fixture_has_the_expected_counts_and_checksums() {
        let fixture = build_fixture(SMOKE_PLACEMENTS).expect("fixture");
        assert_eq!(fixture.placements.placements.len(), SMOKE_PLACEMENTS);
        assert_eq!(fixture.construction.site_count(), SMOKE_PLACEMENTS);
        assert_eq!(fixture.runtime.households.len(), SMOKE_PLACEMENTS);
        assert_eq!(
            fixture.runtime.population(),
            u64::try_from(SMOKE_PLACEMENTS).expect("count") * u64::from(RESIDENTS_PER_HOUSEHOLD)
        );
        assert_eq!(fixture.named.len(), NAMED_CITIZENS);
        assert_eq!(fixture.placements.next_id, SMOKE_PLACEMENTS as u64 + 1);
        assert_eq!(fixture.terrain.edit_count(), TERRAIN_EDITS);
        assert_eq!(fixture.terrain.chunk_count(), TERRAIN_CHUNKS);
        // The placement, population, and named checksums are unchanged from
        // the three-store harness; the full fixture pins every store.
        let checksums = fixture.checksums();
        assert_eq!(checksums[0], 0x2282_a192_60d4_58a3);
        assert_eq!(checksums[2], 0xba8b_5730_dc27_7d66);
        assert_eq!(checksums[8], 0xccf4_ddd6_5c95_1f76);
    }

    #[test]
    fn smoke_envelope_has_the_full_world_keys() {
        let fixture = build_fixture(MIN_PLACEMENTS).expect("fixture");
        let value: serde_json::Value =
            serde_json::from_str(&fixture.encode().expect("encode")).expect("json");
        let object = value.as_object().expect("object");
        for key in [
            "placements",
            "construction",
            "population",
            "economy",
            "industry",
            "utilities",
            "services",
            "research",
            "named_citizens",
            "settlements",
            "roads",
            "lots",
            "terrain_deltas",
            "terrain_checksum",
        ] {
            assert!(object.contains_key(key), "missing envelope key {key}");
        }
        assert_eq!(object.len(), 14);
    }

    #[test]
    fn smoke_save_load_round_trip_is_deterministic() {
        let first = run_case(SMOKE_PLACEMENTS, None).expect("first run");
        let second = run_case(SMOKE_PLACEMENTS, None).expect("second run");
        assert_eq!(first["status"], "ok");
        assert_eq!(first["placements"], SMOKE_PLACEMENTS);
        assert_eq!(first["construction_sites"], SMOKE_PLACEMENTS);
        assert_eq!(first["population"], SMOKE_PLACEMENTS * 4);
        assert_eq!(first["named"], NAMED_CITIZENS);
        assert_eq!(first["terrain_edits"], TERRAIN_EDITS);
        assert!(first["json_bytes"].as_u64().expect("bytes") > 0);
        assert_eq!(first["checksums"], second["checksums"]);
        assert_eq!(first["combined_checksum"], second["combined_checksum"]);
        assert_eq!(first["terrain_checksum"], second["terrain_checksum"]);
    }

    #[test]
    fn terrain_fixture_is_deterministic() {
        let first = terrain_fixture(TERRAIN_EDITS).expect("terrain");
        let second = terrain_fixture(TERRAIN_EDITS).expect("terrain");
        assert_eq!(first, second);
        assert_eq!(first.edit_count(), TERRAIN_EDITS);
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
}
