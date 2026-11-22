//! City advisor: deterministic warnings and suggestions from a metrics snapshot.
//!
//! The advisor is deliberately pure. It does not read stores, tick simulation,
//! or persist state; the bridge gathers one snapshot and asks for a report.
//! This keeps the rules testable and lets the dashboard refresh cheaply.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

/// Maximum messages returned in one report.
pub const MAX_ADVISOR_MESSAGES: usize = 12;

/// Message severity, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Healthy or informational.
    Info,
    /// Needs attention soon.
    Warning,
    /// Immediate risk to the city.
    Critical,
}

impl Severity {
    /// Lowercase name used by the bridge and HUD.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One advisor message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvisorMessage {
    /// Stable message code, for example `city.food_shortage`.
    pub code: String,
    /// Severity.
    pub severity: Severity,
    /// Short title.
    pub title: String,
    /// Suggested action or explanation in English.
    ///
    /// Clients localize `base.advisor.<code>.detail` and use this string only
    /// as the fallback for an unknown code or locale.
    pub detail: String,
    /// Named values referenced by detail placeholders.
    ///
    /// Keys and values are stable strings so the bridge can send them as a
    /// dictionary and clients can substitute `{name}` placeholders without
    /// knowing the placeholder types.
    pub params: BTreeMap<String, String>,
}

impl AdvisorMessage {
    fn new(
        code: &str,
        severity: Severity,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity,
            title: title.into(),
            detail: detail.into(),
            params: BTreeMap::new(),
        }
    }

    /// Attach one named parameter used by detail placeholders.
    #[must_use]
    fn with_param(mut self, key: &str, value: impl Into<String>) -> Self {
        self.params.insert(key.to_string(), value.into());
        self
    }
}

/// Format a `0.0..=1.0` share as a whole percentage.
fn percent(value: f32) -> String {
    format!("{:.0}", value.clamp(0.0, 1.0) * 100.0)
}

/// Format a positive money amount without a currency symbol.
fn money(value: f64) -> String {
    format!("{:.0}", value.abs())
}

/// Format a distance in whole metres.
fn metres(value: f32) -> String {
    format!("{:.0}", value.max(0.0))
}

/// Plain city snapshot used by the advisor.
///
/// Supplies are optional so the advisor can distinguish "no service store"
/// from "a measured shortage". A missing supply is treated as zero when the
/// city has residents.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AdvisorInput {
    /// Residents.
    pub population: u64,
    /// Completed housing capacity.
    pub housing_capacity: u64,
    /// Vacant housing slots.
    pub vacancies: u64,
    /// Completed job slots.
    pub jobs: u64,
    /// Employed residents.
    pub employed: u64,
    /// Residents without a job.
    pub unemployed: u64,
    /// Resident-weighted happiness.
    pub happiness: f32,
    /// Resident-weighted food satisfaction.
    pub food: f32,
    /// Resident-weighted water satisfaction.
    pub water: f32,
    /// Resident-weighted rest satisfaction.
    pub rest: f32,
    /// Resident-weighted average commute.
    pub average_commute_m: f32,
    /// Food supply share, if an industry store exists.
    pub food_supply: Option<f32>,
    /// Water service share, if a utility store exists.
    pub water_supply: Option<f32>,
    /// Power service share, if a utility store exists.
    pub power_supply: Option<f32>,
    /// School service share, if a civic service store exists.
    pub school_supply: Option<f32>,
    /// Clinic service share, if a civic service store exists.
    pub clinic_supply: Option<f32>,
    /// Fire service share, if a civic service store exists.
    pub fire_supply: Option<f32>,
    /// Police service share, if a civic service store exists.
    pub police_supply: Option<f32>,
    /// Food stockpile.
    pub food_stock: f64,
    /// Wood stockpile.
    pub wood_stock: f64,
    /// Treasury balance, if a treasury exists.
    pub balance: Option<f64>,
    /// Net cash flow per game day, if a treasury exists.
    pub daily_net: Option<f64>,
    /// Outstanding debt, if a treasury exists.
    pub debt: Option<f64>,
    /// Bankruptcy flag.
    pub bankrupt: bool,
    /// Connected road-graph components, if a road store exists.
    pub traffic_components: Option<usize>,
    /// Road-graph nodes.
    pub traffic_nodes: Option<usize>,
    /// Construction sites in progress.
    pub active_sites: usize,
    /// Construction sites currently stalled.
    pub stalled_sites: usize,
    /// Completed buildings.
    pub completed_buildings: usize,
}

/// Ordered advisor result.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct AdvisorReport {
    /// Messages, most urgent first.
    pub messages: Vec<AdvisorMessage>,
}

impl AdvisorReport {
    /// Count messages of one severity.
    #[must_use]
    pub fn count(&self, severity: Severity) -> usize {
        self.messages
            .iter()
            .filter(|message| message.severity == severity)
            .count()
    }

    /// Critical messages.
    #[must_use]
    pub fn critical(&self) -> usize {
        self.count(Severity::Critical)
    }

    /// Warning messages.
    #[must_use]
    pub fn warnings(&self) -> usize {
        self.count(Severity::Warning)
    }

    /// Informational messages.
    #[must_use]
    pub fn infos(&self) -> usize {
        self.count(Severity::Info)
    }

    /// True when there are no critical or warning messages.
    #[must_use]
    pub fn healthy(&self) -> bool {
        self.critical() == 0 && self.warnings() == 0
    }
}

/// Build a deterministic advisor report from one snapshot.
///
/// Messages are sorted by severity (critical first) and then by stable code.
/// At most [`MAX_ADVISOR_MESSAGES`] are returned.
#[must_use]
pub fn advise(input: &AdvisorInput) -> AdvisorReport {
    let mut messages = Vec::new();
    let populated = input.population > 0;

    if input.bankrupt {
        messages.push(AdvisorMessage::new(
            "city.bankrupt",
            Severity::Critical,
            "Treasury bankrupt",
            "Placement and borrowing are blocked. Raise taxes or repay debt before building.",
        ));
    }

    if populated {
        let food = input.food_supply.unwrap_or(0.0).clamp(0.0, 1.0);
        if food < 0.25 {
            let supply = percent(food);
            messages.push(
                AdvisorMessage::new(
                    "city.food_shortage",
                    Severity::Critical,
                    "Food shortage",
                    format!(
                        "Food supply is at {supply}% — residents are eating below a quarter of demand. Add farms or hunters."
                    ),
                )
                .with_param("supply", supply),
            );
        } else if food < 0.99 {
            let supply = percent(food);
            messages.push(
                AdvisorMessage::new(
                    "city.food_shortage",
                    Severity::Warning,
                    "Food supply is short",
                    format!(
                        "Food supply is at {supply}% and falling. Add production before the stock empties."
                    ),
                )
                .with_param("supply", supply),
            );
        }

        let water = input.water_supply.unwrap_or(0.0).clamp(0.0, 1.0);
        if water < 0.25 {
            let supply = percent(water);
            messages.push(
                AdvisorMessage::new(
                    "city.water_shortage",
                    Severity::Critical,
                    "Water shortage",
                    format!("Water service covers only {supply}% of demand. Build a water site."),
                )
                .with_param("supply", supply),
            );
        } else if water < 0.99 {
            let supply = percent(water);
            messages.push(
                AdvisorMessage::new(
                    "city.water_shortage",
                    Severity::Warning,
                    "Water service is short",
                    format!(
                        "Water service covers {supply}% of demand. Check radius, capacity, and site completion."
                    ),
                )
                .with_param("supply", supply),
            );
        }

        let power = input.power_supply.unwrap_or(0.0).clamp(0.0, 1.0);
        if power < 0.25 {
            let supply = percent(power);
            messages.push(
                AdvisorMessage::new(
                    "city.power_shortage",
                    Severity::Critical,
                    "Power outage",
                    format!(
                        "Power service covers {supply}% of demand and industry has stopped. Build a power site."
                    ),
                )
                .with_param("supply", supply),
            );
        } else if power < 0.99 {
            let supply = percent(power);
            messages.push(
                AdvisorMessage::new(
                    "city.power_shortage",
                    Severity::Warning,
                    "Power service is short",
                    format!(
                        "Power service covers {supply}% of demand. Production and rest recovery are reduced."
                    ),
                )
                .with_param("supply", supply),
            );
        }

        let school = input.school_supply.unwrap_or(0.0).clamp(0.0, 1.0);
        if school < 0.25 {
            let supply = percent(school);
            messages.push(
                AdvisorMessage::new(
                    "city.school_shortage",
                    Severity::Critical,
                    "School shortage",
                    format!("School service covers {supply}% of demand. Build or extend a school."),
                )
                .with_param("supply", supply),
            );
        } else if school < 0.99 {
            let supply = percent(school);
            messages.push(
                AdvisorMessage::new(
                    "city.school_shortage",
                    Severity::Warning,
                    "School service is short",
                    format!(
                        "School service covers {supply}% of demand. Check radius, capacity, and site completion."
                    ),
                )
                .with_param("supply", supply),
            );
        }

        let clinic = input.clinic_supply.unwrap_or(0.0).clamp(0.0, 1.0);
        if clinic < 0.25 {
            let supply = percent(clinic);
            messages.push(
                AdvisorMessage::new(
                    "city.clinic_shortage",
                    Severity::Critical,
                    "Clinic shortage",
                    format!("Clinic service covers {supply}% of demand. Build a clinic."),
                )
                .with_param("supply", supply),
            );
        } else if clinic < 0.99 {
            let supply = percent(clinic);
            messages.push(
                AdvisorMessage::new(
                    "city.clinic_shortage",
                    Severity::Warning,
                    "Clinic service is short",
                    format!(
                        "Clinic service covers {supply}% of demand. Check radius, capacity, and site completion."
                    ),
                )
                .with_param("supply", supply),
            );
        }

        let fire = input.fire_supply.unwrap_or(0.0).clamp(0.0, 1.0);
        if fire < 0.25 {
            let supply = percent(fire);
            messages.push(
                AdvisorMessage::new(
                    "city.fire_shortage",
                    Severity::Critical,
                    "Fire coverage is low",
                    format!(
                        "Fire coverage reaches only {supply}% of demand. Build a fire station."
                    ),
                )
                .with_param("supply", supply),
            );
        } else if fire < 0.99 {
            let supply = percent(fire);
            messages.push(
                AdvisorMessage::new(
                    "city.fire_shortage",
                    Severity::Warning,
                    "Fire service is short",
                    format!(
                        "Fire coverage reaches {supply}% of demand. Check radius, capacity, and site completion."
                    ),
                )
                .with_param("supply", supply),
            );
        }

        let police = input.police_supply.unwrap_or(0.0).clamp(0.0, 1.0);
        if police < 0.25 {
            let supply = percent(police);
            messages.push(
                AdvisorMessage::new(
                    "city.police_shortage",
                    Severity::Critical,
                    "Police coverage is low",
                    format!(
                        "Police coverage reaches only {supply}% of demand. Build a police station."
                    ),
                )
                .with_param("supply", supply),
            );
        } else if police < 0.99 {
            let supply = percent(police);
            messages.push(
                AdvisorMessage::new(
                    "city.police_shortage",
                    Severity::Warning,
                    "Police service is short",
                    format!(
                        "Police coverage reaches {supply}% of demand. Check radius, capacity, and site completion."
                    ),
                )
                .with_param("supply", supply),
            );
        }

        let employment = input.employed as f32 / input.population as f32;
        if input.unemployed > 0 && employment < 0.9 {
            let employment = percent(employment);
            messages.push(
                AdvisorMessage::new(
                    "city.unemployment",
                    Severity::Warning,
                    "Residents are unemployed",
                    format!(
                        "Employment is at {employment}% with {unemployed} residents looking for work. Add workplaces or improve road access.",
                        unemployed = input.unemployed
                    ),
                )
                .with_param("employment", employment)
                .with_param("unemployed", input.unemployed.to_string()),
            );
        }

        if input.vacancies > 0 && input.housing_capacity > input.population {
            messages.push(
                AdvisorMessage::new(
                    "city.vacant_homes",
                    Severity::Warning,
                    "Homes are standing empty",
                    format!(
                        "{vacancies} of {housing} housing slots are vacant. Move-in is still filling them; check jobs and services if it stalls.",
                        vacancies = input.vacancies,
                        housing = input.housing_capacity
                    ),
                )
                .with_param("vacancies", input.vacancies.to_string())
                .with_param("housing", input.housing_capacity.to_string()),
            );
        }

        if input.happiness < 0.35 {
            let happiness = percent(input.happiness);
            messages.push(
                AdvisorMessage::new(
                    "city.unhappy",
                    Severity::Critical,
                    "Residents are unhappy",
                    format!(
                        "Happiness is at {happiness}%. Fix food, water, power, jobs, or commutes before the city stops growing."
                    ),
                )
                .with_param("happiness", happiness),
            );
        } else if input.happiness < 0.55 {
            let happiness = percent(input.happiness);
            messages.push(
                AdvisorMessage::new(
                    "city.unhappy",
                    Severity::Warning,
                    "Happiness is low",
                    format!(
                        "Happiness is at {happiness}%. Review needs, employment, and commute distance in the dashboard."
                    ),
                )
                .with_param("happiness", happiness),
            );
        }

        if input.average_commute_m > 120.0 {
            let commute = metres(input.average_commute_m);
            messages.push(
                AdvisorMessage::new(
                    "city.long_commutes",
                    Severity::Warning,
                    "Commutes are long",
                    format!(
                        "Residents commute {commute} m on average. Add a connecting road or move workplaces closer to housing."
                    ),
                )
                .with_param("commute", commute),
            );
        }
    } else if input.housing_capacity > 0 {
        messages.push(
            AdvisorMessage::new(
                "city.no_residents",
                Severity::Info,
                "No residents yet",
                format!(
                    "Housing has capacity for {housing} residents but nobody has moved in. Keep jobs and services ready.",
                    housing = input.housing_capacity
                ),
            )
            .with_param("housing", input.housing_capacity.to_string()),
        );
    }

    if let Some(net) = input.daily_net {
        if net < 0.0 && !input.bankrupt {
            messages.push(AdvisorMessage::new(
                "city.budget_deficit",
                Severity::Warning,
                "The city is running a deficit",
                "Raise taxes, borrow deliberately, or wait for the current tax policy to recover.",
            )
            .with_param("net", money(input.daily_net.unwrap_or(0.0))));
        }
    }

    if input.debt.unwrap_or(0.0) > 0.0 {
        messages.push(
            AdvisorMessage::new(
                "city.debt",
                Severity::Info,
                "Outstanding loans",
                "Loan service is already included in the daily net. Repay before taking more debt.",
            )
            .with_param("debt", money(input.debt.unwrap_or(0.0))),
        );
    }

    if input
        .traffic_components
        .is_some_and(|components| components > 1)
    {
        messages.push(
            AdvisorMessage::new(
                "city.traffic_disconnected",
                Severity::Warning,
                "Road network is disconnected",
                "Commutes fall back to straight-line distance across gaps. Join the components.",
            )
            .with_param(
                "components",
                input.traffic_components.unwrap_or(0).to_string(),
            ),
        );
    } else if input.traffic_nodes.is_some_and(|nodes| nodes == 0) && populated {
        messages.push(AdvisorMessage::new(
            "city.no_roads",
            Severity::Warning,
            "No roads",
            "Residents commute in a straight line. Draw a road network to connect homes and jobs.",
        ).with_param("population", input.population.to_string()));
    }

    if input.stalled_sites > 0 {
        messages.push(
            AdvisorMessage::new(
                "city.construction_stalled",
                Severity::Warning,
                "Construction is stalled",
                "A site is waiting for crew or materials. Add capacity or deliver materials.",
            )
            .with_param("stalled", input.stalled_sites.to_string()),
        );
    } else if input.active_sites > 0 {
        messages.push(
            AdvisorMessage::new(
                "city.construction_active",
                Severity::Info,
                "Construction is in progress",
                "Completed sites add housing and jobs when the final phase finishes.",
            )
            .with_param("active", input.active_sites.to_string()),
        );
    }

    if messages.is_empty()
        && populated
        && input.food_supply.unwrap_or(1.0) >= 0.99
        && input.water_supply.unwrap_or(1.0) >= 0.99
        && input.power_supply.unwrap_or(1.0) >= 0.99
        && input.school_supply.unwrap_or(0.0) >= 0.99
        && input.clinic_supply.unwrap_or(0.0) >= 0.99
        && input.fire_supply.unwrap_or(0.0) >= 0.99
        && input.police_supply.unwrap_or(0.0) >= 0.99
        && input.happiness >= 0.7
        && input.daily_net.unwrap_or(0.0) >= 0.0
    {
        messages.push(AdvisorMessage::new(
            "city.healthy",
            Severity::Info,
            "The city is healthy",
            "Needs, services, employment, and cash flow are all in good shape.",
        ));
    } else if messages.is_empty()
        && populated
        && input.food_supply.is_some()
        && input.water_supply.is_some()
        && input.power_supply.is_some()
        && input.school_supply.is_some()
        && input.clinic_supply.is_some()
        && input.fire_supply.is_some()
        && input.police_supply.is_some()
        && input.food_supply.unwrap_or(0.0) >= 0.99
        && input.water_supply.unwrap_or(0.0) >= 0.99
        && input.power_supply.unwrap_or(0.0) >= 0.99
        && input.school_supply.unwrap_or(0.0) >= 0.99
        && input.clinic_supply.unwrap_or(0.0) >= 0.99
        && input.fire_supply.unwrap_or(0.0) >= 0.99
        && input.police_supply.unwrap_or(0.0) >= 0.99
        && input.happiness >= 0.7
        && input.daily_net.is_none()
    {
        messages.push(AdvisorMessage::new(
            "city.services_healthy",
            Severity::Info,
            "Services are healthy",
            "Food, water, power, and civic services are meeting demand. Open a treasury to track cash flow.",
        ));
    }

    if messages.is_empty() {
        messages.push(AdvisorMessage::new(
            "city.quiet",
            Severity::Info,
            "No urgent issues",
            "Place a blueprint or let the simulation run to generate advice.",
        ));
    }

    messages.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.code.cmp(&right.code))
    });
    messages.truncate(MAX_ADVISOR_MESSAGES);
    AdvisorReport { messages }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_city() -> AdvisorInput {
        AdvisorInput {
            population: 100,
            housing_capacity: 100,
            jobs: 100,
            employed: 100,
            happiness: 0.9,
            food: 1.0,
            water: 1.0,
            rest: 1.0,
            food_supply: Some(1.0),
            water_supply: Some(1.0),
            power_supply: Some(1.0),
            school_supply: Some(1.0),
            clinic_supply: Some(1.0),
            fire_supply: Some(1.0),
            police_supply: Some(1.0),
            food_stock: 500.0,
            wood_stock: 200.0,
            balance: Some(50_000.0),
            daily_net: Some(120.0),
            debt: Some(0.0),
            traffic_components: Some(1),
            traffic_nodes: Some(20),
            completed_buildings: 10,
            ..AdvisorInput::default()
        }
    }

    #[test]
    fn a_healthy_city_with_full_services_reports_one_info_message() {
        let input = healthy_city();
        assert_eq!(input.school_supply, Some(1.0));
        assert_eq!(input.clinic_supply, Some(1.0));
        assert_eq!(input.fire_supply, Some(1.0));
        assert_eq!(input.police_supply, Some(1.0));
        let report = advise(&input);
        assert!(report.healthy());
        assert_eq!(report.messages.len(), 1);
        assert_eq!(report.messages[0].code, "city.healthy");
        assert_eq!(report.messages[0].severity, Severity::Info);
    }

    fn assert_civic_rule(
        mut input: AdvisorInput,
        set: impl Fn(&mut AdvisorInput, Option<f32>),
        code: &str,
    ) {
        for (supply, expected) in [
            (Some(0.99), None),
            (Some(0.98), Some(Severity::Warning)),
            (Some(0.25), Some(Severity::Warning)),
            (Some(0.24), Some(Severity::Critical)),
            (Some(0.0), Some(Severity::Critical)),
        ] {
            set(&mut input, supply);
            let report = advise(&input);
            match expected {
                None => {
                    assert!(report.healthy(), "supply {supply:?} should be healthy");
                    assert_eq!(report.messages[0].code, "city.healthy");
                }
                Some(severity) => {
                    assert!(!report.healthy(), "supply {supply:?} should not be healthy");
                    assert_eq!(report.messages[0].code, code);
                    assert_eq!(report.messages[0].severity, severity);
                }
            }
        }
    }

    #[test]
    fn a_school_shortage_warns_then_goes_critical() {
        assert_civic_rule(
            healthy_city(),
            |input, supply| input.school_supply = supply,
            "city.school_shortage",
        );
    }

    #[test]
    fn a_clinic_shortage_warns_then_goes_critical() {
        assert_civic_rule(
            healthy_city(),
            |input, supply| input.clinic_supply = supply,
            "city.clinic_shortage",
        );
    }

    #[test]
    fn a_fire_shortage_warns_then_goes_critical() {
        assert_civic_rule(
            healthy_city(),
            |input, supply| input.fire_supply = supply,
            "city.fire_shortage",
        );
    }

    #[test]
    fn a_police_shortage_warns_then_goes_critical() {
        assert_civic_rule(
            healthy_city(),
            |input, supply| input.police_supply = supply,
            "city.police_shortage",
        );
    }

    #[test]
    fn a_missing_civic_store_is_a_shortage_for_residents() {
        let mut input = healthy_city();
        input.school_supply = None;
        input.clinic_supply = None;
        input.fire_supply = None;
        input.police_supply = None;
        let report = advise(&input);
        assert_eq!(report.critical(), 4);
        let codes: Vec<&str> = report
            .messages
            .iter()
            .map(|message| message.code.as_str())
            .collect();
        assert!(codes.contains(&"city.school_shortage"));
        assert!(codes.contains(&"city.clinic_shortage"));
        assert!(codes.contains(&"city.fire_shortage"));
        assert!(codes.contains(&"city.police_shortage"));
        assert!(!codes.contains(&"city.healthy"));

        let mut empty = AdvisorInput {
            school_supply: None,
            clinic_supply: None,
            fire_supply: None,
            police_supply: None,
            ..AdvisorInput::default()
        };
        empty.population = 0;
        let report = advise(&empty);
        assert_eq!(report.messages[0].code, "city.quiet");
    }

    #[test]
    fn service_shortages_are_critical_and_sorted_first() {
        let mut input = healthy_city();
        input.food_supply = Some(0.1);
        input.water_supply = Some(0.5);
        input.power_supply = Some(0.0);
        let report = advise(&input);
        assert_eq!(report.critical(), 2);
        assert_eq!(report.messages[0].severity, Severity::Critical);
        let codes: Vec<&str> = report
            .messages
            .iter()
            .map(|message| message.code.as_str())
            .collect();
        assert!(codes.contains(&"city.food_shortage"));
        assert!(codes.contains(&"city.power_shortage"));
        assert!(codes.contains(&"city.water_shortage"));
        assert!(!codes.contains(&"city.healthy"));
    }

    #[test]
    fn no_supply_store_is_a_shortage_for_residents() {
        let mut input = healthy_city();
        input.food_supply = None;
        input.water_supply = None;
        input.power_supply = None;
        let report = advise(&input);
        assert_eq!(report.critical(), 3);
    }

    #[test]
    fn budget_deficit_and_debt_are_reported() {
        let mut input = healthy_city();
        input.balance = Some(500.0);
        input.daily_net = Some(-42.0);
        input.debt = Some(50_000.0);
        let report = advise(&input);
        let codes: Vec<&str> = report
            .messages
            .iter()
            .map(|message| message.code.as_str())
            .collect();
        assert!(codes.contains(&"city.budget_deficit"));
        assert!(codes.contains(&"city.debt"));
        assert!(!codes.contains(&"city.healthy"));
    }

    #[test]
    fn bankruptcy_outranks_every_warning() {
        let mut input = healthy_city();
        input.bankrupt = true;
        input.balance = Some(-60_000.0);
        input.daily_net = Some(-500.0);
        let report = advise(&input);
        assert_eq!(report.messages[0].code, "city.bankrupt");
        assert_eq!(report.messages[0].severity, Severity::Critical);
    }

    #[test]
    fn vacant_homes_and_unemployment_are_reported() {
        let mut input = healthy_city();
        input.vacancies = 40;
        input.housing_capacity = 140;
        input.unemployed = 20;
        input.employed = 80;
        let report = advise(&input);
        let codes: Vec<&str> = report
            .messages
            .iter()
            .map(|message| message.code.as_str())
            .collect();
        assert!(codes.contains(&"city.vacant_homes"));
        assert!(codes.contains(&"city.unemployment"));
    }

    #[test]
    fn disconnected_roads_and_long_commutes_are_reported() {
        let mut input = healthy_city();
        input.traffic_components = Some(3);
        input.average_commute_m = 180.0;
        let report = advise(&input);
        let codes: Vec<&str> = report
            .messages
            .iter()
            .map(|message| message.code.as_str())
            .collect();
        assert!(codes.contains(&"city.traffic_disconnected"));
        assert!(codes.contains(&"city.long_commutes"));
    }

    #[test]
    fn empty_city_reports_quiet_and_no_residents() {
        let report = advise(&AdvisorInput::default());
        assert_eq!(report.messages.len(), 1);
        assert_eq!(report.messages[0].code, "city.quiet");
        let input = AdvisorInput {
            housing_capacity: 10,
            ..AdvisorInput::default()
        };
        let report = advise(&input);
        assert_eq!(report.messages[0].code, "city.no_residents");
    }

    #[test]
    fn messages_are_capped_and_codes_are_stable() {
        let mut input = healthy_city();
        input.food_supply = Some(0.0);
        input.water_supply = Some(0.0);
        input.power_supply = Some(0.0);
        input.happiness = 0.1;
        input.unemployed = 50;
        input.employed = 50;
        input.vacancies = 10;
        input.average_commute_m = 300.0;
        input.daily_net = Some(-100.0);
        input.debt = Some(80_000.0);
        input.traffic_components = Some(2);
        input.stalled_sites = 2;
        let report = advise(&input);
        assert!(report.messages.len() <= MAX_ADVISOR_MESSAGES);
        assert_eq!(report.messages[0].code, "city.food_shortage");
        let second = advise(&input);
        assert_eq!(report, second);
    }
}
