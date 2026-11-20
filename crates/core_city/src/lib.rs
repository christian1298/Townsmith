//! Synthetic city layouts and world-placement rules.
//!
//! Townsmith's Phase 0 rendering work needs a repeatable city that stresses the
//! MultiMesh path at several scales before terrain, roads, and simulation are
//! involved. This crate builds that city from a seed: box-shaped buildings on a
//! street grid, split into Early, Mid, and Late tiers.
//!
//! Phase 2 adds [`placement`]: the validated instance of a saved blueprint on
//! the terrain, including slope, water, and overlap checks plus a small JSON
//! save format.
//!
//! The crate is engine-agnostic and has no dependencies. It does not read
//! terrain, generate road geometry, place citizens, or render anything. A shell
//! can consume [`CityLayout::buildings`] directly, for example by adding one
//! transformed box instance per building to a `MultiMesh`.
//!
//! # Coordinates
//!
//! - The synthetic ground is the `y = 0` plane.
//! - `position` is the center of the building's axis-aligned box. Its `y` is
//!   therefore `size[1] / 2` for a flat city.
//! - `size` is the full box extent in meters: width, height, depth.
//! - `bounds_m` is `[min_x, min_z, max_x, max_z]`, centered on the origin.
//!
//! # Determinism
//!
//! Generation uses only integer PCG32 arithmetic plus `f32` add, subtract,
//! multiply, divide, and square root in a fixed order. There are no
//! transcendental functions and no external `rand` dependency, so the same
//! seed, tier, and binary produce an identical [`CityLayout`].

#![forbid(unsafe_code)]

pub mod construction;
pub mod economy;
pub mod lot;
pub mod metrics;
pub mod placement;
pub mod population;
pub mod road;
pub mod settlement;

use std::error::Error;
use std::fmt;

pub use construction::{
    total_seconds_for, ConstructionError, ConstructionPhase, ConstructionSite, ConstructionStore,
    ConstructionTick, CONSTRUCTION_FORMAT_VERSION, MAX_SITES as MAX_CONSTRUCTION_SITES,
    MAX_TICK_SECONDS,
};
pub use economy::{
    Economy, EconomyError, EconomyTick, BANKRUPTCY_THRESHOLD, DAY_SECONDS,
    DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY, DEFAULT_TAX_PER_RESIDENT_PER_DAY,
    DEFAULT_UPKEEP_PER_BUILDING_PER_DAY, ECONOMY_FORMAT_VERSION, MAX_ECONOMY_TICK_SECONDS,
    STARTING_BALANCE,
};
pub use lot::{
    Lot, LotError, LotRequest, LotStore, Zone, LOT_FORMAT_VERSION, LOT_ROAD_GAP_M, MAX_LOTS,
    MAX_LOT_AREA_M2, MAX_LOT_SIDE_M, MAX_LOT_SLOPE_M, MIN_LOT_AREA_M2, MIN_LOT_SIDE_M,
};
pub use metrics::{summarize, CitySummary};
pub use placement::{
    footprint_for, normalize_rotation, validate_placement, Placement, PlacementError,
    PlacementPlan, PlacementRequest, PlacementStore, MAX_BUILDING_HEIGHT_M, MAX_FOOTPRINT_M,
    MAX_SLOPE_M, MAX_WORLD_COORD_M, PLACEMENT_GAP_M, WATER_LEVEL_M, WORLD_SAVE_FORMAT_VERSION,
};
pub use population::{
    CityRuntime, Home, Household, PopulationError, PopulationReport, Workplace, MAX_COMMUTE_M,
    MAX_HOUSEHOLDS, MAX_MOVE_IN_PER_SECOND, MAX_TICK_SECONDS as MAX_POPULATION_TICK_SECONDS,
    POPULATION_FORMAT_VERSION, WAGE_PER_WORKER_PER_DAY,
};
pub use road::{
    RoadError, RoadRequest, RoadSegment, RoadStore, MAX_ROADS, MAX_ROAD_LENGTH_M, MAX_ROAD_SLOPE_M,
    MAX_ROAD_WIDTH_M, MIN_ROAD_WIDTH_M, ROAD_FORMAT_VERSION,
};
pub use settlement::{
    Settlement, SettlementError, SettlementRequest, SettlementStore, MAX_SETTLEMENTS,
    MAX_SETTLEMENT_ID, MAX_SETTLEMENT_NAME_CHARS, MAX_SETTLEMENT_OWNER_CHARS,
    MAX_SETTLEMENT_RADIUS_M, MIN_SETTLEMENT_RADIUS_M, SETTLEMENT_FORMAT_VERSION,
};

/// Floor-to-floor height used by the synthetic layout, in meters.
pub const FLOOR_HEIGHT_M: f32 = 3.0;

/// Gap between a building footprint and its lot boundary, in meters.
///
/// Adjacent lots therefore have at least `2 * LOT_MARGIN_M` of open space
/// between buildings, and road strips between blocks stay empty.
pub const LOT_MARGIN_M: f32 = 1.5;

/// Scale of the synthetic city to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CityTier {
    /// Village-scale settlement: low-rise, sparse, and compact.
    Early = 0,
    /// Town-scale settlement: mid-rise, mixed use, and denser.
    Mid = 1,
    /// Metropolis-scale settlement: high-rise core with low-rise outskirts.
    Late = 2,
}

impl CityTier {
    /// Every tier, in ascending scale order.
    pub const ALL: [Self; 3] = [Self::Early, Self::Mid, Self::Late];

    /// Short lowercase name used in benchmark output and documentation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Early => "early",
            Self::Mid => "mid",
            Self::Late => "late",
        }
    }

    /// Population target for the tier.
    #[must_use]
    pub const fn target_population(self) -> u32 {
        match self {
            Self::Early => 500,
            Self::Mid => 100_000,
            Self::Late => 1_000_000,
        }
    }

    /// Building-count target for the tier.
    #[must_use]
    pub const fn target_buildings(self) -> u32 {
        match self {
            Self::Early => 200,
            Self::Mid => 10_000,
            Self::Late => 100_000,
        }
    }

    /// Side length of the square city bounds, in meters.
    #[must_use]
    pub const fn extent_m(self) -> f32 {
        match self {
            Self::Early => 600.0,
            Self::Mid => 3_000.0,
            Self::Late => 8_000.0,
        }
    }
}

impl fmt::Display for CityTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Use of a synthetic building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BuildingKind {
    /// Detached low-rise house.
    House = 0,
    /// Multi-storey residential building.
    Apartment = 1,
    /// Street-level shop or mixed-use low-rise.
    Shop = 2,
    /// Office or tower.
    Office = 3,
    /// Warehouse, workshop, or light industry.
    Industry = 4,
}

impl BuildingKind {
    /// Every kind, in documentation order.
    pub const ALL: [Self; 5] = [
        Self::House,
        Self::Apartment,
        Self::Shop,
        Self::Office,
        Self::Industry,
    ];

    /// Short lowercase name used in documentation and tests.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::House => "house",
            Self::Apartment => "apartment",
            Self::Shop => "shop",
            Self::Office => "office",
            Self::Industry => "industry",
        }
    }
}

impl fmt::Display for BuildingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One box-shaped building in a synthetic layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildingInstance {
    /// Stable, zero-based identity within the layout.
    pub id: u32,
    /// Center of the building box, in meters.
    pub position: [f32; 3],
    /// Full box extent `[width, height, depth]`, in meters.
    pub size: [f32; 3],
    /// Linear RGB color in `0.0..=1.0` used by the render benchmark.
    pub color: [f32; 3],
    /// Building use.
    pub kind: BuildingKind,
}

impl BuildingInstance {
    /// Number of 3 m floors implied by [`Self::size`].
    #[must_use]
    pub fn floors(self) -> u16 {
        (self.size[1] / FLOOR_HEIGHT_M).round() as u16
    }
}

/// A complete synthetic city for one tier and seed.
#[derive(Debug, Clone, PartialEq)]
pub struct CityLayout {
    /// Tier this layout was generated for.
    pub tier: CityTier,
    /// Seed used for generation.
    pub seed: u64,
    /// Population target for the tier.
    ///
    /// This is a benchmark input, not a sum of simulated residents. Building
    /// occupancy is intentionally out of scope.
    pub population: u32,
    /// Buildings in deterministic generation order.
    pub buildings: Vec<BuildingInstance>,
    /// City bounds as `[min_x, min_z, max_x, max_z]`, in meters.
    pub bounds_m: [f32; 4],
}

impl CityLayout {
    /// Generate a deterministic layout for `tier` and `seed`.
    ///
    /// The layout uses a centered street grid. Blocks are separated by empty
    /// road strips; buildings sit inside disjoint lots, inset by
    /// [`LOT_MARGIN_M`], so footprints never overlap.
    ///
    /// # Errors
    ///
    /// Returns [`CityError`] if the tier configuration cannot fit its building
    /// target or if generation misses the tier count tolerance. The current
    /// tier constants satisfy both checks, so this is an invariant guard rather
    /// than a normal runtime failure.
    pub fn generate(tier: CityTier, seed: u64) -> Result<Self, CityError> {
        let config = TierConfig::for_tier(tier);
        let blocks = config.blocks_per_axis();
        if blocks == 0 {
            return Err(CityError::EmptyLotGrid { tier });
        }

        let candidate_lots = config.candidate_lots(blocks);
        let target = usize::try_from(config.buildings).unwrap_or(usize::MAX);
        if candidate_lots < target {
            return Err(CityError::InsufficientLots {
                tier,
                required: config.buildings,
                available: u32::try_from(candidate_lots).unwrap_or(u32::MAX),
            });
        }

        let pitch = config.block_m + config.road_m;
        let grid_extent = blocks as f32 * config.block_m + (blocks - 1) as f32 * config.road_m;
        let grid_origin = -grid_extent * 0.5;
        let lot_m = config.block_m / config.lots_per_axis as f32;
        let max_footprint = (lot_m - 2.0 * LOT_MARGIN_M).max(1.0);
        let half_extent = config.extent_m * 0.5;

        let tier_salt = 0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(tier as u64 + 1);
        let mut rng = Pcg32::new(seed ^ tier_salt);
        let mut buildings = Vec::with_capacity(target);
        let mut lot_index = 0_u64;
        let target_u64 = target as u64;
        let candidate_u64 = candidate_lots as u64;

        for block_z in 0..blocks {
            let block_z0 = grid_origin + block_z as f32 * pitch;
            for block_x in 0..blocks {
                let block_x0 = grid_origin + block_x as f32 * pitch;
                for lot_z in 0..config.lots_per_axis {
                    let lot_center_z = block_z0 + (lot_z as f32 + 0.5) * lot_m;
                    for lot_x in 0..config.lots_per_axis {
                        // Exactly `target` lots survive, spread across the grid
                        // by an arithmetic progression modulo the lot count.
                        let keep = (lot_index * target_u64) % candidate_u64 < target_u64;
                        lot_index += 1;
                        if !keep {
                            continue;
                        }

                        let lot_center_x = block_x0 + (lot_x as f32 + 0.5) * lot_m;
                        let radius = (lot_center_x * lot_center_x + lot_center_z * lot_center_z)
                            .sqrt()
                            / half_extent;
                        let centrality = (1.0 - radius).clamp(0.0, 1.0);
                        let core = smoothstep(0.30, 0.85, centrality);

                        let kind = pick_kind(tier, core, &mut rng);
                        let floors = pick_floors(tier, kind, core, &mut rng);
                        let height = floors as f32 * FLOOR_HEIGHT_M;
                        let (width, depth) = pick_footprint(kind, max_footprint, &mut rng);

                        // Jitter inside the lot, never past the margin.
                        let slack_x = (lot_m - 2.0 * LOT_MARGIN_M - width).max(0.0);
                        let slack_z = (lot_m - 2.0 * LOT_MARGIN_M - depth).max(0.0);
                        let x = lot_center_x + (rng.next_f32() - 0.5) * slack_x;
                        let z = lot_center_z + (rng.next_f32() - 0.5) * slack_z;
                        let color = pick_color(tier, kind, &mut rng);

                        // The tier targets top out at 100,000 buildings, so the
                        // index always fits in `u32`.
                        buildings.push(BuildingInstance {
                            id: buildings.len() as u32,
                            position: [x, height * 0.5, z],
                            size: [width, height, depth],
                            color,
                            kind,
                        });
                    }
                }
            }
        }

        let actual = u32::try_from(buildings.len()).unwrap_or(u32::MAX);
        let tolerance = config.buildings / 10;
        if actual.abs_diff(config.buildings) > tolerance {
            return Err(CityError::CountOutOfTolerance {
                tier,
                target: config.buildings,
                actual,
            });
        }

        Ok(Self {
            tier,
            seed,
            population: config.population,
            buildings,
            bounds_m: [-half_extent, -half_extent, half_extent, half_extent],
        })
    }

    /// Number of buildings in the layout.
    #[must_use]
    pub fn building_count(&self) -> usize {
        self.buildings.len()
    }

    /// Width of the city bounds, in meters.
    #[must_use]
    pub fn bounds_width(&self) -> f32 {
        self.bounds_m[2] - self.bounds_m[0]
    }

    /// Depth of the city bounds, in meters.
    #[must_use]
    pub fn bounds_depth(&self) -> f32 {
        self.bounds_m[3] - self.bounds_m[1]
    }

    /// Stable 64-bit checksum of the layout contents.
    ///
    /// The checksum is FNV-1a over the tier, seed, population, bounds, and every
    /// building field. It is intended for benchmark logs and regression
    /// comparisons within the same build, not as a cryptographic hash.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = Fnv1a::new();
        hash.write_u8(self.tier as u8);
        hash.write_u64(self.seed);
        hash.write_u32(self.population);
        for bound in self.bounds_m {
            hash.write_f32(bound);
        }
        hash.write_u64(self.buildings.len() as u64);
        for building in &self.buildings {
            hash.write_u32(building.id);
            hash.write_u8(building.kind as u8);
            for component in building.position {
                hash.write_f32(component);
            }
            for component in building.size {
                hash.write_f32(component);
            }
            for component in building.color {
                hash.write_f32(component);
            }
        }
        hash.finish()
    }
}

/// Generation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CityError {
    /// The tier configuration produced no blocks.
    EmptyLotGrid {
        /// Tier that failed.
        tier: CityTier,
    },
    /// The lot grid cannot fit the tier's building target.
    InsufficientLots {
        /// Tier that failed.
        tier: CityTier,
        /// Buildings the tier asks for.
        required: u32,
        /// Candidate lots available in the grid.
        available: u32,
    },
    /// Generation produced a building count outside the tier tolerance.
    CountOutOfTolerance {
        /// Tier that failed.
        tier: CityTier,
        /// Target building count.
        target: u32,
        /// Actual building count.
        actual: u32,
    },
}

impl fmt::Display for CityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLotGrid { tier } => {
                write!(f, "{tier} tier produced an empty lot grid")
            }
            Self::InsufficientLots {
                tier,
                required,
                available,
            } => write!(
                f,
                "{tier} tier needs {required} lots but the grid has {available}"
            ),
            Self::CountOutOfTolerance {
                tier,
                target,
                actual,
            } => write!(
                f,
                "{tier} tier generated {actual} buildings, outside the target of {target} ±10%"
            ),
        }
    }
}

impl Error for CityError {}

/// Per-tier generation constants.
struct TierConfig {
    extent_m: f32,
    road_m: f32,
    block_m: f32,
    lots_per_axis: usize,
    population: u32,
    buildings: u32,
}

impl TierConfig {
    const fn for_tier(tier: CityTier) -> Self {
        match tier {
            CityTier::Early => Self {
                extent_m: tier.extent_m(),
                road_m: 12.0,
                block_m: 48.0,
                lots_per_axis: 2,
                population: tier.target_population(),
                buildings: tier.target_buildings(),
            },
            CityTier::Mid => Self {
                extent_m: tier.extent_m(),
                road_m: 14.0,
                block_m: 40.0,
                lots_per_axis: 2,
                population: tier.target_population(),
                buildings: tier.target_buildings(),
            },
            CityTier::Late => Self {
                extent_m: tier.extent_m(),
                road_m: 16.0,
                block_m: 34.0,
                lots_per_axis: 2,
                population: tier.target_population(),
                buildings: tier.target_buildings(),
            },
        }
    }

    fn blocks_per_axis(&self) -> usize {
        let pitch = self.block_m + self.road_m;
        let blocks = ((self.extent_m - self.road_m) / pitch).floor();
        blocks.max(0.0) as usize
    }

    fn candidate_lots(&self, blocks: usize) -> usize {
        let lots_per_block = self.lots_per_axis * self.lots_per_axis;
        blocks.saturating_mul(blocks).saturating_mul(lots_per_block)
    }
}

/// Weighted building-kind choice for a tier and normalized core factor.
///
/// `core` is 0 on the outskirts and 1 in the center.
fn kind_weights(tier: CityTier, core: f32) -> [f32; 5] {
    match tier {
        CityTier::Early => [0.70, 0.06, 0.16, 0.02, 0.06],
        CityTier::Mid => [
            0.22 - 0.12 * core,
            0.30 + 0.12 * core,
            0.20,
            0.20 + 0.10 * core,
            0.08 - 0.04 * core,
        ],
        CityTier::Late => [
            0.48 - 0.46 * core,
            0.16 + 0.26 * core,
            0.14 - 0.04 * core,
            0.05 + 0.37 * core,
            0.17 - 0.10 * core,
        ],
    }
}

fn pick_kind(tier: CityTier, core: f32, rng: &mut Pcg32) -> BuildingKind {
    let weights = kind_weights(tier, core);
    let total: f32 = weights.iter().sum();
    let mut roll = rng.next_f32() * total;
    for (index, weight) in weights.iter().enumerate() {
        if roll < *weight {
            return BuildingKind::ALL[index];
        }
        roll -= *weight;
    }
    BuildingKind::Industry
}

/// Floor range before the tier's central-height scaling.
fn floor_range(tier: CityTier, kind: BuildingKind) -> (u32, u32) {
    match tier {
        CityTier::Early => match kind {
            BuildingKind::House => (2, 3),
            BuildingKind::Apartment => (3, 5),
            BuildingKind::Shop => (1, 2),
            BuildingKind::Office => (3, 5),
            BuildingKind::Industry => (2, 3),
        },
        CityTier::Mid => match kind {
            BuildingKind::House => (2, 4),
            BuildingKind::Apartment => (6, 14),
            BuildingKind::Shop => (2, 3),
            BuildingKind::Office => (8, 22),
            BuildingKind::Industry => (3, 5),
        },
        CityTier::Late => match kind {
            BuildingKind::House => (2, 4),
            BuildingKind::Apartment => (8, 28),
            BuildingKind::Shop => (2, 4),
            BuildingKind::Office => (16, 60),
            BuildingKind::Industry => (3, 6),
        },
    }
}

fn pick_floors(tier: CityTier, kind: BuildingKind, core: f32, rng: &mut Pcg32) -> u32 {
    let (min, max) = floor_range(tier, kind);
    let scale = match (tier, kind) {
        (CityTier::Mid, BuildingKind::Apartment | BuildingKind::Office) => 0.70 + 0.30 * core,
        (CityTier::Late, BuildingKind::Apartment) => 0.40 + 0.60 * core,
        (CityTier::Late, BuildingKind::Office) => 0.28 + 0.72 * core,
        _ => 1.0,
    };
    let low = ((min as f32) * scale).round().max(1.0) as u32;
    let high = ((max as f32) * scale).round().max(low as f32) as u32;
    let span = high - low + 1;
    low + rng.next_u32() % span
}

/// Footprint ranges `(min_width, max_width, min_depth, max_depth)` by kind.
fn footprint_range(kind: BuildingKind) -> (f32, f32, f32, f32) {
    match kind {
        BuildingKind::House => (7.0, 12.0, 8.0, 14.0),
        BuildingKind::Apartment => (10.0, 15.0, 12.0, 16.0),
        BuildingKind::Shop => (8.0, 14.0, 9.0, 16.0),
        BuildingKind::Office => (11.0, 15.0, 12.0, 16.0),
        BuildingKind::Industry => (12.0, 16.0, 12.0, 18.0),
    }
}

fn pick_footprint(kind: BuildingKind, limit: f32, rng: &mut Pcg32) -> (f32, f32) {
    let (min_width, max_width, min_depth, max_depth) = footprint_range(kind);
    (
        pick_extent(min_width, max_width, limit, rng),
        pick_extent(min_depth, max_depth, limit, rng),
    )
}

fn pick_extent(min: f32, max: f32, limit: f32, rng: &mut Pcg32) -> f32 {
    let low = min.min(limit).max(1.0);
    let high = max.min(limit).max(low);
    low + rng.next_f32() * (high - low)
}

const HOUSE_PALETTE: [[f32; 3]; 3] = [[0.78, 0.70, 0.56], [0.68, 0.47, 0.35], [0.55, 0.44, 0.32]];

const APARTMENT_PALETTE: [[f32; 3]; 3] =
    [[0.74, 0.73, 0.69], [0.66, 0.50, 0.40], [0.80, 0.78, 0.72]];

const SHOP_PALETTE: [[f32; 3]; 3] = [[0.86, 0.78, 0.62], [0.40, 0.58, 0.56], [0.72, 0.36, 0.30]];

const OFFICE_PALETTE: [[f32; 3]; 3] = [[0.45, 0.58, 0.68], [0.62, 0.64, 0.67], [0.32, 0.38, 0.44]];

const INDUSTRY_PALETTE: [[f32; 3]; 3] =
    [[0.58, 0.42, 0.32], [0.52, 0.52, 0.49], [0.66, 0.56, 0.36]];

fn palette(kind: BuildingKind) -> &'static [[f32; 3]; 3] {
    match kind {
        BuildingKind::House => &HOUSE_PALETTE,
        BuildingKind::Apartment => &APARTMENT_PALETTE,
        BuildingKind::Shop => &SHOP_PALETTE,
        BuildingKind::Office => &OFFICE_PALETTE,
        BuildingKind::Industry => &INDUSTRY_PALETTE,
    }
}

fn tier_tint(tier: CityTier) -> [f32; 3] {
    match tier {
        CityTier::Early => [0.02, 0.01, -0.02],
        CityTier::Mid => [0.0, 0.0, 0.0],
        CityTier::Late => [-0.03, 0.0, 0.035],
    }
}

fn pick_color(tier: CityTier, kind: BuildingKind, rng: &mut Pcg32) -> [f32; 3] {
    let palette = palette(kind);
    let base = palette[(rng.next_u32() as usize) % palette.len()];
    let tint = tier_tint(tier);
    let mut color = [0.0; 3];
    for (channel, value) in color.iter_mut().enumerate() {
        let jitter = (rng.next_f32() - 0.5) * 0.07;
        *value = (base[channel] + tint[channel] + jitter).clamp(0.0, 1.0);
    }
    color
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Small PCG32 generator, following the pattern in `core_sim::active`.
///
/// Only wrapping integer arithmetic is used for the state transition.
#[derive(Debug, Clone)]
struct Pcg32 {
    state: u64,
    increment: u64,
}

impl Pcg32 {
    const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
    const F32_SCALE: f32 = 1.0 / 16_777_216.0;

    fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            increment: (seed << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(self.increment);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rotation = (old >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * Self::F32_SCALE
    }
}

/// FNV-1a 64-bit hasher for [`CityLayout::checksum`].
struct Fnv1a(u64);

impl Fnv1a {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn write_u8(&mut self, value: u8) {
        self.0 ^= u64::from(value);
        self.0 = self.0.wrapping_mul(Self::PRIME);
    }

    fn write_u32(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.write_u8(byte);
        }
    }

    fn write_u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.write_u8(byte);
        }
    }

    fn write_f32(&mut self, value: f32) {
        self.write_u32(value.to_bits());
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{BuildingInstance, CityLayout, CityTier, FLOOR_HEIGHT_M};

    const SEED: u64 = 0x5EED_0000_0000_0001;
    const CELL_M: f32 = 32.0;

    #[test]
    fn counts_match_tier_targets_exactly() {
        for tier in CityTier::ALL {
            let layout = CityLayout::generate(tier, SEED).expect("layout generates");
            let target = tier.target_buildings();
            let actual = layout.building_count() as u32;
            assert_eq!(actual, target, "{tier}: building count");
            assert_eq!(layout.population, tier.target_population());
        }
    }

    #[test]
    fn golden_checksums_pin_the_fixture_layouts() {
        let expected = [
            (CityTier::Early, 16_539_281_832_476_610_770_u64),
            (CityTier::Mid, 1_997_117_545_588_945_985_u64),
            (CityTier::Late, 13_094_279_148_273_860_104_u64),
        ];
        for (tier, checksum) in expected {
            let layout = CityLayout::generate(tier, SEED).expect("layout generates");
            assert_eq!(layout.checksum(), checksum, "{tier}: checksum drifted");
        }
    }

    #[test]
    fn smoke_seed_late_checksum_is_stable() {
        let layout = CityLayout::generate(CityTier::Late, 2026).expect("layout generates");
        assert_eq!(layout.checksum(), 2_595_171_946_372_580_485);
    }

    #[test]
    fn same_seed_produces_identical_layouts() {
        for tier in CityTier::ALL {
            let first = CityLayout::generate(tier, SEED).expect("layout generates");
            let second = CityLayout::generate(tier, SEED).expect("layout generates");
            assert_eq!(first, second, "{tier}: same seed changed the layout");
            assert_eq!(first.checksum(), second.checksum());
        }
    }

    #[test]
    fn different_seeds_produce_different_layouts() {
        for tier in CityTier::ALL {
            let first = CityLayout::generate(tier, 1).expect("layout generates");
            let second = CityLayout::generate(tier, 2).expect("layout generates");
            assert_ne!(
                first.checksum(),
                second.checksum(),
                "{tier}: different seeds produced the same checksum"
            );
        }
    }

    #[test]
    fn bounds_contain_every_building() {
        for tier in CityTier::ALL {
            let layout = CityLayout::generate(tier, SEED).expect("layout generates");
            let [min_x, min_z, max_x, max_z] = layout.bounds_m;
            for building in &layout.buildings {
                assert!(building.size[0] > 0.0);
                assert!(building.size[1] > 0.0);
                assert!(building.size[2] > 0.0);
                let half_x = building.size[0] * 0.5;
                let half_z = building.size[2] * 0.5;
                assert!(
                    building.position[0] - half_x >= min_x - f32::EPSILON,
                    "{tier}: building {} escapes min x",
                    building.id
                );
                assert!(
                    building.position[0] + half_x <= max_x + f32::EPSILON,
                    "{tier}: building {} escapes max x",
                    building.id
                );
                assert!(
                    building.position[2] - half_z >= min_z - f32::EPSILON,
                    "{tier}: building {} escapes min z",
                    building.id
                );
                assert!(
                    building.position[2] + half_z <= max_z + f32::EPSILON,
                    "{tier}: building {} escapes max z",
                    building.id
                );
            }
        }
    }

    #[test]
    fn footprints_do_not_overlap() {
        for tier in CityTier::ALL {
            let layout = CityLayout::generate(tier, SEED).expect("layout generates");
            assert_no_overlaps(&layout.buildings);
        }
    }

    #[test]
    fn tiers_scale_in_population_buildings_and_area() {
        let early = CityLayout::generate(CityTier::Early, SEED).expect("layout");
        let mid = CityLayout::generate(CityTier::Mid, SEED).expect("layout");
        let late = CityLayout::generate(CityTier::Late, SEED).expect("layout");

        assert!(early.building_count() < mid.building_count());
        assert!(mid.building_count() < late.building_count());
        assert!(early.population < mid.population);
        assert!(mid.population < late.population);
        assert!(early.bounds_width() < mid.bounds_width());
        assert!(mid.bounds_width() < late.bounds_width());
        assert!(early.bounds_depth() < mid.bounds_depth());
        assert!(mid.bounds_depth() < late.bounds_depth());
        assert_eq!(early.bounds_width(), early.bounds_depth());
        assert_eq!(mid.bounds_width(), mid.bounds_depth());
        assert_eq!(late.bounds_width(), late.bounds_depth());
    }

    #[test]
    fn ids_are_unique_and_sequential() {
        for tier in CityTier::ALL {
            let layout = CityLayout::generate(tier, SEED).expect("layout generates");
            for (expected, building) in layout.buildings.iter().enumerate() {
                assert_eq!(
                    building.id,
                    u32::try_from(expected).expect("count fits u32"),
                    "{tier}: building ids are not sequential"
                );
            }
        }
    }

    #[test]
    fn buildings_are_whole_floor_boxes_with_valid_colors() {
        for tier in CityTier::ALL {
            let layout = CityLayout::generate(tier, SEED).expect("layout generates");
            for building in &layout.buildings {
                assert!(building.floors() >= 1);
                let floors = f32::from(building.floors());
                assert!(
                    (building.size[1] - floors * FLOOR_HEIGHT_M).abs() < 1.0e-3,
                    "{tier}: building {} height is not a whole floor count",
                    building.id
                );
                for channel in building.color {
                    assert!(
                        (0.0..=1.0).contains(&channel),
                        "{tier}: building {} has color channel {channel}",
                        building.id
                    );
                }
            }
        }
    }

    fn assert_no_overlaps(buildings: &[BuildingInstance]) {
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (index, building) in buildings.iter().enumerate() {
            let min_x = building.position[0] - building.size[0] * 0.5;
            let max_x = building.position[0] + building.size[0] * 0.5;
            let min_z = building.position[2] - building.size[2] * 0.5;
            let max_z = building.position[2] + building.size[2] * 0.5;
            for cell_z in cell_range(min_z, max_z) {
                for cell_x in cell_range(min_x, max_x) {
                    cells.entry((cell_x, cell_z)).or_default().push(index);
                }
            }
        }

        for bucket in cells.values() {
            for (offset, first) in bucket.iter().enumerate() {
                for second in &bucket[offset + 1..] {
                    let a = &buildings[*first];
                    let b = &buildings[*second];
                    assert!(
                        !overlaps(a, b),
                        "buildings {} and {} overlap: {:?} vs {:?}",
                        a.id,
                        b.id,
                        a,
                        b
                    );
                }
            }
        }
    }

    fn cell_range(min: f32, max: f32) -> std::ops::RangeInclusive<i32> {
        ((min / CELL_M).floor() as i32)..=((max / CELL_M).floor() as i32)
    }

    fn overlaps(a: &BuildingInstance, b: &BuildingInstance) -> bool {
        let a_min_x = a.position[0] - a.size[0] * 0.5;
        let a_max_x = a.position[0] + a.size[0] * 0.5;
        let a_min_z = a.position[2] - a.size[2] * 0.5;
        let a_max_z = a.position[2] + a.size[2] * 0.5;
        let b_min_x = b.position[0] - b.size[0] * 0.5;
        let b_max_x = b.position[0] + b.size[0] * 0.5;
        let b_min_z = b.position[2] - b.size[2] * 0.5;
        let b_max_z = b.position[2] + b.size[2] * 0.5;

        a_min_x < b_max_x && b_min_x < a_max_x && a_min_z < b_max_z && b_min_z < a_max_z
    }
}
