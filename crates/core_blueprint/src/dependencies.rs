//! Content-aware dependency checks for `.tsbp` packages.
//!
//! The package loader is deny-all by default: a blueprint that lists any mod
//! or resource pack is rejected because there is no content registry yet.
//! Callers that know which content packs are active (for example the bridge
//! once it passes its live pack ids) can opt into a narrower check with
//! [`validate`].

use crate::metadata::Dependencies;

/// The base game namespace, always present and never reported as missing.
const BASE_ID: &str = "base";

/// Why a declared dependency could not be satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyReason {
    /// A required mod id has no matching active id.
    MissingMod,
    /// A required resource pack has no registry to resolve it.
    MissingResourcePack,
}

impl DependencyReason {
    /// Stable sort rank: mods before resource packs.
    fn sort_key(self) -> u8 {
        match self {
            Self::MissingMod => 0,
            Self::MissingResourcePack => 1,
        }
    }

    /// Stable human-readable label used in error messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingMod => "missing mod",
            Self::MissingResourcePack => "missing resource pack",
        }
    }
}

/// One unsatisfied dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyIssue {
    /// Required content id, trimmed.
    pub id: String,
    /// Why the id could not be satisfied.
    pub reason: DependencyReason,
}

/// Validate the declared dependencies against the active content ids.
///
/// Active IDs are trimmed; empty entries and `base` are ignored. A required
/// mod matches an active id on exact string equality or on namespace equality
/// (`mymod` vs `mymod:pack`; both sides are split at the first `:`). Required
/// mods are trimmed, deduplicated, and empty and `base` entries are dropped.
/// Non-empty `resource_packs` always produce
/// [`DependencyReason::MissingResourcePack`] because the runtime has no
/// resource-pack registry yet.
///
/// Issues are sorted by `(id, reason)` with mods before resource packs for the
/// same id. Returns `Ok(())` when the dependency list is satisfied.
///
/// # Errors
///
/// Returns every unsatisfied dependency, sorted deterministically. The list is
/// never empty on failure.
pub fn validate(
    dependencies: &Dependencies,
    active_ids: &[&str],
) -> Result<(), Vec<DependencyIssue>> {
    let active: Vec<&str> = active_ids.iter().filter_map(|id| normalize(id)).collect();
    let mut issues = Vec::new();

    for required in &dependencies.mods {
        let Some(required) = normalize(required) else {
            continue;
        };
        let matched = active
            .iter()
            .any(|active| *active == required || namespace(active) == namespace(required));
        if !matched {
            issues.push(DependencyIssue {
                id: required.to_string(),
                reason: DependencyReason::MissingMod,
            });
        }
    }

    for required in &dependencies.resource_packs {
        let Some(required) = normalize(required) else {
            continue;
        };
        issues.push(DependencyIssue {
            id: required.to_string(),
            reason: DependencyReason::MissingResourcePack,
        });
    }

    if issues.is_empty() {
        return Ok(());
    }

    issues.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.reason.sort_key().cmp(&right.reason.sort_key()))
    });
    issues.dedup();
    Err(issues)
}

/// Trim an id, dropping empty entries and the always-present `base` namespace.
fn normalize(id: &str) -> Option<&str> {
    let trimmed = id.trim();
    if trimmed.is_empty() || trimmed == BASE_ID {
        None
    } else {
        Some(trimmed)
    }
}

/// Namespace of an id, split at the first `:`.
fn namespace(id: &str) -> &str {
    id.split_once(':').map_or(id, |(namespace, _)| namespace)
}

#[cfg(test)]
mod tests {
    use super::{validate, DependencyIssue, DependencyReason};
    use crate::metadata::Dependencies;

    fn dependencies(mods: &[&str], resource_packs: &[&str]) -> Dependencies {
        Dependencies {
            mods: mods.iter().map(|id| (*id).to_string()).collect(),
            resource_packs: resource_packs.iter().map(|id| (*id).to_string()).collect(),
            ..Dependencies::default()
        }
    }

    fn issue(id: &str, reason: DependencyReason) -> DependencyIssue {
        DependencyIssue {
            id: id.to_string(),
            reason,
        }
    }

    #[test]
    fn empty_dependencies_validate() {
        assert_eq!(validate(&Dependencies::default(), &[]), Ok(()));
        assert_eq!(
            validate(&Dependencies::default(), &["mymod", "base"]),
            Ok(())
        );
    }

    #[test]
    fn exact_active_id_matches() {
        let deps = dependencies(&["mymod"], &[]);
        assert_eq!(validate(&deps, &["mymod"]), Ok(()));
    }

    #[test]
    fn bare_requirement_matches_namespaced_active_id() {
        let deps = dependencies(&["mymod"], &[]);
        assert_eq!(validate(&deps, &["mymod:furniture"]), Ok(()));
    }

    #[test]
    fn namespaced_requirement_matches_bare_active_id() {
        let deps = dependencies(&["mymod:furniture"], &[]);
        assert_eq!(validate(&deps, &["mymod"]), Ok(()));
    }

    #[test]
    fn mismatched_mod_is_reported() {
        let deps = dependencies(&["mymod"], &[]);
        assert_eq!(
            validate(&deps, &["othermod"]),
            Err(vec![issue("mymod", DependencyReason::MissingMod)])
        );
        assert_eq!(
            validate(&deps, &["mymod2:furniture"]),
            Err(vec![issue("mymod", DependencyReason::MissingMod)])
        );
    }

    #[test]
    fn base_is_always_available() {
        assert_eq!(
            validate(&dependencies(&["base"], &[]), &[]),
            Ok(()),
            "a base-only requirement is always satisfied"
        );
        assert_eq!(validate(&dependencies(&[], &[]), &["base"]), Ok(()));
    }

    #[test]
    fn duplicates_and_blanks_collapse() {
        let deps = dependencies(&[" mymod ", "mymod", "", "   ", "base"], &["", "  "]);
        assert_eq!(
            validate(&deps, &[]),
            Err(vec![issue("mymod", DependencyReason::MissingMod)])
        );
    }

    #[test]
    fn issues_are_sorted_by_id_then_reason() {
        let deps = dependencies(&["zeta", "alpha", "same", ""], &["beta", "same"]);
        assert_eq!(
            validate(&deps, &[]),
            Err(vec![
                issue("alpha", DependencyReason::MissingMod),
                issue("beta", DependencyReason::MissingResourcePack),
                issue("same", DependencyReason::MissingMod),
                issue("same", DependencyReason::MissingResourcePack),
                issue("zeta", DependencyReason::MissingMod),
            ])
        );
    }

    #[test]
    fn resource_packs_fail_closed_even_when_active() {
        let deps = dependencies(&[], &["terrain-pack"]);
        assert_eq!(
            validate(&deps, &["terrain-pack"]),
            Err(vec![issue(
                "terrain-pack",
                DependencyReason::MissingResourcePack
            )])
        );
    }

    #[test]
    fn active_ids_are_trimmed() {
        let deps = dependencies(&["mymod"], &[]);
        assert_eq!(validate(&deps, &[" mymod "]), Ok(()));
    }
}
