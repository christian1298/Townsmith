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
        let cols = ((width / cell_size).ceil() as usize).max(1);
        let rows = ((height / cell_size).ceil() as usize).max(1);
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

    /// Insert an index at `position`.
    ///
    /// Positions outside the covered area are clamped to the nearest edge cell.
    pub fn insert(&mut self, index: u32, position: [f32; 2]) {
        let (col, row) = self.cell_coord(position[0], position[1]);
        self.cells[row * self.cols + col].push(CellEntry { index, position });
    }

    /// Collect indices within `radius` of `center`, in deterministic cell and
    /// insertion order.
    ///
    /// `out` is cleared first. The result is exact: entries in overlapping
    /// cells that lie outside the circle are filtered out.
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
        let col = (x / self.cell_size).floor();
        let row = (y / self.cell_size).floor();
        let col = if col.is_finite() {
            (col as usize).min(self.cols - 1)
        } else {
            0
        };
        let row = if row.is_finite() {
            (row as usize).min(self.rows - 1)
        } else {
            0
        };
        (col, row)
    }
}

/// Active agents, their spatial index, and fixed-timestep steering.
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
    /// Invalid or too-small world sizes fall back to [`DEFAULT_WORLD_SIZE`].
    #[must_use]
    pub fn with_world_size(agent_count: usize, seed: u64, world_size: f32) -> Self {
        let world_size = if world_size.is_finite() && world_size > SPAWN_MARGIN * 2.0 {
            world_size
        } else {
            DEFAULT_WORLD_SIZE
        };

        let mut rng = Pcg32::new(seed);
        let agents = (0..agent_count)
            .map(|index| spawn_agent(index as u32, &mut rng, world_size))
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
            self.grid.insert(index as u32, agent.position);
        }

        self.steering.clear();
        self.steering.reserve(self.agents.len());
        for (index, agent) in self.agents.iter().enumerate() {
            let desired =
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

fn desired_velocity(
    index: usize,
    agent: &ActiveAgent,
    agents: &[ActiveAgent],
    grid: &SpatialHash,
    neighbors: &mut Vec<u32>,
) -> [f32; 2] {
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

    let push_length = (push[0] * push[0] + push[1] * push[1]).sqrt();
    if push_length > 0.0 {
        let scale = agent.speed * SEPARATION_STRENGTH / push_length;
        desired[0] += push[0] * scale;
        desired[1] += push[1] * scale;
    }

    let desired_length = (desired[0] * desired[0] + desired[1] * desired[1]).sqrt();
    if desired_length > agent.speed && desired_length > 0.0 {
        let scale = agent.speed / desired_length;
        desired[0] *= scale;
        desired[1] *= scale;
    }
    desired
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
        assert!(before > MIN_TARGET_DISTANCE);

        sim.tick();

        let after = sim.agents()[0].distance_to_target();
        assert!(after < before, "expected {after} < {before}");
        assert!(sim.agents()[0].velocity[0].is_finite());
        assert!(sim.agents()[0].velocity[1].is_finite());
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
        let desired = desired_velocity(0, &agents[0], &agents, &grid, &mut scratch);
        assert!(desired[0] < 0.0, "agent 0 should be pushed away from +x");
        assert!(desired[0].abs() <= agents[0].speed);
        assert!(desired[1].abs() <= agents[0].speed);
        assert!(desired[0].is_finite());
        assert!(desired[1].is_finite());
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
