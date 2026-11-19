//! Active agents near the camera.
//!
//! Phase 0 targets **2,000 active agents at 10 Hz**. This module is
//! deliberately small: it is not an ECS, a pathfinder, or a renderer. It stores
//! a compact array of agents, steers them toward fixed targets, and uses a
//! uniform spatial hash for local separation so crowds do not collapse onto one
//! point.
//!
//! The tick uses a fixed timestep ([`TICK_DT`]) and all randomness comes from a
//! seeded [`Pcg32`]. Two simulations with the same seed and tick count produce
//! identical state within the same build and target.

/// Fixed simulation step for active agents: 10 Hz.
pub const TICK_DT: f32 = 0.1;
/// Default number of active agents, matching the Phase 0 budget.
pub const DEFAULT_AGENT_COUNT: usize = 2_000;
/// Side length of the default square agent world, in meters.
pub const DEFAULT_WORLD_SIZE: f32 = 128.0;
/// Smallest square world accepted by [`ActiveAgentSim::with_world_size`], in
/// meters.
///
/// The spawn target rule needs [`MIN_TARGET_DISTANCE`] of room on both sides of
/// a spawn position, plus [`SPAWN_MARGIN`] at both world edges:
/// `2 * SPAWN_MARGIN + 2 * MIN_TARGET_DISTANCE`. Smaller worlds fall back to
/// [`DEFAULT_WORLD_SIZE`], where the target rule is always satisfiable.
pub const MIN_WORLD_SIZE: f32 = 2.0 * SPAWN_MARGIN + 2.0 * MIN_TARGET_DISTANCE;
/// Distance at which an agent stops steering toward its target.
pub const ARRIVAL_RADIUS: f32 = 0.5;
/// Radius at which agents push each other apart.
pub const SEPARATION_RADIUS: f32 = 2.0;
/// Maximum number of neighbors considered per agent per tick.
pub const MAX_NEIGHBORS: usize = 16;

/// Separation weight relative to walking speed, before the final clamp.
///
/// Slightly above 1.0 so that overlapping agents walking in the same direction
/// still separate instead of exactly cancelling each other out.
const SEPARATION_STRENGTH: f32 = 1.5;
/// Start of the target-slowdown band. Agents ease in over this distance.
const SLOWDOWN_RADIUS: f32 = 3.0;
/// Steering responsiveness in 1/s; the fixed tick applies `response * dt`.
const STEERING_RESPONSE: f32 = 4.0;
/// Spawn inset from the world edge, in meters.
const SPAWN_MARGIN: f32 = 2.0;
/// Guaranteed minimum target distance on the x axis, in meters.
const MIN_TARGET_DISTANCE: f32 = 8.0;
/// Uniform hash cell size, in meters.
const SPATIAL_CELL_SIZE: f32 = 4.0;
/// Hard cap on the number of cells along either axis.
///
/// The grid is a flat `Vec<Vec<CellEntry>>`, so `cols * rows` must stay small
/// enough to allocate and index safely for any world size. Larger worlds are
/// clamped to their edge cells, the same rule used for out-of-bounds
/// positions. 256 × 256 = 65,536 cells is ~1.5 MB of empty buckets.
const MAX_CELLS_PER_AXIS: usize = 256;
/// Squared distance below which two agents are treated as coincident.
const COINCIDENT_DISTANCE_SQUARED: f32 = 1.0e-12;

/// A small PCG32 generator for deterministic placement.
///
/// Only integer arithmetic is used for the state transition; [`Self::next_f32`]
/// returns the top 24 bits as an `f32` in `0.0..1.0`.
#[derive(Debug, Clone)]
pub struct Pcg32 {
    state: u64,
    increment: u64,
}

impl Pcg32 {
    const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
    const F32_SCALE: f32 = 1.0 / 16_777_216.0;

    /// Create a generator seeded with `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            increment: (seed << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// Next raw 32-bit output.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(self.increment);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rotation = (old >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    /// Next value in `0.0..1.0`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * Self::F32_SCALE
    }
}

/// One active citizen near the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveAgent {
    /// Stable identity, assigned at spawn and never reused.
    pub id: u32,
    /// Position on the 2D ground plane, in meters.
    pub position: [f32; 2],
    /// Current velocity, in meters per second.
    pub velocity: [f32; 2],
    /// Destination of the current, straight-line trip.
    pub target: [f32; 2],
    /// Walking speed, in meters per second.
    pub speed: f32,
}

impl ActiveAgent {
    /// Euclidean distance to the current target, in meters.
    #[must_use]
    pub fn distance_to_target(&self) -> f32 {
        let dx = self.target[0] - self.position[0];
        let dy = self.target[1] - self.position[1];
        (dx * dx + dy * dy).sqrt()
    }
}

/// Broad-phase index for nearby-agent queries.
///
/// A uniform grid of fixed cells. Entries are rebuilt every tick: [`Self::clear`]
/// retains each bucket's capacity, so a steady-state tick does not allocate.
#[derive(Debug, Clone)]
pub struct SpatialHash {
    cell_size: f32,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<CellEntry>>,
}

#[derive(Debug, Clone, Copy)]
struct CellEntry {
    index: u32,
    position: [f32; 2],
}

impl SpatialHash {
    /// Create a hash covering `width` × `height` meters with roughly
    /// `cell_size`-meter cells.
    ///
    /// Each axis is capped at 256 cells, so very large worlds share one edge
    /// cell per direction instead of allocating without bound.
    #[must_use]
    pub fn new(cell_size: f32, width: f32, height: f32) -> Self {
        let cell_size = if cell_size.is_finite() && cell_size > 0.0 {
            cell_size
        } else {
            1.0
        };
        let width = if width.is_finite() && width > 0.0 {
            width
        } else {
            1.0
        };
        let height = if height.is_finite() && height > 0.0 {
            height
        } else {
            1.0
        };
        let cols = ((width / cell_size).ceil() as usize).clamp(1, MAX_CELLS_PER_AXIS);
        let rows = ((height / cell_size).ceil() as usize).clamp(1, MAX_CELLS_PER_AXIS);
        Self {
            cell_size,
            cols,
            rows,
            cells: vec![Vec::new(); cols * rows],
        }
    }

    /// Remove every entry while retaining bucket capacity.
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            cell.clear();
        }
    }

    /// Total number of cells (`cols × rows`).
    #[must_use]
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// Insert an index at `position`.
    ///
    /// Positions outside the covered area are clamped to the nearest edge
    /// cell. `+∞` maps to the far edge; `-∞` and NaN map to the first cell.
    /// A NaN position is stored but never returned by [`Self::query_circle`],
    /// because its distance is NaN and fails the exact filter.
    pub fn insert(&mut self, index: u32, position: [f32; 2]) {
        let (col, row) = self.cell_coord(position[0], position[1]);
        self.cells[row * self.cols + col].push(CellEntry { index, position });
    }

    /// Collect indices within `radius` of `center`, in deterministic cell and
    /// insertion order.
    ///
    /// `out` is cleared first. The result is exact: entries in overlapping
    /// cells that lie outside the circle are filtered out. A non-finite center
    /// is clamped like an inserted position (`+∞` far edge, `-∞`/NaN first
    /// cell), and a non-finite `radius` yields no results.
    pub fn query_circle(&self, center: [f32; 2], radius: f32, out: &mut Vec<u32>) {
        out.clear();
        if !radius.is_finite() || radius < 0.0 {
            return;
        }

        let (min_col, min_row) = self.cell_coord(center[0] - radius, center[1] - radius);
        let (max_col, max_row) = self.cell_coord(center[0] + radius, center[1] + radius);
        let radius_squared = radius * radius;

        for row in min_row..=max_row {
            let row_start = row * self.cols;
            for col in min_col..=max_col {
                for entry in &self.cells[row_start + col] {
                    let dx = entry.position[0] - center[0];
                    let dy = entry.position[1] - center[1];
                    if dx * dx + dy * dy <= radius_squared {
                        out.push(entry.index);
                    }
                }
            }
        }
    }

    fn cell_coord(&self, x: f32, y: f32) -> (usize, usize) {
        (
            axis_cell(x, self.cell_size, self.cols),
            axis_cell(y, self.cell_size, self.rows),
        )
    }
}

/// Map one coordinate to a cell index, clamping non-finite values to an edge.
fn axis_cell(value: f32, cell_size: f32, len: usize) -> usize {
    debug_assert!(len > 0);
    let cell = (value / cell_size).floor();
    if cell.is_nan() || cell <= 0.0 {
        return 0;
    }
    let last = len - 1;
    if cell >= last as f32 {
        return last;
    }
    cell as usize
}

/// Diagnostic neighbor counts for the current agent state.
///
/// Counts are exact: every other agent within [`SEPARATION_RADIUS`], without
/// the [`MAX_NEIGHBORS`] cap. [`Self::at_cap`] reports how many agents have
/// more neighbors than a tick can consider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NeighborStats {
    /// Mean number of neighbors per agent.
    pub mean: f32,
    /// Largest neighbor count seen for one agent.
    pub max: usize,
    /// Agents with more than [`MAX_NEIGHBORS`] neighbors, so the tick ignores
    /// the rest.
    pub at_cap: usize,
}

/// Active agents, their spatial index, and fixed-timestep steering.
///
/// Agent ids are `u32`; a simulation holds fewer than `u32::MAX` agents.
#[derive(Debug, Clone)]
pub struct ActiveAgentSim {
    agents: Vec<ActiveAgent>,
    grid: SpatialHash,
    neighbors: Vec<u32>,
    steering: Vec<[f32; 2]>,
    world_size: f32,
    tick_count: u64,
}

impl ActiveAgentSim {
    /// Create `agent_count` agents in the default world, seeded by `seed`.
    #[must_use]
    pub fn new(agent_count: usize, seed: u64) -> Self {
        Self::with_world_size(agent_count, seed, DEFAULT_WORLD_SIZE)
    }

    /// Create `agent_count` agents in a square world of `world_size` meters.
    ///
    /// World sizes below [`MIN_WORLD_SIZE`] (or non-finite sizes) fall back to
    /// [`DEFAULT_WORLD_SIZE`]. The minimum guarantees that a spawn position has
    /// room for a target [`MIN_TARGET_DISTANCE`] away on the x axis in either
    /// direction, without relying on wrap-around.
    ///
    /// # Panics
    ///
    /// Panics if `agent_count` exceeds `u32::MAX`, since ids are `u32`.
    #[must_use]
    pub fn with_world_size(agent_count: usize, seed: u64, world_size: f32) -> Self {
        let world_size = if world_size.is_finite() && world_size >= MIN_WORLD_SIZE {
            world_size
        } else {
            DEFAULT_WORLD_SIZE
        };

        let mut rng = Pcg32::new(seed);
        let agents = (0..agent_count)
            .map(|index| spawn_agent(agent_id(index), &mut rng, world_size))
            .collect();

        Self {
            agents,
            grid: SpatialHash::new(SPATIAL_CELL_SIZE, world_size, world_size),
            neighbors: Vec::with_capacity(MAX_NEIGHBORS * 4),
            steering: Vec::new(),
            world_size,
            tick_count: 0,
        }
    }

    /// Advance the simulation by the fixed [`TICK_DT`].
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);

        self.grid.clear();
        for (index, agent) in self.agents.iter().enumerate() {
            self.grid.insert(agent_id(index), agent.position);
        }

        self.steering.clear();
        self.steering.reserve(self.agents.len());
        for (index, agent) in self.agents.iter().enumerate() {
            let (desired, _) =
                desired_velocity(index, agent, &self.agents, &self.grid, &mut self.neighbors);
            self.steering.push(desired);
        }

        let response = (STEERING_RESPONSE * TICK_DT).min(1.0);
        for (agent, desired) in self.agents.iter_mut().zip(&self.steering) {
            agent.velocity[0] += (desired[0] - agent.velocity[0]) * response;
            agent.velocity[1] += (desired[1] - agent.velocity[1]) * response;
            agent.position[0] += agent.velocity[0] * TICK_DT;
            agent.position[1] += agent.velocity[1] * TICK_DT;
            clamp_to_world(agent, self.world_size);
        }
    }

    /// Compute exact neighbor statistics for the current state.
    ///
    /// Diagnostic use only: this rebuilds the spatial hash and allocates, so it
    /// is not part of the per-tick path.
    #[must_use]
    pub fn neighbor_stats(&self) -> NeighborStats {
        if self.agents.is_empty() {
            return NeighborStats {
                mean: 0.0,
                max: 0,
                at_cap: 0,
            };
        }

        let mut grid = SpatialHash::new(SPATIAL_CELL_SIZE, self.world_size, self.world_size);
        for (index, agent) in self.agents.iter().enumerate() {
            grid.insert(agent_id(index), agent.position);
        }

        let mut scratch = Vec::new();
        let mut total = 0usize;
        let mut max = 0usize;
        let mut at_cap = 0usize;
        for (index, agent) in self.agents.iter().enumerate() {
            grid.query_circle(agent.position, SEPARATION_RADIUS, &mut scratch);
            let neighbors = scratch.iter().filter(|&&id| id as usize != index).count();
            total += neighbors;
            max = max.max(neighbors);
            if neighbors > MAX_NEIGHBORS {
                at_cap += 1;
            }
        }

        NeighborStats {
            mean: total as f32 / self.agents.len() as f32,
            max,
            at_cap,
        }
    }

    /// Read-only agents.
    #[must_use]
    pub fn agents(&self) -> &[ActiveAgent] {
        &self.agents
    }

    /// Number of agents.
    #[must_use]
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Whether the simulation has no agents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Number of completed ticks.
    #[must_use]
    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    /// Side length of the square world, in meters.
    #[must_use]
    pub fn world_size(&self) -> f32 {
        self.world_size
    }
}

/// Convert an agent index to its stable `u32` id.
///
/// One simulation holds fewer than `u32::MAX` agents; this conversion fails
/// loudly instead of truncating if that invariant is ever violated.
fn agent_id(index: usize) -> u32 {
    u32::try_from(index).expect("active agent count exceeds u32::MAX")
}

fn spawn_agent(id: u32, rng: &mut Pcg32, world_size: f32) -> ActiveAgent {
    let usable = world_size - 2.0 * SPAWN_MARGIN;
    let position = [
        SPAWN_MARGIN + rng.next_f32() * usable,
        SPAWN_MARGIN + rng.next_f32() * usable,
    ];

    let mut target = [
        SPAWN_MARGIN + rng.next_f32() * usable,
        SPAWN_MARGIN + rng.next_f32() * usable,
    ];
    if (target[0] - position[0]).abs() < MIN_TARGET_DISTANCE {
        target[0] = (position[0] + MIN_TARGET_DISTANCE).rem_euclid(world_size);
    }

    ActiveAgent {
        id,
        position,
        velocity: [0.0, 0.0],
        target,
        speed: 1.0 + rng.next_f32() * 0.8,
    }
}

/// Desired velocity for one agent, plus the number of neighbors considered.
///
/// The neighbor count is returned for tests and diagnostics; the tick ignores
/// it. It is capped at [`MAX_NEIGHBORS`].
fn desired_velocity(
    index: usize,
    agent: &ActiveAgent,
    agents: &[ActiveAgent],
    grid: &SpatialHash,
    neighbors: &mut Vec<u32>,
) -> ([f32; 2], usize) {
    let to_target = [
        agent.target[0] - agent.position[0],
        agent.target[1] - agent.position[1],
    ];
    let target_distance = (to_target[0] * to_target[0] + to_target[1] * to_target[1]).sqrt();

    let mut desired = [0.0, 0.0];
    if target_distance > ARRIVAL_RADIUS {
        let slowdown = (target_distance / SLOWDOWN_RADIUS).min(1.0);
        let scale = agent.speed * slowdown / target_distance;
        desired[0] = to_target[0] * scale;
        desired[1] = to_target[1] * scale;
    }

    grid.query_circle(agent.position, SEPARATION_RADIUS, neighbors);

    let mut push = [0.0, 0.0];
    let mut considered = 0;
    for &neighbor in neighbors.iter() {
        if neighbor as usize == index {
            continue;
        }
        if considered >= MAX_NEIGHBORS {
            break;
        }
        considered += 1;

        let other = &agents[neighbor as usize];
        let dx = agent.position[0] - other.position[0];
        let dy = agent.position[1] - other.position[1];
        let distance_squared = dx * dx + dy * dy;
        if distance_squared >= SEPARATION_RADIUS * SEPARATION_RADIUS {
            continue;
        }

        if distance_squared <= COINCIDENT_DISTANCE_SQUARED {
            // Deterministic tie-break for exactly overlapping agents:
            // the lower id is pushed toward -x, the higher id toward +x.
            push[0] += if agent.id < other.id { -1.0 } else { 1.0 };
        } else {
            let distance = distance_squared.sqrt();
            let weight = 1.0 - distance / SEPARATION_RADIUS;
            push[0] += dx / distance * weight;
            push[1] += dy / distance * weight;
        }
    }

    // Apply the raw weighted sum: the linear weight sets both the direction and
    // the magnitude of the push. Normalizing here would make a 1.99 m neighbor
    // push as hard as a 0.2 m one. The final clamp below bounds the result to
    // the agent's walking speed.
    if push[0] != 0.0 || push[1] != 0.0 {
        let scale = agent.speed * SEPARATION_STRENGTH;
        desired[0] += push[0] * scale;
        desired[1] += push[1] * scale;
    }

    let desired_length = (desired[0] * desired[0] + desired[1] * desired[1]).sqrt();
    if desired_length > agent.speed && desired_length > 0.0 {
        let scale = agent.speed / desired_length;
        desired[0] *= scale;
        desired[1] *= scale;
    }
    (desired, considered)
}

fn clamp_to_world(agent: &mut ActiveAgent, world_size: f32) {
    if agent.position[0] < 0.0 {
        agent.position[0] = 0.0;
        agent.velocity[0] = agent.velocity[0].max(0.0);
    } else if agent.position[0] > world_size {
        agent.position[0] = world_size;
        agent.velocity[0] = agent.velocity[0].min(0.0);
    }

    if agent.position[1] < 0.0 {
        agent.position[1] = 0.0;
        agent.velocity[1] = agent.velocity[1].max(0.0);
    } else if agent.position[1] > world_size {
        agent.position[1] = world_size;
        agent.velocity[1] = agent.velocity[1].min(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_agent(id: u32, x: f32, y: f32) -> ActiveAgent {
        ActiveAgent {
            id,
            position: [x, y],
            velocity: [0.0, 0.0],
            target: [x + 10.0, y],
            speed: 1.4,
        }
    }

    /// An agent with no steering: its target is its own position, so any
    /// desired velocity comes from separation alone.
    fn stationary_agent(id: u32, position: [f32; 2], speed: f32) -> ActiveAgent {
        ActiveAgent {
            id,
            position,
            velocity: [0.0, 0.0],
            target: position,
            speed,
        }
    }

    #[test]
    fn pcg_is_deterministic_and_in_range() {
        let mut a = Pcg32::new(123);
        let mut b = Pcg32::new(123);
        for _ in 0..256 {
            assert_eq!(a.next_u32(), b.next_u32());
        }

        let mut rng = Pcg32::new(0);
        for _ in 0..1_000 {
            let value = rng.next_f32();
            assert!((0.0..1.0).contains(&value));
        }
    }

    #[test]
    fn same_seed_produces_identical_state() {
        let mut a = ActiveAgentSim::new(256, 42);
        let mut b = ActiveAgentSim::new(256, 42);
        assert_eq!(a.agents(), b.agents());

        for _ in 0..50 {
            a.tick();
            b.tick();
        }
        assert_eq!(a.agents(), b.agents());
        assert_eq!(a.tick_count(), 50);

        let different = ActiveAgentSim::new(256, 43);
        assert_ne!(a.agents(), different.agents());
    }

    #[test]
    fn agents_move_toward_their_targets() {
        let mut sim = ActiveAgentSim::new(1, 7);
        let before = sim.agents()[0].distance_to_target();
        assert!(before >= MIN_TARGET_DISTANCE);

        sim.tick();

        let after = sim.agents()[0].distance_to_target();
        assert!(after < before, "expected {after} < {before}");
        assert!(sim.agents()[0].velocity[0].is_finite());
        assert!(sim.agents()[0].velocity[1].is_finite());
    }

    #[test]
    fn zero_agents_tick_is_a_no_op() {
        let mut sim = ActiveAgentSim::new(0, 99);
        assert!(sim.is_empty());
        assert_eq!(sim.agent_count(), 0);

        for _ in 0..10 {
            sim.tick();
        }
        assert_eq!(sim.tick_count(), 10);
        assert!(sim.agents().is_empty());

        let stats = sim.neighbor_stats();
        assert_eq!(
            stats,
            NeighborStats {
                mean: 0.0,
                max: 0,
                at_cap: 0,
            }
        );
    }

    #[test]
    fn separation_falls_off_with_distance() {
        fn push_magnitude(distance: f32) -> f32 {
            let agent = stationary_agent(0, [1.0, 1.0], 0.5);
            let other = stationary_agent(1, [1.0 + distance, 1.0], 0.5);
            let agents = vec![agent, other];
            let mut grid = SpatialHash::new(4.0, 16.0, 16.0);
            for (index, agent) in agents.iter().enumerate() {
                grid.insert(u32::try_from(index).unwrap(), agent.position);
            }

            let mut scratch = Vec::new();
            let (desired, considered) =
                desired_velocity(0, &agents[0], &agents, &grid, &mut scratch);
            assert_eq!(considered, 1);
            assert!(desired[0] < 0.0, "push should point away from the neighbor");
            (desired[0] * desired[0] + desired[1] * desired[1]).sqrt()
        }

        // Weight is 1 - d / radius; the raw weighted sum is scaled by
        // speed * SEPARATION_STRENGTH before the final speed clamp.
        let near = push_magnitude(0.75);
        let far = push_magnitude(1.9);
        let expected_near = 0.5 * SEPARATION_STRENGTH * (1.0 - 0.75 / SEPARATION_RADIUS);
        let expected_far = 0.5 * SEPARATION_STRENGTH * (1.0 - 1.9 / SEPARATION_RADIUS);
        assert!(
            (near - expected_near).abs() < 1.0e-5,
            "near push {near} != {expected_near}"
        );
        assert!(
            (far - expected_far).abs() < 1.0e-5,
            "far push {far} != {expected_far}"
        );
        assert!(
            near > far * 5.0,
            "near push {near} should be much stronger than far push {far}"
        );

        // A touching neighbor exceeds the speed clamp; the result is bounded by
        // speed instead of growing without limit.
        let touching = push_magnitude(0.2);
        assert!((touching - 0.5).abs() < 1.0e-5, "touching push {touching}");
    }

    #[test]
    fn coincident_agents_push_apart_deterministically() {
        let agents = vec![
            stationary_agent(0, [5.0, 5.0], 0.5),
            stationary_agent(1, [5.0, 5.0], 0.5),
            stationary_agent(2, [5.0, 5.0], 0.5),
        ];
        let mut grid = SpatialHash::new(4.0, 16.0, 16.0);
        for (index, agent) in agents.iter().enumerate() {
            grid.insert(u32::try_from(index).unwrap(), agent.position);
        }

        let mut scratch = Vec::new();
        let desired: Vec<[f32; 2]> = (0..agents.len())
            .map(|index| {
                let (velocity, _) =
                    desired_velocity(index, &agents[index], &agents, &grid, &mut scratch);
                velocity
            })
            .collect();

        // The lower id is pushed toward -x, the higher toward +x, and the
        // middle agent's two pushes cancel.
        assert!(desired[0][0] < 0.0, "agent 0 should move toward -x");
        assert_eq!(desired[1], [0.0, 0.0], "middle agent should cancel out");
        assert!(desired[2][0] > 0.0, "agent 2 should move toward +x");
        for velocity in &desired {
            assert!(velocity[0].is_finite());
            assert!(velocity[1].is_finite());
        }
    }

    #[test]
    fn neighbor_cap_limits_considered_neighbors() {
        let count = MAX_NEIGHBORS + 4;
        let agents: Vec<ActiveAgent> = (0..count)
            .map(|id| stationary_agent(u32::try_from(id).unwrap(), [2.0, 2.0], 0.5))
            .collect();
        let mut grid = SpatialHash::new(4.0, 16.0, 16.0);
        for (index, agent) in agents.iter().enumerate() {
            grid.insert(u32::try_from(index).unwrap(), agent.position);
        }

        let mut scratch = Vec::new();
        let (desired, considered) = desired_velocity(0, &agents[0], &agents, &grid, &mut scratch);
        assert_eq!(considered, MAX_NEIGHBORS);
        assert!(desired[0].is_finite());
        assert!(desired[1].is_finite());

        // Neighbor statistics are exact and report the overflow instead.
        let sim = ActiveAgentSim {
            agents,
            grid,
            neighbors: Vec::new(),
            steering: Vec::new(),
            world_size: 16.0,
            tick_count: 0,
        };
        let stats = sim.neighbor_stats();
        assert_eq!(stats.max, count - 1);
        assert_eq!(stats.at_cap, count);
        assert!((stats.mean - (count - 1) as f32).abs() < 1.0e-5);
    }

    #[test]
    fn worlds_below_the_minimum_fall_back_to_the_default() {
        for bad in [
            MIN_WORLD_SIZE - 0.5,
            16.0,
            0.0,
            -8.0,
            f32::NAN,
            f32::INFINITY,
        ] {
            let sim = ActiveAgentSim::with_world_size(8, 1, bad);
            assert_eq!(
                sim.world_size(),
                DEFAULT_WORLD_SIZE,
                "world size {bad} should fall back"
            );
        }
    }

    #[test]
    fn minimum_world_size_guarantees_target_distance() {
        let sim = ActiveAgentSim::with_world_size(512, 11, MIN_WORLD_SIZE);
        assert_eq!(sim.world_size(), MIN_WORLD_SIZE);
        for agent in sim.agents() {
            let dx = (agent.target[0] - agent.position[0]).abs();
            assert!(
                dx + 1.0e-3 >= MIN_TARGET_DISTANCE,
                "x offset {dx} is below {MIN_TARGET_DISTANCE}"
            );
            assert!(agent.distance_to_target() + 1.0e-3 >= MIN_TARGET_DISTANCE);
        }
    }

    #[test]
    fn spatial_hash_queries_exact_neighbors() {
        let mut grid = SpatialHash::new(4.0, 32.0, 32.0);
        grid.insert(0, [1.0, 1.0]);
        grid.insert(1, [3.0, 3.0]);
        grid.insert(2, [9.0, 1.0]);
        grid.insert(3, [1.0, 9.0]);
        grid.insert(4, [20.0, 20.0]);

        let mut out = Vec::new();
        grid.query_circle([2.0, 2.0], 2.5, &mut out);
        out.sort_unstable();
        assert_eq!(out, vec![0, 1]);

        grid.query_circle([9.0, 1.0], 2.0, &mut out);
        out.sort_unstable();
        assert_eq!(out, vec![2]);

        grid.query_circle([1.0, 1.0], 0.25, &mut out);
        out.sort_unstable();
        assert_eq!(out, vec![0]);

        // A query crossing a cell boundary sees both cells.
        let mut boundary = SpatialHash::new(4.0, 16.0, 16.0);
        boundary.insert(0, [3.9, 4.0]);
        boundary.insert(1, [4.1, 4.0]);
        boundary.query_circle([4.0, 4.0], 0.25, &mut out);
        out.sort_unstable();
        assert_eq!(out, vec![0, 1]);

        // Entries in a shared cell but outside the radius are filtered out.
        boundary.query_circle([3.0, 4.0], 0.25, &mut out);
        assert!(out.is_empty());

        // Out-of-bounds input is clamped instead of panicking.
        boundary.insert(9, [-100.0, 100.0]);
        boundary.query_circle([-100.0, 100.0], 1.0, &mut out);
        assert_eq!(out, vec![9]);
    }

    #[test]
    fn separation_pushes_agents_apart() {
        let agents = vec![test_agent(0, 0.0, 0.0), test_agent(1, 0.2, 0.0)];
        let mut grid = SpatialHash::new(4.0, 16.0, 16.0);
        for (index, agent) in agents.iter().enumerate() {
            grid.insert(index as u32, agent.position);
        }

        let mut scratch = Vec::new();
        let (desired, considered) = desired_velocity(0, &agents[0], &agents, &grid, &mut scratch);
        assert_eq!(considered, 1);
        assert!(desired[0] < 0.0, "agent 0 should be pushed away from +x");
        assert!(desired[0].abs() <= agents[0].speed);
        assert!(desired[1].abs() <= agents[0].speed);
        assert!(desired[0].is_finite());
        assert!(desired[1].is_finite());
    }

    #[test]
    fn spatial_hash_clamps_non_finite_coordinates() {
        let grid = SpatialHash::new(4.0, 16.0, 16.0);
        assert_eq!(grid.cell_coord(f32::INFINITY, f32::INFINITY), (3, 3));
        assert_eq!(
            grid.cell_coord(f32::NEG_INFINITY, f32::NEG_INFINITY),
            (0, 0)
        );
        assert_eq!(grid.cell_coord(f32::NAN, f32::NAN), (0, 0));
        assert_eq!(grid.cell_coord(1.0e9, -1.0e9), (3, 0));

        let mut grid = grid;
        grid.insert(0, [f32::INFINITY, 4.0]);
        grid.insert(1, [f32::NEG_INFINITY, 4.0]);
        grid.insert(2, [f32::NAN, 4.0]);

        // Non-finite distances never satisfy the exact circle filter, but the
        // query and insert must not panic or loop.
        let mut out = Vec::new();
        grid.query_circle([f32::INFINITY, 4.0], 1.0, &mut out);
        assert!(out.is_empty());
        grid.query_circle([f32::NAN, f32::NAN], 1.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn spatial_hash_caps_extreme_world_sizes() {
        let mut grid = SpatialHash::new(4.0, 1.0e30, 1.0e30);
        assert_eq!(grid.cell_count(), MAX_CELLS_PER_AXIS * MAX_CELLS_PER_AXIS);

        grid.insert(0, [1.0e30, 1.0e30]);
        let mut out = Vec::new();
        grid.query_circle([1.0e30, 1.0e30], 1.0, &mut out);
        assert_eq!(out, vec![0]);

        grid.query_circle([-1.0e30, -1.0e30], 1.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn positions_stay_finite_and_in_bounds() {
        let mut sim = ActiveAgentSim::new(DEFAULT_AGENT_COUNT, 1_234);
        for _ in 0..200 {
            sim.tick();
        }

        let world = sim.world_size();
        for agent in sim.agents() {
            assert!(agent.position[0].is_finite());
            assert!(agent.position[1].is_finite());
            assert!(agent.velocity[0].is_finite());
            assert!(agent.velocity[1].is_finite());
            assert!((0.0..=world).contains(&agent.position[0]));
            assert!((0.0..=world).contains(&agent.position[1]));
        }
    }

    #[test]
    fn two_thousand_agents_survive_a_hundred_ticks() {
        let mut sim = ActiveAgentSim::new(DEFAULT_AGENT_COUNT, 2_026);
        for _ in 0..100 {
            sim.tick();
        }

        assert_eq!(sim.agent_count(), DEFAULT_AGENT_COUNT);
        assert_eq!(sim.tick_count(), 100);
        assert!(sim.agents().iter().all(|agent| {
            agent.position[0].is_finite()
                && agent.position[1].is_finite()
                && agent.velocity[0].is_finite()
                && agent.velocity[1].is_finite()
        }));
    }
}
