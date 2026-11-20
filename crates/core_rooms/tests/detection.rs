//! Integration tests for room detection v0.

use core_rooms::{
    detect_rooms, PropInput, PropKind, RoomMaterials, RoomRecord, RoomStats, VOXEL_AREA_M2,
};
use core_voxel::{MaterialId, VolumePos, VoxelVolume, AIR};

const FLOOR: MaterialId = 1;
const WALL: MaterialId = 2;
const CEILING: MaterialId = 3;
const DOOR: MaterialId = 4;
const WINDOW: MaterialId = 5;
const BRICK: MaterialId = 6;

fn pos(x: u32, y: u32, z: u32) -> VolumePos {
    VolumePos::new(x, y, z)
}

fn materials() -> RoomMaterials {
    RoomMaterials::new(WALL, FLOOR, CEILING, DOOR, WINDOW)
}

/// A 4 m x 4 m shell with a 3.6 m x 3.6 m interior on floor 0.
fn enclosed_room() -> VoxelVolume {
    enclosed_rect(40, 80, 40, 80)
}

/// A rectangular shell: 2-voxel walls, 2-voxel floor, 2-voxel ceiling.
fn enclosed_rect(x0: u32, x1: u32, z0: u32, z1: u32) -> VoxelVolume {
    let mut volume = VoxelVolume::default();
    volume
        .fill_box(pos(x0, 0, z0), pos(x1, 2, z1), FLOOR)
        .unwrap();
    volume
        .fill_box(pos(x0, 2, z0), pos(x1, 32, z1), WALL)
        .unwrap();
    volume
        .fill_box(pos(x0 + 2, 2, z0 + 2), pos(x1 - 2, 30, z1 - 2), AIR)
        .unwrap();
    volume
        .fill_box(pos(x0, 30, z0), pos(x1, 32, z1), CEILING)
        .unwrap();
    volume
}

/// Two rooms split by a solid partition, with a door material block and a
/// window in the left room's south wall.
fn two_room_shell() -> VoxelVolume {
    let mut volume = enclosed_rect(40, 120, 40, 80);
    volume
        .fill_box(pos(78, 2, 42), pos(80, 30, 78), WALL)
        .unwrap();
    volume
        .fill_box(pos(78, 2, 58), pos(80, 20, 62), DOOR)
        .unwrap();
    volume
        .fill_box(pos(50, 10, 40), pos(54, 20, 42), WINDOW)
        .unwrap();
    volume
}

/// One enclosed room on each of two floors.
fn two_floor_shell() -> VoxelVolume {
    let mut volume = VoxelVolume::default();
    volume
        .fill_box(pos(40, 0, 40), pos(80, 60, 80), WALL)
        .unwrap();
    volume
        .fill_box(pos(40, 0, 40), pos(80, 2, 80), FLOOR)
        .unwrap();
    volume
        .fill_box(pos(42, 2, 42), pos(78, 58, 78), AIR)
        .unwrap();
    volume
        .fill_box(pos(42, 30, 42), pos(78, 32, 78), CEILING)
        .unwrap();
    volume
        .fill_box(pos(40, 58, 40), pos(80, 60, 80), CEILING)
        .unwrap();
    volume
}

fn detect(volume: &VoxelVolume, props: &[PropInput]) -> RoomStats {
    detect_rooms(volume, &materials(), props)
}

fn only_room(stats: &RoomStats) -> &RoomRecord {
    assert_eq!(stats.rooms.len(), 1, "rooms: {:?}", stats.rooms);
    &stats.rooms[0]
}

#[test]
fn empty_volume_has_no_rooms() {
    let volume = VoxelVolume::default();
    let stats = detect(&volume, &[]);

    assert!(stats.rooms.is_empty());
    assert!(stats.materials.is_empty());
    assert_eq!(stats.solid_voxels, 0);
    assert_eq!(stats.resident_capacity, 0);
    assert_eq!(stats.job_slots, 0);
    assert_eq!(stats.class, "lower_middle");
}

#[test]
fn enclosed_room_is_detected() {
    let volume = enclosed_room();
    let stats = detect(&volume, &[]);
    let room = only_room(&stats);

    assert_eq!(room.id, "room_0");
    assert_eq!(room.floor, 0);
    assert_eq!(room.room_type, "empty");
    assert_eq!(room.beds, 0);
    assert_eq!(room.doors, 0);
    assert_eq!(room.windows, 0);

    let expected_cells = 36.0 * 36.0;
    assert!((room.area_m2 - expected_cells * VOXEL_AREA_M2).abs() < 1e-3);
    assert!((room.volume_m3 - room.area_m2 * 3.0).abs() < 1e-3);
    assert_eq!(stats.solid_voxels, u64::from(volume.solid_count()));
}

#[test]
fn partition_with_door_yields_two_rooms_and_counts_doors() {
    let volume = two_room_shell();
    let stats = detect(&volume, &[]);

    assert_eq!(stats.rooms.len(), 2);
    assert_eq!(stats.rooms[0].id, "room_0");
    assert_eq!(stats.rooms[1].id, "room_1");

    // Left room: 36 x 36 cells; right room: 38 x 36 cells.
    assert!((stats.rooms[0].area_m2 - 12.96).abs() < 1e-3);
    assert!((stats.rooms[1].area_m2 - 13.68).abs() < 1e-3);

    // The solid door block spans 4 columns at the sampled layer and is
    // adjacent to both rooms.
    assert_eq!(stats.rooms[0].doors, 4);
    assert_eq!(stats.rooms[1].doors, 4);

    // Only the left room has the south window.
    assert_eq!(stats.rooms[0].windows, 4);
    assert_eq!(stats.rooms[1].windows, 0);
}

#[test]
fn two_floors_yield_two_rooms_with_correct_floor_indices() {
    let volume = two_floor_shell();
    let stats = detect(&volume, &[]);

    assert_eq!(stats.rooms.len(), 2);
    assert_eq!(stats.rooms[0].floor, 0);
    assert_eq!(stats.rooms[1].floor, 1);
    assert!((stats.rooms[0].area_m2 - stats.rooms[1].area_m2).abs() < 1e-4);
    assert!((stats.rooms[0].area_m2 - 12.96).abs() < 1e-3);
}

#[test]
fn exterior_air_is_not_a_room() {
    let mut volume = VoxelVolume::default();
    volume
        .fill_box(pos(40, 0, 40), pos(120, 2, 100), FLOOR)
        .unwrap();

    let stats = detect(&volume, &[]);
    assert!(stats.rooms.is_empty());
    assert_eq!(stats.solid_voxels, 80 * 2 * 60);
}

#[test]
fn props_assign_room_types_and_capacity() {
    let volume = enclosed_room();

    let bedroom = detect(
        &volume,
        &[
            PropInput::new("base:bed_a", PropKind::Bed, [5.0, 1.0, 5.0]),
            PropInput::new("base:bed_b", PropKind::Bed, [6.0, 1.0, 5.0]),
        ],
    );
    let room = only_room(&bedroom);
    assert_eq!(room.room_type, "bedroom");
    assert_eq!(room.beds, 2);
    assert_eq!(bedroom.resident_capacity, 2);
    assert_eq!(bedroom.job_slots, 0);

    let kitchen = detect(
        &volume,
        &[PropInput::new(
            "base:stove",
            PropKind::Stove,
            [5.0, 1.0, 5.0],
        )],
    );
    assert_eq!(only_room(&kitchen).room_type, "kitchen");

    let bathroom = detect(
        &volume,
        &[
            PropInput::new("base:toilet", PropKind::Toilet, [5.0, 1.0, 5.0]),
            PropInput::new("base:shower", PropKind::Shower, [5.5, 1.0, 5.0]),
        ],
    );
    assert_eq!(only_room(&bathroom).room_type, "bathroom");

    let office = detect(
        &volume,
        &[PropInput::new("base:desk", PropKind::Desk, [5.0, 1.0, 5.0])],
    );
    assert_eq!(only_room(&office).room_type, "office");
    assert_eq!(office.job_slots, 1);
    assert_eq!(office.resident_capacity, 0);

    let living = detect(
        &volume,
        &[PropInput::new("base:sofa", PropKind::Sofa, [5.0, 1.0, 5.0])],
    );
    assert_eq!(only_room(&living).room_type, "living_dining");

    let dining = detect(
        &volume,
        &[PropInput::new(
            "base:table",
            PropKind::Table,
            [5.0, 1.0, 5.0],
        )],
    );
    assert_eq!(only_room(&dining).room_type, "living_dining");

    // A bed wins over a stove when both are present.
    let mixed = detect(
        &volume,
        &[
            PropInput::new("base:bed", PropKind::Bed, [5.0, 1.0, 5.0]),
            PropInput::new("base:stove", PropKind::Stove, [6.0, 1.0, 5.0]),
        ],
    );
    assert_eq!(only_room(&mixed).room_type, "bedroom");
}

#[test]
fn props_above_the_volume_are_ignored() {
    let mut volume = VoxelVolume::default();
    // A room in the partial top band (90..96 voxels).
    volume
        .fill_box(pos(40, 90, 40), pos(80, 96, 80), WALL)
        .unwrap();
    volume
        .fill_box(pos(42, 90, 42), pos(78, 96, 78), AIR)
        .unwrap();
    volume
        .fill_box(pos(40, 90, 40), pos(80, 91, 80), FLOOR)
        .unwrap();
    let props = [PropInput::new(
        "base:bed_single",
        PropKind::Bed,
        [5.0, 9.7, 5.0],
    )];
    let stats = detect_rooms(&volume, &materials(), &props);
    assert_eq!(stats.rooms.len(), 1);
    assert_eq!(
        stats.resident_capacity, 0,
        "prop above 9.6 m must be ignored"
    );
}

#[test]
fn props_outside_the_room_are_ignored() {
    let volume = enclosed_room();
    let stats = detect(
        &volume,
        &[
            // Outside the shell.
            PropInput::new("base:bed", PropKind::Bed, [1.0, 1.0, 1.0]),
            // Inside the shell footprint but above the roof.
            PropInput::new("base:desk", PropKind::Desk, [5.0, 5.0, 5.0]),
        ],
    );

    let room = only_room(&stats);
    assert_eq!(room.room_type, "empty");
    assert_eq!(stats.resident_capacity, 0);
    assert_eq!(stats.job_slots, 0);
}

#[test]
fn material_counts_and_solid_count_are_exact() {
    let mut volume = VoxelVolume::default();
    volume
        .fill_box(pos(10, 10, 10), pos(12, 12, 12), FLOOR)
        .unwrap();
    volume
        .fill_box(pos(20, 10, 10), pos(23, 11, 11), BRICK)
        .unwrap();
    volume.set_voxel(pos(30, 10, 10), WALL).unwrap();

    let stats = detect(&volume, &[]);

    assert_eq!(stats.solid_voxels, 12);
    assert_eq!(stats.materials.get("material:1"), Some(&8));
    assert_eq!(stats.materials.get("material:6"), Some(&3));
    assert_eq!(stats.materials.get("material:2"), Some(&1));
    assert_eq!(stats.materials.len(), 3);
    assert_eq!(stats.materials.values().sum::<u64>(), 12);
}

#[test]
fn detection_is_deterministic() {
    let first = detect(&two_room_shell(), &[]);
    let second = detect(&two_room_shell(), &[]);

    assert_eq!(first, second);
    assert_eq!(
        first
            .rooms
            .iter()
            .map(|room| room.id.as_str())
            .collect::<Vec<_>>(),
        ["room_0", "room_1"]
    );

    // Repeated detection on the same volume is stable too.
    let volume = two_room_shell();
    assert_eq!(detect(&volume, &[]), detect(&volume, &[]));
}
