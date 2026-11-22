//! Per-citizen movement over cached home-work routes.
//!
//! The materialization layer answers *which* named residents are active around
//! the camera. This module answers *where they are* at a presentation time. It
//! groups the active selection by household, chooses one reachable workplace
//! per household, and reuses one cached polyline for every resident of that
//! household:
//!
//! - the workplace is the nearest one with `slots > 0` by road distance
//!   ([`RoadGraph::distance_m`]), tie-broken by workplace id, so the choice does
//!   not depend on the order of the input slice;
//! - the polyline is built once through [`RouteCache`] and shared by the
//!   household's residents;
//! - each resident walks out for the first half of a round trip and back for
//!   the second half, with a stable phase derived from its identity id.
//!
//! The layer is derived presentation state. It is not saved, it does not mutate
//! the road graph, and it does not change household jobs, needs, or commute
//! fields. A household with no workplace, no reachable workplace, or a
//! non-finite home is skipped: it contributes no route and no positions.

use std::collections::{BTreeMap, BTreeSet};

use crate::named::{mix64, MaterializationSelection, MaterializedCitizen};
use crate::population::{Household, Workplace};
use crate::route_cache::{CachedRoute, RouteCache};
use crate::traffic::{fnv1a_feed, FlowField, RoadGraph, FNV_OFFSET_BASIS};

/// Shortest round-trip cycle used by [`CitizenMotion::positions_at`].
///
/// A route can be degenerate (coincident home and workplace), which would
/// otherwise produce a zero-length cycle and a division by zero. Clamping to
/// one second keeps the sample finite and still deterministic.
pub const MIN_MOVEMENT_CYCLE_S: f64 = 1.0;

/// Salt for the identity-to-phase hash. Changing it rephases every citizen.
const PHASE_SALT: u64 = 0x6d6f_7665_6d65_6e74;

/// Which half of the round trip a sample is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionLeg {
    /// Home to workplace, the first half of the cycle.
    Outbound,
    /// Workplace to home, the second half of the cycle.
    Return,
}

/// One citizen's presentation position at a sampled time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CitizenPosition {
    /// Stable 63-bit identity.
    pub id: u64,
    /// Home placement id.
    pub placement_id: u64,
    /// Resident slot inside the aggregate household.
    pub slot: u32,
    /// World position in metres on the home-work polyline.
    pub position: [f32; 2],
    /// Direction of travel for this sample.
    pub leg: MotionLeg,
    /// Fraction of the outbound polyline traversed: `0.0` at home, `1.0` at the
    /// workplace. It rises on the outbound leg and falls on the return leg, so
    /// a return samples the same polyline in reverse.
    pub route_progress: f32,
}

/// Snapshot of the route cache used while building a [`CitizenMotion`].
///
/// The cache itself is released after construction; the counters and checksum
/// are kept so a HUD or test can prove that households shared entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MotionCacheStats {
    /// Capacity after [`RouteCache::new`] clamps it to its hard bounds.
    pub capacity: usize,
    /// Routes retained in the cache when construction finished.
    pub len: usize,
    /// Cache hits during construction.
    pub hits: u64,
    /// Cache misses during construction, including failed routes.
    pub misses: u64,
}

impl MotionCacheStats {
    /// Whether the construction cache retained no route.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// One household's shared home-work polyline.
#[derive(Debug, Clone, PartialEq)]
struct HouseholdRoute {
    placement_id: u64,
    workplace_id: u64,
    points: Vec<[f32; 2]>,
    /// Arc length at each point; `cumulative[0] == 0.0`.
    cumulative: Vec<f32>,
    length_m: f32,
    /// Round-trip duration: out and back.
    cycle_s: f64,
}

impl HouseholdRoute {
    fn from_cached(placement_id: u64, workplace_id: u64, route: &CachedRoute) -> Self {
        let points = route.points().to_vec();
        let mut cumulative = Vec::with_capacity(points.len());
        let mut length_m = 0.0_f32;
        cumulative.push(0.0);
        for pair in points.windows(2) {
            let dx = pair[1][0] - pair[0][0];
            let dz = pair[1][1] - pair[0][1];
            length_m += (dx * dx + dz * dz).sqrt();
            cumulative.push(length_m);
        }
        Self {
            placement_id,
            workplace_id,
            points,
            cumulative,
            length_m,
            cycle_s: (2.0 * f64::from(route.time_s())).max(MIN_MOVEMENT_CYCLE_S),
        }
    }

    /// Sample the polyline at an arc length in metres.
    ///
    /// Returns the interpolated point and the outbound fraction in `0..=1`.
    /// The degree of the polyline is low (a handful of road nodes plus walking
    /// legs), so a binary search over the cumulative lengths is cheap.
    fn sample(&self, distance_m: f32) -> ([f32; 2], f32) {
        let Some(&first) = self.points.first() else {
            return ([0.0, 0.0], 0.0);
        };
        if self.points.len() < 2 || self.length_m <= 0.0 {
            return (first, 0.0);
        }
        let target = distance_m.clamp(0.0, self.length_m);
        let progress = target / self.length_m;
        if target <= 0.0 {
            return (first, 0.0);
        }
        let Some(&last) = self.points.last() else {
            return ([0.0, 0.0], 0.0);
        };
        if target >= self.length_m {
            return (last, 1.0);
        }

        let index = self
            .cumulative
            .partition_point(|&value| value <= target)
            .saturating_sub(1)
            .min(self.points.len() - 2);
        let start_m = self.cumulative[index];
        let segment_m = self.cumulative[index + 1] - start_m;
        if segment_m <= 0.0 {
            return (self.points[index], progress);
        }
        let t = (target - start_m) / segment_m;
        let from = self.points[index];
        let to = self.points[index + 1];
        (
            [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ],
            progress,
        )
    }
}

/// Borrowed, read-only view of one household's shared home-work polyline.
///
/// Returned by [`CitizenMotion::routes`]. The underlying route stays private and
/// immutable: a view cannot mutate the motion layer, and it borrows the points
/// instead of cloning them. Point order is home first, then the road nodes,
/// then the workplace, matching [`RoadGraph::route_points`].
///
/// [`RoadGraph::route_points`]: crate::traffic::RoadGraph::route_points
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HouseholdRouteView<'a> {
    /// Home placement id.
    pub placement_id: u64,
    /// Chosen workplace id.
    pub workplace_id: u64,
    /// Polyline points in metres on the world `x/z` plane.
    pub points: &'a [[f32; 2]],
    /// Outbound polyline length in metres.
    pub length_m: f32,
    /// Round-trip duration: out and back, in seconds.
    pub cycle_s: f64,
}

/// One resident's stable track on a household route.
#[derive(Debug, Clone, PartialEq)]
struct CitizenTrack {
    id: u64,
    placement_id: u64,
    slot: u32,
    /// Index into [`CitizenMotion::routes`].
    route: usize,
    /// Offset into the cycle at which this resident leaves home.
    phase_s: f64,
}

/// Derived per-citizen movement for one materialization selection.
///
/// Construction groups [`MaterializationSelection::active`] by
/// [`MaterializedCitizen::placement_id`], orders households by placement id, and
/// orders residents by `(slot, id)`. [`Self::positions_at`] returns samples in
/// that same order, so repeated samples and repeated builds are directly
/// comparable. Only the active set is used; detailed citizens are already a
/// subset of active in the selection layer.
///
/// # Determinism
///
/// Workplace choice follows `(road distance, workplace id)`, route-cache
/// eviction follows insertion order, and phase offsets are hashes of the
/// identity id. No wall clock, hash iteration order, or floating-point
/// transcendental is involved.
#[derive(Debug, Clone, PartialEq)]
pub struct CitizenMotion {
    routes: Vec<HouseholdRoute>,
    citizens: Vec<CitizenTrack>,
    cache_stats: MotionCacheStats,
    cache_checksum: u64,
}

impl CitizenMotion {
    /// Build the movement layer for a materialization selection.
    ///
    /// `route_cache_capacity` is passed to [`RouteCache::new`], so it is clamped
    /// to `1..=MAX_ROUTE_CACHE_CAPACITY`. The cache is used only during
    /// construction: each household route is copied into the motion layer once,
    /// and the cache counters plus content checksum are retained for diagnostics.
    ///
    /// A household is unrouted when it has no workplace with positive slots, no
    /// reachable workplace, or a non-finite home. Its residents are omitted from
    /// [`Self::positions_at`]; the build does not panic.
    ///
    /// Duplicate citizen ids are tolerated: the first occurrence in household
    /// order (placement id, then slot and id) wins.
    #[must_use]
    pub fn new(
        selection: &MaterializationSelection,
        workplaces: &[Workplace],
        graph: &RoadGraph,
        route_cache_capacity: usize,
    ) -> Self {
        Self::new_with_households(
            selection,
            &BTreeMap::new(),
            workplaces,
            graph,
            route_cache_capacity,
        )
    }

    /// Build movement with remembered household workplaces.
    ///
    /// A household with `workplace_id` set tries that workplace first; unknown,
    /// zero-slot, unreachable, or disconnected ids fall back to the nearest
    /// reachable workplace. Old saves and callers without households use
    /// [`Self::new`].
    #[must_use]
    pub fn new_with_households(
        selection: &MaterializationSelection,
        remembered: &BTreeMap<u64, Household>,
        workplaces: &[Workplace],
        graph: &RoadGraph,
        route_cache_capacity: usize,
    ) -> Self {
        let mut households: BTreeMap<u64, BTreeMap<u64, &MaterializedCitizen>> = BTreeMap::new();
        for citizen in &selection.active {
            households
                .entry(citizen.placement_id)
                .or_default()
                .entry(citizen.id)
                .or_insert(citizen);
        }

        // One shortest-path field per workplace is far cheaper than running a
        // pair query for every home/workplace combination. Flow distance uses
        // the same access legs and integer path weights as `distance_m`.
        let fields: Vec<Option<FlowField>> = workplaces
            .iter()
            .map(|workplace| {
                if workplace.slots == 0 || !workplace.center.iter().all(|value| value.is_finite()) {
                    None
                } else {
                    graph.flow_to(workplace.center)
                }
            })
            .collect();
        let mut cache = RouteCache::new(route_cache_capacity);
        let mut routes = Vec::new();
        let mut citizens = Vec::new();
        let mut seen_ids = BTreeSet::new();

        for (placement_id, members) in households {
            let mut members: Vec<&MaterializedCitizen> = members
                .into_values()
                .filter(|c| !seen_ids.contains(&c.id))
                .collect();
            members.sort_by_key(|citizen| (citizen.slot, citizen.id));
            let Some(home_citizen) = members.first() else {
                continue;
            };
            let home = home_citizen.home;
            if !home.iter().all(|value| value.is_finite()) {
                continue;
            }
            let remembered_workplace = remembered
                .get(&placement_id)
                .and_then(|household| household.workplace_id);
            let Some(route) = choose_route(
                placement_id,
                home,
                remembered_workplace,
                workplaces,
                &fields,
                graph,
                &mut cache,
            ) else {
                continue;
            };

            let route_index = routes.len();
            let cycle_s = route.cycle_s;
            routes.push(route);
            for member in members {
                seen_ids.insert(member.id);
                citizens.push(CitizenTrack {
                    id: member.id,
                    placement_id,
                    slot: member.slot,
                    route: route_index,
                    phase_s: phase_offset_s(member.id, cycle_s),
                });
            }
        }

        let cache_stats = MotionCacheStats {
            capacity: cache.capacity(),
            len: cache.len(),
            hits: cache.hits(),
            misses: cache.misses(),
        };
        let cache_checksum = cache.checksum();

        Self {
            routes,
            citizens,
            cache_stats,
            cache_checksum,
        }
    }

    /// Sample every routed citizen at `time_s`.
    ///
    /// Time is presentation time in seconds. Motion is periodic: each citizen
    /// leaves home at its phase offset and completes one out-and-back cycle in
    /// its household's `cycle_s`. The first half of the cycle walks the
    /// home-to-work polyline; the second half walks it back. Segment positions
    /// are interpolated by arc length, not by vertex count.
    ///
    /// A non-finite `time_s` returns an empty vector. Negative finite time is
    /// valid and wraps periodically. A huge finite time is reduced with
    /// `rem_euclid`, so it cannot panic.
    ///
    /// Results are ordered by placement id, then `(slot, id)`, matching the
    /// selection grouping. Citizens in unrouted households are absent.
    #[must_use]
    pub fn positions_at(&self, time_s: f64) -> Vec<CitizenPosition> {
        if !time_s.is_finite() {
            return Vec::new();
        }
        let mut positions = Vec::with_capacity(self.citizens.len());
        for citizen in &self.citizens {
            let route = &self.routes[citizen.route];
            let half_cycle_s = route.cycle_s * 0.5;
            let elapsed_s = (time_s - citizen.phase_s).rem_euclid(route.cycle_s);
            let (leg, distance_m) = if elapsed_s < half_cycle_s {
                let fraction = if half_cycle_s > 0.0 {
                    elapsed_s / half_cycle_s
                } else {
                    0.0
                };
                (
                    MotionLeg::Outbound,
                    (fraction * f64::from(route.length_m)) as f32,
                )
            } else {
                let fraction = if half_cycle_s > 0.0 {
                    (elapsed_s - half_cycle_s) / half_cycle_s
                } else {
                    1.0
                };
                (
                    MotionLeg::Return,
                    ((1.0 - fraction) * f64::from(route.length_m)) as f32,
                )
            };
            let (position, route_progress) = route.sample(distance_m);
            positions.push(CitizenPosition {
                id: citizen.id,
                placement_id: citizen.placement_id,
                slot: citizen.slot,
                position,
                leg,
                route_progress,
            });
        }
        positions
    }

    /// Number of distinct household routes.
    #[must_use]
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Read-only views of every routed household, ordered by placement id.
    ///
    /// The iterator borrows each route's polyline, so drawing a debug overlay
    /// does not clone the geometry. The internal route type is never exposed
    /// mutably. A motion with no routed household yields an empty iterator.
    pub fn routes(&self) -> impl Iterator<Item = HouseholdRouteView<'_>> + '_ {
        self.routes.iter().map(|route| HouseholdRouteView {
            placement_id: route.placement_id,
            workplace_id: route.workplace_id,
            points: &route.points,
            length_m: route.length_m,
            cycle_s: route.cycle_s,
        })
    }

    /// Number of routed citizens with a position track.
    #[must_use]
    pub fn citizen_count(&self) -> usize {
        self.citizens.len()
    }

    /// Whether no citizen has a route.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.citizens.is_empty()
    }

    /// Snapshot of the construction route cache.
    #[must_use]
    pub const fn cache_stats(&self) -> MotionCacheStats {
        self.cache_stats
    }

    /// FNV-1a checksum of the construction route cache contents.
    ///
    /// This covers the routes retained when construction finished, in key
    /// order. It is stable across rebuilds with the same inputs and cache
    /// capacity even though the cache itself is not kept.
    #[must_use]
    pub const fn cache_checksum(&self) -> u64 {
        self.cache_checksum
    }

    /// Stable FNV-1a checksum over routes and citizen tracks.
    ///
    /// The checksum depends on derived content, not on the ordering of the
    /// input selection, so a rebuilt motion with the same households hashes
    /// identically.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        fnv1a_feed(&mut hash, self.routes.len() as u64);
        for route in &self.routes {
            fnv1a_feed(&mut hash, route.placement_id);
            fnv1a_feed(&mut hash, route.workplace_id);
            fnv1a_feed(&mut hash, route.points.len() as u64);
            for point in &route.points {
                fnv1a_feed(&mut hash, u64::from(point[0].to_bits()));
                fnv1a_feed(&mut hash, u64::from(point[1].to_bits()));
            }
            fnv1a_feed(&mut hash, u64::from(route.length_m.to_bits()));
            fnv1a_feed(&mut hash, route.cycle_s.to_bits());
        }
        fnv1a_feed(&mut hash, self.citizens.len() as u64);
        for citizen in &self.citizens {
            fnv1a_feed(&mut hash, citizen.id);
            fnv1a_feed(&mut hash, citizen.placement_id);
            fnv1a_feed(&mut hash, u64::from(citizen.slot));
            fnv1a_feed(&mut hash, citizen.route as u64);
            fnv1a_feed(&mut hash, citizen.phase_s.to_bits());
        }
        hash
    }
}

/// Stable phase offset in `0..cycle_s` for one identity.
///
/// The value is a 53-bit hash of the identity id scaled by `cycle_s`. It is the
/// time at which the citizen leaves home; adding one cycle returns it to the
/// same phase. A non-finite or non-positive `cycle_s` returns `0.0`.
#[must_use]
pub fn phase_offset_s(id: u64, cycle_s: f64) -> f64 {
    if !cycle_s.is_finite() || cycle_s <= 0.0 {
        return 0.0;
    }
    let unit = (mix64(id ^ PHASE_SALT) >> 11) as f64 / (1_u64 << 53) as f64;
    (unit * cycle_s).rem_euclid(cycle_s)
}

/// Pick the nearest reachable workplace and build its route.
///
/// Candidates are workplaces with `slots > 0`, finite centres, and a finite
/// road distance from `home`. They are ordered by `(distance, id, input index)`,
/// and the first candidate whose polyline builds wins. A failed build is treated
/// like an unreachable candidate rather than an error.
fn choose_route(
    placement_id: u64,
    home: [f32; 2],
    remembered_workplace: Option<u64>,
    workplaces: &[Workplace],
    fields: &[Option<FlowField>],
    graph: &RoadGraph,
    cache: &mut RouteCache,
) -> Option<HouseholdRoute> {
    if let Some(remembered) = remembered_workplace {
        if let Some(index) = workplaces
            .iter()
            .position(|workplace| workplace.id == remembered && workplace.slots > 0)
        {
            let workplace = workplaces[index];
            if fields
                .get(index)
                .and_then(Option::as_ref)
                .and_then(|field| graph.flow_distance_m(home, field))
                .is_some_and(f32::is_finite)
            {
                if let Some(route) = cache.get_or_build(graph, home, workplace.center) {
                    return Some(HouseholdRoute::from_cached(
                        placement_id,
                        workplace.id,
                        route,
                    ));
                }
            }
        }
    }
    let mut candidates: Vec<(f32, u64, usize)> = workplaces
        .iter()
        .enumerate()
        .filter(|(_, workplace)| workplace.slots > 0)
        .filter_map(|(index, workplace)| {
            let field = fields.get(index)?.as_ref()?;
            graph
                .flow_distance_m(home, field)
                .filter(|distance| distance.is_finite())
                .map(|distance| (distance, workplace.id, index))
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });

    for (_, workplace_id, index) in candidates {
        let workplace = workplaces[index];
        let Some(route) = cache.get_or_build(graph, home, workplace.center) else {
            continue;
        };
        return Some(HouseholdRoute::from_cached(
            placement_id,
            workplace_id,
            route,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::named::{citizen_id, name_for};
    use crate::road::RoadSegment;

    const SEED: u64 = 2026;

    fn road(id: u64, start: [i32; 2], end: [i32; 2]) -> RoadSegment {
        RoadSegment {
            id,
            start,
            end,
            width_m: 8,
        }
    }

    fn straight_graph() -> RoadGraph {
        RoadGraph::build(&[road(1, [0, 0], [40, 0])]).expect("build")
    }

    fn workplace(id: u64, slots: u32, center: [f32; 2]) -> Workplace {
        Workplace { id, slots, center }
    }

    fn citizen(placement_id: u64, slot: u32) -> MaterializedCitizen {
        let id = citizen_id(SEED, placement_id, slot);
        MaterializedCitizen {
            id,
            placement_id,
            slot,
            name: name_for(id),
            home: [0.0, 0.0],
            distance: 0.0,
        }
    }

    fn selection(citizens: Vec<MaterializedCitizen>) -> MaterializationSelection {
        MaterializationSelection {
            active: citizens,
            detailed: Vec::new(),
        }
    }

    #[test]
    fn empty_inputs_produce_an_empty_motion() {
        let graph = straight_graph();
        let workplaces = [workplace(90, 4, [40.0, 0.0])];

        let empty = CitizenMotion::new(&MaterializationSelection::default(), &[], &graph, 4);
        assert_eq!(empty.route_count(), 0);
        assert_eq!(empty.citizen_count(), 0);
        assert!(empty.is_empty());
        assert!(empty.positions_at(0.0).is_empty());
        assert_eq!(
            empty.cache_stats(),
            MotionCacheStats {
                capacity: 4,
                len: 0,
                hits: 0,
                misses: 0,
            }
        );
        assert_eq!(empty.cache_checksum(), RouteCache::new(4).checksum());

        let no_workplaces = CitizenMotion::new(&selection(vec![citizen(7, 0)]), &[], &graph, 4);
        assert_eq!(no_workplaces.route_count(), 0);
        assert_eq!(no_workplaces.citizen_count(), 0);
        assert_eq!(no_workplaces.cache_stats().misses, 0);

        let no_citizens =
            CitizenMotion::new(&MaterializationSelection::default(), &workplaces, &graph, 0);
        assert_eq!(no_citizens.citizen_count(), 0);
        assert_eq!(no_citizens.cache_stats().capacity, 1);
    }

    #[test]
    fn one_household_shares_one_route_with_distinct_offsets() {
        let graph = straight_graph();
        let residents = vec![citizen(7, 0), citizen(7, 1), citizen(7, 2)];
        let motion = CitizenMotion::new(
            &selection(residents.clone()),
            &[workplace(90, 1, [40.0, 0.0])],
            &graph,
            8,
        );

        assert_eq!(motion.route_count(), 1);
        assert_eq!(motion.citizen_count(), 3);
        assert_eq!(motion.cache_stats().misses, 1);
        assert_eq!(motion.cache_stats().hits, 0);
        assert!(!motion.is_empty());

        let positions = motion.positions_at(1.0);
        assert_eq!(positions.len(), 3);
        for position in &positions {
            assert!((0.0..=40.0).contains(&position.position[0]));
            assert_eq!(position.position[1], 0.0);
            assert!((0.0..=1.0).contains(&position.route_progress));
        }
        let xs: Vec<f32> = positions
            .iter()
            .map(|position| position.position[0])
            .collect();
        assert_ne!(xs[0], xs[1]);
        assert_ne!(xs[0], xs[2]);
        assert_ne!(xs[1], xs[2]);
        assert_eq!(
            positions
                .iter()
                .map(|position| position.id)
                .collect::<Vec<_>>(),
            residents
                .iter()
                .map(|citizen| citizen.id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn routes_expose_read_only_polylines_home_to_work() {
        let graph = straight_graph();
        let motion = CitizenMotion::new(
            &selection(vec![citizen(1, 0), citizen(2, 0)]),
            &[workplace(90, 4, [40.0, 0.0])],
            &graph,
            4,
        );

        let routes: Vec<HouseholdRouteView<'_>> = motion.routes().collect();
        assert_eq!(routes.len(), motion.route_count());
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].placement_id, 1);
        assert_eq!(routes[1].placement_id, 2);
        for view in &routes {
            assert_eq!(view.workplace_id, 90);
            assert!(view.points.len() >= 2);
            assert_eq!(view.points.first().copied(), Some([0.0, 0.0]));
            assert_eq!(view.points.last().copied(), Some([40.0, 0.0]));
            assert!((view.length_m - 40.0).abs() < 0.001);
            assert!(view.cycle_s >= MIN_MOVEMENT_CYCLE_S);
        }

        let empty = CitizenMotion::new(&MaterializationSelection::default(), &[], &graph, 4);
        assert_eq!(empty.routes().count(), 0);
    }

    #[test]
    fn households_with_the_same_route_share_a_cache_entry() {
        let graph = straight_graph();
        let motion = CitizenMotion::new(
            &selection(vec![citizen(1, 0), citizen(2, 0)]),
            &[workplace(90, 4, [40.0, 0.0])],
            &graph,
            4,
        );

        assert_eq!(motion.route_count(), 2);
        assert_eq!(motion.citizen_count(), 2);
        let stats = motion.cache_stats();
        assert_eq!(stats.capacity, 4);
        assert_eq!(stats.len, 1);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn a_disconnected_household_is_skipped_without_panicking() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [50, 50], [60, 50])])
            .expect("build");
        let motion = CitizenMotion::new(
            &selection(vec![citizen(1, 0)]),
            &[workplace(90, 1, [55.0, 50.0])],
            &graph,
            4,
        );
        assert_eq!(motion.route_count(), 0);
        assert_eq!(motion.citizen_count(), 0);
        assert!(motion.positions_at(0.0).is_empty());
        assert!(motion.positions_at(f64::NAN).is_empty());

        let empty_graph = RoadGraph::build(&[]).expect("build");
        let empty = CitizenMotion::new(
            &selection(vec![citizen(1, 0)]),
            &[workplace(90, 1, [40.0, 0.0])],
            &empty_graph,
            4,
        );
        assert_eq!(empty.citizen_count(), 0);
        assert!(empty.positions_at(1.0).is_empty());
    }

    #[test]
    fn builds_are_deterministic_and_input_order_independent() {
        let graph = straight_graph();
        let workplaces = [workplace(90, 4, [40.0, 0.0]), workplace(91, 4, [20.0, 0.0])];
        let forward = selection(vec![citizen(1, 0), citizen(1, 1), citizen(2, 0)]);
        let mut reversed = forward.clone();
        reversed.active.reverse();

        let first = CitizenMotion::new(&forward, &workplaces, &graph, 4);
        let second = CitizenMotion::new(&reversed, &workplaces, &graph, 4);

        assert_eq!(first, second);
        assert_eq!(first.checksum(), second.checksum());
        assert_eq!(first.cache_checksum(), second.cache_checksum());
        assert_eq!(first.positions_at(3.25), second.positions_at(3.25));
    }

    #[test]
    fn nearest_workplace_uses_road_distance_then_workplace_id() {
        let graph = straight_graph();
        // Both are 20 m away. The lower id wins the tie.
        let workplaces = [
            workplace(20, 1, [20.0, 0.0]),
            workplace(10, 1, [-20.0, 0.0]),
            workplace(1, 0, [1.0, 0.0]),
        ];
        let fields: Vec<Option<FlowField>> = workplaces
            .iter()
            .map(|workplace| graph.flow_to(workplace.center))
            .collect();
        let mut cache = RouteCache::new(4);
        let route = choose_route(
            7,
            [0.0, 0.0],
            None,
            &workplaces,
            &fields,
            &graph,
            &mut cache,
        )
        .expect("route");
        assert_eq!(route.workplace_id, 10);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn positions_loop_back_to_home_at_the_cycle_boundary() {
        let graph = straight_graph();
        let residents = vec![citizen(7, 0), citizen(7, 1), citizen(7, 2)];
        let motion = CitizenMotion::new(
            &selection(residents.clone()),
            &[workplace(90, 4, [40.0, 0.0])],
            &graph,
            4,
        );
        // 40 m at 8 m/s each way: 5 s out, 5 s back.
        let cycle_s = 10.0;

        for resident in &residents {
            let phase_s = phase_offset_s(resident.id, cycle_s);
            let home = motion.positions_at(phase_s);
            let home_again = motion.positions_at(phase_s + cycle_s);
            let start = home
                .iter()
                .find(|position| position.id == resident.id)
                .expect("start position");
            let end = home_again
                .iter()
                .find(|position| position.id == resident.id)
                .expect("end position");
            assert_eq!(start.position, [0.0, 0.0]);
            assert_eq!(start.leg, MotionLeg::Outbound);
            assert_eq!(start.route_progress, 0.0);
            assert!(end.position[0].abs() < 1.0e-4);
            assert!(end.position[1].abs() < 1.0e-4);
        }
    }

    #[test]
    fn invalid_time_is_empty_and_negative_time_wraps() {
        let graph = straight_graph();
        let motion = CitizenMotion::new(
            &selection(vec![citizen(7, 0), citizen(7, 1)]),
            &[workplace(90, 4, [40.0, 0.0])],
            &graph,
            4,
        );

        assert!(motion.positions_at(f64::NAN).is_empty());
        assert!(motion.positions_at(f64::INFINITY).is_empty());
        assert!(motion.positions_at(f64::NEG_INFINITY).is_empty());

        let forward = motion.positions_at(3.0);
        let backward = motion.positions_at(3.0 - 10.0);
        assert_eq!(forward.len(), 2);
        assert_eq!(backward.len(), 2);
        for (left, right) in forward.iter().zip(&backward) {
            assert_eq!(left.id, right.id);
            assert!((left.position[0] - right.position[0]).abs() < 1.0e-4);
            assert!((left.position[1] - right.position[1]).abs() < 1.0e-4);
            assert_eq!(left.leg, right.leg);
        }

        let huge = motion.positions_at(1.0e12);
        assert_eq!(huge.len(), 2);
        for position in huge {
            assert!(position.position.iter().all(|value| value.is_finite()));
        }
    }
}
