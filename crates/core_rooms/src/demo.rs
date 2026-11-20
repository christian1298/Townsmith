//! Deterministic acceptance fixtures shared by tests, benchmarks, and the
//! Godot slice smoke.
//!
//! The fixture is an 8 m × 6 m × 6 m two-story house inside the default
//! 5 × 3 × 5 chunk volume (16 m × 9.6 m × 16 m): a kitchen and bathroom on the
//! ground floor, two bedrooms upstairs, doors, windows, and functional props.
//! It is the room-detection target in [[31 - Room Detection]] and the
//! end-to-end acceptance house in [[26 - Vertical Slice Spec]].
//!
//! Keep the geometry deterministic: room ids, material counts, and the
//! blueprint content hash depend on it.

use core_voxel::{MaterialId, VolumePos, VoxelVolume, AIR};

use crate::{PropInput, PropKind, RoomMaterials};

/// Wall and partition material, matching the editor palette.
pub const WALL: MaterialId = 1;
/// Ground slab and first-floor material, matching the editor palette.
pub const FLOOR: MaterialId = 2;
/// Roof and ceiling material, matching the editor palette.
pub const CEILING: MaterialId = 3;
/// Window material, matching the editor palette.
pub const WINDOW: MaterialId = 4;
/// Door material, matching the editor palette.
pub const DOOR: MaterialId = 5;

fn pos(x: u32, y: u32, z: u32) -> VolumePos {
    VolumePos::new(x, y, z)
}

/// Material roles used by [`acceptance_house`].
#[must_use]
pub fn acceptance_room_materials() -> RoomMaterials {
    RoomMaterials::new(WALL, FLOOR, CEILING, DOOR, WINDOW)
}

/// Build the deterministic two-story acceptance house.
///
/// The default volume config is a compile-time-valid 5 × 3 × 5 volume; a
/// failure here is a programming error in this fixture, not a caller-facing
/// condition.
#[must_use]
pub fn acceptance_house() -> VoxelVolume {
    let mut volume =
        VoxelVolume::new(core_voxel::VolumeConfig::default()).expect("default volume config");
    build_house(&mut volume);
    volume
}

fn build_house(volume: &mut VoxelVolume) {
    // Ground slab and outer shell: 8 m x 6 m footprint, 6 m tall.
    volume
        .fill_box(pos(40, 0, 40), pos(120, 2, 100), FLOOR)
        .expect("ground slab inside the default volume");
    volume
        .fill_box(pos(40, 2, 40), pos(120, 60, 100), WALL)
        .expect("outer shell inside the default volume");
    volume
        .fill_box(pos(42, 2, 42), pos(118, 58, 98), AIR)
        .expect("interior inside the default volume");

    // First-floor slab and roof.
    volume
        .fill_box(pos(42, 30, 42), pos(118, 32, 98), CEILING)
        .expect("first-floor slab inside the default volume");
    volume
        .fill_box(pos(40, 58, 40), pos(120, 60, 100), CEILING)
        .expect("roof inside the default volume");

    // Ground-floor partition (kitchen / bathroom) with a door block.
    volume
        .fill_box(pos(78, 2, 42), pos(80, 30, 98), WALL)
        .expect("ground partition inside the default volume");
    volume
        .fill_box(pos(78, 2, 60), pos(80, 20, 64), DOOR)
        .expect("ground door inside the default volume");

    // Upper-floor partition (two bedrooms) with a door block.
    volume
        .fill_box(pos(78, 32, 42), pos(80, 58, 98), WALL)
        .expect("upper partition inside the default volume");
    volume
        .fill_box(pos(78, 32, 60), pos(80, 50, 64), DOOR)
        .expect("upper door inside the default volume");

    // Exterior door and one window per room on the south wall.
    volume
        .fill_box(pos(60, 2, 40), pos(68, 20, 42), DOOR)
        .expect("exterior door inside the default volume");
    volume
        .fill_box(pos(48, 10, 40), pos(56, 20, 42), WINDOW)
        .expect("ground window inside the default volume");
    volume
        .fill_box(pos(96, 10, 40), pos(104, 20, 42), WINDOW)
        .expect("ground window inside the default volume");
    volume
        .fill_box(pos(48, 40, 40), pos(56, 50, 42), WINDOW)
        .expect("upper window inside the default volume");
    volume
        .fill_box(pos(96, 40, 40), pos(104, 50, 42), WINDOW)
        .expect("upper window inside the default volume");
}

/// Functional props for [`acceptance_house`], in metres.
///
/// Positions mirror the benchmark fixture: stove and sink in the kitchen,
/// toilet and shower in the bathroom, and a bed plus desk in each bedroom.
#[must_use]
pub fn acceptance_props() -> Vec<PropInput> {
    vec![
        PropInput::new("base:stove", PropKind::Stove, [5.5, 1.0, 6.0]),
        PropInput::new("base:sink", PropKind::Sink, [5.0, 1.0, 7.0]),
        PropInput::new("base:toilet", PropKind::Toilet, [9.0, 1.0, 6.0]),
        PropInput::new("base:shower", PropKind::Shower, [10.0, 1.0, 6.0]),
        PropInput::new("base:bed", PropKind::Bed, [5.0, 4.0, 6.0]),
        PropInput::new("base:desk", PropKind::Desk, [6.5, 4.0, 6.0]),
        PropInput::new("base:bed", PropKind::Bed, [9.0, 4.0, 6.0]),
        PropInput::new("base:desk", PropKind::Desk, [10.5, 4.0, 6.0]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect_rooms;

    #[test]
    fn acceptance_house_is_deterministic() {
        let first = acceptance_house();
        let second = acceptance_house();
        assert_eq!(first, second);
        assert_eq!(first.solid_count(), 64_224);
        assert_eq!(first.config().voxels(), [160, 96, 160]);
    }

    #[test]
    fn acceptance_house_has_the_slice_rooms() {
        let volume = acceptance_house();
        let stats = detect_rooms(&volume, &acceptance_room_materials(), &acceptance_props());

        assert_eq!(stats.rooms.len(), 4, "rooms: {:?}", stats.rooms);
        assert_eq!(
            stats
                .rooms
                .iter()
                .filter(|room| room.room_type == "bedroom")
                .count(),
            2
        );
        assert_eq!(
            stats
                .rooms
                .iter()
                .filter(|room| room.room_type == "kitchen")
                .count(),
            1
        );
        assert_eq!(
            stats
                .rooms
                .iter()
                .filter(|room| room.room_type == "bathroom")
                .count(),
            1
        );
        assert_eq!(stats.resident_capacity, 2);
        assert_eq!(stats.job_slots, 2);
        assert_eq!(stats.solid_voxels, 64_224);
    }
}
