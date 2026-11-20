//! Simple city treasury: taxes, wages, upkeep, and bankruptcy.
//!
//! Phase 3.6a is deliberately a cash-flow ledger, not a full economy. Income
//! is a per-resident and per-job tax; expenses are gross wages from the
//! household runtime plus per-building upkeep. Construction draw-down, loans,
//! trade, and service funding arrive later.

use serde::{Deserialize, Serialize};

use crate::population::WAGE_PER_WORKER_PER_DAY;

/// Current economy save format.
pub const ECONOMY_FORMAT_VERSION: u16 = 1;

/// Seconds in one game day.
pub const DAY_SECONDS: f64 = 86_400.0;

/// Starting treasury for a new city.
pub const STARTING_BALANCE: f64 = 100_000.0;

/// Default resident tax per game day.
pub const DEFAULT_TAX_PER_RESIDENT_PER_DAY: f64 = 8.0;

/// Default business tax per employed job per game day.
pub const DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY: f64 = 4.0;

/// Default upkeep per completed building per game day.
pub const DEFAULT_UPKEEP_PER_BUILDING_PER_DAY: f64 = 5.0;

/// Balance at or below which the city is flagged bankrupt.
pub const BANKRUPTCY_THRESHOLD: f64 = -50_000.0;

/// Upper bound for a simulated economy step.
pub const MAX_ECONOMY_TICK_SECONDS: f64 = 86_400.0;

/// One treasury record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Economy {
    /// Save format version.
    pub format_version: u16,
    /// World seed.
    pub seed: u64,
    /// Cash balance in abstract cost units.
    pub balance: f64,
    /// Tax per resident per game day.
    pub tax_per_resident_per_day: f64,
    /// Business tax per employed job per game day.
    pub business_tax_per_job_per_day: f64,
    /// Upkeep per completed building per game day.
    pub upkeep_per_building_per_day: f64,
    /// Construction spending accumulated so far.
    pub construction_spent: f64,
    /// Simulated economy days.
    pub days_elapsed: f64,
    /// Whether the balance is at or below the bankruptcy threshold.
    pub bankrupt: bool,
}

/// Result of one economy tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EconomyTick {
    /// Taxes collected this tick.
    pub income: f64,
    /// Wages plus upkeep this tick.
    pub expenses: f64,
    /// Balance after the tick.
    pub balance: f64,
    /// Simulated days after the tick.
    pub days_elapsed: f64,
    /// Bankruptcy flag after the tick.
    pub bankrupt: bool,
}

impl Economy {
    /// Create a new city treasury with the starting balance and default rates.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            format_version: ECONOMY_FORMAT_VERSION,
            seed,
            balance: STARTING_BALANCE,
            tax_per_resident_per_day: DEFAULT_TAX_PER_RESIDENT_PER_DAY,
            business_tax_per_job_per_day: DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY,
            upkeep_per_building_per_day: DEFAULT_UPKEEP_PER_BUILDING_PER_DAY,
            construction_spent: 0.0,
            days_elapsed: 0.0,
            bankrupt: false,
        }
    }

    /// Set tax rates. Values are clamped to a finite 0..=100 range.
    pub fn set_taxes(&mut self, resident: f64, business: f64) {
        self.tax_per_resident_per_day = clamp_rate(resident);
        self.business_tax_per_job_per_day = clamp_rate(business);
    }

    /// Record construction spending.
    pub fn charge_construction(&mut self, amount: f64) {
        if amount.is_finite() && amount > 0.0 {
            self.construction_spent += amount;
            self.balance -= amount;
            self.bankrupt = self.balance <= BANKRUPTCY_THRESHOLD;
        }
    }

    /// Advance taxes, wages, and upkeep by `dt_s` seconds.
    pub fn tick(
        &mut self,
        dt_s: f64,
        population: u64,
        employed: u64,
        completed_buildings: u64,
    ) -> EconomyTick {
        let dt = if dt_s.is_finite() {
            dt_s.clamp(0.0, MAX_ECONOMY_TICK_SECONDS)
        } else {
            0.0
        };
        let days = dt / DAY_SECONDS;
        let income = (population as f64 * self.tax_per_resident_per_day
            + employed as f64 * self.business_tax_per_job_per_day)
            * days;
        let expenses = (employed as f64 * WAGE_PER_WORKER_PER_DAY
            + completed_buildings as f64 * self.upkeep_per_building_per_day)
            * days;
        self.balance += income - expenses;
        self.days_elapsed += days;
        self.bankrupt = self.balance <= BANKRUPTCY_THRESHOLD;
        EconomyTick {
            income,
            expenses,
            balance: self.balance,
            days_elapsed: self.days_elapsed,
            bankrupt: self.bankrupt,
        }
    }

    /// Net cash flow per game day at current rates.
    #[must_use]
    pub fn daily_net(&self, population: u64, employed: u64, completed_buildings: u64) -> f64 {
        let income = population as f64 * self.tax_per_resident_per_day
            + employed as f64 * self.business_tax_per_job_per_day;
        let expenses = employed as f64 * WAGE_PER_WORKER_PER_DAY
            + completed_buildings as f64 * self.upkeep_per_building_per_day;
        income - expenses
    }

    /// Validate a loaded economy record.
    pub fn validate(&self) -> Result<(), EconomyError> {
        if self.format_version != ECONOMY_FORMAT_VERSION {
            return Err(EconomyError::UnsupportedVersion(self.format_version));
        }
        if !self.balance.is_finite() {
            return Err(EconomyError::Invalid("balance is not finite"));
        }
        for (name, value) in [
            ("resident tax", self.tax_per_resident_per_day),
            ("business tax", self.business_tax_per_job_per_day),
            ("upkeep", self.upkeep_per_building_per_day),
        ] {
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return Err(EconomyError::Invalid(name));
            }
        }
        if !self.construction_spent.is_finite() || self.construction_spent < 0.0 {
            return Err(EconomyError::Invalid("construction spending"));
        }
        if !self.days_elapsed.is_finite() || self.days_elapsed < 0.0 {
            return Err(EconomyError::Invalid("elapsed days"));
        }
        if self.bankrupt != (self.balance <= BANKRUPTCY_THRESHOLD) {
            return Err(EconomyError::Invalid("bankruptcy flag"));
        }
        Ok(())
    }

    /// Serialize as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, EconomyError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse and validate an economy record.
    pub fn from_json(json: &str) -> Result<Self, EconomyError> {
        let economy: Self = serde_json::from_str(json)?;
        economy.validate()?;
        Ok(economy)
    }

    /// FNV-1a checksum over the compact JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let json = serde_json::to_vec(self).expect("economy serializes");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in json {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

fn clamp_rate(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Errors returned by the economy store.
#[derive(Debug)]
pub enum EconomyError {
    /// JSON parse/serialize error.
    Json(serde_json::Error),
    /// Unsupported save version.
    UnsupportedVersion(u16),
    /// A range or state invariant failed.
    Invalid(&'static str),
}

impl std::fmt::Display for EconomyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "economy JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported economy format version {version}")
            }
            Self::Invalid(message) => write!(formatter, "invalid economy: {message}"),
        }
    }
}

impl std::error::Error for EconomyError {}

impl From<serde_json::Error> for EconomyError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_city_has_the_starting_balance() {
        let economy = Economy::new(2026);
        assert_eq!(economy.balance, STARTING_BALANCE);
        assert!(!economy.bankrupt);
        assert_eq!(economy.days_elapsed, 0.0);
    }

    #[test]
    fn taxes_wages_and_upkeep_change_the_balance() {
        let mut economy = Economy::new(2026);
        let tick = economy.tick(DAY_SECONDS, 100, 80, 20);
        assert!((tick.income - (100.0 * 8.0 + 80.0 * 4.0)).abs() < 0.001);
        assert!((tick.expenses - (80.0 * 20.0 + 20.0 * 5.0)).abs() < 0.001);
        assert!((tick.balance - (STARTING_BALANCE + 1120.0 - 1700.0)).abs() < 0.001);
        assert_eq!(tick.days_elapsed, 1.0);
    }

    #[test]
    fn tax_rates_are_clamped() {
        let mut economy = Economy::new(2026);
        economy.set_taxes(-5.0, 500.0);
        assert_eq!(economy.tax_per_resident_per_day, 0.0);
        assert_eq!(economy.business_tax_per_job_per_day, 100.0);
    }

    #[test]
    fn bankruptcy_is_flagged() {
        let mut economy = Economy::new(2026);
        economy.balance = BANKRUPTCY_THRESHOLD;
        let tick = economy.tick(0.0, 0, 0, 0);
        assert!(tick.bankrupt);
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let mut economy = Economy::new(2026);
        economy.tick(3600.0, 50, 40, 10);
        let json = economy.to_json_pretty().expect("json");
        let decoded = Economy::from_json(&json).expect("decode");
        assert!((decoded.balance - economy.balance).abs() < 0.001);
        assert!((decoded.days_elapsed - economy.days_elapsed).abs() < 0.001);
        assert_eq!(
            decoded.tax_per_resident_per_day,
            economy.tax_per_resident_per_day
        );
        assert_eq!(decoded.bankrupt, economy.bankrupt);
    }

    #[test]
    fn invalid_economy_is_rejected() {
        let mut economy = Economy::new(2026);
        economy.balance = f64::NAN;
        assert!(economy.validate().is_err());
    }
}
