//! City-wide research pool and data-driven technology tree.
//!
//! Research is a single pool of points, not a per-building tree (v1). The
//! default tree is compiled in for the vertical slice, but [`TechTree`] is a
//! serde type with a validated JSON form so mods and resource packs can supply
//! their own nodes later. Ages gate nodes; population supplies a small research
//! rate until civic buildings add explicit output in Phase 4.4.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::progression::Age;

/// Current research save format.
pub const RESEARCH_FORMAT_VERSION: u16 = 1;

/// Current technology-tree format.
pub const TECH_TREE_FORMAT_VERSION: u16 = 1;

/// Research points a new city starts with.
pub const STARTING_RESEARCH: f64 = 50.0;

/// Research generated per resident per game day.
///
/// This is the v0 administration rate. Civic buildings will add explicit
/// output in Phase 4.4; keeping the base rate small means a town still makes
/// slow progress without a school.
pub const RESEARCH_PER_RESIDENT_PER_DAY: f64 = 0.05;

/// Maximum research points held in one city.
pub const MAX_RESEARCH_POINTS: f64 = 1.0e12;

/// Maximum research generated per game day by one tick.
pub const MAX_RESEARCH_RATE_PER_DAY: f64 = 1.0e6;

/// Maximum simulated step accepted by [`ResearchStore::tick`].
pub const MAX_RESEARCH_TICK_SECONDS: f64 = 86_400.0;

/// Maximum nodes in one technology tree.
pub const MAX_TECH_NODES: usize = 500;

/// Maximum cost of one technology node.
pub const MAX_TECH_COST: f64 = 1.0e9;

/// Seconds in one game day.
const DAY_SECONDS: f64 = 86_400.0;

/// One technology node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TechNode {
    /// Stable namespaced id, for example `construction.masonry`.
    pub id: String,
    /// Player-facing name.
    pub name: String,
    /// Minimum age required to research the node.
    pub age: Age,
    /// Research points required.
    pub cost: f64,
    /// Node ids that must be completed first.
    #[serde(default)]
    pub prerequisites: Vec<String>,
    /// Content ids unlocked by the node, for example `structure:steel_frame`.
    #[serde(default)]
    pub unlocks: Vec<String>,
}

/// A validated, data-driven technology tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TechTree {
    /// Tree format version.
    pub format_version: u16,
    /// Nodes keyed by id.
    pub nodes: BTreeMap<String, TechNode>,
}

impl TechTree {
    /// Build a tree from nodes, replacing duplicate ids deterministically.
    #[must_use]
    pub fn new(nodes: impl IntoIterator<Item = TechNode>) -> Self {
        Self {
            format_version: TECH_TREE_FORMAT_VERSION,
            nodes: nodes
                .into_iter()
                .map(|node| (node.id.clone(), node))
                .collect(),
        }
    }

    /// The compiled-in vertical-slice tree.
    #[must_use]
    pub fn default_tree() -> Self {
        Self::new([
            node(
                "construction.timber_frame",
                "Timber framing",
                Age::Wood,
                20.0,
                &[],
                &["structure:timber_frame"],
            ),
            node(
                "industry.farming",
                "Crop farming",
                Age::Wood,
                25.0,
                &[],
                &["building:farm"],
            ),
            node(
                "utilities.well",
                "Community wells",
                Age::Wood,
                30.0,
                &[],
                &["utility:water"],
            ),
            node(
                "construction.masonry",
                "Masonry construction",
                Age::Masonry,
                100.0,
                &["construction.timber_frame"],
                &[
                    "structure:load_bearing_stone",
                    "structure:half_timber",
                    "structure:load_bearing_brick",
                ],
            ),
            node(
                "utilities.aqueduct",
                "Aqueducts",
                Age::Masonry,
                90.0,
                &["utilities.well"],
                &["utility:water:large"],
            ),
            node(
                "services.school",
                "Schools",
                Age::Masonry,
                120.0,
                &["construction.masonry"],
                &["service:school"],
            ),
            node(
                "industry.factory",
                "Factories",
                Age::Industrial,
                500.0,
                &["construction.masonry"],
                &["building:factory"],
            ),
            node(
                "construction.concrete",
                "Reinforced concrete",
                Age::Industrial,
                600.0,
                &["construction.masonry"],
                &["structure:reinforced_concrete"],
            ),
            node(
                "utilities.power",
                "Power grid",
                Age::Industrial,
                700.0,
                &["utilities.aqueduct"],
                &["utility:power"],
            ),
            node(
                "services.hospital",
                "Hospitals",
                Age::Modern,
                3_000.0,
                &["services.school"],
                &["service:hospital"],
            ),
            node(
                "construction.steel",
                "Steel frames",
                Age::Modern,
                2_500.0,
                &["construction.concrete"],
                &["structure:steel_frame"],
            ),
            node(
                "utilities.solar",
                "Solar farms",
                Age::Information,
                6_000.0,
                &["utilities.power"],
                &["utility:solar"],
            ),
            node(
                "services.university",
                "Universities",
                Age::Information,
                8_000.0,
                &["services.school"],
                &["service:university"],
            ),
            node(
                "construction.composite",
                "Composite cores",
                Age::Information,
                9_000.0,
                &["construction.steel"],
                &["structure:composite_core"],
            ),
        ])
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the tree has no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Look up one node.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&TechNode> {
        self.nodes.get(id)
    }

    /// Nodes sorted by age, then cost, then id.
    #[must_use]
    pub fn ordered(&self) -> Vec<&TechNode> {
        let mut nodes: Vec<&TechNode> = self.nodes.values().collect();
        nodes.sort_by(|left, right| {
            left.age
                .cmp(&right.age)
                .then(left.cost.total_cmp(&right.cost))
                .then(left.id.cmp(&right.id))
        });
        nodes
    }

    /// Validate ids, ages, costs, prerequisites, and acyclicity.
    pub fn validate(&self) -> Result<(), ResearchError> {
        if self.format_version != TECH_TREE_FORMAT_VERSION {
            return Err(ResearchError::UnsupportedVersion(self.format_version));
        }
        if self.nodes.is_empty() {
            return Err(ResearchError::InvalidTree("tree has no nodes"));
        }
        if self.nodes.len() > MAX_TECH_NODES {
            return Err(ResearchError::InvalidTree("tree has too many nodes"));
        }
        for (key, node) in &self.nodes {
            if key != &node.id || node.id.is_empty() || node.id.len() > 128 {
                return Err(ResearchError::InvalidTree("node id is invalid"));
            }
            if node.name.is_empty() || node.name.len() > 128 {
                return Err(ResearchError::InvalidTree("node name is invalid"));
            }
            if !node.cost.is_finite() || node.cost <= 0.0 || node.cost > MAX_TECH_COST {
                return Err(ResearchError::InvalidTree("node cost is out of range"));
            }
            if node.prerequisites.len() > MAX_TECH_NODES {
                return Err(ResearchError::InvalidTree(
                    "node has too many prerequisites",
                ));
            }
            for prerequisite in &node.prerequisites {
                if prerequisite == &node.id {
                    return Err(ResearchError::InvalidTree("node depends on itself"));
                }
                if !self.nodes.contains_key(prerequisite) {
                    return Err(ResearchError::InvalidTree(
                        "node has an unknown prerequisite",
                    ));
                }
            }
            if node.unlocks.len() > MAX_TECH_NODES
                || node
                    .unlocks
                    .iter()
                    .any(|unlock| unlock.is_empty() || unlock.len() > 128)
            {
                return Err(ResearchError::InvalidTree("node unlock is invalid"));
            }
        }
        let mut state: BTreeMap<&str, u8> = BTreeMap::new();
        for id in self.nodes.keys() {
            self.visit(id, &mut state)?;
        }
        Ok(())
    }

    fn visit<'a>(
        &'a self,
        id: &'a str,
        state: &mut BTreeMap<&'a str, u8>,
    ) -> Result<(), ResearchError> {
        match state.get(id) {
            Some(1) => return Err(ResearchError::InvalidTree("tree contains a cycle")),
            Some(2) => return Ok(()),
            _ => {}
        }
        state.insert(id, 1);
        if let Some(node) = self.nodes.get(id) {
            for prerequisite in &node.prerequisites {
                self.visit(prerequisite, state)?;
            }
        }
        state.insert(id, 2);
        Ok(())
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, ResearchError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate a technology tree.
    pub fn from_json(json: &str) -> Result<Self, ResearchError> {
        let tree: Self = serde_json::from_str(json)?;
        tree.validate()?;
        Ok(tree)
    }
}

/// One successful unlock.
#[derive(Debug, Clone, PartialEq)]
pub struct ResearchUnlock {
    /// The completed node.
    pub node: TechNode,
    /// Points spent.
    pub spent: f64,
    /// Points left after spending.
    pub points: f64,
}

/// Result of one research tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResearchTick {
    /// Points generated this tick.
    pub gained: f64,
    /// Points after the tick.
    pub points: f64,
    /// Lifetime points generated.
    pub lifetime_points: f64,
}

/// Persistent city research pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed.
    pub seed: u64,
    /// Points available to spend.
    pub points: f64,
    /// Points generated since the city began.
    pub lifetime_points: f64,
    /// Completed node ids.
    #[serde(default)]
    pub completed: BTreeSet<String>,
}

impl ResearchStore {
    /// Create a new pool with the starting points.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: RESEARCH_FORMAT_VERSION,
            seed,
            points: STARTING_RESEARCH,
            lifetime_points: STARTING_RESEARCH,
            completed: BTreeSet::new(),
        }
    }

    /// Create an empty pool without the starting grant.
    #[must_use]
    pub fn empty(seed: u64) -> Self {
        Self {
            format_version: RESEARCH_FORMAT_VERSION,
            seed,
            points: 0.0,
            lifetime_points: 0.0,
            completed: BTreeSet::new(),
        }
    }

    /// Whether a node is complete.
    #[must_use]
    pub fn is_completed(&self, id: &str) -> bool {
        self.completed.contains(id)
    }

    /// Number of completed nodes.
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.completed.len()
    }

    /// Generate research for `dt_s` seconds at a daily rate.
    pub fn tick(&mut self, dt_s: f64, rate_per_day: f64) -> ResearchTick {
        let dt = if dt_s.is_finite() {
            dt_s.clamp(0.0, MAX_RESEARCH_TICK_SECONDS)
        } else {
            0.0
        };
        let rate = if rate_per_day.is_finite() {
            rate_per_day.clamp(0.0, MAX_RESEARCH_RATE_PER_DAY)
        } else {
            0.0
        };
        let gained = rate * dt / DAY_SECONDS;
        let before = self.points;
        self.points = (self.points + gained).clamp(0.0, MAX_RESEARCH_POINTS);
        let real_gain = self.points - before;
        self.lifetime_points = (self.lifetime_points + real_gain).clamp(0.0, MAX_RESEARCH_POINTS);
        ResearchTick {
            gained: real_gain,
            points: self.points,
            lifetime_points: self.lifetime_points,
        }
    }

    /// Unlock a node if the age, prerequisites, and points allow it.
    pub fn unlock(
        &mut self,
        id: &str,
        tree: &TechTree,
        age: Age,
    ) -> Result<ResearchUnlock, ResearchError> {
        let node = tree
            .node(id)
            .ok_or_else(|| ResearchError::UnknownNode(id.to_string()))?;
        if self.completed.contains(id) {
            return Err(ResearchError::AlreadyUnlocked(id.to_string()));
        }
        if age < node.age {
            return Err(ResearchError::AgeLocked {
                node: id.to_string(),
                required: node.age,
                current: age,
            });
        }
        if let Some(missing) = node
            .prerequisites
            .iter()
            .find(|prerequisite| !self.completed.contains(*prerequisite))
        {
            return Err(ResearchError::PrerequisiteMissing(missing.clone()));
        }
        if self.points + 0.000_001 < node.cost {
            return Err(ResearchError::InsufficientPoints {
                required: node.cost,
                available: self.points,
            });
        }
        self.points = (self.points - node.cost).max(0.0);
        self.completed.insert(id.to_string());
        Ok(ResearchUnlock {
            node: node.clone(),
            spent: node.cost,
            points: self.points,
        })
    }

    /// Whether a node is unlocked and its prerequisites are complete.
    #[must_use]
    pub fn can_unlock(&self, node: &TechNode, age: Age) -> bool {
        age >= node.age
            && !self.completed.contains(&node.id)
            && node
                .prerequisites
                .iter()
                .all(|prerequisite| self.completed.contains(prerequisite))
            && self.points + 0.000_001 >= node.cost
    }

    /// Validate ranges and the completed set against a tree.
    pub fn validate(&self, tree: &TechTree) -> Result<(), ResearchError> {
        if self.format_version != RESEARCH_FORMAT_VERSION {
            return Err(ResearchError::UnsupportedVersion(self.format_version));
        }
        if self.seed > i64::MAX as u64 {
            return Err(ResearchError::InvalidState("seed is above i64::MAX"));
        }
        if !self.points.is_finite() || !(0.0..=MAX_RESEARCH_POINTS).contains(&self.points) {
            return Err(ResearchError::InvalidState("points are out of range"));
        }
        if !self.lifetime_points.is_finite()
            || !(self.points..=MAX_RESEARCH_POINTS).contains(&self.lifetime_points)
        {
            return Err(ResearchError::InvalidState(
                "lifetime points are out of range",
            ));
        }
        if self.completed.len() > tree.len() {
            return Err(ResearchError::InvalidState("too many completed nodes"));
        }
        for id in &self.completed {
            if !tree.nodes.contains_key(id) {
                return Err(ResearchError::InvalidState(
                    "completed node is not in the tree",
                ));
            }
        }
        Ok(())
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, ResearchError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate a research save against a tree.
    pub fn from_json(json: &str, tree: &TechTree) -> Result<Self, ResearchError> {
        let store: Self = serde_json::from_str(json)?;
        store.validate(tree)?;
        Ok(store)
    }

    /// FNV-1a checksum over the compact JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let json = serde_json::to_vec(self).expect("research always serializes");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in json {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

fn node(
    id: &str,
    name: &str,
    age: Age,
    cost: f64,
    prerequisites: &[&str],
    unlocks: &[&str],
) -> TechNode {
    TechNode {
        id: id.to_string(),
        name: name.to_string(),
        age,
        cost,
        prerequisites: prerequisites
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        unlocks: unlocks.iter().map(|value| (*value).to_string()).collect(),
    }
}

/// Errors returned by the research pool or tree.
#[derive(Debug)]
pub enum ResearchError {
    /// JSON parse/serialize error.
    Json(serde_json::Error),
    /// Unsupported save or tree version.
    UnsupportedVersion(u16),
    /// A technology-tree invariant failed.
    InvalidTree(&'static str),
    /// A research-pool invariant failed.
    InvalidState(&'static str),
    /// The node id is not in the tree.
    UnknownNode(String),
    /// The current age is below the node's age.
    AgeLocked {
        /// Node id.
        node: String,
        /// Required age.
        required: Age,
        /// Current age.
        current: Age,
    },
    /// A prerequisite is not complete.
    PrerequisiteMissing(String),
    /// The node is already complete.
    AlreadyUnlocked(String),
    /// The pool does not have enough points.
    InsufficientPoints {
        /// Node cost.
        required: f64,
        /// Available points.
        available: f64,
    },
}

impl ResearchError {
    /// Stable error code for the bridge and UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Json(_) => "invalid_json",
            Self::UnsupportedVersion(_) => "unsupported_research_version",
            Self::InvalidTree(_) => "invalid_tech_tree",
            Self::InvalidState(_) => "invalid_research",
            Self::UnknownNode(_) => "unknown_tech",
            Self::AgeLocked { .. } => "age_locked",
            Self::PrerequisiteMissing(_) => "prerequisite_missing",
            Self::AlreadyUnlocked(_) => "already_unlocked",
            Self::InsufficientPoints { .. } => "insufficient_research",
        }
    }
}

impl fmt::Display for ResearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "research JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported research version {version}")
            }
            Self::InvalidTree(message) => write!(formatter, "invalid tech tree: {message}"),
            Self::InvalidState(message) => write!(formatter, "invalid research: {message}"),
            Self::UnknownNode(node) => write!(formatter, "unknown technology {node}"),
            Self::AgeLocked {
                node,
                required,
                current,
            } => write!(
                formatter,
                "{node} requires the {} age; the city is in the {} age",
                required.name(),
                current.name()
            ),
            Self::PrerequisiteMissing(node) => {
                write!(formatter, "research prerequisite {node} is not complete")
            }
            Self::AlreadyUnlocked(node) => write!(formatter, "{node} is already researched"),
            Self::InsufficientPoints {
                required,
                available,
            } => write!(
                formatter,
                "research needs {required:.1} points but only {available:.1} are available"
            ),
        }
    }
}

impl std::error::Error for ResearchError {}

impl From<serde_json::Error> for ResearchError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> TechTree {
        TechTree::default_tree()
    }

    #[test]
    fn default_tree_is_valid_and_has_all_ages() {
        let tree = tree();
        tree.validate().expect("valid");
        assert_eq!(tree.len(), 14);
        for age in Age::ALL {
            assert!(tree.nodes.values().any(|node| node.age == age));
        }
        assert_eq!(tree.ordered().first().expect("first").age, Age::Wood);
    }

    #[test]
    fn a_new_city_starts_with_points() {
        let store = ResearchStore::new(2026);
        assert_eq!(store.points, STARTING_RESEARCH);
        assert_eq!(store.lifetime_points, STARTING_RESEARCH);
        assert_eq!(store.completed_count(), 0);
    }

    #[test]
    fn tick_accumulates_points_at_the_daily_rate() {
        let mut store = ResearchStore::new(2026);
        let tick = store.tick(DAY_SECONDS, 10.0);
        assert!((tick.gained - 10.0).abs() < 0.000_001);
        assert!((store.points - 60.0).abs() < 0.000_001);
        let half = store.tick(DAY_SECONDS / 2.0, 10.0);
        assert!((half.gained - 5.0).abs() < 0.000_001);
    }

    #[test]
    fn unlock_spends_points_and_completes_the_node() {
        let tree = tree();
        let mut store = ResearchStore::new(2026);
        let unlock = store
            .unlock("construction.timber_frame", &tree, Age::Wood)
            .expect("unlock");
        assert_eq!(unlock.spent, 20.0);
        assert!((store.points - 30.0).abs() < 0.000_001);
        assert!(store.is_completed("construction.timber_frame"));
        assert_eq!(store.completed_count(), 1);
    }

    #[test]
    fn duplicate_and_unknown_nodes_are_rejected() {
        let tree = tree();
        let mut store = ResearchStore::new(2026);
        store
            .unlock("utilities.well", &tree, Age::Wood)
            .expect("unlock");
        assert_eq!(
            store
                .unlock("utilities.well", &tree, Age::Wood)
                .expect_err("duplicate")
                .code(),
            "already_unlocked"
        );
        assert_eq!(
            store
                .unlock("nope", &tree, Age::Wood)
                .expect_err("unknown")
                .code(),
            "unknown_tech"
        );
    }

    #[test]
    fn age_and_prerequisites_gate_unlocks() {
        let tree = tree();
        let mut store = ResearchStore::new(2026);
        assert_eq!(
            store
                .unlock("construction.masonry", &tree, Age::Wood)
                .expect_err("age")
                .code(),
            "age_locked"
        );
        assert_eq!(
            store
                .unlock("construction.masonry", &tree, Age::Masonry)
                .expect_err("prerequisite")
                .code(),
            "prerequisite_missing"
        );
    }

    #[test]
    fn insufficient_points_are_rejected() {
        let tree = tree();
        let mut store = ResearchStore::empty(2026);
        assert_eq!(
            store
                .unlock("construction.timber_frame", &tree, Age::Wood)
                .expect_err("points")
                .code(),
            "insufficient_research"
        );
        assert!(store.completed.is_empty());
    }

    #[test]
    fn can_unlock_matches_the_rules() {
        let tree = tree();
        let store = ResearchStore::new(2026);
        let timber = tree.node("construction.timber_frame").expect("timber");
        let masonry = tree.node("construction.masonry").expect("masonry");
        assert!(store.can_unlock(timber, Age::Wood));
        assert!(!store.can_unlock(masonry, Age::Wood));
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let tree = tree();
        let mut store = ResearchStore::new(2026);
        store
            .unlock("industry.farming", &tree, Age::Wood)
            .expect("unlock");
        let json = store.to_json_pretty().expect("json");
        let decoded = ResearchStore::from_json(&json, &tree).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
        let tree_json = tree.to_json_pretty().expect("tree json");
        assert_eq!(TechTree::from_json(&tree_json).expect("tree"), tree);
    }

    #[test]
    fn invalid_state_is_rejected() {
        let tree = tree();
        let mut store = ResearchStore::new(2026);
        store.points = -1.0;
        assert!(store.validate(&tree).is_err());
        let mut store = ResearchStore::new(2026);
        store.completed.insert("missing.node".to_string());
        assert!(store.validate(&tree).is_err());
    }

    #[test]
    fn invalid_trees_are_rejected() {
        let mut tree = TechTree::new([node("a", "A", Age::Wood, 10.0, &["b"], &[])]);
        assert!(tree.validate().is_err());
        tree.nodes.insert(
            "b".to_string(),
            node("b", "B", Age::Wood, 10.0, &["a"], &[]),
        );
        assert_eq!(
            tree.validate().expect_err("cycle").code(),
            "invalid_tech_tree"
        );
    }

    #[test]
    fn zero_or_infinite_costs_are_rejected() {
        for cost in [0.0, -1.0, f64::INFINITY, MAX_TECH_COST * 2.0] {
            let tree = TechTree::new([node("a", "A", Age::Wood, cost, &[], &[])]);
            assert!(tree.validate().is_err());
        }
    }
}
