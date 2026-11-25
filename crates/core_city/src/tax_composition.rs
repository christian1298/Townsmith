//! Settlement tax composition (D-039 option A): pure, data-only math.
//!
//! [`Settlement::tax_rate`](crate::Settlement::tax_rate) is validated data with
//! no economy consumer. This module implements the proposed option-A
//! composition so a future treasury consumer can reuse one deterministic rule:
//!
//! ```text
//! combined_i = min(8.0, multiplier_i * (1 + settlement_rate_i / 100))
//! result     = sum(multiplier_i * combined_i) / sum(multiplier_i)
//! ```
//!
//! The base multiplier is the weight. A location with a larger district
//! multiplier carries proportionally more tax base, so the result is the
//! effective factor over the district-weighted tax base:
//!
//! ```text
//! collected = base_rate * sum(multiplier_i * combined_i)
//! neutral   = base_rate * sum(multiplier_i)
//! result    = collected / neutral
//! ```
//!
//! This is not a mean of means and not an unweighted per-resident mean. The
//! mean-of-products trap in [[81 - Settlement Tax Composition]] §4 applies:
//! resolve the settlement rate at each location first, then weight. A consumer
//! that needs population weighting expands one entry per resident or job and
//! owns that aggregation; [`TaxLocation::combined_multiplier`] exposes the
//! per-location factor for that purpose.
//!
//! # Neutral cases
//!
//! An empty slice, or a slice whose multipliers all sum to zero, returns the
//! neutral factor `1.0`. Unowned settlements, locations outside every
//! settlement, and zero-rate settlements are represented by rate `0`, which
//! preserves the base multiplier.
//!
//! # Determinism
//!
//! Input values are summed in a canonical order (ascending multiplier bits,
//! then settlement rate), so the same multiset returns the same bits regardless
//! of input order. The helper allocates one `Vec` per call; it is not on the
//! economy tick path because no tick consumer exists.
//!
//! # Status
//!
//! D-039 remains **Proposed** and Q-121/Q-122 are open. Nothing here reads or
//! writes the treasury, the server, a bridge payload, or a save. [`TaxLocation`]
//! is transient helper input and is never serialized or persisted.

use std::error::Error;
use std::fmt;

/// Largest combined tax multiplier for one location.
///
/// This is the product of the largest district multiplier (`4.0`) and a 100%
/// settlement surcharge (`2.0`). The cap applies per location, before the
/// weighted mean; it is not a cap on the result.
pub const TAX_COMPOSITION_CAP: f64 = 8.0;

/// Largest accepted settlement tax rate, in whole percent.
///
/// This mirrors [`MAX_SETTLEMENT_TAX_RATE`](crate::MAX_SETTLEMENT_TAX_RATE).
pub const MAX_TAX_COMPOSITION_SETTLEMENT_RATE: u16 = 100;

/// One tax location: a district multiplier plus the settlement rate at it.
///
/// Construct with [`TaxLocation::new`]. The fields are private so every value
/// satisfies the documented bounds. The type is transient helper input and is
/// never serialized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaxLocation {
    multiplier: f64,
    settlement_rate: u16,
}

impl TaxLocation {
    /// Validate one tax location.
    ///
    /// `multiplier` must be finite and non-negative. Negative zero is
    /// normalized to positive zero. `settlement_rate` must be at most
    /// [`MAX_TAX_COMPOSITION_SETTLEMENT_RATE`].
    ///
    /// # Errors
    ///
    /// Returns [`TaxCompositionError::InvalidMultiplier`] for a non-finite or
    /// negative multiplier and
    /// [`TaxCompositionError::InvalidSettlementRate`] for a rate above 100.
    pub fn new(multiplier: f64, settlement_rate: u16) -> Result<Self, TaxCompositionError> {
        if !multiplier.is_finite() || multiplier < 0.0 {
            return Err(TaxCompositionError::InvalidMultiplier { multiplier });
        }
        if settlement_rate > MAX_TAX_COMPOSITION_SETTLEMENT_RATE {
            return Err(TaxCompositionError::InvalidSettlementRate {
                rate: settlement_rate,
            });
        }
        Ok(Self {
            // `abs` is the identity for non-negative values and turns `-0.0`
            // into `0.0`, keeping the zero-weight check exact.
            multiplier: multiplier.abs(),
            settlement_rate,
        })
    }

    /// Base district multiplier before the settlement surcharge.
    #[must_use]
    pub const fn multiplier(self) -> f64 {
        self.multiplier
    }

    /// Settlement rate in whole percent, `0..=100`.
    #[must_use]
    pub const fn settlement_rate(self) -> u16 {
        self.settlement_rate
    }

    /// `min(8.0, multiplier * (1 + settlement_rate / 100))`.
    ///
    /// The result is finite and inside `0.0..=8.0` for every validated value.
    #[must_use]
    pub fn combined_multiplier(self) -> f64 {
        (self.multiplier * (1.0 + f64::from(self.settlement_rate) / 100.0)).min(TAX_COMPOSITION_CAP)
    }
}

/// Errors from validating a [`TaxLocation`] or composing a multiplier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaxCompositionError {
    /// The base multiplier was not finite or was negative.
    InvalidMultiplier {
        /// Rejected multiplier.
        multiplier: f64,
    },
    /// The settlement rate was above 100 percent.
    InvalidSettlementRate {
        /// Rejected rate.
        rate: u16,
    },
    /// Accumulation overflowed `f64` and produced a non-finite result.
    NonFiniteResult,
}

impl TaxCompositionError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidMultiplier { .. } => "tax.composition.invalid_multiplier",
            Self::InvalidSettlementRate { .. } => "tax.composition.invalid_settlement_rate",
            Self::NonFiniteResult => "tax.composition.non_finite_result",
        }
    }
}

impl fmt::Display for TaxCompositionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMultiplier { multiplier } => write!(
                f,
                "tax composition multiplier {multiplier} must be finite and non-negative"
            ),
            Self::InvalidSettlementRate { rate } => write!(
                f,
                "tax composition settlement rate {rate}% is above {MAX_TAX_COMPOSITION_SETTLEMENT_RATE}%"
            ),
            Self::NonFiniteResult => f.write_str("tax composition result is not finite"),
        }
    }
}

impl Error for TaxCompositionError {}

/// Compose one effective tax multiplier from per-location inputs.
///
/// Each location contributes `min(8.0, multiplier * (1 + rate / 100))`, and the
/// result is the base-multiplier-weighted mean
/// `sum(multiplier_i * combined_i) / sum(multiplier_i)`.
///
/// An empty slice, or a slice whose multipliers all sum to zero, returns the
/// neutral factor `1.0`. The same multiset returns the same bits regardless of
/// input order because the values are summed in a canonical order.
///
/// # Errors
///
/// Returns [`TaxCompositionError::NonFiniteResult`] if the accumulation
/// overflows `f64` (for example, several near-`f64::MAX` multipliers). Every
/// [`TaxLocation`] is already validated at construction, so this is the only
/// runtime error.
///
/// # Examples
///
/// ```
/// use core_city::{compose_tax_multiplier, TaxLocation};
///
/// let locations = [
///     TaxLocation::new(2.0, 50).expect("valid location"),
///     TaxLocation::new(0.5, 0).expect("valid location"),
/// ];
/// let factor = compose_tax_multiplier(&locations).expect("composes");
/// // (2.0 * 3.0 + 0.5 * 0.5) / (2.0 + 0.5)
/// assert!((factor - 2.5).abs() < 1.0e-12);
/// ```
pub fn compose_tax_multiplier(locations: &[TaxLocation]) -> Result<f64, TaxCompositionError> {
    if locations.is_empty() {
        return Ok(1.0);
    }

    let mut ordered: Vec<(u64, u16, f64, f64)> = locations
        .iter()
        .map(|location| {
            (
                location.multiplier.to_bits(),
                location.settlement_rate,
                location.multiplier,
                location.combined_multiplier(),
            )
        })
        .collect();
    // Positive finite floats order by their bit pattern, so this is also
    // ascending numeric order with a deterministic tie-break on the rate.
    ordered.sort_unstable_by_key(|entry| (entry.0, entry.1));

    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;
    for (_, _, weight, combined) in ordered {
        weighted_sum += weight * combined;
        total_weight += weight;
    }

    if !weighted_sum.is_finite() || !total_weight.is_finite() {
        return Err(TaxCompositionError::NonFiniteResult);
    }
    if total_weight == 0.0 {
        return Ok(1.0);
    }

    let result = weighted_sum / total_weight;
    if result.is_finite() {
        Ok(result)
    } else {
        Err(TaxCompositionError::NonFiniteResult)
    }
}

#[cfg(test)]
mod tests {
    use super::{compose_tax_multiplier, TaxCompositionError, TaxLocation, TAX_COMPOSITION_CAP};

    fn location(multiplier: f64, settlement_rate: u16) -> TaxLocation {
        TaxLocation::new(multiplier, settlement_rate).expect("valid location")
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1.0e-12,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn empty_input_is_neutral() {
        assert_eq!(compose_tax_multiplier(&[]), Ok(1.0));
    }

    #[test]
    fn zero_settlement_rate_preserves_the_base_multiplier() {
        assert_close(
            compose_tax_multiplier(&[location(1.5, 0)]).expect("composes"),
            1.5,
        );

        let mixed = [location(1.0, 0), location(0.25, 0), location(4.0, 0)];
        // (1*1 + 0.25*0.25 + 4*4) / (1 + 0.25 + 4) = 17.0625 / 5.25
        assert_close(compose_tax_multiplier(&mixed).expect("composes"), 3.25);
    }

    #[test]
    fn ten_percent_rate_adds_ten_percent_per_location() {
        assert_close(location(1.5, 10).combined_multiplier(), 1.65);
        assert_close(
            compose_tax_multiplier(&[location(1.5, 10)]).expect("composes"),
            1.65,
        );
        assert_close(location(4.0, 10).combined_multiplier(), 4.4);
    }

    #[test]
    fn cap_applies_per_location_not_to_the_mean() {
        assert_eq!(
            location(4.0, 100).combined_multiplier(),
            TAX_COMPOSITION_CAP
        );
        assert_eq!(location(8.0, 0).combined_multiplier(), TAX_COMPOSITION_CAP);
        assert_eq!(
            location(100.0, 100).combined_multiplier(),
            TAX_COMPOSITION_CAP
        );

        let locations = [location(4.0, 100), location(1.0, 0)];
        // (4*8 + 1*1) / (4 + 1) = 6.6, below the per-location cap.
        assert_close(compose_tax_multiplier(&locations).expect("composes"), 6.6);
    }

    #[test]
    fn weighted_mean_differs_from_mean_of_products() {
        // 2.0 at 50% -> 3.0; 0.5 at 0% -> 0.5.
        let locations = [location(2.0, 50), location(0.5, 0)];
        let composed = compose_tax_multiplier(&locations).expect("composes");

        // Base-multiplier-weighted mean: (2*3.0 + 0.5*0.5) / 2.5 = 2.5.
        assert_close(composed, 2.5);

        // Multiplying separate means is a different, wrong number:
        // ((2.0 + 0.5) / 2) * ((1.5 + 1.0) / 2) = 1.5625.
        let mean_of_products = 1.25 * 1.25;
        assert!((composed - mean_of_products).abs() > 0.9);
    }

    #[test]
    fn order_does_not_change_the_result() {
        let locations = [
            location(0.25, 0),
            location(1.0, 100),
            location(0.7, 33),
            location(4.0, 25),
            location(2.2, 10),
        ];
        let forward = compose_tax_multiplier(&locations).expect("composes");

        let mut reversed = locations;
        reversed.reverse();
        let reversed = compose_tax_multiplier(&reversed).expect("composes");

        let rotated = [
            locations[2],
            locations[4],
            locations[0],
            locations[3],
            locations[1],
        ];
        let rotated = compose_tax_multiplier(&rotated).expect("composes");

        assert!((forward - reversed).abs() <= 1.0e-12);
        assert!((forward - rotated).abs() <= 1.0e-12);
    }

    #[test]
    fn zero_weight_inputs_are_neutral() {
        assert_eq!(compose_tax_multiplier(&[location(0.0, 100)]), Ok(1.0));
        assert_eq!(
            compose_tax_multiplier(&[location(0.0, 0), location(0.0, 50)]),
            Ok(1.0)
        );
    }

    #[test]
    fn negative_zero_is_normalized_and_neutral() {
        let zero = location(-0.0, 100);
        assert!(zero.multiplier().is_sign_positive());
        assert_eq!(zero.multiplier(), 0.0);
        assert_eq!(compose_tax_multiplier(&[zero]), Ok(1.0));
    }

    #[test]
    fn rejects_invalid_multipliers() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let error = TaxLocation::new(bad, 0).expect_err("multiplier is rejected");
            assert_eq!(error.code(), "tax.composition.invalid_multiplier");
            assert!(matches!(
                error,
                TaxCompositionError::InvalidMultiplier { .. }
            ));
        }
    }

    #[test]
    fn rejects_rates_above_one_hundred() {
        let error = TaxLocation::new(1.0, 101).expect_err("rate is rejected");
        assert_eq!(
            error,
            TaxCompositionError::InvalidSettlementRate { rate: 101 }
        );
        assert_eq!(error.code(), "tax.composition.invalid_settlement_rate");
        assert!(error.to_string().contains("101"));
    }

    #[test]
    fn accepts_inclusive_bounds() {
        assert!(TaxLocation::new(0.0, 0).is_ok());
        assert!(TaxLocation::new(4.0, 100).is_ok());
        assert!(TaxLocation::new(f64::MAX, 100).is_ok());
    }

    #[test]
    fn overflow_is_rejected_as_non_finite() {
        let huge = location(f64::MAX, 0);
        let error = compose_tax_multiplier(&[huge, huge]).expect_err("overflow is rejected");
        assert_eq!(error, TaxCompositionError::NonFiniteResult);
        assert_eq!(error.code(), "tax.composition.non_finite_result");
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(
            TaxCompositionError::InvalidMultiplier { multiplier: -1.0 }.code(),
            "tax.composition.invalid_multiplier"
        );
        assert_eq!(
            TaxCompositionError::InvalidSettlementRate { rate: 101 }.code(),
            "tax.composition.invalid_settlement_rate"
        );
        assert_eq!(
            TaxCompositionError::NonFiniteResult.code(),
            "tax.composition.non_finite_result"
        );
    }
}
