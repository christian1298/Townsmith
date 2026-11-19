//! Per-district service inputs to the cohort simulation.

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
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            food: self.food.clamp(0.0, 1.0),
            water: self.water.clamp(0.0, 1.0),
            rest: self.rest.clamp(0.0, 1.0),
            hygiene: self.hygiene.clamp(0.0, 1.0),
            social: self.social.clamp(0.0, 1.0),
            safety: self.safety.clamp(0.0, 1.0),
            jobs: self.jobs.clamp(0.0, 1.0),
            housing: self.housing.clamp(0.0, 1.0),
            pollution: self.pollution.clamp(0.0, 1.0),
            noise: self.noise.clamp(0.0, 1.0),
            commute: self.commute.clamp(0.0, 1.0),
            wages: self.wages.clamp(0.0, 10.0),
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
}
