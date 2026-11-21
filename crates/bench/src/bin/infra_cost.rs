//! Utility/civic infrastructure cost report.
//!
//! Data-only accounting for the Industrial and Modern probe fixtures. It does
//! not change placement, saves, or the bridge.

use core_city::{ServiceKind, ServiceTier, UtilityKind};

const WATER_INDUSTRIAL: u64 = 67;
const POWER_INDUSTRIAL: u64 = 84;
const WATER_MODERN: u64 = 510;
const POWER_MODERN: u64 = 640;

fn main() {
    let industrial = report(
        "industrial",
        20_000,
        ServiceTier::Improved,
        WATER_INDUSTRIAL,
        POWER_INDUSTRIAL,
        (13, 50, 38, 38),
    );
    let modern = report(
        "modern",
        150_000,
        ServiceTier::Advanced,
        WATER_MODERN,
        POWER_MODERN,
        (49, 196, 144, 144),
    );
    let output = serde_json::json!({
        "scene": "infra_cost",
        "scenarios": [industrial, modern],
        "status": "ok",
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&output).expect("infra cost json")
    );
    if std::env::args().any(|argument| argument == "--smoke") {
        let checksum = checksum_json(&output);
        println!("INFRA_COST_SMOKE checksum={checksum:016x} status=ok");
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    name: &str,
    residents: u64,
    tier: ServiceTier,
    water: u64,
    power: u64,
    services: (u64, u64, u64, u64),
) -> serde_json::Value {
    let utility_construction = water as f64 * UtilityKind::Water.construction_cost()
        + power as f64 * UtilityKind::Power.construction_cost();
    let utility_upkeep = water as f64 * UtilityKind::Water.upkeep_per_day()
        + power as f64 * UtilityKind::Power.upkeep_per_day();
    let service_construction = services.0 as f64 * ServiceKind::School.construction_cost(tier)
        + services.1 as f64 * ServiceKind::Clinic.construction_cost(tier)
        + services.2 as f64 * ServiceKind::Fire.construction_cost(tier)
        + services.3 as f64 * ServiceKind::Police.construction_cost(tier);
    let service_upkeep = services.0 as f64 * ServiceKind::School.upkeep_per_day(tier)
        + services.1 as f64 * ServiceKind::Clinic.upkeep_per_day(tier)
        + services.2 as f64 * ServiceKind::Fire.upkeep_per_day(tier)
        + services.3 as f64 * ServiceKind::Police.upkeep_per_day(tier);
    let construction = utility_construction + service_construction;
    let upkeep = utility_upkeep + service_upkeep;
    serde_json::json!({
        "scenario": name,
        "residents": residents,
        "tier": tier.as_str(),
        "water_sites": water,
        "power_sites": power,
        "school_sites": services.0,
        "clinic_sites": services.1,
        "fire_sites": services.2,
        "police_sites": services.3,
        "utility_construction_cost": utility_construction,
        "service_construction_cost": service_construction,
        "construction_cost": construction,
        "utility_upkeep_per_day": utility_upkeep,
        "service_upkeep_per_day": service_upkeep,
        "upkeep_per_day": upkeep,
        "construction_per_resident": construction / residents as f64,
        "upkeep_per_resident_per_day": upkeep / residents as f64,
    })
}

fn checksum_json(value: &serde_json::Value) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in serde_json::to_vec(value).expect("json") {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_totals_are_stable() {
        let industrial = report(
            "industrial",
            20_000,
            ServiceTier::Improved,
            WATER_INDUSTRIAL,
            POWER_INDUSTRIAL,
            (13, 50, 38, 38),
        );
        assert_eq!(industrial["construction_cost"], 1_219_500.0);
        assert_eq!(industrial["upkeep_per_day"], 879.0);
        let modern = report(
            "modern",
            150_000,
            ServiceTier::Advanced,
            WATER_MODERN,
            POWER_MODERN,
            (49, 196, 144, 144),
        );
        assert_eq!(modern["construction_cost"], 8_261_000.0);
        assert_eq!(modern["upkeep_per_day"], 5_948.0);
    }
}
