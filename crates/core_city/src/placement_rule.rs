//! Shared placement rules for the bridge and the authoritative server.
//!
//! The GDExtension bridge and the headless server must agree on the age gate
//! for a declared structural system (D-024). This module owns that rule once;
//! callers pass the blueprint's [`StructureSpec`] and the settlement [`Age`].
//!
//! The rule is purely derived from the spec: no stores, no mutation, and no
//! engine types.

use core_structure::{StructuralSystem, StructureSpec};

use crate::progression::Age;

/// Rank of an age in the progression order.
///
/// Unknown values cannot occur for a compiled structural system, but the
/// fallback keeps the helper panic-free for hand-built input.
#[must_use]
fn age_rank(age: Age) -> usize {
    Age::ALL
        .iter()
        .position(|candidate| *candidate == age)
        .unwrap_or(0)
}

/// The age and structural system that gate `spec`.
///
/// The above-ground system and an optional ground-floor system are both
/// considered; the later age wins and is the system named in the error. This
/// keeps composite construction from unlocking a brick ground floor in the
/// timber age through a later upper floor.
#[must_use]
pub fn structure_required_age(spec: &StructureSpec) -> (Age, StructuralSystem) {
    let mut required_system = spec.system;
    let mut required =
        Age::parse(spec.system.min_age()).expect("compiled structural systems have a known age");
    if let Some(ground) = spec.ground_system {
        let ground_age =
            Age::parse(ground.min_age()).expect("compiled structural systems have a known age");
        if age_rank(ground_age) > age_rank(required) {
            required = ground_age;
            required_system = ground;
        }
    }
    (required, required_system)
}

/// The bridge/server age-gate error, or `None` when the age is unlocked.
///
/// The tuple is `(code, message)` with the stable `age_locked` code and the
/// bridge's existing message text. The server maps the code to its own wire
/// code.
#[must_use]
pub fn structure_age_error(spec: &StructureSpec, current: Age) -> Option<(&'static str, String)> {
    let (required, required_system) = structure_required_age(spec);
    if age_rank(current) >= age_rank(required) {
        return None;
    }
    Some((
        "age_locked",
        format!(
            "{} requires the {} age; the city is in the {} age",
            required_system.name(),
            required.name(),
            current.name()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timber_is_available_in_the_wood_age() {
        let timber = StructureSpec::new(StructuralSystem::TimberFrame, 1, 3.2, 0.0);
        assert_eq!(
            structure_required_age(&timber),
            (Age::Wood, StructuralSystem::TimberFrame)
        );
        assert!(structure_age_error(&timber, Age::Wood).is_none());
        assert!(structure_age_error(&timber, Age::Information).is_none());
    }

    #[test]
    fn masonry_is_locked_until_the_masonry_age() {
        let brick = StructureSpec::new(StructuralSystem::LoadBearingBrick, 2, 6.4, 0.0);
        let (required, system) = structure_required_age(&brick);
        assert_eq!(required, Age::Masonry);
        assert_eq!(system, StructuralSystem::LoadBearingBrick);

        let (code, message) = structure_age_error(&brick, Age::Wood).expect("locked");
        assert_eq!(code, "age_locked");
        assert_eq!(
            message,
            format!(
                "{} requires the masonry age; the city is in the wood age",
                StructuralSystem::LoadBearingBrick.name()
            )
        );
        assert!(structure_age_error(&brick, Age::Masonry).is_none());
    }

    #[test]
    fn a_composite_ground_floor_uses_the_later_age() {
        let composite = StructureSpec::new(StructuralSystem::TimberFrame, 2, 6.4, 0.0)
            .with_ground_system(StructuralSystem::LoadBearingBrick);
        let (required, system) = structure_required_age(&composite);
        assert_eq!(required, Age::Masonry);
        assert_eq!(system, StructuralSystem::LoadBearingBrick);

        let (_, message) = structure_age_error(&composite, Age::Wood).expect("ground locked");
        assert!(message.contains("load-bearing brick"));
    }

    #[test]
    fn the_rule_is_deterministic() {
        let spec = StructureSpec::new(StructuralSystem::SteelFrame, 8, 30.0, 8.0);
        assert_eq!(structure_required_age(&spec), structure_required_age(&spec));
    }
}
