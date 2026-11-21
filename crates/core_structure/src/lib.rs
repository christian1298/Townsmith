//! Material-based structural systems, limits, and composite construction.
//!
//! Townsmith's first structural model is rule-based rather than a stress
//! simulation. A [`StructureSpec`] declares a system, story count, height,
//! span, and foundation depth; [`StructureSpec::validate`] checks it against
//! the readable limits in [`StructuralSystem`]. Composite construction is
//! supported through [`StructureSpec::ground_system`]: a masonry ground floor
//! raises a timber frame by one story, and a concrete podium can carry a
//! taller steel frame whose own limits then govern.
//!
//! A zero `max_span_m` means "unspecified": the blueprint does not declare a
//! beam span, so the span rule is skipped. [`StructureSpec::derive_from_bounds`]
//! leaves the span at zero because a building footprint is not a beam span.
//!
//! # Example
//!
//! ```
//! use core_structure::{StructuralSystem, StructureSpec};
//!
//! let spec = StructureSpec::new(StructuralSystem::TimberFrame, 3, 10.0, 7.0);
//! let summary = spec.summary().expect("timber frame limits");
//! assert_eq!(summary.effective_max_stories, 3);
//!
//! let composite = spec.with_ground_system(StructuralSystem::LoadBearingStone);
//! assert_eq!(
//!     composite.summary().expect("composite").effective_max_stories,
//!     4
//! );
//! ```

#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};

/// Height used to derive a story count from a volume height.
pub const STORY_HEIGHT_M: f32 = 3.2;

/// Structural metadata format version.
///
/// This is a crate constant only; the format is versioned by the enclosing
/// blueprint package, so [`StructureSpec`] carries no version field.
pub const STRUCTURE_FORMAT_VERSION: u16 = 1;

/// Allowed float error when comparing declared metres against a limit.
const LIMIT_TOLERANCE_M: f32 = 0.001;

fn is_zero_f32(value: &f32) -> bool {
    *value == 0.0
}

/// A construction system with readable story, height, and span limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralSystem {
    /// Poles and canvas; temporary, one level.
    Tent,
    /// Cheap timber posts and boards; one story.
    TimberShed,
    /// Timber frame or post-and-beam house.
    TimberFrame,
    /// Load-bearing stone walls with small openings.
    LoadBearingStone,
    /// Stone ground floor with a timber upper structure.
    HalfTimber,
    /// Load-bearing brick masonry.
    LoadBearingBrick,
    /// Reinforced-concrete frame.
    ReinforcedConcrete,
    /// Steel frame.
    SteelFrame,
    /// Steel frame with a reinforced-concrete core.
    CompositeCore,
}

impl StructuralSystem {
    /// Every system, in progression order.
    pub const ALL: [Self; 9] = [
        Self::Tent,
        Self::TimberShed,
        Self::TimberFrame,
        Self::LoadBearingStone,
        Self::HalfTimber,
        Self::LoadBearingBrick,
        Self::ReinforcedConcrete,
        Self::SteelFrame,
        Self::CompositeCore,
    ];

    /// Stable serde id, for example `timber_frame`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Tent => "tent",
            Self::TimberShed => "timber_shed",
            Self::TimberFrame => "timber_frame",
            Self::LoadBearingStone => "load_bearing_stone",
            Self::HalfTimber => "half_timber",
            Self::LoadBearingBrick => "load_bearing_brick",
            Self::ReinforcedConcrete => "reinforced_concrete",
            Self::SteelFrame => "steel_frame",
            Self::CompositeCore => "composite_core",
        }
    }

    /// Human-readable name used in player-facing validation messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tent => "tent",
            Self::TimberShed => "timber shed",
            Self::TimberFrame => "timber frame",
            Self::LoadBearingStone => "load-bearing stone",
            Self::HalfTimber => "half-timber",
            Self::LoadBearingBrick => "load-bearing brick",
            Self::ReinforcedConcrete => "reinforced concrete",
            Self::SteelFrame => "steel frame",
            Self::CompositeCore => "composite core",
        }
    }

    /// Parse a system id or alias.
    ///
    /// Matching trims, lowercases, and treats spaces and hyphens as
    /// underscores, so `Half-Timber` and ` half timber ` both parse.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let normalized: String = value
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|ch| {
                if ch.is_whitespace() || ch == '-' {
                    '_'
                } else {
                    ch
                }
            })
            .collect();
        match normalized.as_str() {
            "tent" => Some(Self::Tent),
            "shed" | "timber_shed" => Some(Self::TimberShed),
            "timber" | "wood" | "wood_frame" | "timber_frame" => Some(Self::TimberFrame),
            "stone" | "masonry" | "load_bearing_stone" => Some(Self::LoadBearingStone),
            "half_timber" => Some(Self::HalfTimber),
            "brick" | "load_bearing_brick" => Some(Self::LoadBearingBrick),
            "concrete" | "rc" | "reinforced_concrete" => Some(Self::ReinforcedConcrete),
            "steel" | "steel_frame" => Some(Self::SteelFrame),
            "composite" | "composite_core" => Some(Self::CompositeCore),
            _ => None,
        }
    }

    /// Maximum number of above-ground stories.
    #[must_use]
    pub const fn max_stories(self) -> u16 {
        match self {
            Self::Tent => 1,
            Self::TimberShed => 1,
            Self::TimberFrame => 3,
            Self::LoadBearingStone => 3,
            Self::HalfTimber => 4,
            Self::LoadBearingBrick => 6,
            Self::ReinforcedConcrete => 30,
            Self::SteelFrame => 80,
            Self::CompositeCore => 120,
        }
    }

    /// Maximum above-ground height in metres.
    #[must_use]
    pub const fn max_height_m(self) -> f32 {
        match self {
            Self::Tent => 3.0,
            Self::TimberShed => 4.0,
            Self::TimberFrame => 11.0,
            Self::LoadBearingStone => 11.0,
            Self::HalfTimber => 14.5,
            Self::LoadBearingBrick => 20.0,
            Self::ReinforcedConcrete => 100.0,
            Self::SteelFrame => 260.0,
            Self::CompositeCore => 400.0,
        }
    }

    /// Maximum beam or slab span in metres.
    #[must_use]
    pub const fn max_span_m(self) -> f32 {
        match self {
            Self::Tent => 3.0,
            Self::TimberShed => 4.0,
            Self::TimberFrame => 8.0,
            Self::LoadBearingStone => 5.0,
            Self::HalfTimber => 6.0,
            Self::LoadBearingBrick => 7.0,
            Self::ReinforcedConcrete => 10.0,
            Self::SteelFrame => 16.0,
            Self::CompositeCore => 16.0,
        }
    }

    /// Whether this is the timber-frame system eligible for the masonry
    /// ground-floor bonus.
    #[must_use]
    pub const fn is_timber(self) -> bool {
        matches!(self, Self::TimberFrame)
    }

    /// Age id that first unlocks this system.
    ///
    /// This is a stable string rather than an `Age` value so `core_structure`
    /// stays independent of `core_city`; the bridge compares it with the
    /// city's age rank.
    #[must_use]
    pub const fn min_age(self) -> &'static str {
        match self {
            Self::Tent | Self::TimberShed | Self::TimberFrame => "wood",
            Self::LoadBearingStone | Self::HalfTimber | Self::LoadBearingBrick => "masonry",
            Self::ReinforcedConcrete | Self::SteelFrame => "industrial",
            Self::CompositeCore => "information",
        }
    }

    /// Whether this system is masonry that can support a timber upper floor.
    #[must_use]
    pub const fn is_masonry(self) -> bool {
        matches!(
            self,
            Self::LoadBearingStone | Self::HalfTimber | Self::LoadBearingBrick
        )
    }

    /// Whether this system is a high-capacity podium that can carry a taller
    /// frame above it.
    const fn is_podium(self) -> bool {
        matches!(self, Self::ReinforcedConcrete | Self::CompositeCore)
    }
}

/// Foundation depth required for a story count.
///
/// `stories <= 2`: 0 m; `<= 4`: 0.5 m; `<= 10`: 1 m; `<= 30`: 2 m; taller:
/// 4 m.
#[must_use]
pub const fn recommended_foundation_m(stories: u16) -> f32 {
    match stories {
        0..=2 => 0.0,
        3..=4 => 0.5,
        5..=10 => 1.0,
        11..=30 => 2.0,
        _ => 4.0,
    }
}

/// Declared structural system and limits.
///
/// `max_span_m == 0.0` means the blueprint does not declare a span; the span
/// rule is then skipped. Older metadata without the optional span and
/// foundation fields deserializes with zero values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StructureSpec {
    /// Above-ground structural system.
    pub system: StructuralSystem,
    /// Ground-floor system for composite construction, when different.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_system: Option<StructuralSystem>,
    /// Declared number of above-ground stories.
    pub stories: u16,
    /// Declared above-ground height in metres.
    pub height_m: f32,
    /// Declared beam or slab span in metres; `0` means unspecified.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub max_span_m: f32,
    /// Declared foundation depth in metres.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub foundation_depth_m: f32,
}

impl StructureSpec {
    /// Construct a spec with the recommended foundation for `stories`.
    #[must_use]
    pub fn new(system: StructuralSystem, stories: u16, height_m: f32, max_span_m: f32) -> Self {
        Self {
            system,
            ground_system: None,
            stories,
            height_m,
            max_span_m,
            foundation_depth_m: recommended_foundation_m(stories),
        }
    }

    /// Set a different ground-floor system.
    #[must_use]
    pub fn with_ground_system(mut self, ground: StructuralSystem) -> Self {
        self.ground_system = Some(ground);
        self
    }

    /// Override the recommended foundation depth.
    #[must_use]
    pub fn with_foundation_depth(mut self, depth_m: f32) -> Self {
        self.foundation_depth_m = depth_m;
        self
    }

    /// Derive a spec from a volume size in metres `[width, height, depth]`.
    ///
    /// Stories are `ceil(height / 3.2)`, at least one. The span is left at
    /// `0.0` (unspecified): a building footprint is not a beam span, and
    /// inferring one would reject wide buildings with internal walls. The
    /// span rule only applies when a blueprint declares a span explicitly.
    ///
    /// # Errors
    ///
    /// Returns [`StructureError::InvalidBounds`] unless every axis is finite
    /// and positive.
    pub fn derive_from_bounds(
        system: StructuralSystem,
        bounds_m: [f32; 3],
    ) -> Result<Self, StructureError> {
        if bounds_m
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(StructureError::InvalidBounds { bounds: bounds_m });
        }
        let stories = (bounds_m[1] / STORY_HEIGHT_M).ceil().max(1.0) as u16;
        Ok(Self::new(system, stories, bounds_m[1], 0.0))
    }

    /// Validate declared fields without applying material limits.
    ///
    /// Saves use this so an invalid design can be kept as a draft; placement
    /// and load validation use [`Self::validate`] or
    /// [`Self::validate_against_bounds`] instead.
    pub fn validate_declared(&self) -> Result<(), StructureError> {
        if self.stories == 0 {
            return Err(StructureError::InvalidStories {
                stories: self.stories,
            });
        }
        if !self.height_m.is_finite() || self.height_m <= 0.0 {
            return Err(StructureError::InvalidHeight {
                height_m: self.height_m,
            });
        }
        if !self.max_span_m.is_finite() || self.max_span_m < 0.0 {
            return Err(StructureError::InvalidSpan {
                span_m: self.max_span_m,
            });
        }
        if !self.foundation_depth_m.is_finite() || self.foundation_depth_m < 0.0 {
            return Err(StructureError::InvalidFoundation {
                depth_m: self.foundation_depth_m,
            });
        }
        Ok(())
    }

    /// Validate declared fields and require the height to match the volume.
    pub fn validate_declared_against_bounds(
        &self,
        bounds_m: [f32; 3],
    ) -> Result<(), StructureError> {
        self.validate_declared()?;
        self.check_bounds(bounds_m)
    }

    /// Validate fields, volume height, and every material limit.
    ///
    /// # Errors
    ///
    /// Returns the first failing rule in this order: invalid fields, weak
    /// ground system, story limit, height limit, span limit, foundation depth,
    /// then a height/bounds mismatch.
    pub fn validate(&self) -> Result<(), StructureError> {
        self.checked_limits().map(|_| ())
    }

    /// Validate every material limit and the volume height.
    pub fn validate_against_bounds(&self, bounds_m: [f32; 3]) -> Result<(), StructureError> {
        self.validate()?;
        self.check_bounds(bounds_m)
    }

    fn check_bounds(&self, bounds_m: [f32; 3]) -> Result<(), StructureError> {
        if (self.height_m - bounds_m[1]).abs() > 0.001 {
            return Err(StructureError::HeightMismatch {
                height_m: self.height_m,
                bounds_height_m: bounds_m[1],
            });
        }
        Ok(())
    }

    /// Build a serializable summary of the declared values and effective
    /// limits.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate`].
    pub fn summary(&self) -> Result<StructureSummary, StructureError> {
        let limits = self.checked_limits()?;
        Ok(StructureSummary {
            system: self.system,
            ground_system: self.ground_system,
            stories: self.stories,
            height_m: self.height_m,
            max_span_m: self.max_span_m,
            foundation_depth_m: self.foundation_depth_m,
            effective_max_stories: limits.max_stories,
            effective_max_height_m: limits.max_height_m,
            effective_max_span_m: limits.max_span_m,
            composite_bonus: limits.composite_bonus,
        })
    }

    fn checked_limits(&self) -> Result<EffectiveLimits, StructureError> {
        self.validate_declared()?;

        let limits = self.effective_limits()?;

        if self.stories > limits.max_stories {
            return Err(StructureError::StoriesExceeded {
                system: self.system,
                stories: self.stories,
                limit: limits.max_stories,
            });
        }
        if self.height_m > limits.max_height_m + LIMIT_TOLERANCE_M {
            return Err(StructureError::HeightExceeded {
                system: self.system,
                height_m: self.height_m,
                limit: limits.max_height_m,
            });
        }
        if self.max_span_m > 0.0 && self.max_span_m > limits.max_span_m + LIMIT_TOLERANCE_M {
            return Err(StructureError::SpanExceeded {
                system: self.system,
                span_m: self.max_span_m,
                limit: limits.max_span_m,
            });
        }
        let required_m = recommended_foundation_m(self.stories);
        if self.foundation_depth_m + LIMIT_TOLERANCE_M < required_m {
            return Err(StructureError::FoundationTooShallow {
                required_m,
                actual_m: self.foundation_depth_m,
            });
        }
        Ok(limits)
    }

    fn effective_limits(&self) -> Result<EffectiveLimits, StructureError> {
        let base = EffectiveLimits {
            max_stories: self.system.max_stories(),
            max_height_m: self.system.max_height_m(),
            max_span_m: self.system.max_span_m(),
            composite_bonus: false,
        };
        let Some(ground) = self.ground_system else {
            return Ok(base);
        };
        if ground == self.system {
            return Ok(base);
        }
        if ground.is_masonry() && self.system.is_timber() {
            return Ok(EffectiveLimits {
                max_stories: base.max_stories.saturating_add(1),
                max_height_m: base.max_height_m + STORY_HEIGHT_M,
                max_span_m: base.max_span_m,
                composite_bonus: true,
            });
        }
        if ground.max_stories() >= self.system.max_stories() {
            return Ok(base);
        }
        // D-024 allows composite construction such as a "concrete podium +
        // steel tower". A high-capacity podium can carry a taller steel or
        // composite frame whose own limits then govern; no story bonus is
        // added, and the podium is not the limiting support.
        if ground.is_podium() {
            return Ok(base);
        }
        Err(StructureError::GroundTooWeak {
            ground,
            system: self.system,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct EffectiveLimits {
    max_stories: u16,
    max_height_m: f32,
    max_span_m: f32,
    composite_bonus: bool,
}

/// Validated summary of a [`StructureSpec`] and its effective limits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StructureSummary {
    /// Above-ground structural system.
    pub system: StructuralSystem,
    /// Ground-floor system for composite construction, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_system: Option<StructuralSystem>,
    /// Declared number of above-ground stories.
    pub stories: u16,
    /// Declared above-ground height in metres.
    pub height_m: f32,
    /// Declared beam or slab span in metres; `0` means unspecified.
    pub max_span_m: f32,
    /// Declared foundation depth in metres.
    pub foundation_depth_m: f32,
    /// Effective story limit after the ground-system rule.
    pub effective_max_stories: u16,
    /// Effective height limit in metres after the ground-system rule.
    pub effective_max_height_m: f32,
    /// Effective span limit in metres.
    pub effective_max_span_m: f32,
    /// Whether the masonry ground floor raised the timber frame's limits.
    pub composite_bonus: bool,
}

impl StructureSummary {
    /// Validate a spec and summarize its effective limits.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`StructureSpec::validate`].
    pub fn from_spec(spec: &StructureSpec) -> Result<Self, StructureError> {
        spec.summary()
    }

    /// Whether the summary is structurally valid.
    ///
    /// Summaries are normally only produced by [`StructureSpec::summary`], but
    /// a deserialized or hand-built summary is revalidated here.
    #[must_use]
    pub fn valid(&self) -> bool {
        StructureSpec {
            system: self.system,
            ground_system: self.ground_system,
            stories: self.stories,
            height_m: self.height_m,
            max_span_m: self.max_span_m,
            foundation_depth_m: self.foundation_depth_m,
        }
        .validate()
        .is_ok()
    }
}

/// A structural rule violation.
#[derive(Debug, Clone, PartialEq)]
pub enum StructureError {
    /// The story count is zero.
    InvalidStories {
        /// Declared story count.
        stories: u16,
    },
    /// Height is not finite and positive.
    InvalidHeight {
        /// Declared height in metres.
        height_m: f32,
    },
    /// Span is not finite and non-negative.
    InvalidSpan {
        /// Declared span in metres.
        span_m: f32,
    },
    /// Foundation depth is not finite and non-negative.
    InvalidFoundation {
        /// Declared foundation depth in metres.
        depth_m: f32,
    },
    /// Bounds are not all finite and positive.
    InvalidBounds {
        /// Declared bounds `[width, height, depth]` in metres.
        bounds: [f32; 3],
    },
    /// Declared structure height does not match the blueprint volume height.
    HeightMismatch {
        /// Declared structure height in metres.
        height_m: f32,
        /// Blueprint bounds height in metres.
        bounds_height_m: f32,
    },
    /// The declared story count exceeds the effective limit.
    StoriesExceeded {
        /// Structural system governing the limit.
        system: StructuralSystem,
        /// Declared story count.
        stories: u16,
        /// Effective story limit.
        limit: u16,
    },
    /// The declared height exceeds the effective limit.
    HeightExceeded {
        /// Structural system governing the limit.
        system: StructuralSystem,
        /// Declared height in metres.
        height_m: f32,
        /// Effective height limit in metres.
        limit: f32,
    },
    /// The declared span exceeds the effective limit.
    SpanExceeded {
        /// Structural system governing the limit.
        system: StructuralSystem,
        /// Declared span in metres.
        span_m: f32,
        /// Effective span limit in metres.
        limit: f32,
    },
    /// The foundation is shallower than the declared story count requires.
    FoundationTooShallow {
        /// Required foundation depth in metres.
        required_m: f32,
        /// Declared foundation depth in metres.
        actual_m: f32,
    },
    /// The ground-floor system cannot carry the declared upper system.
    GroundTooWeak {
        /// Declared ground-floor system.
        ground: StructuralSystem,
        /// Declared upper system.
        system: StructuralSystem,
    },
}

impl StructureError {
    /// Stable machine-readable code for UI and save validation.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidStories { .. }
            | Self::InvalidHeight { .. }
            | Self::InvalidSpan { .. }
            | Self::InvalidFoundation { .. }
            | Self::InvalidBounds { .. }
            | Self::HeightMismatch { .. } => "invalid_structure",
            Self::StoriesExceeded { .. } => "structure_stories",
            Self::HeightExceeded { .. } => "structure_height",
            Self::SpanExceeded { .. } => "structure_span",
            Self::FoundationTooShallow { .. } => "structure_foundation",
            Self::GroundTooWeak { .. } => "structure_ground",
        }
    }
}

impl fmt::Display for StructureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStories { stories } => write!(
                formatter,
                "a structure needs at least 1 story; this blueprint declares {stories}"
            ),
            Self::InvalidHeight { height_m } => write!(
                formatter,
                "structure height must be a positive, finite number; got {height_m} m"
            ),
            Self::InvalidSpan { span_m } => write!(
                formatter,
                "structure span must be a non-negative, finite number; got {span_m} m"
            ),
            Self::InvalidFoundation { depth_m } => write!(
                formatter,
                "foundation depth must be a non-negative, finite number; got {depth_m} m"
            ),
            Self::InvalidBounds { bounds } => write!(
                formatter,
                "structure bounds must be positive and finite; got {} x {} x {} m",
                bounds[0], bounds[1], bounds[2]
            ),
            Self::StoriesExceeded {
                system,
                stories,
                limit,
            } => write!(
                formatter,
                "{} allows {limit} stories; this blueprint has {stories}",
                system.name()
            ),
            Self::HeightExceeded {
                system,
                height_m,
                limit,
            } => write!(
                formatter,
                "{} allows {limit} m of height; this blueprint is {height_m} m",
                system.name()
            ),
            Self::SpanExceeded {
                system,
                span_m,
                limit,
            } => write!(
                formatter,
                "{} allows a {limit} m span; this blueprint spans {span_m} m",
                system.name()
            ),
            Self::FoundationTooShallow {
                required_m,
                actual_m,
            } => write!(
                formatter,
                "a {required_m} m foundation is required; this blueprint declares {actual_m} m"
            ),
            Self::GroundTooWeak { ground, system } => write!(
                formatter,
                "{} is too weak to carry {}; use a stronger ground system",
                ground.name(),
                system.name()
            ),
            Self::HeightMismatch {
                height_m,
                bounds_height_m,
            } => write!(
                formatter,
                "structure height {height_m} m does not match the blueprint height {bounds_height_m} m"
            ),
        }
    }
}

impl std::error::Error for StructureError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn spec(
        system: StructuralSystem,
        stories: u16,
        height_m: f32,
        max_span_m: f32,
    ) -> StructureSpec {
        StructureSpec::new(system, stories, height_m, max_span_m)
    }

    #[test]
    fn all_systems_round_trip_their_ids_and_names() {
        assert_eq!(StructuralSystem::ALL.len(), 9);
        let mut ids = BTreeSet::new();
        for system in StructuralSystem::ALL {
            assert!(ids.insert(system.id()), "duplicate id {}", system.id());
            assert_eq!(StructuralSystem::parse(system.id()), Some(system));
            assert!(!system.name().is_empty());
        }
        assert_eq!(
            StructuralSystem::ALL,
            [
                StructuralSystem::Tent,
                StructuralSystem::TimberShed,
                StructuralSystem::TimberFrame,
                StructuralSystem::LoadBearingStone,
                StructuralSystem::HalfTimber,
                StructuralSystem::LoadBearingBrick,
                StructuralSystem::ReinforcedConcrete,
                StructuralSystem::SteelFrame,
                StructuralSystem::CompositeCore,
            ]
        );
        assert_eq!(StructuralSystem::TimberFrame.name(), "timber frame");
        assert_eq!(
            StructuralSystem::LoadBearingBrick.name(),
            "load-bearing brick"
        );
    }

    #[test]
    fn parse_accepts_aliases_and_normalizes_text() {
        for (text, expected) in [
            ("tent", StructuralSystem::Tent),
            ("Shed", StructuralSystem::TimberShed),
            (" timber_shed ", StructuralSystem::TimberShed),
            ("timber", StructuralSystem::TimberFrame),
            ("wood", StructuralSystem::TimberFrame),
            ("wood_frame", StructuralSystem::TimberFrame),
            ("TIMBER-FRAME", StructuralSystem::TimberFrame),
            ("stone", StructuralSystem::LoadBearingStone),
            ("masonry", StructuralSystem::LoadBearingStone),
            ("load bearing stone", StructuralSystem::LoadBearingStone),
            ("half-timber", StructuralSystem::HalfTimber),
            ("Half Timber", StructuralSystem::HalfTimber),
            ("brick", StructuralSystem::LoadBearingBrick),
            ("load_bearing_brick", StructuralSystem::LoadBearingBrick),
            ("concrete", StructuralSystem::ReinforcedConcrete),
            ("rc", StructuralSystem::ReinforcedConcrete),
            ("reinforced concrete", StructuralSystem::ReinforcedConcrete),
            ("steel", StructuralSystem::SteelFrame),
            ("STEEL FRAME", StructuralSystem::SteelFrame),
            ("composite", StructuralSystem::CompositeCore),
            ("composite-core", StructuralSystem::CompositeCore),
        ] {
            assert_eq!(StructuralSystem::parse(text), Some(expected), "{text}");
        }
        assert_eq!(StructuralSystem::parse("glass"), None);
        assert_eq!(StructuralSystem::parse(""), None);
    }

    #[test]
    fn limits_match_the_material_table() {
        let expected = [
            (StructuralSystem::Tent, 1, 3.0, 3.0, false, false),
            (StructuralSystem::TimberShed, 1, 4.0, 4.0, false, false),
            (StructuralSystem::TimberFrame, 3, 11.0, 8.0, true, false),
            (
                StructuralSystem::LoadBearingStone,
                3,
                11.0,
                5.0,
                false,
                true,
            ),
            (StructuralSystem::HalfTimber, 4, 14.5, 6.0, false, true),
            (
                StructuralSystem::LoadBearingBrick,
                6,
                20.0,
                7.0,
                false,
                true,
            ),
            (
                StructuralSystem::ReinforcedConcrete,
                30,
                100.0,
                10.0,
                false,
                false,
            ),
            (StructuralSystem::SteelFrame, 80, 260.0, 16.0, false, false),
            (
                StructuralSystem::CompositeCore,
                120,
                400.0,
                16.0,
                false,
                false,
            ),
        ];
        for (system, stories, height_m, span_m, timber, masonry) in expected {
            assert_eq!(system.max_stories(), stories, "{}", system.id());
            assert_eq!(system.max_height_m(), height_m, "{}", system.id());
            assert_eq!(system.max_span_m(), span_m, "{}", system.id());
            assert_eq!(system.is_timber(), timber, "{}", system.id());
            assert_eq!(system.is_masonry(), masonry, "{}", system.id());
        }
    }

    #[test]
    fn timber_frame_three_stories_is_valid() {
        let spec = spec(StructuralSystem::TimberFrame, 3, 10.0, 8.0);
        spec.validate().expect("three timber stories fit");
        let summary = spec.summary().expect("summary");
        assert_eq!(summary.system, StructuralSystem::TimberFrame);
        assert_eq!(summary.ground_system, None);
        assert_eq!(summary.effective_max_stories, 3);
        assert_eq!(summary.effective_max_height_m, 11.0);
        assert_eq!(summary.effective_max_span_m, 8.0);
        assert!(!summary.composite_bonus);
        assert!(summary.valid());
    }

    #[test]
    fn timber_frame_four_stories_is_rejected() {
        let error = spec(StructuralSystem::TimberFrame, 4, 12.8, 6.0)
            .validate()
            .expect_err("four timber stories exceed the limit");
        assert_eq!(
            error,
            StructureError::StoriesExceeded {
                system: StructuralSystem::TimberFrame,
                stories: 4,
                limit: 3,
            }
        );
        assert_eq!(error.code(), "structure_stories");
        assert_eq!(
            error.to_string(),
            "timber frame allows 3 stories; this blueprint has 4"
        );
    }

    #[test]
    fn brick_allows_four_and_six_stories_but_not_seven() {
        for stories in [4, 6] {
            spec(StructuralSystem::LoadBearingBrick, stories, 20.0, 7.0)
                .validate()
                .unwrap_or_else(|error| panic!("{stories} brick stories failed: {error}"));
        }
        let error = spec(StructuralSystem::LoadBearingBrick, 7, 20.0, 6.0)
            .validate()
            .expect_err("seven brick stories exceed the limit");
        assert_eq!(error.code(), "structure_stories");
    }

    #[test]
    fn masonry_ground_floor_grants_the_composite_bonus() {
        for ground in [
            StructuralSystem::LoadBearingStone,
            StructuralSystem::HalfTimber,
            StructuralSystem::LoadBearingBrick,
        ] {
            let composite =
                spec(StructuralSystem::TimberFrame, 4, 13.0, 8.0).with_ground_system(ground);
            composite
                .validate()
                .unwrap_or_else(|error| panic!("{ground:?} composite failed: {error}"));
            let summary = composite.summary().expect("summary");
            assert!(summary.composite_bonus);
            assert_eq!(summary.ground_system, Some(ground));
            assert_eq!(summary.effective_max_stories, 4);
            assert!((summary.effective_max_height_m - 14.2).abs() < 0.0001);
            assert_eq!(summary.effective_max_span_m, 8.0);
        }

        let error = spec(StructuralSystem::TimberFrame, 4, 12.8, 6.0)
            .validate()
            .expect_err("pure timber still has three stories");
        assert_eq!(error.code(), "structure_stories");
    }

    #[test]
    fn height_limit_is_enforced() {
        spec(StructuralSystem::TimberFrame, 3, 11.0, 6.0)
            .validate()
            .expect("11 m timber frame fits");
        spec(StructuralSystem::TimberFrame, 3, 11.0005, 6.0)
            .validate()
            .expect("half-millimetre tolerance");

        let error = spec(StructuralSystem::TimberFrame, 3, 11.5, 6.0)
            .validate()
            .expect_err("11.5 m exceeds the timber frame");
        assert_eq!(
            error,
            StructureError::HeightExceeded {
                system: StructuralSystem::TimberFrame,
                height_m: 11.5,
                limit: 11.0,
            }
        );
        assert_eq!(error.code(), "structure_height");
    }

    #[test]
    fn systems_have_age_requirements() {
        assert_eq!(StructuralSystem::Tent.min_age(), "wood");
        assert_eq!(StructuralSystem::TimberFrame.min_age(), "wood");
        assert_eq!(StructuralSystem::LoadBearingBrick.min_age(), "masonry");
        assert_eq!(StructuralSystem::ReinforcedConcrete.min_age(), "industrial");
        assert_eq!(StructuralSystem::SteelFrame.min_age(), "industrial");
        assert_eq!(StructuralSystem::CompositeCore.min_age(), "information");
    }

    #[test]
    fn span_limit_is_enforced_only_when_declared() {
        spec(StructuralSystem::TimberFrame, 3, 10.0, 8.0)
            .validate()
            .expect("8 m is the timber frame span limit");
        spec(StructuralSystem::TimberFrame, 3, 10.0, 7.0)
            .validate()
            .expect("7 m fits");
        spec(StructuralSystem::TimberFrame, 3, 10.0, 0.0)
            .validate()
            .expect("zero span means unspecified");

        let error = spec(StructuralSystem::TimberFrame, 3, 10.0, 8.5)
            .validate()
            .expect_err("8.5 m exceeds the timber frame span");
        assert_eq!(
            error,
            StructureError::SpanExceeded {
                system: StructuralSystem::TimberFrame,
                span_m: 8.5,
                limit: 8.0,
            }
        );
        assert_eq!(error.code(), "structure_span");
    }

    #[test]
    fn foundation_depth_must_match_the_story_count() {
        let shallow = StructureSpec::new(StructuralSystem::LoadBearingBrick, 4, 12.8, 6.0)
            .with_foundation_depth(0.0);
        let error = shallow.validate().expect_err("four stories need 0.5 m");
        assert_eq!(
            error,
            StructureError::FoundationTooShallow {
                required_m: 0.5,
                actual_m: 0.0,
            }
        );
        assert_eq!(error.code(), "structure_foundation");

        shallow
            .with_foundation_depth(0.5)
            .validate()
            .expect("recommended foundation fits");

        let recommended = StructureSpec::new(StructuralSystem::LoadBearingBrick, 4, 12.8, 6.0);
        assert_eq!(recommended.foundation_depth_m, 0.5);
        recommended
            .validate()
            .expect("new() uses the recommendation");
    }

    #[test]
    fn recommended_foundation_increases_with_stories() {
        assert_eq!(recommended_foundation_m(0), 0.0);
        assert_eq!(recommended_foundation_m(2), 0.0);
        assert_eq!(recommended_foundation_m(4), 0.5);
        assert_eq!(recommended_foundation_m(10), 1.0);
        assert_eq!(recommended_foundation_m(30), 2.0);
        assert_eq!(recommended_foundation_m(31), 4.0);
        assert_eq!(recommended_foundation_m(120), 4.0);
    }

    #[test]
    fn derive_from_bounds_counts_stories_and_leaves_span_unspecified() {
        let derived =
            StructureSpec::derive_from_bounds(StructuralSystem::TimberFrame, [6.0, 6.0, 6.0])
                .expect("valid bounds");
        assert_eq!(derived.system, StructuralSystem::TimberFrame);
        assert_eq!(derived.stories, 2);
        assert_eq!(derived.height_m, 6.0);
        // A footprint is not a declared beam span; the span rule must stay off.
        assert_eq!(derived.max_span_m, 0.0);
        assert_eq!(derived.foundation_depth_m, 0.0);
        derived.validate().expect("derived spec is valid");
        let summary = derived.summary().expect("summary");
        assert_eq!(summary.effective_max_stories, 3);

        let wide =
            StructureSpec::derive_from_bounds(StructuralSystem::TimberFrame, [8.0, 6.0, 8.0])
                .expect("valid bounds");
        assert_eq!(wide.max_span_m, 0.0);
        wide.validate()
            .expect("an 8 m footprint does not imply an 8 m beam");
    }

    #[test]
    fn derive_from_bounds_rejects_invalid_bounds() {
        for bounds in [
            [0.0, 6.0, 6.0],
            [-1.0, 6.0, 6.0],
            [6.0, 0.0, 6.0],
            [6.0, 6.0, 0.0],
            [6.0, f32::NAN, 6.0],
            [6.0, 6.0, f32::INFINITY],
        ] {
            let error = StructureSpec::derive_from_bounds(StructuralSystem::Tent, bounds)
                .expect_err("invalid bounds");
            assert!(
                matches!(error, StructureError::InvalidBounds { .. }),
                "{bounds:?} produced {error:?}"
            );
            assert_eq!(error.code(), "invalid_structure");
        }
        assert_eq!(
            StructureSpec::derive_from_bounds(StructuralSystem::Tent, [-1.0, 6.0, 6.0])
                .expect_err("negative width"),
            StructureError::InvalidBounds {
                bounds: [-1.0, 6.0, 6.0],
            }
        );
    }

    #[test]
    fn concrete_podium_carries_a_steel_upper_without_a_bonus() {
        let spec = spec(StructuralSystem::SteelFrame, 60, 200.0, 12.0)
            .with_ground_system(StructuralSystem::ReinforcedConcrete);
        spec.validate().expect("concrete podium carries steel");
        let summary = spec.summary().expect("summary");
        assert_eq!(
            summary.ground_system,
            Some(StructuralSystem::ReinforcedConcrete)
        );
        assert!(!summary.composite_bonus);
        assert_eq!(summary.effective_max_stories, 80);
        assert_eq!(summary.effective_max_height_m, 260.0);
    }

    #[test]
    fn a_timber_ground_floor_cannot_carry_concrete() {
        let error = spec(StructuralSystem::ReinforcedConcrete, 4, 12.8, 8.0)
            .with_ground_system(StructuralSystem::TimberFrame)
            .validate()
            .expect_err("timber cannot carry concrete");
        assert_eq!(
            error,
            StructureError::GroundTooWeak {
                ground: StructuralSystem::TimberFrame,
                system: StructuralSystem::ReinforcedConcrete,
            }
        );
        assert_eq!(error.code(), "structure_ground");
        assert!(error.to_string().contains("timber frame"));
    }

    #[test]
    fn invalid_fields_are_rejected_before_limits() {
        assert_eq!(
            StructureSpec::new(StructuralSystem::Tent, 0, 3.0, 3.0)
                .validate()
                .expect_err("zero stories"),
            StructureError::InvalidStories { stories: 0 }
        );
        assert_eq!(
            StructureSpec::new(StructuralSystem::Tent, 1, 0.0, 3.0)
                .validate()
                .expect_err("zero height"),
            StructureError::InvalidHeight { height_m: 0.0 }
        );
        assert_eq!(
            StructureSpec::new(StructuralSystem::Tent, 1, 3.0, -1.0)
                .validate()
                .expect_err("negative span"),
            StructureError::InvalidSpan { span_m: -1.0 }
        );
        assert_eq!(
            StructureSpec::new(StructuralSystem::Tent, 1, 3.0, 3.0)
                .with_foundation_depth(-0.1)
                .validate()
                .expect_err("negative foundation"),
            StructureError::InvalidFoundation { depth_m: -0.1 }
        );

        for invalid in [f32::NAN, f32::INFINITY] {
            let error = StructureSpec::new(StructuralSystem::Tent, 1, invalid, 3.0)
                .validate()
                .expect_err("non-finite height");
            assert_eq!(error.code(), "invalid_structure");
            assert!(matches!(error, StructureError::InvalidHeight { .. }));

            let error = StructureSpec::new(StructuralSystem::Tent, 1, 3.0, 3.0)
                .with_foundation_depth(invalid)
                .validate()
                .expect_err("non-finite foundation");
            assert_eq!(error.code(), "invalid_structure");
        }
    }

    #[test]
    fn error_codes_are_stable() {
        let cases = [
            (
                StructureError::InvalidStories { stories: 0 },
                "invalid_structure",
            ),
            (
                StructureError::InvalidHeight { height_m: 0.0 },
                "invalid_structure",
            ),
            (
                StructureError::InvalidSpan { span_m: -1.0 },
                "invalid_structure",
            ),
            (
                StructureError::InvalidFoundation { depth_m: -1.0 },
                "invalid_structure",
            ),
            (
                StructureError::InvalidBounds { bounds: [0.0; 3] },
                "invalid_structure",
            ),
            (
                StructureError::StoriesExceeded {
                    system: StructuralSystem::Tent,
                    stories: 2,
                    limit: 1,
                },
                "structure_stories",
            ),
            (
                StructureError::HeightExceeded {
                    system: StructuralSystem::Tent,
                    height_m: 4.0,
                    limit: 3.0,
                },
                "structure_height",
            ),
            (
                StructureError::SpanExceeded {
                    system: StructuralSystem::Tent,
                    span_m: 4.0,
                    limit: 3.0,
                },
                "structure_span",
            ),
            (
                StructureError::FoundationTooShallow {
                    required_m: 0.5,
                    actual_m: 0.0,
                },
                "structure_foundation",
            ),
            (
                StructureError::GroundTooWeak {
                    ground: StructuralSystem::Tent,
                    system: StructuralSystem::SteelFrame,
                },
                "structure_ground",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code, "{error:?}");
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn story_height_and_format_version_are_stable() {
        assert_eq!(STORY_HEIGHT_M, 3.2);
        assert_eq!(STRUCTURE_FORMAT_VERSION, 1);
    }
}
