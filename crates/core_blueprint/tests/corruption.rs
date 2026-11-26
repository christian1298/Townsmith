//! Deterministic corruption corpus for the v1 `.tsbp` reader.
//!
//! Every case starts from a freshly saved, valid package and mutates a copy in
//! its own temporary directory. The seed package is never modified, so the
//! tests also pin that corruption handling does not touch a valid artifact.
//! All assertions are on stable typed errors (`BlueprintError` variants), not
//! on panics or on zip-crate implementation detail.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use core_blueprint::{
    load_tsbp, load_tsbp_with_dependencies, save_tsbp_with_hash, Blueprint, BlueprintError,
    BlueprintLibrary, BlueprintLibraryError, BlueprintMetadata, MAX_CHUNKS, MAX_METADATA_BYTES,
    MAX_SOLID_VOXELS, MAX_VOLUME_VOXELS,
};
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};
use core_voxel::{VoxelError, AIR, CHUNK_SIZE, CHUNK_VOLUME};

const GOLDEN_HASH: &str = "blake3:9f5bcd407d25c5b5204315c873faa1321dfb5dd969c9a22d2322f7502f95a070";

/// Self-cleaning unique directory under the OS temp directory.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "townsmith-blueprint-corruption-{}-{stamp}-{unique}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp directory");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A valid seed package plus its two entry payloads, held in memory.
struct Fixture {
    dir: TempDir,
    path: PathBuf,
    bytes: Vec<u8>,
    json: Vec<u8>,
    voxels: Vec<u8>,
    hash: String,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = TempDir::new(label);
        let path = dir.join("seed.tsbp");
        let hash = save_tsbp_with_hash(&path, &test_blueprint()).expect("save seed package");
        let bytes = fs::read(&path).expect("read seed package");
        let json = zip_entry(&bytes, "blueprint.json");
        let voxels = zip_entry(&bytes, "voxels.bin");
        Self {
            dir,
            path,
            bytes,
            json,
            voxels,
            hash,
        }
    }

    fn write_package(&self, label: &str, json: &[u8], voxels: &[u8]) -> PathBuf {
        let path = self.dir.join(&format!("{label}.tsbp"));
        write_zip(&path, &[("blueprint.json", json), ("voxels.bin", voxels)]);
        path
    }

    fn write_voxels(&self, label: &str, voxels: &[u8]) -> PathBuf {
        self.write_package(label, &self.json, voxels)
    }
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
    // Keep the metadata identical to the golden fixture in `roundtrip.rs` so
    // the corpus reuses the published checksum.
    let mut metadata = BlueprintMetadata::new("local:test", "Test House");
    metadata.tags.push("test".to_string());
    Blueprint::new(metadata, test_volume())
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).expect("zip file");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, data) in entries {
        zip.start_file(*name, options).expect("start file");
        zip.write_all(data).expect("write entry");
    }
    zip.finish().expect("finish zip");
}

fn zip_entry(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("seed zip");
    let mut entry = archive.by_name(name).expect("seed entry");
    let mut data = Vec::new();
    entry.read_to_end(&mut data).expect("read seed entry");
    data
}

fn edited_json(json: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(json).expect("seed json");
    edit(&mut value);
    serde_json::to_vec(&value).expect("encode edited json")
}

fn load_error(path: &Path) -> BlueprintError {
    load_tsbp(path).expect_err("corrupt package must not load")
}

fn header(chunks: [u32; 3], count: u32, declared_solid: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(b"TSBV");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(CHUNK_SIZE as u16).to_le_bytes());
    for axis in chunks {
        out.extend_from_slice(&axis.to_le_bytes());
    }
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&declared_solid.to_le_bytes());
    assert_eq!(out.len(), 32);
    out
}

fn push_chunk(out: &mut Vec<u8>, palette: &[u16], runs: &[(u16, u16)]) {
    for axis in [0u32; 3] {
        out.extend_from_slice(&axis.to_le_bytes());
    }
    out.extend_from_slice(
        &u16::try_from(palette.len())
            .expect("palette fits")
            .to_le_bytes(),
    );
    out.extend_from_slice(&u16::try_from(runs.len()).expect("runs fit").to_le_bytes());
    for material in palette {
        out.extend_from_slice(&material.to_le_bytes());
    }
    for (index, count) in runs {
        out.extend_from_slice(&index.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
    }
}

#[test]
fn valid_seed_still_loads_with_its_golden_hash() {
    let fixture = Fixture::new("seed");
    let loaded = load_tsbp(&fixture.path).expect("seed loads");

    assert_eq!(fixture.hash, GOLDEN_HASH);
    assert_eq!(loaded.metadata.content_hash, GOLDEN_HASH);
    assert_eq!(loaded.metadata.solid_voxels, 17);
    assert_eq!(loaded.volume, test_volume());
    assert_eq!(fs::read(&fixture.path).expect("read seed"), fixture.bytes);
}

#[test]
fn corruption_does_not_mutate_the_seed_package() {
    let fixture = Fixture::new("immutable");
    let before = fs::read(&fixture.path).expect("read seed before");

    let mut corrupt = fixture.bytes.clone();
    corrupt[0] ^= 0xff;
    let corrupt_path = fixture.dir.join("bad-magic.tsbp");
    fs::write(&corrupt_path, &corrupt).expect("write corrupt copy");
    assert!(matches!(load_error(&corrupt_path), BlueprintError::Zip(_)));

    assert_eq!(
        fs::read(&fixture.path).expect("read seed after"),
        before,
        "corruption testing must not rewrite the valid package"
    );
    load_tsbp(&fixture.path).expect("seed still loads after a corrupt sibling");
}

#[test]
fn rejects_corrupt_zip_magic() {
    let fixture = Fixture::new("zip-magic");
    let mut corrupt = fixture.bytes.clone();
    corrupt[0] ^= 0xff;
    let path = fixture.dir.join("zip-magic.tsbp");
    fs::write(&path, &corrupt).expect("write corrupt package");

    match load_error(&path) {
        BlueprintError::Zip(_) => {}
        other => panic!("expected a typed zip error, got {other:?}"),
    }
}

#[test]
fn rejects_unsupported_format_version() {
    let fixture = Fixture::new("version");
    let json = edited_json(&fixture.json, |value| {
        value["format_version"] = serde_json::Value::from(99);
    });
    let path = fixture.write_package("version", &json, &fixture.voxels);

    let error = load_error(&path);
    match &error {
        BlueprintError::UnsupportedVersion(99) => {
            assert_eq!(error.to_string(), "unsupported blueprint format version 99");
        }
        other => panic!("expected UnsupportedVersion(99), got {other:?}"),
    }
}

#[test]
fn rejects_path_traversal_entry_names() {
    let fixture = Fixture::new("traversal");
    let path = fixture.dir.join("traversal.tsbp");
    write_zip(
        &path,
        &[
            ("../blueprint.json", &fixture.json),
            ("voxels.bin", &fixture.voxels),
        ],
    );

    let error = load_error(&path);
    match &error {
        BlueprintError::UnknownEntry(name) => {
            assert_eq!(name, "../blueprint.json");
            assert_eq!(
                error.to_string(),
                "blueprint contains unknown entry ../blueprint.json"
            );
        }
        other => panic!("expected a traversal entry rejection, got {other:?}"),
    }

    // The reader never extracts, so the temp dir only contains the seed and
    // the crafted package, with no escaped `blueprint.json`.
    let mut written: Vec<_> = fs::read_dir(&fixture.dir.path)
        .expect("read temp dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    written.sort();
    assert_eq!(
        written,
        vec![
            std::ffi::OsString::from("seed.tsbp"),
            std::ffi::OsString::from("traversal.tsbp"),
        ]
    );
}

#[test]
fn rejects_malformed_manifest_json() {
    let fixture = Fixture::new("manifest");
    let path = fixture.write_package("manifest", b"{\"format_version\": 1,", &fixture.voxels);

    match load_error(&path) {
        BlueprintError::Json(_) => {}
        other => panic!("expected a typed JSON error, got {other:?}"),
    }
}

#[test]
fn rejects_truncated_zip_at_deterministic_offsets() {
    let fixture = Fixture::new("truncated");
    let length = fixture.bytes.len();
    let mut offsets = vec![
        0,
        1,
        4,
        16,
        31,
        length / 2,
        length.saturating_sub(22),
        length - 1,
    ];
    offsets.sort_unstable();
    offsets.dedup();

    for offset in offsets {
        let path = fixture.dir.join(&format!("truncated-{offset}.tsbp"));
        fs::write(&path, &fixture.bytes[..offset]).expect("write truncated package");
        match load_error(&path) {
            BlueprintError::Zip(_) | BlueprintError::Io(_) => {}
            other => panic!("truncation at {offset} returned {other:?}"),
        }
    }

    assert_eq!(
        fs::read(&fixture.path).expect("seed unchanged"),
        fixture.bytes
    );
}

fn expect_voxel_error(fixture: &Fixture, label: &str, voxels: &[u8], expected: &str) {
    let path = fixture.write_voxels(label, voxels);
    let error = load_error(&path);
    assert!(
        error.to_string().contains(expected),
        "{label}: expected an error containing {expected:?}, got {error:?}"
    );
}

#[test]
fn rejects_rle_and_stream_corruption() {
    let fixture = Fixture::new("rle");

    // A zero-length run is a validation error, not an infinite loop.
    let mut zero_run = header([1, 1, 1], 1, 0);
    push_chunk(
        &mut zero_run,
        &[AIR, 1],
        &[(1, 0), (1, CHUNK_VOLUME as u16)],
    );
    let path = fixture.write_voxels("zero-run", &zero_run);
    match load_error(&path) {
        BlueprintError::Voxel(VoxelError::ZeroLengthRun) => {}
        other => panic!("expected ZeroLengthRun, got {other:?}"),
    }

    // Runs that end before the chunk is full.
    let mut short = header([1, 1, 1], 1, 0);
    push_chunk(&mut short, &[AIR, 1], &[(1, 100)]);
    let path = fixture.write_voxels("short-run", &short);
    match load_error(&path) {
        BlueprintError::Voxel(VoxelError::InvalidRunLength { expected, actual }) => {
            assert_eq!(expected, CHUNK_VOLUME);
            assert_eq!(actual, 100);
        }
        other => panic!("expected a short-stream InvalidRunLength, got {other:?}"),
    }

    // A count that crosses the chunk end is rejected before the slice fill.
    let mut overlong = header([1, 1, 1], 1, 0);
    push_chunk(&mut overlong, &[AIR, 1], &[(1, CHUNK_VOLUME as u16 + 1)]);
    let path = fixture.write_voxels("overlong-run", &overlong);
    match load_error(&path) {
        BlueprintError::Voxel(VoxelError::InvalidRunLength { expected, actual }) => {
            assert_eq!(expected, CHUNK_VOLUME);
            assert_eq!(actual, CHUNK_VOLUME + 1);
        }
        other => panic!("expected an overlong-run InvalidRunLength, got {other:?}"),
    }

    // A palette index outside the palette (an out-of-range material id).
    let mut index = header([1, 1, 1], 1, 0);
    push_chunk(&mut index, &[AIR, 1], &[(2, CHUNK_VOLUME as u16)]);
    let path = fixture.write_voxels("palette-index", &index);
    match load_error(&path) {
        BlueprintError::Voxel(VoxelError::InvalidPaletteIndex { index, palette_len }) => {
            assert_eq!(index, 2);
            assert_eq!(palette_len, 2);
        }
        other => panic!("expected InvalidPaletteIndex, got {other:?}"),
    }

    // Material 0 must be air, otherwise the palette is rejected.
    let mut no_air = header([1, 1, 1], 1, 0);
    push_chunk(&mut no_air, &[1, 2], &[(0, CHUNK_VOLUME as u16)]);
    let path = fixture.write_voxels("palette-air", &no_air);
    match load_error(&path) {
        BlueprintError::Voxel(VoxelError::PaletteMustStartWithAir) => {}
        other => panic!("expected PaletteMustStartWithAir, got {other:?}"),
    }

    // A zero run count is rejected by the stream framing.
    let mut no_runs = header([1, 1, 1], 1, 0);
    push_chunk(&mut no_runs, &[AIR], &[]);
    expect_voxel_error(&fixture, "no-runs", &no_runs, "invalid run count");

    // Stream-level magic, version, and chunk-size corruption.
    let mut bad_magic = header([1, 1, 1], 0, 0);
    bad_magic[0] = b'X';
    let path = fixture.write_voxels("bad-magic", &bad_magic);
    assert_eq!(
        load_error(&path).to_string(),
        "invalid blueprint: bad voxel stream magic"
    );

    let mut bad_version = header([1, 1, 1], 0, 0);
    bad_version[4..6].copy_from_slice(&2u16.to_le_bytes());
    let path = fixture.write_voxels("bad-version", &bad_version);
    match load_error(&path) {
        BlueprintError::UnsupportedVersion(2) => {}
        other => panic!("expected UnsupportedVersion(2), got {other:?}"),
    }

    let mut bad_chunk_size = header([1, 1, 1], 0, 0);
    bad_chunk_size[6..8].copy_from_slice(&16u16.to_le_bytes());
    let path = fixture.write_voxels("bad-chunk-size", &bad_chunk_size);
    assert_eq!(
        load_error(&path).to_string(),
        "invalid blueprint: voxel stream chunk size 16, expected 32"
    );

    // A stream cut between the header and its declared chunk record.
    let path = fixture.write_voxels("stream-truncated", &header([1, 1, 1], 1, 0));
    assert_eq!(
        load_error(&path).to_string(),
        "invalid blueprint: voxel stream is truncated"
    );

    // An all-air chunk is not a legal record.
    let mut empty = header([1, 1, 1], 1, 0);
    push_chunk(&mut empty, &[AIR], &[(0, CHUNK_VOLUME as u16)]);
    let path = fixture.write_voxels("empty-chunk", &empty);
    assert_eq!(
        load_error(&path).to_string(),
        "invalid blueprint: voxel stream contains an empty chunk"
    );

    // A full valid stream with one appended byte is rejected, not ignored.
    let mut trailing = header([1, 1, 1], 1, CHUNK_VOLUME as u64);
    push_chunk(&mut trailing, &[AIR, 1], &[(1, CHUNK_VOLUME as u16)]);
    assert!(core_blueprint::decode_voxels(&trailing).is_ok());
    trailing.push(0);
    let path = fixture.write_voxels("trailing", &trailing);
    assert_eq!(
        load_error(&path).to_string(),
        "invalid blueprint: voxel stream has trailing bytes"
    );
}

#[test]
fn flips_the_content_hash_and_rejects_before_library_insert() {
    let fixture = Fixture::new("content-hash");
    let forged = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
    let json = edited_json(&fixture.json, |value| {
        value["content_hash"] = serde_json::Value::String(forged.to_string());
    });
    let path = fixture.write_package("content-hash", &json, &fixture.voxels);

    match load_error(&path) {
        BlueprintError::HashMismatch { expected, actual } => {
            assert_eq!(expected, forged);
            assert_eq!(actual, GOLDEN_HASH);
        }
        other => panic!("expected HashMismatch, got {other:?}"),
    }

    let root = fixture.dir.join("library");
    fs::create_dir(&root).expect("library root");
    let library = BlueprintLibrary::new(&root).expect("open library");
    match library.insert_file(&path).expect_err("corrupt insert") {
        BlueprintLibraryError::InvalidPackage { source, .. } => {
            assert!(matches!(source, BlueprintError::HashMismatch { .. }));
        }
        other => panic!("expected InvalidPackage, got {other:?}"),
    }
    assert!(library.list().expect("list").is_empty());
    assert!(!library.contains(GOLDEN_HASH));
    assert_eq!(
        fs::read_dir(&root).expect("read library root").count(),
        0,
        "a rejected insert must not create a shard"
    );
}

#[test]
fn dependency_tampering_is_a_hash_mismatch_before_insert() {
    let fixture = Fixture::new("dependency-hash");
    let json = edited_json(&fixture.json, |value| {
        value["dependencies"]["mods"] =
            serde_json::Value::Array(vec![serde_json::Value::String("mymod".to_string())]);
    });
    let path = fixture.write_package("dependency-hash", &json, &fixture.voxels);

    // Dependencies are part of the hashed canonical metadata; there is no
    // separate dependency hash to forge. Editing the list without recomputing
    // the content hash fails as HashMismatch, before MissingDependency.
    match load_error(&path) {
        BlueprintError::HashMismatch { expected, actual } => {
            assert_eq!(expected, GOLDEN_HASH);
            assert_ne!(actual, GOLDEN_HASH);
        }
        other => panic!("expected HashMismatch, got {other:?}"),
    }
    assert!(matches!(
        load_tsbp_with_dependencies(&path, &["mymod"]),
        Err(BlueprintError::HashMismatch { .. })
    ));

    let root = fixture.dir.join("library");
    fs::create_dir(&root).expect("library root");
    let library = BlueprintLibrary::new(&root).expect("open library");
    assert!(matches!(
        library.insert_file(&path),
        Err(BlueprintLibraryError::InvalidPackage {
            source: BlueprintError::HashMismatch { .. },
            ..
        })
    ));
    assert!(library.list().expect("list").is_empty());
}

#[test]
fn rejects_oversized_stream_headers_before_allocation() {
    let fixture = Fixture::new("oversized");

    // A dense 32³ grid is 1.07 billion voxels: rejected from the header.
    let path = fixture.write_voxels("declared-volume", &header([32, 32, 32], 0, 0));
    match load_error(&path) {
        BlueprintError::TooLarge { what, limit } => {
            assert_eq!(what, "declared volume");
            assert_eq!(limit, MAX_VOLUME_VOXELS);
        }
        other => panic!("expected a declared-volume rejection, got {other:?}"),
    }

    // The u128 capacity product still fits, so the same guard catches the
    // maximum representable axes instead of overflowing.
    let path = fixture.write_voxels("huge-axes", &header([u32::MAX, u32::MAX, u32::MAX], 0, 0));
    match load_error(&path) {
        BlueprintError::TooLarge { what, limit } => {
            assert_eq!(what, "declared volume");
            assert_eq!(limit, MAX_VOLUME_VOXELS);
        }
        other => panic!("expected a huge-axis rejection, got {other:?}"),
    }

    // 33 chunks on one axis passes the volume budget but not the axis cap.
    let path = fixture.write_voxels("huge-axis", &header([33, 1, 1], 0, 0));
    assert!(matches!(
        load_error(&path),
        BlueprintError::Format(message) if message.contains("axis")
    ));

    // Declared chunk records over the count cap.
    let too_many_chunks = u32::try_from(MAX_CHUNKS + 1).expect("chunk count fits");
    let path = fixture.write_voxels("too-many-chunks", &header([1, 1, 1], too_many_chunks, 0));
    match load_error(&path) {
        BlueprintError::TooLarge { what, limit } => {
            assert_eq!(what, "chunk count");
            assert_eq!(limit, MAX_CHUNKS as u64);
        }
        other => panic!("expected a chunk-count rejection, got {other:?}"),
    }

    // Declared solid count over the budget.
    let path = fixture.write_voxels(
        "too-many-solids",
        &header([1, 1, 1], 0, MAX_SOLID_VOXELS + 1),
    );
    match load_error(&path) {
        BlueprintError::TooLarge { what, limit } => {
            assert_eq!(what, "solid voxel count");
            assert_eq!(limit, MAX_SOLID_VOXELS);
        }
        other => panic!("expected a solid-count rejection, got {other:?}"),
    }
}

#[test]
fn rejects_oversized_manifest_at_the_entry_cap() {
    let fixture = Fixture::new("metadata-cap");
    let mut oversized = fixture.json.clone();
    oversized.resize(MAX_METADATA_BYTES + 1, b' ');
    let path = fixture.write_package("metadata-cap", &oversized, &fixture.voxels);

    match load_error(&path) {
        BlueprintError::TooLarge { what, limit } => {
            assert_eq!(what, "blueprint.json");
            assert_eq!(limit, MAX_METADATA_BYTES as u64);
        }
        other => panic!("expected a metadata-size rejection, got {other:?}"),
    }
}

fn first_signature(bytes: &[u8], signature: &[u8; 4]) -> Option<usize> {
    bytes.windows(4).position(|window| window == signature)
}

#[test]
fn rejects_non_stored_compression_before_decompression() {
    let fixture = Fixture::new("zip-bomb");

    // v1 is Stored-only. Patch the first entry (blueprint.json) to Deflate in
    // both the local header and the central directory. The reader must reject
    // the method instead of trying to inflate an untrusted ratio.
    let mut bomb = fixture.bytes.clone();
    let local = first_signature(&bomb, b"PK\x03\x04").expect("local header");
    bomb[local + 8..local + 10].copy_from_slice(&8u16.to_le_bytes());
    let central = first_signature(&bomb, b"PK\x01\x02").expect("central header");
    bomb[central + 10..central + 12].copy_from_slice(&8u16.to_le_bytes());
    let path = fixture.dir.join("deflated.tsbp");
    fs::write(&path, &bomb).expect("write deflated package");

    // The zip reader surfaces the method through its `Read` adapter as an
    // `io::Error` of kind `Unsupported`; both that and the direct zip variant
    // are accepted as the same typed rejection.
    match load_error(&path) {
        BlueprintError::Io(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
            assert!(error
                .to_string()
                .contains("compression method not supported: 8"));
        }
        BlueprintError::Zip(zip::result::ZipError::CompressionMethodNotSupported(8)) => {}
        other => panic!("expected a non-Stored method rejection, got {other:?}"),
    }

    // The seed package is untouched and still loads.
    load_tsbp(&fixture.path).expect("seed loads");
}
