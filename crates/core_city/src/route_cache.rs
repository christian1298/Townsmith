//! Bounded home-work route polylines for materialized citizens.
//!
//! N3 needs a visible route for every active citizen without running
//! pathfinding per citizen per tick. [`RoadGraph::route_points`] builds one
//! polyline for a home/workplace pair; [`RouteCache`] keeps the result until
//! the pair is evicted, so a repeated pair costs one map lookup.
//!
//! The cache is derived state. It is not saved, it does not mutate the road
//! graph, and it does not change job assignment or household commute data.
//! Keys are endpoints rounded to one centimetre; eviction is least-recently
//! used with a hard cap, and every decision follows insertion/lookup order
//! rather than wall time or hashing.
//!
//! [`RoadGraph::route_points`]: crate::traffic::RoadGraph::route_points

use std::collections::{BTreeMap, VecDeque};

use crate::traffic::{
    fnv1a_feed, travel_time_seconds, RoadGraph, DEFAULT_TRAVEL_SPEED_MPS, FNV_OFFSET_BASIS,
};

/// Default number of home-work routes kept by [`RouteCache`].
pub const DEFAULT_ROUTE_CACHE_CAPACITY: usize = 512;

/// Hard cap for [`RouteCache`], independent of the requested capacity.
pub const MAX_ROUTE_CACHE_CAPACITY: usize = 4_096;

/// One cached route polyline and its measurement-layer distance and time.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedRoute {
    points: Vec<[f32; 2]>,
    length_m: f32,
    time_s: f32,
}

impl CachedRoute {
    /// Route polyline from home to workplace, including both walking legs.
    #[must_use]
    pub fn points(&self) -> &[[f32; 2]] {
        &self.points
    }

    /// Polyline length in metres, the sum of the segment lengths.
    #[must_use]
    pub const fn length_m(&self) -> f32 {
        self.length_m
    }

    /// Travel time in seconds at [`DEFAULT_TRAVEL_SPEED_MPS`].
    ///
    /// This is the existing measurement-layer speed, not a per-road or
    /// congested speed.
    #[must_use]
    pub const fn time_s(&self) -> f32 {
        self.time_s
    }

    /// FNV-1a checksum over the polyline, length, and time.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        fnv1a_feed(&mut hash, self.points.len() as u64);
        for point in &self.points {
            fnv1a_feed(&mut hash, u64::from(point[0].to_bits()));
            fnv1a_feed(&mut hash, u64::from(point[1].to_bits()));
        }
        fnv1a_feed(&mut hash, u64::from(self.length_m.to_bits()));
        fnv1a_feed(&mut hash, u64::from(self.time_s.to_bits()));
        hash
    }

    fn from_points(points: Vec<[f32; 2]>) -> Option<Self> {
        if points.len() < 2 || !points.iter().flatten().all(|value| value.is_finite()) {
            return None;
        }
        let mut length_m = 0.0_f32;
        for pair in points.windows(2) {
            let dx = pair[1][0] - pair[0][0];
            let dz = pair[1][1] - pair[0][1];
            length_m += (dx * dx + dz * dz).sqrt();
        }
        if !length_m.is_finite() {
            return None;
        }
        let time_s = travel_time_seconds(length_m, DEFAULT_TRAVEL_SPEED_MPS)?;
        Some(Self {
            points,
            length_m,
            time_s,
        })
    }
}

/// Bounded least-recently-used cache of home-work route polylines.
///
/// The key is `(home, workplace)` rounded to one centimetre, direction
/// included: reversing a commute is a separate route. A hit refreshes the
/// entry's recency; a miss builds through [`RoadGraph::route_points`] and
/// evicts the least-recently used entry when the cache is full. Invalid or
/// disconnected pairs are counted as misses and are not stored.
///
/// A hit returns the exact polyline the graph would build, without another
/// Dijkstra pass. The cache holds no road-edit version: callers must
/// [`clear`](Self::clear) it (or drop it) when the road graph changes.
#[derive(Debug, Clone)]
pub struct RouteCache {
    routes: BTreeMap<RouteKey, CachedRoute>,
    order: VecDeque<RouteKey>,
    capacity: usize,
    hits: u64,
    misses: u64,
}

/// Endpoints quantized to centimetres, ordered `(from, to)`.
type RouteKey = [i64; 4];

impl Default for RouteCache {
    fn default() -> Self {
        Self::new(DEFAULT_ROUTE_CACHE_CAPACITY)
    }
}

impl RouteCache {
    /// Create a cache holding at most `capacity` routes.
    ///
    /// The capacity is clamped to `1..=MAX_ROUTE_CACHE_CAPACITY` so every
    /// successful build can be returned by reference and one cache cannot
    /// grow without a hard bound.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            routes: BTreeMap::new(),
            order: VecDeque::new(),
            capacity: capacity.clamp(1, MAX_ROUTE_CACHE_CAPACITY),
            hits: 0,
            misses: 0,
        }
    }

    /// Return a cached route for a home/workplace pair, if present.
    ///
    /// This is a read-only lookup: it does not count a hit, refresh recency,
    /// or build anything.
    #[must_use]
    pub fn get(&self, home: [f32; 2], workplace: [f32; 2]) -> Option<&CachedRoute> {
        self.routes.get(&route_key(home, workplace)?)
    }

    /// Return the route for a pair, building and caching it on a miss.
    ///
    /// Returns `None` for non-finite endpoints, an empty graph, or a
    /// disconnected pair. The miss is counted even when no route is stored.
    pub fn get_or_build(
        &mut self,
        graph: &RoadGraph,
        home: [f32; 2],
        workplace: [f32; 2],
    ) -> Option<&CachedRoute> {
        self.get_or_insert_with(home, workplace, |from, to| graph.route_points(from, to))
    }

    /// Closure variant of [`Self::get_or_build`].
    ///
    /// `build` receives the original endpoints and returns a polyline with at
    /// least two finite points. Any other result is treated like a failed
    /// build: the miss is counted and nothing is cached.
    pub fn get_or_insert_with<F>(
        &mut self,
        home: [f32; 2],
        workplace: [f32; 2],
        build: F,
    ) -> Option<&CachedRoute>
    where
        F: FnOnce([f32; 2], [f32; 2]) -> Option<Vec<[f32; 2]>>,
    {
        let Some(key) = route_key(home, workplace) else {
            self.misses += 1;
            return None;
        };
        if self.routes.contains_key(&key) {
            self.hits += 1;
            self.touch(key);
            return self.routes.get(&key);
        }

        self.misses += 1;
        let route = CachedRoute::from_points(build(home, workplace)?)?;
        if self.routes.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.routes.remove(&oldest);
            }
        }
        self.order.push_back(key);
        self.routes.insert(key, route);
        self.routes.get(&key)
    }

    /// Number of cached routes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether the cache holds no route.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Maximum number of cached routes.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Cache-hit count since creation.
    #[must_use]
    pub const fn hits(&self) -> u64 {
        self.hits
    }

    /// Cache-miss count since creation, including invalid and disconnected
    /// pairs.
    #[must_use]
    pub const fn misses(&self) -> u64 {
        self.misses
    }

    /// Drop every cached route. Hit and miss counters stay cumulative for the
    /// lifetime of the cache.
    pub fn clear(&mut self) {
        self.routes.clear();
        self.order.clear();
    }

    /// FNV-1a checksum over the cached routes in key order.
    ///
    /// The checksum depends on content, not insertion order or lookup
    /// counters, so a rebuilt cache with the same routes hashes identically.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        fnv1a_feed(&mut hash, self.routes.len() as u64);
        for (key, route) in &self.routes {
            for value in key {
                fnv1a_feed(&mut hash, *value as u64);
            }
            fnv1a_feed(&mut hash, route.checksum());
        }
        hash
    }

    /// Refresh recency for a key that is already cached.
    fn touch(&mut self, key: RouteKey) {
        if let Some(position) = self.order.iter().position(|candidate| *candidate == key) {
            self.order.remove(position);
        }
        self.order.push_back(key);
    }
}

fn route_key(from: [f32; 2], to: [f32; 2]) -> Option<RouteKey> {
    if !from.iter().chain(to.iter()).all(|value| value.is_finite()) {
        return None;
    }
    Some([
        quantize(from[0]),
        quantize(from[1]),
        quantize(to[0]),
        quantize(to[1]),
    ])
}

/// Round a metre coordinate to the nearest centimetre.
fn quantize(value: f32) -> i64 {
    // f64 keeps the scaling exact for the f32 input; `as i64` saturates.
    (f64::from(value) * 100.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::road::RoadSegment;

    fn road(id: u64, start: [i32; 2], end: [i32; 2]) -> RoadSegment {
        RoadSegment {
            id,
            start,
            end,
            width_m: 8,
        }
    }

    fn l_graph() -> RoadGraph {
        RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])]).expect("build")
    }

    fn straight_graph() -> RoadGraph {
        RoadGraph::build(&[road(1, [0, 0], [20, 0])]).expect("build")
    }

    #[test]
    fn a_cache_hit_reuses_the_built_route() {
        let graph = straight_graph();
        let home = [0.0, 0.0];
        let workplace = [20.0, 0.0];
        let direct = graph.route_points(home, workplace).expect("route");

        let mut cache = RouteCache::new(4);
        let first = cache.get_or_build(&graph, home, workplace).expect("cached");
        assert_eq!(first.points(), direct.as_slice());
        assert!((first.length_m() - 20.0).abs() < 0.001);
        assert!((first.time_s() - 2.5).abs() < 0.001);
        let first_checksum = first.checksum();

        let second = cache.get_or_build(&graph, home, workplace).expect("cached");
        assert_eq!(second.checksum(), first_checksum);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.capacity(), 4);
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert!(cache.get(home, workplace).is_some());
        assert_eq!(
            cache.get(home, workplace).expect("route").checksum(),
            first_checksum
        );
    }

    #[test]
    fn a_full_cache_evicts_the_least_recently_used_route() {
        let graph = l_graph();
        let first = ([0.0, 0.0], [10.0, 0.0]);
        let second = ([10.0, 0.0], [10.0, 10.0]);
        let third = ([0.0, 0.0], [10.0, 10.0]);

        let mut cache = RouteCache::new(2);
        assert!(cache.get_or_build(&graph, first.0, first.1).is_some());
        assert!(cache.get_or_build(&graph, second.0, second.1).is_some());
        // Touch the first entry so the second is now least recently used.
        assert!(cache.get_or_build(&graph, first.0, first.1).is_some());
        assert!(cache.get_or_build(&graph, third.0, third.1).is_some());

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.capacity(), 2);
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 3);
        assert!(cache.get(first.0, first.1).is_some());
        assert!(cache.get(second.0, second.1).is_none());
        assert!(cache.get(third.0, third.1).is_some());
    }

    #[test]
    fn disconnected_and_invalid_routes_are_not_cached() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [50, 50], [60, 50])])
            .expect("build");
        let mut cache = RouteCache::new(2);
        assert!(cache
            .get_or_build(&graph, [0.0, 0.0], [50.0, 50.0])
            .is_none());
        assert!(cache
            .get_or_build(&graph, [f32::NAN, 0.0], [0.0, 0.0])
            .is_none());
        assert!(cache.is_empty());
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
        assert!(cache.get([0.0, 0.0], [50.0, 50.0]).is_none());
    }

    #[test]
    fn cache_checksums_are_deterministic_and_content_sensitive() {
        let graph = l_graph();
        let first = ([0.0, 0.0], [10.0, 0.0]);
        let second = ([10.0, 0.0], [10.0, 10.0]);

        let mut forward = RouteCache::new(4);
        assert!(forward.get_or_build(&graph, first.0, first.1).is_some());
        assert!(forward.get_or_build(&graph, second.0, second.1).is_some());

        let mut reverse = RouteCache::new(4);
        assert!(reverse.get_or_build(&graph, second.0, second.1).is_some());
        assert!(reverse.get_or_build(&graph, first.0, first.1).is_some());

        let mut single = RouteCache::new(4);
        assert!(single.get_or_build(&graph, first.0, first.1).is_some());

        assert_eq!(forward.checksum(), reverse.checksum());
        assert_ne!(forward.checksum(), single.checksum());

        let rebuilt = l_graph();
        let mut again = RouteCache::new(4);
        assert!(again.get_or_build(&rebuilt, first.0, first.1).is_some());
        assert!(again.get_or_build(&rebuilt, second.0, second.1).is_some());
        assert_eq!(forward.checksum(), again.checksum());
    }

    #[test]
    fn capacity_is_bounded_and_eviction_keeps_the_cap() {
        assert_eq!(RouteCache::new(0).capacity(), 1);
        assert_eq!(
            RouteCache::new(usize::MAX).capacity(),
            MAX_ROUTE_CACHE_CAPACITY
        );

        let graph = RoadGraph::build(&[
            road(1, [0, 0], [30, 0]),
            road(2, [30, 0], [30, 30]),
            road(3, [30, 30], [0, 30]),
            road(4, [0, 30], [0, 0]),
        ])
        .expect("build");
        let corners = [[0.0, 0.0], [30.0, 0.0], [30.0, 30.0], [0.0, 30.0]];
        let mut cache = RouteCache::new(3);
        let mut inserted = 0_u64;
        for (index, from) in corners.iter().enumerate() {
            for to in corners.iter().skip(index + 1) {
                assert!(cache.get_or_build(&graph, *from, *to).is_some());
                inserted += 1;
                assert!(cache.len() <= cache.capacity());
            }
        }
        assert_eq!(inserted, 6);
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.misses(), 6);
        assert_eq!(cache.hits(), 0);
    }
}
