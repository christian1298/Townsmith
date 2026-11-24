//! Read-only blueprint-library enumeration for the Godot placement UI.
//!
//! The existing library API can summarize or resolve one known hash, but the
//! placement UI needs the catalog to cycle blueprints and size a ghost. This
//! module only reads metadata from already-stored packages; it never inserts,
//! rewrites, downloads, or mutates a library or world.

use core_blueprint::{load_tsbp, BlueprintLibrary, BlueprintLibraryError, LibrarySummary};
use godot::prelude::*;

use super::{
    blueprint_library_error_pair, blueprint_library_open, blueprint_library_summary_dictionary,
};

/// One validated library entry, in Rust form so it can be tested without Godot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacementLibraryEntry {
    /// Canonical `blake3:<hex>` content hash.
    pub hash: String,
    /// Package id, for example `base:cottage`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Stored package size in bytes.
    pub bytes: u64,
    /// Bounds in metres `[x, y, z]`.
    pub bounds_m: [f32; 3],
    /// Ground footprint `[x, z]` in metres, before rotation.
    pub footprint_m: [f32; 2],
}

/// Catalog plus summary and skipped-entry count.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacementLibraryCatalog {
    /// Library totals.
    pub summary: LibrarySummary,
    /// Valid entries, sorted by hash.
    pub entries: Vec<PlacementLibraryEntry>,
    /// Packages that existed but could not be read or validated.
    pub skipped: usize,
}

/// Enumerate every stored package and load its metadata.
///
/// Entries are sorted by canonical hash. A package whose bytes fail the
/// existing loader, whose stored hash disagrees with its metadata, or whose
/// bounds are missing/non-finite/non-positive is skipped rather than failing
/// the whole catalog. The library itself is opened and summarized by the
/// caller, so root validation and stable error codes stay in one place.
pub(crate) fn collect_catalog(
    library: &BlueprintLibrary,
) -> Result<PlacementLibraryCatalog, BlueprintLibraryError> {
    let summary = library.summary()?;
    let mut entries = library.list()?;
    entries.sort_by(|left, right| left.hash.cmp(&right.hash));

    let mut catalog = Vec::with_capacity(entries.len());
    let mut skipped = 0usize;
    for entry in entries {
        let package = match load_tsbp(&entry.path) {
            Ok(package) => package,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if package.metadata.content_hash != entry.hash {
            skipped += 1;
            continue;
        }
        let bounds = package.metadata.bounds_m;
        if bounds
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            skipped += 1;
            continue;
        }
        catalog.push(PlacementLibraryEntry {
            hash: entry.hash,
            id: package.metadata.id,
            name: package.metadata.name,
            bytes: entry.bytes,
            bounds_m: bounds,
            footprint_m: [bounds[0], bounds[2]],
        });
    }

    Ok(PlacementLibraryCatalog {
        summary,
        entries: catalog,
        skipped,
    })
}

/// Build the GDScript dictionary for a validated library root.
pub(crate) fn blueprint_library_entries_from_arg(
    raw_root: &str,
) -> Result<VarDictionary, (&'static str, String)> {
    let library = blueprint_library_open(raw_root)?;
    let catalog =
        collect_catalog(&library).map_err(|error| blueprint_library_error_pair(&error))?;

    let mut array = VarArray::new();
    for entry in &catalog.entries {
        let mut item = VarDictionary::new();
        item.set("hash", &GString::from(entry.hash.as_str()));
        item.set("id", &GString::from(entry.id.as_str()));
        item.set("name", &GString::from(entry.name.as_str()));
        item.set("bytes", i64::try_from(entry.bytes).unwrap_or(i64::MAX));
        let mut bounds = VarArray::new();
        for value in entry.bounds_m {
            bounds.push(value);
        }
        item.set("bounds_m", &bounds);
        let mut footprint = VarArray::new();
        for value in entry.footprint_m {
            footprint.push(value);
        }
        item.set("footprint_m", &footprint);
        array.push(&item);
    }

    let mut result = blueprint_library_summary_dictionary(&catalog.summary);
    result.set(
        "entry_count",
        i64::try_from(catalog.entries.len()).unwrap_or(i64::MAX),
    );
    result.set(
        "skipped",
        i64::try_from(catalog.skipped).unwrap_or(i64::MAX),
    );
    result.set("entries", &array);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_blueprint::{save_tsbp_with_hash, Blueprint, BlueprintMetadata};
    use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "townsmith-placement-ui-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_package(path: &Path, id: &str, width: u32) {
        let mut volume =
            VoxelVolume::new(VolumeConfig::new(1, 1, 1).expect("config")).expect("volume");
        volume
            .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(width, 1, 3), 1)
            .expect("fill box");
        let blueprint = Blueprint::new(BlueprintMetadata::new(id, "UI House"), volume);
        save_tsbp_with_hash(path, &blueprint).expect("save package");
    }

    #[test]
    fn catalog_is_sorted_and_skips_broken_packages() {
        let temp = TempDir::new();
        let root = temp.path().join("library");
        fs::create_dir_all(&root).expect("root");
        let library = BlueprintLibrary::new(&root).expect("library");

        write_package(&temp.path().join("one.tsbp"), "base:one", 4);
        write_package(&temp.path().join("two.tsbp"), "base:two", 6);
        library
            .insert_file(&temp.path().join("one.tsbp"))
            .expect("insert one");
        library
            .insert_file(&temp.path().join("two.tsbp"))
            .expect("insert two");

        // A corrupt shard is skipped, not surfaced as a catalog failure.
        let broken_shard = root.join("ff");
        fs::create_dir_all(&broken_shard).expect("broken shard");
        let broken_name = format!("{}.tsbp", "f".repeat(64));
        fs::write(broken_shard.join(broken_name), b"not a package").expect("broken package");

        let catalog = collect_catalog(&library).expect("catalog");
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.skipped, 1);
        assert!(catalog
            .entries
            .windows(2)
            .all(|pair| pair[0].hash < pair[1].hash));
        assert!(catalog
            .entries
            .iter()
            .any(|entry| entry.id == "base:one" && entry.footprint_m == [3.2, 3.2]));
        assert!(catalog.entries.iter().any(|entry| entry.id == "base:two"));
    }

    #[test]
    fn catalog_reports_library_summary() {
        let temp = TempDir::new();
        let root = temp.path().join("library");
        fs::create_dir_all(&root).expect("root");
        let library = BlueprintLibrary::new(&root).expect("library");
        write_package(&temp.path().join("one.tsbp"), "base:one", 4);
        library
            .insert_file(&temp.path().join("one.tsbp"))
            .expect("insert");

        let catalog = collect_catalog(&library).expect("catalog");
        assert_eq!(catalog.summary.count, 1);
        assert!(catalog.summary.total_bytes > 0);
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(
            catalog.entries[0].hash,
            catalog.entries[0].hash.to_lowercase()
        );
    }
}
