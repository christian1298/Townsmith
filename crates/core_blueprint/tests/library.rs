use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use core_blueprint::{
    load_tsbp, save_tsbp, save_tsbp_with_hash, Blueprint, BlueprintLibrary, BlueprintLibraryError,
    BlueprintMetadata, MAX_PACKAGE_BYTES,
};
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};

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
            "townsmith-blueprint-library-{}-{stamp}-{unique}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
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

fn test_blueprint(id: &str) -> Blueprint {
    let mut metadata = BlueprintMetadata::new(id, "Library Test House");
    metadata.tags.push("test".to_string());
    Blueprint::new(metadata, test_volume())
}

/// The fixture used by the committed golden hash in `docs/28`.
fn golden_blueprint() -> Blueprint {
    let mut metadata = BlueprintMetadata::new("local:test", "Test House");
    metadata.tags.push("test".to_string());
    Blueprint::new(metadata, test_volume())
}

fn save_package(dir: &Path, name: &str, id: &str) -> (PathBuf, String) {
    let path = dir.join(name);
    let hash = save_tsbp_with_hash(&path, &test_blueprint(id)).expect("save package");
    (path, hash)
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).expect("zip file");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, data) in entries {
        zip.start_file(*name, options).expect("zip entry");
        zip.write_all(data).expect("zip body");
    }
    zip.finish().expect("zip finish");
}

fn read_entry(path: &Path, name: &str) -> Vec<u8> {
    let file = fs::File::open(path).expect("package");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    let mut entry = archive.by_name(name).expect("entry");
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes).expect("read entry");
    bytes
}

/// Write the same validated payload with an extra thumbnail. The content hash
/// ignores the thumbnail, so this is a different byte stream under the same
/// address.
fn write_same_hash_different_bytes(source: &Path, target: &Path) {
    let json = read_entry(source, "blueprint.json");
    let voxels = read_entry(source, "voxels.bin");
    write_zip(
        target,
        &[
            ("blueprint.json", json.as_slice()),
            ("voxels.bin", voxels.as_slice()),
            ("thumbnail.png", b"not really a png"),
        ],
    );
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).expect("read directory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn insert_valid_package_resolves_lists_and_summarizes() {
    let dir = TempDir::new("insert");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, hash) = save_package(dir.path(), "house.tsbp", "local:house");

    let library = BlueprintLibrary::new(&library_root).expect("library");
    let insert = library.insert_file(&package).expect("insert");
    assert!(insert.inserted);
    assert_eq!(insert.hash, hash);
    let hex = hash.strip_prefix("blake3:").expect("prefix");
    assert_eq!(
        insert.path,
        fs::canonicalize(library_root.join(&hex[..2]).join(format!("{hex}.tsbp")))
            .expect("canonical stored entry")
    );
    assert!(insert.path.is_file());

    assert_eq!(library.resolve(&hash), Some(insert.path.clone()));
    assert!(library.contains(&hash));
    let entries = library.list().expect("list");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hash, hash);
    assert_eq!(entries[0].path, insert.path);
    assert_eq!(entries[0].bytes, fs::metadata(&insert.path).unwrap().len());

    let summary = library.summary().expect("summary");
    assert_eq!(summary.count, 1);
    assert_eq!(summary.total_bytes, entries[0].bytes);
}

#[test]
fn duplicate_insert_is_a_no_op() {
    let dir = TempDir::new("duplicate");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, hash) = save_package(dir.path(), "house.tsbp", "local:house");
    let library = BlueprintLibrary::new(&library_root).expect("library");

    let first = library.insert_file(&package).expect("first insert");
    let second = library.insert_file(&package).expect("second insert");
    assert!(first.inserted);
    assert!(!second.inserted);
    assert_eq!(first.hash, second.hash);
    assert_eq!(first.path, second.path);
    assert_eq!(library.summary().expect("summary").count, 1);
    assert_eq!(library.list().expect("list").len(), 1);
    assert_eq!(library.resolve(&hash), Some(first.path));
}

#[test]
fn two_packages_are_stored_sorted_with_totals() {
    let dir = TempDir::new("two");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let first = save_package(dir.path(), "first.tsbp", "local:first");
    let second = save_package(dir.path(), "second.tsbp", "local:second");
    let library = BlueprintLibrary::new(&library_root).expect("library");

    library.insert_file(&second.0).expect("second");
    library.insert_file(&first.0).expect("first");

    let entries = library.list().expect("list");
    assert_eq!(entries.len(), 2);
    assert!(entries[0].hash < entries[1].hash);
    let summary = library.summary().expect("summary");
    assert_eq!(summary.count, 2);
    assert_eq!(
        summary.total_bytes,
        entries.iter().map(|entry| entry.bytes).sum::<u64>()
    );
    assert!(library.contains(&first.1));
    assert!(library.contains(&second.1));
}

#[test]
fn summary_checksum_is_stable_across_insertion_order() {
    let dir = TempDir::new("order");
    let first = save_package(dir.path(), "first.tsbp", "local:first");
    let second = save_package(dir.path(), "second.tsbp", "local:second");

    let root_a = dir.join("a");
    let root_b = dir.join("b");
    fs::create_dir(&root_a).expect("root a");
    fs::create_dir(&root_b).expect("root b");
    let library_a = BlueprintLibrary::new(&root_a).expect("library a");
    let library_b = BlueprintLibrary::new(&root_b).expect("library b");
    library_a.insert_file(&first.0).expect("a first");
    library_a.insert_file(&second.0).expect("a second");
    library_b.insert_file(&second.0).expect("b second");
    library_b.insert_file(&first.0).expect("b first");

    let summary_a = library_a.summary().expect("summary a");
    let summary_b = library_b.summary().expect("summary b");
    assert_eq!(summary_a, summary_b);
    assert_eq!(summary_a.count, 2);
    let entries_a = library_a.list().expect("list a");
    let entries_b = library_b.list().expect("list b");
    assert_eq!(
        entries_a
            .iter()
            .map(|entry| (&entry.hash, entry.bytes))
            .collect::<Vec<_>>(),
        entries_b
            .iter()
            .map(|entry| (&entry.hash, entry.bytes))
            .collect::<Vec<_>>()
    );
}

#[test]
fn same_hash_with_different_bytes_is_a_collision() {
    let dir = TempDir::new("collision");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, hash) = save_package(dir.path(), "house.tsbp", "local:house");
    let conflicting = dir.join("conflicting.tsbp");
    write_same_hash_different_bytes(&package, &conflicting);
    assert_eq!(
        load_tsbp(&conflicting)
            .expect("conflicting package is valid")
            .metadata
            .content_hash,
        hash
    );
    let library = BlueprintLibrary::new(&library_root).expect("library");
    let first = library.insert_file(&package).expect("insert");
    let before = library.summary().expect("before");

    let error = library.insert_file(&conflicting).expect_err("collision");
    assert!(matches!(error, BlueprintLibraryError::Collision { .. }));
    assert_eq!(library.summary().expect("after"), before);
    assert_eq!(library.resolve(&hash), Some(first.path.clone()));
    assert_eq!(before.count, 1);
    let stored: Vec<PathBuf> = files_under(&library_root)
        .into_iter()
        .map(|path| fs::canonicalize(path).expect("canonical stored entry"))
        .collect();
    assert_eq!(stored, vec![first.path]);
}

#[test]
fn invalid_package_is_rejected_and_leaves_the_library_unchanged() {
    let dir = TempDir::new("invalid");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let bad = dir.join("bad.tsbp");
    fs::write(&bad, b"this is not a zip").expect("bad package");
    let library = BlueprintLibrary::new(&library_root).expect("library");

    let error = library.insert_file(&bad).expect_err("invalid package");
    assert!(matches!(
        error,
        BlueprintLibraryError::InvalidPackage { .. }
    ));
    assert_eq!(library.summary().expect("summary").count, 0);
    assert!(files_under(&library_root).is_empty());
}

#[test]
fn oversized_package_is_rejected_before_reading() {
    let dir = TempDir::new("oversized");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let oversized = dir.join("oversized.tsbp");
    let file = fs::File::create(&oversized).expect("oversized file");
    file.set_len(MAX_PACKAGE_BYTES + 1).expect("set length");
    drop(file);
    let library = BlueprintLibrary::new(&library_root).expect("library");

    let error = library.insert_file(&oversized).expect_err("too large");
    match error {
        BlueprintLibraryError::TooLarge { bytes, limit, .. } => {
            assert_eq!(bytes, MAX_PACKAGE_BYTES + 1);
            assert_eq!(limit, MAX_PACKAGE_BYTES);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
    assert_eq!(library.summary().expect("summary").count, 0);
    assert!(files_under(&library_root).is_empty());
}

#[test]
fn non_tsbp_source_is_rejected() {
    let dir = TempDir::new("extension");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, _) = save_package(dir.path(), "house.tsbp", "local:house");
    let renamed = dir.join("house.zip");
    fs::rename(&package, &renamed).expect("rename");
    let library = BlueprintLibrary::new(&library_root).expect("library");

    let error = library.insert_file(&renamed).expect_err("extension");
    assert!(matches!(error, BlueprintLibraryError::InvalidPath { .. }));
    assert_eq!(library.summary().expect("summary").count, 0);
    assert!(files_under(&library_root).is_empty());
}

#[test]
fn invalid_hashes_are_rejected() {
    let dir = TempDir::new("hashes");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, hash) = save_package(dir.path(), "house.tsbp", "local:house");
    let library = BlueprintLibrary::new(&library_root).expect("library");
    library.insert_file(&package).expect("insert");
    let hex = hash.strip_prefix("blake3:").expect("prefix");
    let uppercase = format!("blake3:{}", hex.to_uppercase());
    let short = format!("blake3:{}", &hex[..63]);
    let long = format!("blake3:{hex}0");
    let traversal = format!("blake3:../{hex}");
    let nested = format!("blake3:{hex}/../x");
    for bad in [
        "",
        "blake3:",
        "   ",
        "house.tsbp",
        "../house.tsbp",
        "/absolute/house.tsbp",
        hex,
        uppercase.as_str(),
        short.as_str(),
        long.as_str(),
        traversal.as_str(),
        nested.as_str(),
    ] {
        assert!(
            BlueprintLibrary::validate_hash(bad).is_err(),
            "hash {bad:?} must be rejected"
        );
        assert!(!library.contains(bad), "hash {bad:?} must not resolve");
        assert!(
            matches!(
                library.resolve_checked(bad),
                Err(BlueprintLibraryError::InvalidHash { .. })
            ),
            "hash {bad:?} must report an invalid hash"
        );
    }
    assert!(library.contains(&hash));
}

#[test]
fn hash_inputs_cannot_traverse_or_address_absolute_paths() {
    let dir = TempDir::new("hash-traversal");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let outside_root = dir.join("outside");
    fs::create_dir(&outside_root).expect("outside root");
    let (outside_package, hash) = save_package(&outside_root, "outside.tsbp", "local:outside");
    let hex = hash.strip_prefix("blake3:").expect("prefix");
    let outside_path = outside_package.to_string_lossy();
    let library = BlueprintLibrary::new(&library_root).expect("library");

    for bad in [
        format!("../{hex}"),
        format!("blake3:../{hex}"),
        format!("blake3:{hex}/../../outside.tsbp"),
        outside_path.to_string(),
    ] {
        assert!(library.resolve_checked(&bad).is_err());
        assert!(library.resolve(&bad).is_none());
    }
    // The outside package exists, so a successful traversal would have found it.
    assert!(outside_package.is_file());
}

#[test]
fn missing_root_and_missing_file_are_errors() {
    let dir = TempDir::new("missing");
    let missing_root = dir.join("absent");
    assert!(matches!(
        BlueprintLibrary::new(&missing_root),
        Err(BlueprintLibraryError::RootMissing { .. })
    ));

    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let library = BlueprintLibrary::new(&library_root).expect("library");
    let missing_file = dir.join("absent.tsbp");
    assert!(matches!(
        library.insert_file(&missing_file),
        Err(BlueprintLibraryError::MissingFile { .. })
    ));
    assert_eq!(library.summary().expect("summary").count, 0);
}

#[test]
fn root_must_be_an_existing_absolute_directory() {
    let dir = TempDir::new("roots");
    assert!(matches!(
        BlueprintLibrary::new("relative-library"),
        Err(BlueprintLibraryError::InvalidRoot { .. })
    ));
    assert!(matches!(
        BlueprintLibrary::new(PathBuf::new()),
        Err(BlueprintLibraryError::InvalidRoot { .. })
    ));
    let traversing = dir.path().join("one").join("..").join("two");
    assert!(matches!(
        BlueprintLibrary::new(&traversing),
        Err(BlueprintLibraryError::InvalidRoot { .. })
    ));
    let file = dir.join("file");
    fs::write(&file, b"not a directory").expect("file");
    assert!(matches!(
        BlueprintLibrary::new(&file),
        Err(BlueprintLibraryError::RootNotDirectory { .. })
    ));
}

#[test]
fn relative_and_traversing_source_paths_are_rejected() {
    let dir = TempDir::new("source-paths");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, _) = save_package(dir.path(), "house.tsbp", "local:house");
    let library = BlueprintLibrary::new(&library_root).expect("library");

    assert!(matches!(
        library.insert_file(Path::new("house.tsbp")),
        Err(BlueprintLibraryError::InvalidPath { .. })
    ));
    let traversing = dir.path().join("one").join("..").join("house.tsbp");
    assert!(matches!(
        library.insert_file(&traversing),
        Err(BlueprintLibraryError::InvalidPath { .. })
    ));
    assert!(package.is_file());
}

#[test]
fn source_directory_and_broken_link_are_rejected() {
    let dir = TempDir::new("unreadable");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let directory = dir.join("directory.tsbp");
    fs::create_dir(&directory).expect("directory");
    let library = BlueprintLibrary::new(&library_root).expect("library");
    assert!(matches!(
        library.insert_file(&directory),
        Err(BlueprintLibraryError::NotAFile { .. })
    ));

    let broken = dir.join("broken.tsbp");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("absent-target"), &broken).expect("broken symlink");
        assert!(matches!(
            library.insert_file(&broken),
            Err(BlueprintLibraryError::MissingFile { .. })
        ));
    }
    #[cfg(not(unix))]
    {
        let _ = &broken;
    }
    assert_eq!(library.summary().expect("summary").count, 0);
}

#[test]
fn insert_leaves_no_temporary_file() {
    let dir = TempDir::new("temp");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, _) = save_package(dir.path(), "house.tsbp", "local:house");
    let library = BlueprintLibrary::new(&library_root).expect("library");
    library.insert_file(&package).expect("insert");

    for file in files_under(&library_root) {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        assert!(
            !name.ends_with(".tmp"),
            "temporary file left behind: {file:?}"
        );
        assert!(
            !name.starts_with(".insert-"),
            "temporary file left behind: {file:?}"
        );
    }
}

#[test]
fn list_ignores_unrelated_files_and_directories() {
    let dir = TempDir::new("junk");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, hash) = save_package(dir.path(), "house.tsbp", "local:house");
    let library = BlueprintLibrary::new(&library_root).expect("library");
    let insert = library.insert_file(&package).expect("insert");

    fs::write(library_root.join("notes.txt"), b"junk").expect("junk file");
    fs::create_dir(library_root.join("zz")).expect("junk dir");
    fs::write(library_root.join("zz").join("other.txt"), b"junk").expect("junk file");
    fs::write(insert.path.with_file_name("notes.txt"), b"junk").expect("shard junk");
    let entries = library.list().expect("list");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hash, hash);
    assert_eq!(library.summary().expect("summary").count, 1);
}

#[cfg(unix)]
#[test]
fn symlinked_shard_escape_is_rejected() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new("shard-symlink");
    let library_root = dir.join("library");
    let outside = dir.join("outside");
    fs::create_dir(&library_root).expect("library root");
    fs::create_dir(&outside).expect("outside");
    let (package, hash) = save_package(dir.path(), "house.tsbp", "local:house");
    let hex = hash.strip_prefix("blake3:").expect("prefix");
    let shard = &hex[..2];
    symlink(&outside, library_root.join(shard)).expect("shard symlink");
    let library = BlueprintLibrary::new(&library_root).expect("library");

    let error = library.insert_file(&package).expect_err("shard escape");
    assert!(matches!(error, BlueprintLibraryError::PathEscape { .. }));
    assert!(!outside.join(format!("{hex}.tsbp")).exists());
    assert!(library.resolve(&hash).is_none());
    assert_eq!(library.summary().expect("summary").count, 0);
}

#[cfg(unix)]
#[test]
fn symlinked_entry_escape_is_rejected() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new("entry-symlink");
    let library_root = dir.join("library");
    let outside = dir.join("outside");
    fs::create_dir(&library_root).expect("library root");
    fs::create_dir(&outside).expect("outside");
    let (package, hash) = save_package(dir.path(), "house.tsbp", "local:house");
    let hex = hash.strip_prefix("blake3:").expect("prefix");
    let shard = library_root.join(&hex[..2]);
    fs::create_dir(&shard).expect("shard");
    let outside_package = outside.join("outside.tsbp");
    fs::copy(&package, &outside_package).expect("copy outside");
    symlink(&outside_package, shard.join(format!("{hex}.tsbp"))).expect("entry symlink");
    let library = BlueprintLibrary::new(&library_root).expect("library");

    assert!(matches!(
        library.resolve_checked(&hash),
        Err(BlueprintLibraryError::PathEscape { .. })
    ));
    assert!(!library.contains(&hash));
    assert_eq!(library.list().expect("list").len(), 0);
}

#[cfg(unix)]
#[test]
fn root_symlink_is_canonicalized() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new("root-symlink");
    let real = dir.join("real");
    fs::create_dir(&real).expect("real root");
    let link = dir.join("link");
    symlink(&real, &link).expect("root symlink");
    let library = BlueprintLibrary::new(&link).expect("library");
    assert_eq!(
        library.root(),
        fs::canonicalize(&real).expect("canonical root")
    );
}

#[test]
fn existing_package_hashes_are_unchanged() {
    let dir = TempDir::new("hash-compat");
    let package = dir.join("golden.tsbp");
    let blueprint = golden_blueprint();
    let saved_hash = save_tsbp_with_hash(&package, &blueprint).expect("save");
    assert_eq!(saved_hash, GOLDEN_HASH);
    let loaded = load_tsbp(&package).expect("load");
    assert_eq!(loaded.metadata.content_hash, GOLDEN_HASH);

    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let library = BlueprintLibrary::new(&library_root).expect("library");
    let insert = library.insert_file(&package).expect("insert");
    assert_eq!(insert.hash, GOLDEN_HASH);
    // The source package is still exactly what save_tsbp wrote.
    let reloaded = load_tsbp(&package).expect("reload save");
    assert_eq!(reloaded.metadata.content_hash, GOLDEN_HASH);
    let stored = library.resolve(GOLDEN_HASH).expect("resolve");
    assert_eq!(
        load_tsbp(&stored)
            .expect("load stored")
            .metadata
            .content_hash,
        GOLDEN_HASH
    );
}

#[test]
fn stored_bytes_are_identical_to_the_source() {
    let dir = TempDir::new("bytes");
    let library_root = dir.join("library");
    fs::create_dir(&library_root).expect("library root");
    let (package, _) = save_package(dir.path(), "house.tsbp", "local:house");
    let library = BlueprintLibrary::new(&library_root).expect("library");
    let insert = library.insert_file(&package).expect("insert");
    assert_eq!(
        fs::read(&insert.path).expect("stored bytes"),
        fs::read(&package).expect("source bytes")
    );
    // The library never consumes or rewrites the source.
    assert!(package.is_file());
    assert!(save_tsbp(&dir.join("another.tsbp"), &test_blueprint("local:another")).is_ok());
}
