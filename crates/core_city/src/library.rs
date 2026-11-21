//! Starter building catalog.
//!
//! Phase 4.4b gives new cities a small, deterministic set of placeable
//! buildings. The catalog is data-only: the Godot bridge turns each entry into
//! a `.tsbp` package with a generated shell and the catalog's capacity, jobs,
//! structure, and material cost. Keeping the catalog in `core_city` means the
//! age gate, the advisor, and future content filters share one source.

use serde::{Deserialize, Serialize};

use crate::progression::Age;
use core_structure::StructuralSystem;

/// Broad use of a starter building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StarterCategory {
    /// Housing.
    Residential,
    /// Production or commerce.
    Industry,
    /// Civic service or administration.
    Civic,
}

impl StarterCategory {
    /// Stable identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Residential => "residential",
            Self::Industry => "industry",
            Self::Civic => "civic",
        }
    }
}

/// One starter building available in the catalog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StarterBuilding {
    /// Stable content id.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Broad use.
    pub category: StarterCategory,
    /// Age that unlocks the building.
    pub age: Age,
    /// Structural system used by the generated shell.
    ///
    /// `None` keeps a starter outside the structural validator, for example a
    /// tent whose 3.2 m chunk height exceeds the 3.0 m tent limit.
    pub system: Option<StructuralSystem>,
    /// Above-grade stories.
    pub stories: u16,
    /// Footprint in metres.
    pub footprint_m: [f32; 2],
    /// Resident capacity.
    pub resident_capacity: u32,
    /// Job slots.
    pub job_slots: u32,
    /// Material cost in abstract cost units.
    pub material_cost: f64,
}

impl StarterBuilding {
    /// Whether the building is available at `age`.
    #[must_use]
    pub fn available_at(&self, age: Age) -> bool {
        age_rank(age) >= age_rank(self.age)
    }

    /// Payload-friendly category.
    #[must_use]
    pub const fn category_str(&self) -> &'static str {
        self.category.as_str()
    }
}

const STARTERS: &[StarterBuilding] = &[
    StarterBuilding {
        id: "base:cottage",
        name: "Cottage",
        category: StarterCategory::Residential,
        age: Age::Wood,
        system: Some(StructuralSystem::TimberFrame),
        stories: 1,
        footprint_m: [6.4, 6.4],
        resident_capacity: 4,
        job_slots: 0,
        material_cost: 12_000.0,
    },
    StarterBuilding {
        id: "base:tent",
        name: "Tent",
        category: StarterCategory::Residential,
        age: Age::Wood,
        system: None,
        stories: 1,
        footprint_m: [3.2, 3.2],
        resident_capacity: 1,
        job_slots: 0,
        material_cost: 1_500.0,
    },
    StarterBuilding {
        id: "base:longhouse",
        name: "Longhouse",
        category: StarterCategory::Residential,
        age: Age::Wood,
        system: Some(StructuralSystem::TimberFrame),
        stories: 2,
        footprint_m: [9.6, 6.4],
        resident_capacity: 6,
        job_slots: 0,
        material_cost: 22_000.0,
    },
    StarterBuilding {
        id: "base:workshop",
        name: "Workshop",
        category: StarterCategory::Industry,
        age: Age::Wood,
        system: Some(StructuralSystem::TimberFrame),
        stories: 1,
        footprint_m: [6.4, 6.4],
        resident_capacity: 0,
        job_slots: 4,
        material_cost: 14_000.0,
    },
    StarterBuilding {
        id: "base:farmhouse",
        name: "Farmhouse",
        category: StarterCategory::Industry,
        age: Age::Wood,
        system: Some(StructuralSystem::TimberFrame),
        stories: 1,
        footprint_m: [9.6, 9.6],
        resident_capacity: 2,
        job_slots: 4,
        material_cost: 18_000.0,
    },
    StarterBuilding {
        id: "base:market",
        name: "Market",
        category: StarterCategory::Industry,
        age: Age::Wood,
        system: Some(StructuralSystem::TimberFrame),
        stories: 1,
        footprint_m: [9.6, 6.4],
        resident_capacity: 0,
        job_slots: 6,
        material_cost: 16_000.0,
    },
    StarterBuilding {
        id: "base:sawmill",
        name: "Sawmill",
        category: StarterCategory::Industry,
        age: Age::Wood,
        system: Some(StructuralSystem::TimberFrame),
        stories: 1,
        footprint_m: [12.8, 9.6],
        resident_capacity: 0,
        job_slots: 6,
        material_cost: 24_000.0,
    },
    StarterBuilding {
        id: "base:stone_house",
        name: "Stone House",
        category: StarterCategory::Residential,
        age: Age::Masonry,
        system: Some(StructuralSystem::LoadBearingStone),
        stories: 2,
        footprint_m: [6.4, 6.4],
        resident_capacity: 8,
        job_slots: 0,
        material_cost: 32_000.0,
    },
    StarterBuilding {
        id: "base:town_hall",
        name: "Town Hall",
        category: StarterCategory::Civic,
        age: Age::Masonry,
        system: Some(StructuralSystem::LoadBearingStone),
        stories: 2,
        footprint_m: [12.8, 9.6],
        resident_capacity: 0,
        job_slots: 8,
        material_cost: 48_000.0,
    },
    StarterBuilding {
        id: "base:school",
        name: "School",
        category: StarterCategory::Civic,
        age: Age::Masonry,
        system: Some(StructuralSystem::LoadBearingStone),
        stories: 2,
        footprint_m: [9.6, 9.6],
        resident_capacity: 0,
        job_slots: 4,
        material_cost: 36_000.0,
    },
    StarterBuilding {
        id: "base:clinic",
        name: "Clinic",
        category: StarterCategory::Civic,
        age: Age::Masonry,
        system: Some(StructuralSystem::LoadBearingStone),
        stories: 2,
        footprint_m: [9.6, 6.4],
        resident_capacity: 0,
        job_slots: 4,
        material_cost: 30_000.0,
    },
    StarterBuilding {
        id: "base:bakery",
        name: "Bakery",
        category: StarterCategory::Industry,
        age: Age::Masonry,
        system: Some(StructuralSystem::LoadBearingBrick),
        stories: 1,
        footprint_m: [6.4, 6.4],
        resident_capacity: 0,
        job_slots: 4,
        material_cost: 20_000.0,
    },
    StarterBuilding {
        id: "base:tenement",
        name: "Tenement",
        category: StarterCategory::Residential,
        age: Age::Industrial,
        system: Some(StructuralSystem::LoadBearingBrick),
        stories: 3,
        footprint_m: [9.6, 9.6],
        resident_capacity: 16,
        job_slots: 0,
        material_cost: 60_000.0,
    },
    StarterBuilding {
        id: "base:factory",
        name: "Factory",
        category: StarterCategory::Industry,
        age: Age::Industrial,
        system: Some(StructuralSystem::ReinforcedConcrete),
        stories: 2,
        footprint_m: [12.8, 12.8],
        resident_capacity: 0,
        job_slots: 12,
        material_cost: 72_000.0,
    },
    StarterBuilding {
        id: "base:apartment",
        name: "Apartment Block",
        category: StarterCategory::Residential,
        age: Age::Modern,
        system: Some(StructuralSystem::ReinforcedConcrete),
        stories: 4,
        footprint_m: [12.8, 9.6],
        resident_capacity: 24,
        job_slots: 0,
        material_cost: 110_000.0,
    },
    StarterBuilding {
        id: "base:office",
        name: "Office",
        category: StarterCategory::Industry,
        age: Age::Modern,
        system: Some(StructuralSystem::SteelFrame),
        stories: 4,
        footprint_m: [12.8, 12.8],
        resident_capacity: 0,
        job_slots: 16,
        material_cost: 120_000.0,
    },
    StarterBuilding {
        id: "base:hospital",
        name: "Hospital",
        category: StarterCategory::Civic,
        age: Age::Modern,
        system: Some(StructuralSystem::ReinforcedConcrete),
        stories: 3,
        footprint_m: [12.8, 12.8],
        resident_capacity: 0,
        job_slots: 8,
        material_cost: 130_000.0,
    },
    StarterBuilding {
        id: "base:tower",
        name: "Tower",
        category: StarterCategory::Residential,
        age: Age::Information,
        system: Some(StructuralSystem::CompositeCore),
        stories: 8,
        footprint_m: [12.8, 12.8],
        resident_capacity: 32,
        job_slots: 0,
        material_cost: 240_000.0,
    },
    StarterBuilding {
        id: "base:university",
        name: "University",
        category: StarterCategory::Civic,
        age: Age::Information,
        system: Some(StructuralSystem::SteelFrame),
        stories: 4,
        footprint_m: [12.8, 12.8],
        resident_capacity: 0,
        job_slots: 12,
        material_cost: 180_000.0,
    },
    StarterBuilding {
        id: "base:data_center",
        name: "Data Center",
        category: StarterCategory::Industry,
        age: Age::Information,
        system: Some(StructuralSystem::SteelFrame),
        stories: 2,
        footprint_m: [12.8, 9.6],
        resident_capacity: 0,
        job_slots: 12,
        material_cost: 160_000.0,
    },
];

/// Every starter building, ordered from Wood to Information.
#[must_use]
pub fn starters() -> &'static [StarterBuilding] {
    STARTERS
}

/// Look up a starter by id.
#[must_use]
pub fn starter(id: &str) -> Option<&'static StarterBuilding> {
    STARTERS.iter().find(|building| building.id == id)
}

/// Starter buildings available at `age`.
#[must_use]
pub fn available(age: Age) -> Vec<&'static StarterBuilding> {
    STARTERS
        .iter()
        .filter(|building| building.available_at(age))
        .collect()
}

fn age_rank(age: Age) -> usize {
    Age::ALL
        .iter()
        .position(|candidate| *candidate == age)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_lookup_works() {
        let mut ids = std::collections::BTreeSet::new();
        for building in starters() {
            assert!(ids.insert(building.id), "duplicate {}", building.id);
            assert_eq!(starter(building.id), Some(building));
            assert!(building.material_cost > 0.0);
            assert!(building.stories >= 1);
            assert!(building.available_at(building.age));
        }
        assert!(starter("base:missing").is_none());
    }

    #[test]
    fn every_age_gets_a_non_empty_catalog() {
        for age in Age::ALL {
            let entries = available(age);
            assert!(!entries.is_empty(), "no starters at {age:?}");
            assert!(entries.iter().all(|building| building.available_at(age)));
        }
        assert!(available(Age::Wood).len() < available(Age::Information).len());
    }

    #[test]
    fn age_and_structure_requirements_agree() {
        for building in starters() {
            let Some(system) = building.system else {
                continue;
            };
            let required = Age::parse(system.min_age()).expect("known age");
            assert!(
                age_rank(building.age) >= age_rank(required),
                "{} is {} but its system needs {}",
                building.id,
                building.age.name(),
                required.name()
            );
        }
    }
}
