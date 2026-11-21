//! Deterministic materialization-selection benchmark.
//!
//! Builds a fixed named-citizen pool on a regular 15 m home grid, then runs
//! [`NamedCitizens::select_near`] at several camera radii with the default
//! 2,000 active / 200 detailed budget. Each radius reports the selected counts,
//! median/p95 selection time over 20 runs, and a stable checksum of the
//! selected ids and distances.
//!
//! The default fixture is a 100,000-identity scan stress, deliberately larger
//! than [`MAX_NAMED_CITIZENS`] (4,096): it measures how the current city-wide
//! scan scales if the identity cap grows. It never enters the save path.
//! `--smoke` uses a smaller indexed fixture and fails on a checksum or
//! invariant mismatch.
//!
//! See `docs/73 - Materialization Core.md` §7.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use core_city::{
    age_years, citizen_id, name_for, MaterializationBudget, MaterializationSelection,
    MaterializedCitizen, NamedCitizen, NamedCitizens, MAX_ACTIVE_CITIZENS, MAX_DETAILED_CITIZENS,
    MAX_NAMED_CITIZENS,
};

/// Fixed seed for every fixture.
const SEED: u64 = 0x4D41_5445_5249_414C; // "MATERIAL"

const DEFAULT_CITIZENS: usize = 100_000;
const DEFAULT_SLOTS_PER_HOME: u32 = 5;
const DEFAULT_COLUMNS: usize = 200;
const DEFAULT_SPACING_M: f32 = 15.0;
const DEFAULT_RUNS: usize = 20;

/// Active (outer) and detailed (inner) radius pairs in metres.
const RADII: [(f32, f32); 3] = [(150.0, 45.0), (300.0, 90.0), (600.0, 180.0)];

/// Smoke fixture: 6,000 identities on a 40 x 30 grid at 20 m spacing.
const SMOKE_CITIZENS: usize = 6_000;
const SMOKE_SLOTS_PER_HOME: u32 = 5;
const SMOKE_COLUMNS: usize = 40;
const SMOKE_SPACING_M: f32 = 20.0;
const SMOKE_RADII: [(f32, f32); 3] = [(80.0, 30.0), (200.0, 60.0), (500.0, 120.0)];
/// Pinned checksum over the smoke selections. A change here is either an
/// intentional fixture change or a selection regression.
const SMOKE_CHECKSUM: u64 = 0x20f4_ac0c_bb20_ff83;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Deterministic scene shape. The citizen count is rounded down to whole homes.
#[derive(Debug, Clone, Copy)]
struct Fixture {
    seed: u64,
    citizens: usize,
    slots_per_home: u32,
    columns: usize,
    spacing_m: f32,
    runs: usize,
    radii: &'static [(f32, f32)],
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            seed: SEED,
            citizens: DEFAULT_CITIZENS,
            slots_per_home: DEFAULT_SLOTS_PER_HOME,
            columns: DEFAULT_COLUMNS,
            spacing_m: DEFAULT_SPACING_M,
            runs: DEFAULT_RUNS,
            radii: &RADII,
        }
    }
}

impl Fixture {
    const fn smoke() -> Self {
        Self {
            seed: SEED,
            citizens: SMOKE_CITIZENS,
            slots_per_home: SMOKE_SLOTS_PER_HOME,
            columns: SMOKE_COLUMNS,
            spacing_m: SMOKE_SPACING_M,
            runs: 2,
            radii: &SMOKE_RADII,
        }
    }

    fn homes(self) -> usize {
        self.citizens / self.slots_per_home as usize
    }

    fn rows(self) -> usize {
        self.homes().div_ceil(self.columns.max(1))
    }
}

/// A built fixture: the named pool, home centres, and camera centre.
struct Scene {
    named: NamedCitizens,
    centers: BTreeMap<u64, [f32; 2]>,
    center: [f32; 2],
    homes: usize,
    slot_citizens: usize,
}

/// Measured result at one radius pair.
#[derive(Debug, Clone)]
struct RadiusOutcome {
    active_radius_m: f32,
    detailed_radius_m: f32,
    active: usize,
    detailed: usize,
    checksum: u64,
    stable: bool,
    median_ms: f64,
    p95_ms: f64,
}

#[derive(Debug)]
struct Outcome {
    fixture: Fixture,
    scene_homes: usize,
    scene_citizens: usize,
    center: [f32; 2],
    setup_ms: f64,
    total_ms: f64,
    radii: Vec<RadiusOutcome>,
}

fn main() {
    if std::env::args().any(|arg| arg == "--smoke") {
        run_smoke();
        return;
    }

    let outcome = measure(Fixture::default(), MaterializationBudget::default());
    let report = serde_json::to_string(&report(&outcome)).expect("json");
    println!("{report}");
}

/// Run the smoke fixture twice, check the pinned checksum, and verify the
/// selection invariants. Exits non-zero on any mismatch.
fn run_smoke() {
    let fixture = Fixture::smoke();
    let budget = MaterializationBudget::default();
    let first = measure(fixture, budget);
    let second = measure(fixture, budget);

    let mut stable = first.radii.len() == second.radii.len();
    let mut counts = true;
    for (left, right) in first.radii.iter().zip(&second.radii) {
        stable &= left.stable && right.stable && left.checksum == right.checksum;
        counts &= left.active == right.active && left.detailed == right.detailed;
    }
    let pinned = smoke_checksum(&first) == SMOKE_CHECKSUM;
    let expected_citizens = fixture.homes() * fixture.slots_per_home as usize;

    println!(
        "MATERIALIZATION_SMOKE citizens={} homes={} radii={} checksum={:016x} \
         deterministic={} pinned={} counts={} status={}",
        first.scene_citizens,
        first.scene_homes,
        first.radii.len(),
        smoke_checksum(&first),
        stable,
        pinned,
        counts,
        if stable && pinned && counts && first.scene_citizens == expected_citizens {
            "ok"
        } else {
            "failed"
        }
    );
    if !(stable && pinned && counts && first.scene_citizens == expected_citizens) {
        std::process::exit(1);
    }
}

fn measure(fixture: Fixture, budget: MaterializationBudget) -> Outcome {
    let start = Instant::now();
    let scene = build_scene(fixture);
    let setup_ms = ms(start.elapsed());

    let mut radii = Vec::with_capacity(fixture.radii.len());
    for &(active_radius_m, detailed_radius_m) in fixture.radii {
        radii.push(measure_radius(
            &scene,
            budget,
            fixture.runs,
            active_radius_m,
            detailed_radius_m,
        ));
    }

    Outcome {
        fixture,
        scene_homes: scene.homes,
        scene_citizens: scene.slot_citizens,
        center: scene.center,
        setup_ms,
        total_ms: ms(start.elapsed()),
        radii,
    }
}

fn measure_radius(
    scene: &Scene,
    budget: MaterializationBudget,
    runs: usize,
    active_radius_m: f32,
    detailed_radius_m: f32,
) -> RadiusOutcome {
    // Warm up so the timed runs use the same allocator and page state.
    let warmup = scene
        .named
        .select_near(
            scene.center,
            &scene.centers,
            budget,
            active_radius_m,
            detailed_radius_m,
        )
        .expect("select");
    check_invariants(&warmup, budget, active_radius_m, detailed_radius_m);
    let checksum = selection_checksum(&warmup);
    let mut stable = true;

    let mut samples = Vec::with_capacity(runs);
    let mut active = warmup.active.len();
    let mut detailed = warmup.detailed.len();
    for _ in 0..runs {
        let start = Instant::now();
        let selection = scene
            .named
            .select_near(
                scene.center,
                &scene.centers,
                budget,
                active_radius_m,
                detailed_radius_m,
            )
            .expect("select");
        samples.push(ms(start.elapsed()));
        stable &= selection_checksum(&selection) == checksum;
        active = selection.active.len();
        detailed = selection.detailed.len();
        black_box(selection);
    }
    samples.sort_by(|left, right| left.total_cmp(right));

    RadiusOutcome {
        active_radius_m,
        detailed_radius_m,
        active,
        detailed,
        checksum,
        stable,
        median_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
    }
}

fn build_scene(fixture: Fixture) -> Scene {
    let homes = fixture.homes();
    let rows = fixture.rows();
    let columns = fixture.columns.max(1);
    let width = (columns - 1) as f32 * fixture.spacing_m;
    let height = (rows.saturating_sub(1)) as f32 * fixture.spacing_m;
    let center = [width * 0.5, height * 0.5];

    let mut named = NamedCitizens::new(fixture.seed);
    let mut centers = BTreeMap::new();
    for index in 0..homes {
        let placement_id = u64::try_from(index).expect("placement id") + 1;
        let column = index % columns;
        let row = index / columns;
        let home = [
            column as f32 * fixture.spacing_m,
            row as f32 * fixture.spacing_m,
        ];
        centers.insert(placement_id, home);
        for slot in 0..fixture.slots_per_home {
            let id = citizen_id(fixture.seed, placement_id, slot);
            named.citizens.insert(
                (placement_id, slot),
                NamedCitizen {
                    id,
                    placement_id,
                    slot,
                    name: name_for(id),
                    age_days: u32::from(age_years(id)) * 365,
                },
            );
        }
    }
    let slot_citizens = named.citizens.len();

    Scene {
        named,
        centers,
        center,
        homes,
        slot_citizens,
    }
}

/// Panic-free invariant check used by the smoke fixture and tests.
fn check_invariants(
    selection: &MaterializationSelection,
    budget: MaterializationBudget,
    active_radius_m: f32,
    detailed_radius_m: f32,
) {
    assert!(selection.active.len() <= budget.active_max);
    assert!(selection.detailed.len() <= budget.detailed_max);
    assert!(selection.detailed.len() <= selection.active.len());
    assert_eq!(
        &selection.active[..selection.detailed.len()],
        &selection.detailed[..]
    );
    assert!(selection
        .active
        .windows(2)
        .all(|pair| ordered(&pair[0], &pair[1])));
    assert!(selection
        .active
        .iter()
        .all(|citizen| citizen.distance <= active_radius_m));
    assert!(selection
        .detailed
        .iter()
        .all(|citizen| citizen.distance <= detailed_radius_m));
    let mut ids: Vec<u64> = selection.active.iter().map(|citizen| citizen.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), selection.active.len());
}

fn ordered(left: &MaterializedCitizen, right: &MaterializedCitizen) -> bool {
    left.distance
        .total_cmp(&right.distance)
        .then_with(|| left.id.cmp(&right.id))
        .is_le()
}

/// Stable FNV-1a checksum over selected ids and distance bits.
fn selection_checksum(selection: &MaterializationSelection) -> u64 {
    let mut hash = FNV_OFFSET;
    hash = fnv_u64(hash, selection.active.len() as u64);
    hash = fnv_u64(hash, selection.detailed.len() as u64);
    for citizen in &selection.active {
        hash = fnv_u64(hash, citizen.id);
        hash = fnv_u64(hash, u64::from(citizen.distance.to_bits()));
    }
    for citizen in &selection.detailed {
        hash = fnv_u64(hash, citizen.id);
        hash = fnv_u64(hash, u64::from(citizen.distance.to_bits()));
    }
    hash
}

fn smoke_checksum(outcome: &Outcome) -> u64 {
    let mut hash = FNV_OFFSET;
    for radius in &outcome.radii {
        hash = fnv_u64(hash, u64::from(radius.active_radius_m.to_bits()));
        hash = fnv_u64(hash, u64::from(radius.detailed_radius_m.to_bits()));
        hash = fnv_u64(hash, radius.active as u64);
        hash = fnv_u64(hash, radius.detailed as u64);
        hash = fnv_u64(hash, radius.checksum);
    }
    hash
}

fn report(outcome: &Outcome) -> serde_json::Value {
    let fixture = outcome.fixture;
    let radii: Vec<serde_json::Value> = outcome
        .radii
        .iter()
        .map(|radius| {
            serde_json::json!({
                "active_radius_m": radius.active_radius_m,
                "detailed_radius_m": radius.detailed_radius_m,
                "active": radius.active,
                "detailed": radius.detailed,
                "select_ms_median": radius.median_ms,
                "select_ms_p95": radius.p95_ms,
                "checksum": format!("{:016x}", radius.checksum),
                "stable": radius.stable,
            })
        })
        .collect();

    serde_json::json!({
        "scene": "materialization_selection",
        "format_version": 1,
        "seed": fixture.seed,
        "fixture": {
            "citizens": outcome.scene_citizens,
            "homes": outcome.scene_homes,
            "slots_per_home": fixture.slots_per_home,
            "columns": fixture.columns,
            "rows": fixture.rows(),
            "spacing_m": fixture.spacing_m,
            "center": outcome.center,
            "identity_cap": MAX_NAMED_CITIZENS,
            "note": "Scan stress: the fixture exceeds the shipped 4,096 identity cap and never enters the save path.",
        },
        "budget": {
            "active_max": MAX_ACTIVE_CITIZENS,
            "detailed_max": MAX_DETAILED_CITIZENS,
        },
        "runs": fixture.runs,
        "setup_ms": outcome.setup_ms,
        "total_ms": outcome.total_ms,
        "radii": radii,
    })
}

fn fnv_u64(mut state: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index.min(samples.len() - 1)]
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_fixture() -> Fixture {
        Fixture {
            citizens: 6_000,
            runs: 3,
            ..Fixture::smoke()
        }
    }

    #[test]
    fn selection_is_deterministic_and_capped() {
        let fixture = small_fixture();
        let budget = MaterializationBudget::default();
        let scene = build_scene(fixture);
        assert_eq!(scene.homes, 1_200);
        assert_eq!(scene.slot_citizens, 6_000);

        let first = scene
            .named
            .select_near(scene.center, &scene.centers, budget, 500.0, 120.0)
            .expect("select");
        let second = scene
            .named
            .select_near(scene.center, &scene.centers, budget, 500.0, 120.0)
            .expect("select");
        assert_eq!(selection_checksum(&first), selection_checksum(&second));
        check_invariants(&first, budget, 500.0, 120.0);

        // 1,200 homes at 20 m spacing put more than 2,000 identities inside
        // 500 m, so both caps must bind.
        assert_eq!(first.active.len(), MAX_ACTIVE_CITIZENS);
        assert_eq!(first.detailed.len(), MAX_DETAILED_CITIZENS);
        assert_eq!(
            &first.active[..first.detailed.len()],
            &first.detailed[..],
            "detailed must be a prefix of active"
        );
        assert!(first.active.len() < scene.slot_citizens);

        // A tighter detailed radius keeps the active cap but shrinks the
        // detailed prefix; distances are still sorted.
        let tight = scene
            .named
            .select_near(scene.center, &scene.centers, budget, 500.0, 40.0)
            .expect("select");
        assert_eq!(tight.active.len(), MAX_ACTIVE_CITIZENS);
        assert!(tight.detailed.len() < MAX_DETAILED_CITIZENS);
        assert!(tight
            .detailed
            .iter()
            .all(|citizen| citizen.distance <= 40.0));

        // Moving the camera changes the selected set, not the pool.
        let moved = scene
            .named
            .select_near([0.0, 0.0], &scene.centers, budget, 500.0, 120.0)
            .expect("select");
        assert_ne!(selection_checksum(&first), selection_checksum(&moved));
    }

    #[test]
    fn ids_and_checksums_are_stable_across_rebuilds() {
        let fixture = small_fixture();
        let first = build_scene(fixture);
        let second = build_scene(fixture);
        assert_eq!(first.named.checksum(), second.named.checksum());
        let budget = MaterializationBudget::default();
        let left = first
            .named
            .select_near(first.center, &first.centers, budget, 80.0, 30.0)
            .expect("select");
        let right = second
            .named
            .select_near(second.center, &second.centers, budget, 80.0, 30.0)
            .expect("select");
        assert_eq!(selection_checksum(&left), selection_checksum(&right));
        assert!(!left.active.is_empty());
    }
}
