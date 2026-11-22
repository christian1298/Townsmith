//! Deterministic 2,000-citizen materialized-movement microbenchmark.
//!
//! Builds a 64x64 road grid (16 m spacing), 400 homes with five residents
//! each, and 200 workplaces with ten slots each. The active materialization
//! selection holds 2,000 citizens; the detailed slice is the nearest 200.
//! `CitizenMotion::new` groups the selection by household, chooses a reachable
//! workplace per home, and builds one cached home-work polyline per household.
//! The benchmark times that route build and samples positions at 0 s, 30 s,
//! and 120 s.
//!
//! This is derived presentation state: no save file, renderer, or game loop is
//! involved. The scene is deterministic for a fixed seed and configuration, so
//! `--smoke` builds it twice and fails unless the checksums and the 30 s
//! positions match.

use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::Instant;

use core_city::{
    citizen_id, name_for, CitizenMotion, CitizenPosition, MaterializationSelection,
    MaterializedCitizen, RoadGraph, RoadSegment, Workplace, DEFAULT_ROUTE_CACHE_CAPACITY,
    MAX_ACTIVE_CITIZENS, MAX_DETAILED_CITIZENS,
};

const SEED: u64 = 2026;
const DEFAULT_GRID: usize = 64;
const SPACING_M: i32 = 16;
const DEFAULT_HOMES: usize = 400;
const RESIDENTS_PER_HOME: usize = 5;
const DEFAULT_WORKPLACES: usize = 200;
const WORKPLACE_SLOTS: u32 = 10;

/// Sane upper bounds for the optional CLI overrides.
const MAX_HOMES: usize = 1_024;
const MAX_WORKPLACES: usize = 1_024;

/// Sample times reported for one motion build.
const SAMPLE_TIMES_S: [f64; 3] = [0.0, 30.0, 120.0];

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

fn main() {
    let homes = arg_value("--homes")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_HOMES);
    let workplaces = arg_value("--workplaces")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_WORKPLACES);
    let citizens = arg_value("--citizens")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(|| homes.saturating_mul(RESIDENTS_PER_HOME));
    let smoke = std::env::args().any(|argument| argument == "--smoke");

    let config = FixtureConfig {
        grid: DEFAULT_GRID,
        homes,
        workplaces,
        citizens,
    };
    if let Err(message) = config.validate() {
        eprintln!(
            "usage: movement [--smoke] [--homes N] [--workplaces N] [--citizens N]\n{message}"
        );
        std::process::exit(2);
    }

    let fixture = Fixture::build(config);
    let measured = measure(&fixture);

    // A second build proves the derived state is reproducible. The second
    // build is not timed: the reported build_ms belongs to the first one.
    let rebuilt = CitizenMotion::new(
        &fixture.selection,
        &fixture.workplaces,
        &fixture.graph,
        DEFAULT_ROUTE_CACHE_CAPACITY,
    );
    let rebuilt_positions = rebuilt.positions_at(SAMPLE_TIMES_S[1]);
    let deterministic = rebuilt.checksum() == measured.motion.checksum()
        && rebuilt.cache_checksum() == measured.motion.cache_checksum()
        && rebuilt_positions == measured.positions[1];

    let status = if deterministic { "ok" } else { "mismatch" };
    let result = serde_json::json!({
        "scene": "movement",
        "status": status,
        "deterministic": deterministic,
        "seed": SEED,
        "grid": config.grid,
        "spacing_m": SPACING_M,
        "roads": fixture.roads.len(),
        "homes": config.homes,
        "residents_per_home": RESIDENTS_PER_HOME,
        "workplaces": fixture.workplaces.len(),
        "workplace_slots": WORKPLACE_SLOTS,
        "citizens": measured.motion.citizen_count(),
        "detailed": fixture.selection.detailed.len(),
        "routes": measured.motion.route_count(),
        "build_ms": measured.build_ms,
        "position_ms": measured.position_ms_total,
        "position_samples": SAMPLE_TIMES_S.len(),
        "positions": measured.positions[0].len(),
        "position_ms_0s": measured.position_ms[0],
        "position_ms_30s": measured.position_ms[1],
        "position_ms_120s": measured.position_ms[2],
        "position_checksum_30s": format!("{:016x}", measured.position_checksums[1]),
        "cache_hits": measured.motion.cache_stats().hits,
        "cache_misses": measured.motion.cache_stats().misses,
        "cache_entries": measured.motion.cache_stats().len,
        "cache_capacity": measured.motion.cache_stats().capacity,
        "cache_checksum": format!("{:016x}", measured.motion.cache_checksum()),
        "checksum": format!("{:016x}", measured.motion.checksum()),
    });

    if smoke && !deterministic {
        eprintln!("movement smoke: non-deterministic results");
        println!("{}", serde_json::to_string(&result).expect("compact json"));
        std::process::exit(1);
    }

    if smoke {
        println!("{}", serde_json::to_string(&result).expect("compact json"));
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("pretty json")
        );
    }
}

/// Fixture dimensions for one benchmark run.
#[derive(Debug, Clone, Copy)]
struct FixtureConfig {
    grid: usize,
    homes: usize,
    workplaces: usize,
    citizens: usize,
}

impl FixtureConfig {
    fn validate(&self) -> Result<(), String> {
        if self.grid < 2 {
            return Err("grid must be at least 2".to_string());
        }
        let cells = self.grid * self.grid;
        if self.homes == 0 || self.homes > MAX_HOMES {
            return Err(format!("homes must be in 1..={MAX_HOMES}"));
        }
        if self.workplaces == 0 || self.workplaces > MAX_WORKPLACES {
            return Err(format!("workplaces must be in 1..={MAX_WORKPLACES}"));
        }
        if self.homes + self.workplaces > cells {
            return Err(format!("homes + workplaces must fit in {cells} grid nodes"));
        }
        if self.citizens == 0 || self.citizens > MAX_ACTIVE_CITIZENS {
            return Err(format!("citizens must be in 1..={MAX_ACTIVE_CITIZENS}"));
        }
        if self.citizens > self.homes * RESIDENTS_PER_HOME {
            return Err(format!(
                "citizens must be at most {} (homes x residents)",
                self.homes * RESIDENTS_PER_HOME
            ));
        }
        Ok(())
    }
}

/// Deterministic scene inputs: roads, workplaces, and the materialization view.
struct Fixture {
    roads: Vec<RoadSegment>,
    graph: RoadGraph,
    workplaces: Vec<Workplace>,
    selection: MaterializationSelection,
}

impl Fixture {
    fn build(config: FixtureConfig) -> Self {
        let extent = (config.grid as i32 - 1) * SPACING_M;
        let roads = grid_roads(config.grid);
        let graph = RoadGraph::build(&roads).expect("movement graph");

        let mut used = BTreeSet::new();
        let home_nodes = pick_nodes(config.homes, config.grid, 37, 11, &mut used);
        let workplace_nodes = pick_nodes(config.workplaces, config.grid, 53, 17, &mut used);

        let centre = [extent as f32 / 2.0, extent as f32 / 2.0];
        let mut active = Vec::with_capacity(config.homes * RESIDENTS_PER_HOME);
        for (index, &home) in home_nodes.iter().enumerate() {
            let placement_id = index as u64 + 1;
            for slot in 0..RESIDENTS_PER_HOME as u32 {
                let id = citizen_id(SEED, placement_id, slot);
                active.push(MaterializedCitizen {
                    id,
                    placement_id,
                    slot,
                    name: name_for(id),
                    home,
                    distance: RoadGraph::straight_line_m(centre, home),
                });
            }
        }
        active.sort_by(|left, right| {
            left.distance
                .total_cmp(&right.distance)
                .then_with(|| left.id.cmp(&right.id))
        });
        active.truncate(config.citizens);

        let detailed = active
            .iter()
            .take(active.len().min(MAX_DETAILED_CITIZENS))
            .cloned()
            .collect();
        let selection = MaterializationSelection { active, detailed };

        let workplaces = workplace_nodes
            .iter()
            .enumerate()
            .map(|(index, &center)| Workplace {
                id: 10_000 + index as u64,
                slots: WORKPLACE_SLOTS,
                center,
            })
            .collect();

        Self {
            roads,
            graph,
            workplaces,
            selection,
        }
    }
}

/// One timed motion build and its position samples.
struct Measurement {
    motion: CitizenMotion,
    build_ms: f64,
    position_ms: [f64; SAMPLE_TIMES_S.len()],
    position_ms_total: f64,
    positions: Vec<Vec<CitizenPosition>>,
    position_checksums: [u64; SAMPLE_TIMES_S.len()],
}

fn measure(fixture: &Fixture) -> Measurement {
    let build_start = Instant::now();
    let motion = CitizenMotion::new(
        &fixture.selection,
        &fixture.workplaces,
        &fixture.graph,
        DEFAULT_ROUTE_CACHE_CAPACITY,
    );
    let build_ms = build_start.elapsed().as_secs_f64() * 1_000.0;

    let mut positions = Vec::with_capacity(SAMPLE_TIMES_S.len());
    let mut position_ms = [0.0_f64; SAMPLE_TIMES_S.len()];
    let mut position_ms_total = 0.0_f64;
    for (index, &time_s) in SAMPLE_TIMES_S.iter().enumerate() {
        let start = Instant::now();
        let sampled = black_box(motion.positions_at(time_s));
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        position_ms[index] = elapsed_ms;
        position_ms_total += elapsed_ms;
        positions.push(sampled);
    }
    let position_checksums = [
        positions_checksum(&positions[0]),
        positions_checksum(&positions[1]),
        positions_checksum(&positions[2]),
    ];

    Measurement {
        motion,
        build_ms,
        position_ms,
        position_ms_total,
        positions,
        position_checksums,
    }
}

/// Same square grid as the traffic benchmark: `grid` roads each way at 16 m.
fn grid_roads(grid: usize) -> Vec<RoadSegment> {
    let extent = (grid as i32 - 1) * SPACING_M;
    let mut roads = Vec::with_capacity(grid * 2);
    let mut id = 1_u64;
    for index in 0..grid {
        let offset = index as i32 * SPACING_M;
        roads.push(RoadSegment {
            id,
            start: [0, offset],
            end: [extent, offset],
            width_m: 8,
        });
        id += 1;
        roads.push(RoadSegment {
            id,
            start: [offset, 0],
            end: [offset, extent],
            width_m: 8,
        });
        id += 1;
    }
    roads
}

/// Pick `count` distinct, unused grid nodes in a deterministic order.
///
/// `stride` is advanced to the first odd value coprime with the node count, so
/// the index walk visits every node before repeating. Used home nodes are
/// skipped, keeping homes and workplaces on distinct intersections.
fn pick_nodes(
    count: usize,
    grid: usize,
    stride: usize,
    offset: usize,
    used: &mut BTreeSet<(usize, usize)>,
) -> Vec<[f32; 2]> {
    let cells = grid * grid;
    let stride = coprime_stride(stride, cells);
    let mut nodes = Vec::with_capacity(count);
    let mut step = 0;
    while nodes.len() < count {
        let index = (offset + step * stride) % cells;
        step += 1;
        let node = (index % grid, index / grid);
        if used.insert(node) {
            nodes.push([
                (node.0 as i32 * SPACING_M) as f32,
                (node.1 as i32 * SPACING_M) as f32,
            ]);
        }
        assert!(step <= cells, "not enough distinct grid nodes");
    }
    nodes
}

fn coprime_stride(start: usize, modulus: usize) -> usize {
    let mut stride = start | 1;
    while gcd(stride, modulus) != 1 {
        stride += 2;
    }
    stride
}

fn gcd(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// Stable checksum over sampled citizen positions.
fn positions_checksum(positions: &[CitizenPosition]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    fnv1a_feed(&mut hash, positions.len() as u64);
    for position in positions {
        fnv1a_feed(&mut hash, position.id);
        fnv1a_feed(&mut hash, position.placement_id);
        fnv1a_feed(&mut hash, u64::from(position.slot));
        fnv1a_feed(&mut hash, u64::from(position.position[0].to_bits()));
        fnv1a_feed(&mut hash, u64::from(position.position[1].to_bits()));
        fnv1a_feed(&mut hash, u64::from(position.route_progress.to_bits()));
        fnv1a_feed(
            &mut hash,
            u64::from(position.leg == core_city::MotionLeg::Return),
        );
    }
    hash
}

fn fnv1a_feed(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
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

    fn small_config() -> FixtureConfig {
        FixtureConfig {
            grid: 8,
            homes: 8,
            workplaces: 5,
            citizens: 40,
        }
    }

    #[test]
    fn fixture_counts_ids_and_positions_are_deterministic() {
        let config = small_config();
        let fixture = Fixture::build(config);

        assert_eq!(fixture.roads.len(), config.grid * 2);
        assert_eq!(fixture.workplaces.len(), config.workplaces);
        assert!(fixture
            .workplaces
            .iter()
            .all(|workplace| workplace.slots == WORKPLACE_SLOTS));
        assert_eq!(fixture.selection.active.len(), config.citizens);
        assert_eq!(
            fixture.selection.detailed.len(),
            config.citizens.min(MAX_DETAILED_CITIZENS)
        );
        assert!(fixture
            .selection
            .detailed
            .iter()
            .all(|citizen| fixture.selection.active.contains(citizen)));

        let ids: BTreeSet<u64> = fixture
            .selection
            .active
            .iter()
            .map(|citizen| citizen.id)
            .collect();
        assert_eq!(ids.len(), config.citizens, "citizen ids must be unique");

        let homes: BTreeSet<(u32, u32)> = fixture
            .selection
            .active
            .iter()
            .map(|citizen| (citizen.home[0].to_bits(), citizen.home[1].to_bits()))
            .collect();
        assert_eq!(homes.len(), config.homes, "home nodes must be distinct");

        let workplaces: BTreeSet<(u32, u32)> = fixture
            .workplaces
            .iter()
            .map(|workplace| (workplace.center[0].to_bits(), workplace.center[1].to_bits()))
            .collect();
        assert_eq!(
            workplaces.len(),
            config.workplaces,
            "workplace nodes must be distinct"
        );
        assert!(
            homes.is_disjoint(&workplaces),
            "homes and workplaces must not share a node"
        );

        let extent = ((config.grid as i32 - 1) * SPACING_M) as f32;
        for citizen in &fixture.selection.active {
            assert!(citizen.id > 0);
            assert!(!citizen.name.is_empty());
            assert!(citizen.home.iter().all(|value| value.is_finite()));
            assert!(citizen.distance.is_finite());
            assert!((0.0..=extent).contains(&citizen.home[0]));
            assert!((0.0..=extent).contains(&citizen.home[1]));
        }
        assert!(fixture
            .workplaces
            .iter()
            .all(|workplace| workplace.center.iter().all(|value| value.is_finite())));
    }

    #[test]
    fn the_default_fixture_matches_the_documented_scene() {
        let config = FixtureConfig {
            grid: DEFAULT_GRID,
            homes: DEFAULT_HOMES,
            workplaces: DEFAULT_WORKPLACES,
            citizens: DEFAULT_HOMES * RESIDENTS_PER_HOME,
        };
        assert!(config.validate().is_ok());
        let fixture = Fixture::build(config);
        assert_eq!(fixture.roads.len(), DEFAULT_GRID * 2);
        assert_eq!(fixture.selection.active.len(), 2_000);
        assert_eq!(fixture.selection.detailed.len(), 200);
        assert_eq!(fixture.workplaces.len(), DEFAULT_WORKPLACES);
    }

    #[test]
    fn two_builds_produce_the_same_motion_and_positions() {
        let config = small_config();
        let fixture = Fixture::build(config);
        let first = CitizenMotion::new(
            &fixture.selection,
            &fixture.workplaces,
            &fixture.graph,
            DEFAULT_ROUTE_CACHE_CAPACITY,
        );
        let second = CitizenMotion::new(
            &fixture.selection,
            &fixture.workplaces,
            &fixture.graph,
            DEFAULT_ROUTE_CACHE_CAPACITY,
        );

        assert!(!first.is_empty());
        assert_eq!(first.checksum(), second.checksum());
        assert_eq!(first.cache_checksum(), second.cache_checksum());
        assert_eq!(first.positions_at(30.0), second.positions_at(30.0));
        assert!(first
            .positions_at(30.0)
            .iter()
            .all(|position| position.position.iter().all(|value| value.is_finite())));
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let too_many_citizens = FixtureConfig {
            citizens: MAX_ACTIVE_CITIZENS + 1,
            ..small_config()
        };
        assert!(too_many_citizens.validate().is_err());

        let overflowing_nodes = FixtureConfig {
            grid: 4,
            homes: 16,
            workplaces: 1,
            citizens: 1,
        };
        assert!(overflowing_nodes.validate().is_err());

        let zero_homes = FixtureConfig {
            homes: 0,
            citizens: 0,
            ..small_config()
        };
        assert!(zero_homes.validate().is_err());
    }
}
