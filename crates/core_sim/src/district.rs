//! Per-district service inputs to the cohort simulation.

/// Replace a non-finite value with zero.
///
/// [`f32::clamp`] returns `NaN` for `NaN` and keeps infinities, so one bad
/// input would poison every derived need, happiness, and income value. Zero is
/// the documented "no service" level, so corrupt state degrades to the worst
/// case instead of an unrecoverable `NaN`.
#[must_use]
pub(crate) fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

/// Coverage and quality values are normalized to `0..=1` unless noted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistrictServices {
    /// Food availability.
    pub food: f32,
    /// Potable water availability.
    pub water: f32,
    /// Rest quality from housing.
    pub rest: f32,
    /// Hygiene and sanitation.
    pub hygiene: f32,
    /// Social and leisure access.
    pub social: f32,
    /// Safety and emergency services.
    pub safety: f32,
    /// Fraction of the population that can find work.
    pub jobs: f32,
    /// Housing quality multiplier.
    pub housing: f32,
    /// Air pollution, 0 = clean.
    pub pollution: f32,
    /// Noise pollution, 0 = quiet.
    pub noise: f32,
    /// Average commute burden, 0 = none.
    pub commute: f32,
    /// Average wage, in abstract income units.
    pub wages: f32,
}

impl Default for DistrictServices {
    fn default() -> Self {
        Self {
            food: 0.9,
            water: 0.95,
            rest: 0.8,
            hygiene: 0.85,
            social: 0.7,
            safety: 0.85,
            jobs: 0.9,
            housing: 0.85,
            pollution: 0.1,
            noise: 0.1,
            commute: 0.15,
            wages: 1.0,
        }
    }
}

impl DistrictServices {
    /// Clamp every input to a usable range.
    ///
    /// This is the validation boundary for service inputs: a non-finite value
    /// (from a bad save, a mod, or a buggy bridge) becomes `0.0` and is then
    /// clamped, so [`crate::Cohort::tick`] can never produce a non-finite
    /// need, happiness, or income from these inputs.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            food: finite_or_zero(self.food).clamp(0.0, 1.0),
            water: finite_or_zero(self.water).clamp(0.0, 1.0),
            rest: finite_or_zero(self.rest).clamp(0.0, 1.0),
            hygiene: finite_or_zero(self.hygiene).clamp(0.0, 1.0),
            social: finite_or_zero(self.social).clamp(0.0, 1.0),
            safety: finite_or_zero(self.safety).clamp(0.0, 1.0),
            jobs: finite_or_zero(self.jobs).clamp(0.0, 1.0),
            housing: finite_or_zero(self.housing).clamp(0.0, 1.0),
            pollution: finite_or_zero(self.pollution).clamp(0.0, 1.0),
            noise: finite_or_zero(self.noise).clamp(0.0, 1.0),
            commute: finite_or_zero(self.commute).clamp(0.0, 1.0),
            wages: finite_or_zero(self.wages).clamp(0.0, 10.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitation_clamps_inputs() {
        let services = DistrictServices {
            food: 2.0,
            water: -1.0,
            wages: -3.0,
            ..DistrictServices::default()
        }
        .sanitized();
        assert_eq!(services.food, 1.0);
        assert_eq!(services.water, 0.0);
        assert_eq!(services.wages, 0.0);
    }

    #[test]
    fn sanitation_maps_non_finite_values_to_zero() {
        for poison in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let services = DistrictServices {
                food: poison,
                water: poison,
                rest: poison,
                hygiene: poison,
                social: poison,
                safety: poison,
                jobs: poison,
                housing: poison,
                pollution: poison,
                noise: poison,
                commute: poison,
                wages: poison,
            }
            .sanitized();
            assert_eq!(
                services,
                DistrictServices {
                    food: 0.0,
                    water: 0.0,
                    rest: 0.0,
                    hygiene: 0.0,
                    social: 0.0,
                    safety: 0.0,
                    jobs: 0.0,
                    housing: 0.0,
                    pollution: 0.0,
                    noise: 0.0,
                    commute: 0.0,
                    wages: 0.0,
                },
                "poison {poison}"
            );
        }
    }

    #[test]
    fn sanitation_keeps_the_default_finite() {
        let services = DistrictServices::default().sanitized();
        assert_eq!(services, DistrictServices::default());
    }
}
