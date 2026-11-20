use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use core_blueprint::{
    compute_hash, encode_voxels, load_tsbp, save_tsbp, Blueprint, BlueprintError,
    BlueprintMetadata, FORMAT_VERSION,
};
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};

fn temp_dir(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir().join(format!(
        "townsmith-blueprint-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temp dir");
    path
}

fn test_volume() -> VoxelVolume {
    let mut volume = VoxelVolume::new(VolumeConfig::new(2, 1, 2).unwrap()).unwrap();
    volume
        .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(4, 1, 4), 1)
        .unwrap();
    volume.set_voxel(VolumePos::new(33, 0, 35), 2).unwrap();
    volume
}

fn test_blueprint() -> Blueprint {
    let volume = test_volume();
    let mut metadata = BlueprintMetadata::new("local:test", "Test House");
    metadata.tags.push("test".to_string());
    Blueprint::new(metadata, volume)
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).expect("zip file");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, data) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn roundtrip_preserves_volume_and_hash() {
    let dir = temp_dir("roundtrip");
    let path = dir.join("test.tsbp");
    let blueprint = test_blueprint();
    save_tsbp(&path, &blueprint).expect("save");
    let loaded = load_tsbp(&path).expect("load");

    assert_eq!(loaded.volume, blueprint.volume);
    assert_eq!(loaded.metadata.id, "local:test");
    assert_eq!(loaded.metadata.format_version, FORMAT_VERSION);
    assert_eq!(loaded.metadata.solid_voxels, 17);
    assert!(loaded.metadata.content_hash.starts_with("blake3:"));
    assert!(!dir.join(".test.tsbp.tmp").exists());

    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn empty_volume_roundtrips() {
    let dir = temp_dir("empty");
    let path = dir.join("empty.tsbp");
    let volume = VoxelVolume::new(VolumeConfig::new(1, 1, 1).unwrap()).unwrap();
    let blueprint = Blueprint::new(BlueprintMetadata::new("local:empty", "Empty"), volume);
    save_tsbp(&path, &blueprint).expect("save");
    let loaded = load_tsbp(&path).expect("load");
    assert_eq!(loaded.volume.solid_count(), 0);
    assert_eq!(loaded.metadata.solid_voxels, 0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn hash_is_stable_and_depends_on_voxels() {
    let blueprint = test_blueprint();
    let voxels = encode_voxels(&blueprint.volume).unwrap();
    let first = compute_hash(&blueprint.metadata, &voxels).unwrap();
    let second = compute_hash(&blueprint.metadata, &voxels).unwrap();
    assert_eq!(first, second);

    let mut changed = blueprint.volume.clone();
    changed.set_voxel(VolumePos::new(0, 0, 0), 3).unwrap();
    let changed_voxels = encode_voxels(&changed).unwrap();
    assert_ne!(
        first,
        compute_hash(&blueprint.metadata, &changed_voxels).unwrap()
    );
}

#[test]
fn rejects_unknown_entries() {
    let dir = temp_dir("entries");
    let unknown = dir.join("unknown.tsbp");
    write_zip(&unknown, &[("evil.txt", b"no")]);
    assert!(matches!(
        load_tsbp(&unknown),
        Err(BlueprintError::UnknownEntry(_))
    ));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_bad_version_and_hash() {
    let dir = temp_dir("bad");
    let blueprint = test_blueprint();
    let voxels = encode_voxels(&blueprint.volume).unwrap();

    let mut bad_version = blueprint.metadata.clone();
    bad_version.format_version = 99;
    let version_path = dir.join("version.tsbp");
    let json = serde_json::to_vec(&bad_version).unwrap();
    write_zip(
        &version_path,
        &[("blueprint.json", &json), ("voxels.bin", &voxels)],
    );
    assert!(matches!(
        load_tsbp(&version_path),
        Err(BlueprintError::UnsupportedVersion(99))
    ));

    let mut bad_hash = blueprint.metadata.clone();
    bad_hash.format_version = FORMAT_VERSION;
    bad_hash.bounds_m = blueprint.volume.config().bounds_m();
    bad_hash.solid_voxels = u64::from(blueprint.volume.solid_count());
    bad_hash.content_hash = "blake3:0000".to_string();
    let hash_path = dir.join("hash.tsbp");
    let json = serde_json::to_vec(&bad_hash).unwrap();
    write_zip(
        &hash_path,
        &[("blueprint.json", &json), ("voxels.bin", &voxels)],
    );
    assert!(matches!(
        load_tsbp(&hash_path),
        Err(BlueprintError::HashMismatch { .. })
    ));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_missing_entries_and_bad_zip() {
    let dir = temp_dir("missing");
    let missing = dir.join("missing.tsbp");
    let mut metadata = test_blueprint().metadata;
    metadata.content_hash = "blake3:placeholder".to_string();
    let json = serde_json::to_vec(&metadata).unwrap();
    write_zip(&missing, &[("blueprint.json", &json)]);
    assert!(matches!(
        load_tsbp(&missing),
        Err(BlueprintError::MissingEntry("voxels.bin"))
    ));

    let bad = dir.join("bad.tsbp");
    fs::write(&bad, b"not a zip").unwrap();
    assert!(matches!(load_tsbp(&bad), Err(BlueprintError::Zip(_))));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn golden_checksum_is_stable() {
    let blueprint = test_blueprint();
    let voxels = encode_voxels(&blueprint.volume).unwrap();
    let hash = compute_hash(&blueprint.metadata, &voxels).unwrap();
    assert_eq!(
        hash,
        "blake3:9f5bcd407d25c5b5204315c873faa1321dfb5dd969c9a22d2322f7502f95a070"
    );
}
