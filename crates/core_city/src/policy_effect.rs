//! Derived, deterministic district-policy effects (data only).
//!
//! [`DistrictPolicyStore::effect_at`] resolves the policy that contains a world
//! point and turns its stored multipliers, flags, and allowlist into an
//! immutable [`DistrictPolicyEffect`]. The helpers in this module are pure
//! functions over registry data:
//!
//! - no simulation, cohort, household, traffic, construction, treasury, or
//!   economy code reads an effect yet;
//! - no UI or bridge consumer displays or edits one;
//! - effects are never persisted. They add no field to [`DistrictPolicy`] or
//!   [`DistrictPolicyStore`] and do not change the world-save envelope or its
//!   checksum.
//!
//! # Resolution
//!
//! [`DistrictPolicyStore::effective_policy`] delegates to
//! [`DistrictPolicyStore::policy_at`], so a point inside overlapping circles
//! uses the **smallest policy id** — not the nearest centre and not the
//! smallest circle. A point outside every circle, or a point with a non-finite
//! coordinate, has no effect.
//!
//! # Rounding
//!
//! The tax helpers multiply in `f64` and never round, floor, ceil, quantize, or
//! format the result. A caller that needs whole currency units picks the
//! rounding point (per household, per tick, or per game day) so a simulation
//! can keep exact intermediate values.
//!
//! # Bounds
//!
//! Results are always finite and non-negative. A base rate is clamped into
//! `0..=100` per game day, matching the `Economy::set_taxes` clamp, and a
//! non-finite base becomes `0.0`. A non-finite multiplier is treated as the
//! neutral factor `1.0`; a negative multiplier yields `0.0`. A validated
//! registry can only store multipliers in `0.25..=4.0`, so these guards matter
//! for direct helper calls and hand-built test data, not for store records.

use std::collections::BTreeSet;

use crate::policy::{DistrictPolicy, DistrictPolicyStore};

/// Largest base rate the effect helpers accept, per game day.
///
/// This mirrors the `0..=100` clamp in `Economy::set_taxes`; callers do not
/// need an economy instance to use the effect layer.
pub const MAX_POLICY_EFFECT_BASE_RATE: f64 = 100.0;

/// One resolved district-policy effect for a query point.
///
/// The effect owns copies of the policy's name and allowlist, so it stays
/// valid while the registry mutates. It is derived data: constructing it never
/// writes the registry, the economy, or a save.
#[derive(Debug, Clone, PartialEq)]
pub struct DistrictPolicyEffect {
    /// Id of the policy that produced this effect.
    pub policy_id: u64,
    /// Policy name at resolution time.
    pub name: String,
    /// Tax multiplier copied from the policy, `0.25..=4.0`.
    pub tax_multiplier: f32,
    /// Service-funding multiplier copied from the policy, `0.0..=2.0`.
    pub service_funding: f32,
    /// Whether construction is nominally allowed at the query point.
    pub construction_allowed: bool,
    /// Blueprint ids allowed at the query point. Empty means unrestricted.
    pub blueprint_allowlist: BTreeSet<String>,
    /// Resident base rate after clamping, before the tax multiplier.
    pub base_resident_rate: f64,
    /// Business base rate after clamping, before the tax multiplier.
    pub base_business_rate: f64,
    /// `base_resident_rate * tax_multiplier`, unrounded, per game day.
    pub resident_tax_rate_per_day: f64,
    /// `base_business_rate * tax_multiplier`, unrounded, per game day.
    pub business_tax_rate_per_day: f64,
}

impl DistrictPolicyEffect {
    /// Resolve `policy` into an effect for one pair of base rates.
    ///
    /// Both base rates are clamped with [`clamp_base_rate`] before the tax
    /// multiplier is applied. The stored allowlist is copied; the policy is
    /// not modified.
    #[must_use]
    pub fn from_policy(policy: &DistrictPolicy, base_resident: f64, base_business: f64) -> Self {
        let base_resident_rate = clamp_base_rate(base_resident);
        let base_business_rate = clamp_base_rate(base_business);
        Self {
            policy_id: policy.id,
            name: policy.name.clone(),
            tax_multiplier: policy.tax_multiplier,
            service_funding: policy.service_funding,
            construction_allowed: policy.construction_allowed,
            blueprint_allowlist: policy.blueprint_allowlist.clone(),
            base_resident_rate,
            base_business_rate,
            resident_tax_rate_per_day: apply_multiplier(base_resident_rate, policy.tax_multiplier),
            business_tax_rate_per_day: apply_multiplier(base_business_rate, policy.tax_multiplier),
        }
    }

    /// Apply this district's tax multiplier to a resident base rate.
    ///
    /// The rate is clamped and scaled exactly as in [`Self::from_policy`], so a
    /// caller can price an individual household without rebuilding the effect.
    /// The result is finite and non-negative and is not rounded.
    #[must_use]
    pub fn resident_tax_rate(&self, base_rate: f64) -> f64 {
        apply_multiplier(base_rate, self.tax_multiplier)
    }

    /// Apply this district's tax multiplier to a business base rate.
    ///
    /// The result is finite and non-negative and is not rounded.
    #[must_use]
    pub fn business_tax_rate(&self, base_rate: f64) -> f64 {
        apply_multiplier(base_rate, self.tax_multiplier)
    }

    /// Whether `blueprint_id` may be placed at the query point.
    ///
    /// An empty allowlist is unrestricted and allows every id. A non-empty
    /// allowlist allows only its exact, case-sensitive entries. This is a pure
    /// query: the effect layer does not enforce the result in construction,
    /// placement, or the blueprint package.
    #[must_use]
    pub fn blueprint_allowed(&self, blueprint_id: &str) -> bool {
        self.blueprint_allowlist.is_empty() || self.blueprint_allowlist.contains(blueprint_id)
    }
}

/// Clamp a per-day base rate into `0..=100`, mapping non-finite values to `0`.
///
/// The bound matches `Economy::set_taxes`, so callers can pass a treasury rate
/// directly without consulting the economy crate. The clamp is total: every
/// `f64` maps to a finite, non-negative value.
#[must_use]
pub fn clamp_base_rate(base_rate: f64) -> f64 {
    if base_rate.is_finite() {
        base_rate.clamp(0.0, MAX_POLICY_EFFECT_BASE_RATE)
    } else {
        0.0
    }
}

/// Apply a district multiplier to a base rate without rounding.
///
/// The base is clamped with [`clamp_base_rate`]. A finite non-negative
/// multiplier scales it directly; a negative multiplier yields `0.0`; a
/// non-finite multiplier is treated as the neutral factor `1.0`. The result is
/// always finite and non-negative, and no rounding is performed.
#[must_use]
pub fn apply_multiplier(base_rate: f64, multiplier: f32) -> f64 {
    let base = clamp_base_rate(base_rate);
    let factor = if multiplier.is_finite() {
        f64::from(multiplier).max(0.0)
    } else {
        1.0
    };
    base * factor
}

impl DistrictPolicyStore {
    /// The policy whose effect applies at `point`.
    ///
    /// This is the effect-layer name for [`DistrictPolicyStore::policy_at`] and
    /// keeps the same rule: the smallest policy id whose inclusive circle
    /// contains the point wins. A non-finite point returns `None`.
    #[must_use]
    pub fn effective_policy(&self, point: [f32; 2]) -> Option<&DistrictPolicy> {
        self.policy_at(point)
    }

    /// Resolve the deterministic effect at `point`, if any.
    ///
    /// Returns `None` when the point is outside every policy circle or is not
    /// finite. Otherwise the smallest containing policy id wins and its stored
    /// multipliers, construction flag, and allowlist are copied into a
    /// [`DistrictPolicyEffect`] with both base rates clamped and scaled.
    ///
    /// This is a read-only query: it does not mutate the registry, the economy,
    /// a construction site, or a save.
    #[must_use]
    pub fn effect_at(
        &self,
        point: [f32; 2],
        base_resident: f64,
        base_business: f64,
    ) -> Option<DistrictPolicyEffect> {
        let policy = self.effective_policy(point)?;
        Some(DistrictPolicyEffect::from_policy(
            policy,
            base_resident,
            base_business,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{DistrictPolicyRequest, DistrictPolicyStore};

    fn request(name: &str, x: f32, z: f32, radius_m: f32) -> DistrictPolicyRequest {
        DistrictPolicyRequest {
            name: name.to_string(),
            center: [x, z],
            radius_m,
            tax_multiplier: 1.0,
            service_funding: 1.0,
            construction_allowed: true,
            blueprint_allowlist: BTreeSet::new(),
        }
    }

    fn store_with_one() -> (DistrictPolicyStore, DistrictPolicy) {
        let mut store = DistrictPolicyStore::new(2026);
        let policy = store
            .create(&request("Riverside", 0.0, 0.0, 100.0))
            .expect("create");
        (store, policy)
    }

    #[test]
    fn no_policy_has_no_effect() {
        let store = DistrictPolicyStore::new(2026);
        assert!(store.effective_policy([0.0, 0.0]).is_none());
        assert!(store.effect_at([0.0, 0.0], 8.0, 4.0).is_none());
        assert!(store.effective_policy([f32::NAN, 0.0]).is_none());
        assert!(store.effect_at([0.0, f32::INFINITY], 8.0, 4.0).is_none());
    }

    #[test]
    fn single_policy_effect_copies_fields_and_scales_rates() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = request("Riverside", 10.0, -10.0, 64.0);
        request.tax_multiplier = 2.0;
        request.service_funding = 0.5;
        request.construction_allowed = false;
        request
            .blueprint_allowlist
            .insert("base:cottage".to_string());
        let policy = store.create(&request).expect("create");

        let effect = store
            .effect_at([10.0, -10.0], 8.0, 4.0)
            .expect("inside the circle");
        assert_eq!(effect.policy_id, policy.id);
        assert_eq!(effect.name, "Riverside");
        assert_eq!(effect.tax_multiplier, 2.0);
        assert_eq!(effect.service_funding, 0.5);
        assert!(!effect.construction_allowed);
        assert_eq!(effect.base_resident_rate, 8.0);
        assert_eq!(effect.base_business_rate, 4.0);
        assert_eq!(effect.resident_tax_rate_per_day, 16.0);
        assert_eq!(effect.business_tax_rate_per_day, 8.0);
        // The query helper agrees with the effect layer.
        assert_eq!(
            store
                .effective_policy([10.0, -10.0])
                .map(|policy| policy.id),
            Some(policy.id)
        );
        assert_eq!(store.effect_at([10.0, -10.0], 8.0, 4.0), Some(effect));
    }

    #[test]
    fn overlapping_policies_choose_the_smallest_id() {
        let mut store = DistrictPolicyStore::new(2026);
        let outer = store
            .create(&request("Outer", 0.0, 0.0, 100.0))
            .expect("outer");
        let mut inner_request = request("Inner", 100.0, 0.0, 50.0);
        inner_request.tax_multiplier = 4.0;
        let inner = store.create(&inner_request).expect("inner");
        assert_eq!(outer.id, 1);
        assert_eq!(inner.id, 2);

        // Both circles contain the origin; the older, larger id 1 wins.
        let effect = store.effect_at([0.0, 0.0], 8.0, 4.0).expect("effect");
        assert_eq!(effect.policy_id, outer.id);
        assert_eq!(effect.tax_multiplier, 1.0);
        assert_eq!(effect.resident_tax_rate_per_day, 8.0);

        // Only the inner circle contains (140, 0), so its multiplier applies.
        let effect = store.effect_at([140.0, 0.0], 8.0, 4.0).expect("effect");
        assert_eq!(effect.policy_id, inner.id);
        assert_eq!(effect.tax_multiplier, 4.0);
        assert_eq!(effect.resident_tax_rate_per_day, 32.0);

        // Removing the older policy promotes the inner one where it applies.
        assert!(store.remove(outer.id));
        let effect = store.effect_at([100.0, 0.0], 8.0, 4.0).expect("effect");
        assert_eq!(effect.policy_id, inner.id);
    }

    #[test]
    fn outside_all_policies_has_no_effect() {
        let (store, _) = store_with_one();
        assert!(store.effective_policy([100.01, 0.0]).is_none());
        assert!(store.effect_at([0.0, 100.01], 8.0, 4.0).is_none());
        assert!(store.effect_at([-500.0, 250.0], 8.0, 4.0).is_none());
        // The radius itself is inclusive.
        assert!(store.effect_at([100.0, 0.0], 8.0, 4.0).is_some());
        assert!(store.effect_at([f32::NAN, 0.0], 8.0, 4.0).is_none());
    }

    #[test]
    fn tax_multiplier_scales_both_rates_exactly() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut scaling = request("Scaling", 0.0, 0.0, 16.0);
        scaling.tax_multiplier = 2.5;
        store.create(&scaling).expect("create");

        let effect = store.effect_at([0.0, 0.0], 8.0, 4.0).expect("effect");
        assert_eq!(effect.resident_tax_rate(8.0), 20.0);
        assert_eq!(
            effect.resident_tax_rate(8.0),
            effect.resident_tax_rate_per_day
        );
        assert_eq!(effect.business_tax_rate(4.0), 10.0);
        assert_eq!(
            effect.business_tax_rate(4.0),
            effect.business_tax_rate_per_day
        );

        // Fractional bases are unrounded: the result is the exact f64 product.
        assert_eq!(effect.resident_tax_rate(8.3), 8.3 * 2.5);
        assert_eq!(effect.business_tax_rate(0.7), 0.7 * 2.5);

        // The multiplier range endpoints, queried away from the first circle.
        let mut minimum = request("Minimum", 200.0, 0.0, 64.0);
        minimum.tax_multiplier = 0.25;
        let minimum = store.create(&minimum).expect("minimum");
        let effect = store.effect_at([200.0, 0.0], 8.0, 4.0).expect("effect");
        assert_eq!(effect.policy_id, minimum.id);
        assert_eq!(effect.resident_tax_rate_per_day, 2.0);
        assert_eq!(effect.business_tax_rate_per_day, 1.0);

        let mut maximum = request("Maximum", -200.0, 0.0, 64.0);
        maximum.tax_multiplier = 4.0;
        let maximum = store.create(&maximum).expect("maximum");
        let effect = store.effect_at([-200.0, 0.0], 25.0, 10.0).expect("effect");
        assert_eq!(effect.policy_id, maximum.id);
        assert_eq!(effect.resident_tax_rate_per_day, 100.0);
        assert_eq!(effect.business_tax_rate_per_day, 40.0);
    }

    #[test]
    fn construction_flag_is_carried_and_mutable_through_update() {
        let (mut store, policy) = store_with_one();
        assert!(
            store
                .effect_at([0.0, 0.0], 8.0, 4.0)
                .expect("effect")
                .construction_allowed
        );

        let mut closed = request("Closed", 0.0, 0.0, 100.0);
        closed.construction_allowed = false;
        store.update(policy.id, &closed).expect("update");
        assert!(
            !store
                .effect_at([0.0, 0.0], 8.0, 4.0)
                .expect("effect")
                .construction_allowed
        );
    }

    #[test]
    fn blueprint_allowlist_allows_and_denies() {
        let (mut store, policy) = store_with_one();
        let effect = store.effect_at([0.0, 0.0], 8.0, 4.0).expect("effect");
        // No entries means unrestricted.
        assert!(effect.blueprint_allowed("base:cottage"));
        assert!(effect.blueprint_allowed("example:pack/v2-house_a"));

        let mut restricted = request("Restricted", 0.0, 0.0, 100.0);
        restricted
            .blueprint_allowlist
            .insert("base:cottage".to_string());
        restricted
            .blueprint_allowlist
            .insert("base:workshop".to_string());
        store.update(policy.id, &restricted).expect("update");
        let effect = store.effect_at([0.0, 0.0], 8.0, 4.0).expect("effect");
        assert!(effect.blueprint_allowed("base:cottage"));
        assert!(effect.blueprint_allowed("base:workshop"));
        assert!(!effect.blueprint_allowed("base:tent"));
        assert!(!effect.blueprint_allowed("base:unknown"));
        // Matching is exact and case-sensitive.
        assert!(!effect.blueprint_allowed("BASE:cottage"));
        assert!(!effect.blueprint_allowed("base:cottage "));
    }

    #[test]
    fn invalid_base_rates_are_clamped_consistently() {
        let (store, _) = store_with_one();
        let effect = store
            .effect_at([0.0, 0.0], f64::NAN, f64::INFINITY)
            .expect("effect still resolves");
        assert_eq!(effect.base_resident_rate, 0.0);
        assert_eq!(effect.base_business_rate, 0.0);
        assert_eq!(effect.resident_tax_rate_per_day, 0.0);
        assert_eq!(effect.business_tax_rate_per_day, 0.0);

        let effect = store.effect_at([0.0, 0.0], -5.0, 500.0).expect("effect");
        assert_eq!(effect.base_resident_rate, 0.0);
        assert_eq!(effect.base_business_rate, MAX_POLICY_EFFECT_BASE_RATE);
        assert_eq!(effect.resident_tax_rate_per_day, 0.0);
        assert_eq!(
            effect.business_tax_rate_per_day,
            MAX_POLICY_EFFECT_BASE_RATE
        );

        for base in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY, -1.0e9, 1.0e9] {
            let rate = clamp_base_rate(base);
            assert!(rate.is_finite(), "base {base} produced {rate}");
            assert!((0.0..=MAX_POLICY_EFFECT_BASE_RATE).contains(&rate));
        }

        // Multiplier guards: negative means zero, non-finite means neutral.
        assert_eq!(apply_multiplier(8.0, -2.0), 0.0);
        assert_eq!(apply_multiplier(8.0, f32::NAN), 8.0);
        assert_eq!(apply_multiplier(8.0, f32::INFINITY), 8.0);
        for multiplier in [0.0_f32, 0.25, 4.0, -1.0, f32::NAN, f32::INFINITY] {
            let rate = apply_multiplier(8.0, multiplier);
            assert!(
                rate.is_finite() && rate >= 0.0,
                "multiplier {multiplier} produced {rate}"
            );
        }
    }

    #[test]
    fn effect_is_deterministic_and_read_only() {
        let (store, policy) = store_with_one();
        let before = store.clone();
        let first = store.effect_at([1.0, 2.0], 8.0, 4.0).expect("effect");
        let second = store.effect_at([1.0, 2.0], 8.0, 4.0).expect("effect");
        assert_eq!(first, second);
        assert_eq!(first, DistrictPolicyEffect::from_policy(&policy, 8.0, 4.0));
        // Querying an effect does not touch the registry or its checksum.
        assert_eq!(store, before);
        assert_eq!(store.checksum(), before.checksum());
    }
}
