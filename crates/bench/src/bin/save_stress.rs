//! Deterministic save/load stress harness for the core city layers.
//!
//! Builds a 120-placement city, composes every core store into one JSON object,
//! and round-trips it repeatedly. This is a core harness, not the Godot world
//! save: it catches layer-local serialization/validation drift without needing
//! a Godot runtime.

use std::collections::BTreeSet;

use core_city::{
    Age, CityRuntime, ConstructionStore, Economy, Home, IndustryKind, IndustryStore, LotRequest,
    LotStore, NamedCitizens, PlacementRequest, PlacementStore, ResearchStore, RoadRequest,
    RoadStore, ServiceKind, ServiceStore, ServiceTier, SettlementRequest, SettlementStore,
    TechTree, UtilityKind, UtilityStore, Workplace, Zone, DAY_SECONDS, DEFAULT_LOAN_RATE_PER_DAY,
    DEFAULT_LOAN_TERM_DAYS,
};

const SEED: u64 = 2026;
const HOUSES: u64 = 100;
const WORKPLACES: u64 = 20;
const SOLID_VOXELS: u64 = 10_000;
const HEIGHT: u16 = 40;

fn main() {
    let cycles = arg_value("--cycles")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(60);
    if cycles == 0 || cycles > 10_000 {
        eprintln!("usage: save_stress [--cycles N] [--smoke]");
        std::process::exit(2);
    }
    match run(cycles) {
        Ok(summary) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&summary).expect("summary json")
            );
        }
        Err(error) => {
            eprintln!("SAVE_STRESS status=failed error={error}");
            std::process::exit(1);
        }
    }
}

fn run(cycles: usize) -> Result<serde_json::Value, String> {
    let city = City::build()?;
    let mut checksum: Option<u64> = None;
    for _ in 0..cycles {
        city.validate()?;
        let value = city.to_value();
        let encoded = serde_json::to_string(&value).map_err(|error| error.to_string())?;
        let decoded: serde_json::Value =
            serde_json::from_str(&encoded).map_err(|error| error.to_string())?;
        city.check_value(&decoded)?;
        let cycle_checksum = combined_checksum(&city);
        if let Some(previous) = checksum {
            if previous != cycle_checksum {
                return Err("combined checksum drifted across cycles".to_string());
            }
        }
        checksum = Some(cycle_checksum);
    }
    let checksum = checksum.ok_or_else(|| "no cycles".to_string())?;
    reject_bad_inputs(&city)?;
    Ok(serde_json::json!({
        "scene": "save_stress",
        "seed": SEED,
        "cycles": cycles,
        "placements": city.placements.placements.len(),
        "construction_sites": city.construction.site_count(),
        "households": city.runtime.households.len(),
        "population": city.runtime.population(),
        "named": city.named.len(),
        "research_completed": city.research.completed_count(),
        "combined_checksum": format!("{checksum:016x}"),
        "status": "ok",
    }))
}

struct City {
    placements: PlacementStore,
    construction: ConstructionStore,
    runtime: CityRuntime,
    economy: Economy,
    industry: IndustryStore,
    utilities: UtilityStore,
    services: ServiceStore,
    research: ResearchStore,
    named: NamedCitizens,
    settlements: SettlementStore,
    roads: RoadStore,
    lots: LotStore,
}

impl City {
    fn build() -> Result<Self, String> {
        let tree = TechTree::default_tree();
        let mut placements = PlacementStore::new(SEED);
        let mut construction = ConstructionStore::new(8, 10_000_000);
        for index in 0..HOUSES + WORKPLACES {
            let workplace = index >= HOUSES;
            let local = index % 20;
            let x = if workplace {
                328.0 + (local % 5) as f32 * 20.0
            } else {
                128.0 + (local % 10) as f32 * 20.0
            };
            let z = if workplace {
                128.0 + (local / 5) as f32 * 20.0
            } else {
                128.0 + (index / 10) as f32 * 20.0
            };
            let request = PlacementRequest {
                blueprint_id: if workplace {
                    "local:save_stress_workshop".to_string()
                } else {
                    "local:save_stress_house".to_string()
                },
                blueprint_hash: "blake3:save-stress".to_string(),
                blueprint_path: "/save_stress/building.tsbp".to_string(),
                name: if workplace { "Workshop" } else { "House" }.to_string(),
                bounds_m: [8.0, 6.0, 8.0],
                center_x: x,
                center_z: z,
                rotation_deg: 0,
                material_cost: 3_000.0,
                resident_capacity: if workplace { 0 } else { 10 },
                job_slots: if workplace { 10 } else { 0 },
                solid_voxels: SOLID_VOXELS,
                structure: None,
            };
            let placement = placements
                .place(&request, |_, _| HEIGHT)
                .map_err(|error| error.to_string())?;
            construction
                .add_site(placement.id, SOLID_VOXELS)
                .map_err(|error| error.to_string())?;
        }
        for _ in 0..20 {
            construction.tick(60.0);
            if construction.sites.values().all(|site| site.is_complete()) {
                break;
            }
        }
        if !construction.sites.values().all(|site| site.is_complete()) {
            return Err("construction did not complete".to_string());
        }

        let homes: Vec<Home> = placements
            .placements
            .iter()
            .filter(|placement| placement.resident_capacity > 0)
            .map(|placement| Home {
                id: placement.id,
                capacity: placement.resident_capacity,
                center: [placement.center[0], placement.center[2]],
            })
            .collect();
        let workplaces: Vec<Workplace> = placements
            .placements
            .iter()
            .filter(|placement| placement.job_slots > 0)
            .map(|placement| Workplace {
                id: placement.id,
                slots: placement.job_slots,
                center: [placement.center[0], placement.center[2]],
            })
            .collect();
        let mut runtime = CityRuntime::new(SEED);
        runtime.set_move_in_per_second(100.0);
        runtime.tick_with_workplaces(60.0, &homes, &workplaces);

        let mut economy = Economy::new(SEED);
        economy
            .take_loan(40_000.0, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS)
            .map_err(|error| error.to_string())?;
        economy.tick(
            DAY_SECONDS,
            runtime.population(),
            runtime.workers(),
            placements.placements.len() as u64,
        );

        let mut industry = IndustryStore::new(SEED);
        for (index, workplace) in workplaces.iter().take(10).enumerate() {
            let kind = match index % 3 {
                0 => IndustryKind::Farm,
                1 => IndustryKind::Lumberjack,
                _ => IndustryKind::Hunt,
            };
            industry
                .add_site(workplace.id, kind)
                .map_err(|error| error.to_string())?;
        }
        industry.tick_with_power(DAY_SECONDS, &runtime.staffing, runtime.population(), 1.0);

        let mut utilities = UtilityStore::new(SEED);
        for (index, workplace) in workplaces.iter().enumerate() {
            let kind = if index % 2 == 0 {
                UtilityKind::Water
            } else {
                UtilityKind::Power
            };
            utilities
                .add_site(
                    workplace.id,
                    kind,
                    [workplace.center[0], 40.0, workplace.center[1]],
                )
                .map_err(|error| error.to_string())?;
        }
        let consumers: Vec<core_city::UtilityConsumer> = placements
            .placements
            .iter()
            .filter(|placement| {
                construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
            })
            .map(|placement| core_city::UtilityConsumer {
                id: placement.id,
                center: [placement.center[0], placement.center[2]],
                residents: runtime
                    .households
                    .get(&placement.id)
                    .map_or(0, |household| household.residents),
                workers: runtime
                    .households
                    .get(&placement.id)
                    .map_or(0, |household| household.workers),
            })
            .collect();
        utilities.refresh(&consumers);

        let mut services = ServiceStore::new(SEED);
        for (index, workplace) in workplaces.iter().take(8).enumerate() {
            let kind = match index % 4 {
                0 => ServiceKind::School,
                1 => ServiceKind::Clinic,
                2 => ServiceKind::Fire,
                _ => ServiceKind::Police,
            };
            services
                .add_site(
                    workplace.id,
                    kind,
                    ServiceTier::Basic,
                    [workplace.center[0], 40.0, workplace.center[1]],
                )
                .map_err(|error| error.to_string())?;
        }
        let service_consumers: Vec<core_city::ServiceConsumer> = consumers
            .iter()
            .map(|consumer| core_city::ServiceConsumer {
                id: consumer.id,
                center: consumer.center,
                residents: consumer.residents,
                workers: consumer.workers,
            })
            .collect();
        services.refresh(&service_consumers);

        let mut research = ResearchStore::new(SEED);
        research.tick(DAY_SECONDS * 10.0, 5.0);
        research
            .unlock("construction.timber_frame", &tree, Age::Wood)
            .map_err(|error| error.to_string())?;

        let mut named = NamedCitizens::new(SEED);
        named.sync(&runtime.households);

        let mut settlements = SettlementStore::new(SEED);
        settlements
            .found(&SettlementRequest {
                name: "Stressford".to_string(),
                owner: "player".to_string(),
                center: [128, 128],
                radius_m: 300,
            })
            .map_err(|error| error.to_string())?;

        let mut roads = RoadStore::new(SEED);
        roads
            .add(
                &RoadRequest {
                    start: [90, 118],
                    end: [130, 118],
                    width_m: 6,
                },
                |_, _| HEIGHT,
            )
            .map_err(|error| error.to_string())?;
        let mut lots = LotStore::new(SEED);
        lots.add(
            &LotRequest {
                bounds: [96, 120, 106, 130],
                zone: Zone::Residential,
            },
            |_, _| HEIGHT,
            &roads,
        )
        .map_err(|error| error.to_string())?;

        let city = Self {
            placements,
            construction,
            runtime,
            economy,
            industry,
            utilities,
            services,
            research,
            named,
            settlements,
            roads,
            lots,
        };
        city.validate()?;
        Ok(city)
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "placements": self.placements,
            "construction": self.construction,
            "population": self.runtime,
            "economy": self.economy,
            "industry": self.industry,
            "utilities": self.utilities,
            "services": self.services,
            "research": self.research,
            "named": self.named.to_save(),
            "settlements": self.settlements,
            "roads": self.roads,
            "lots": self.lots,
        })
    }

    fn check_value(&self, value: &serde_json::Value) -> Result<(), String> {
        let placements: PlacementStore = decode(value, "placements")?;
        let construction: ConstructionStore = decode(value, "construction")?;
        let runtime: CityRuntime = decode(value, "population")?;
        let economy: Economy = decode(value, "economy")?;
        let industry: IndustryStore = decode(value, "industry")?;
        let utilities: UtilityStore = decode(value, "utilities")?;
        let services: ServiceStore = decode(value, "services")?;
        let research: ResearchStore = decode(value, "research")?;
        let named: core_city::NamedSave = decode(value, "named")?;
        let settlements: SettlementStore = decode(value, "settlements")?;
        let roads: RoadStore = decode(value, "roads")?;
        let lots: LotStore = decode(value, "lots")?;
        let expected = self.checksums();
        let actual = [
            placements.checksum(),
            construction.checksum(),
            runtime.checksum(),
            economy.checksum(),
            industry.checksum(),
            utilities.checksum(),
            services.checksum(),
            research.checksum(),
            NamedCitizens::from_save(named)
                .map_err(|error| error.to_string())?
                .checksum(),
            settlements.checksum(),
            roads.checksum(),
            lots.checksum(),
        ];
        // The combined Value roundtrip can normalize f64 text (for example
        // 109.58904109589041 to 109.5890410958904). Compare the economy
        // semantically; the other stores' checksums are byte-stable.
        let mut stable = true;
        for index in [0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11] {
            if actual[index] != expected[index] {
                stable = false;
            }
        }
        let economy_matches = (economy.balance - self.economy.balance).abs() < 1e-6
            && (economy.days_elapsed - self.economy.days_elapsed).abs() < 1e-9
            && economy.loans.len() == self.economy.loans.len();
        if !stable || !economy_matches {
            return Err(format!(
                "per-store state changed during roundtrip: {actual:?} != {expected:?}"
            ));
        }
        Ok(())
    }

    fn checksums(&self) -> [u64; 12] {
        [
            self.placements.checksum(),
            self.construction.checksum(),
            self.runtime.checksum(),
            self.economy.checksum(),
            self.industry.checksum(),
            self.utilities.checksum(),
            self.services.checksum(),
            self.research.checksum(),
            self.named.checksum(),
            self.settlements.checksum(),
            self.roads.checksum(),
            self.lots.checksum(),
        ]
    }

    fn validate(&self) -> Result<(), String> {
        let placement_json = self
            .placements
            .to_json_pretty()
            .map_err(|error| error.to_string())?;
        PlacementStore::from_json(&placement_json).map_err(|error| error.to_string())?;
        self.construction
            .validate()
            .map_err(|error| error.to_string())?;
        self.runtime.validate().map_err(|error| error.to_string())?;
        self.economy.validate().map_err(|error| error.to_string())?;
        self.industry
            .validate()
            .map_err(|error| error.to_string())?;
        self.utilities
            .validate()
            .map_err(|error| error.to_string())?;
        self.services
            .validate()
            .map_err(|error| error.to_string())?;
        self.research
            .validate(&TechTree::default_tree())
            .map_err(|error| error.to_string())?;
        self.named.validate().map_err(|error| error.to_string())?;
        self.settlements
            .validate_loaded()
            .map_err(|error| error.to_string())?;
        self.roads
            .validate_loaded()
            .map_err(|error| error.to_string())?;
        self.lots
            .validate_loaded()
            .map_err(|error| error.to_string())?;

        let completed: BTreeSet<u64> = self
            .construction
            .sites
            .iter()
            .filter(|(_, site)| site.is_complete())
            .map(|(&id, _)| id)
            .collect();
        for placement in &self.placements.placements {
            if !completed.contains(&placement.id) {
                return Err(format!("placement {} has no completed site", placement.id));
            }
        }
        for &id in self.runtime.households.keys() {
            if !completed.contains(&id) {
                return Err(format!("household {id} has no completed home"));
            }
        }
        for &id in self.industry.sites.keys() {
            if !completed.contains(&id) {
                return Err(format!("industry site {id} is not completed"));
            }
        }
        for id in self
            .utilities
            .water_sites
            .keys()
            .chain(self.utilities.power_sites.keys())
        {
            if !completed.contains(id) {
                return Err(format!("utility site {id} is not completed"));
            }
        }
        for &id in self.services.sites.keys() {
            if !completed.contains(&id) {
                return Err(format!("service site {id} is not completed"));
            }
        }
        for ((placement_id, slot), citizen) in &self.named.citizens {
            let household = self
                .runtime
                .households
                .get(placement_id)
                .ok_or_else(|| format!("named citizen {placement_id} has no household"))?;
            if *slot >= household.residents || citizen.id == 0 {
                return Err("named citizen slot is invalid".to_string());
            }
        }
        Ok(())
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    key: &str,
) -> Result<T, String> {
    serde_json::from_value(
        value
            .get(key)
            .ok_or_else(|| format!("missing {key}"))?
            .clone(),
    )
    .map_err(|error| format!("{key}: {error}"))
}

fn reject_bad_inputs(city: &City) -> Result<(), String> {
    if PlacementStore::from_json("{").is_ok() {
        return Err("malformed placement JSON accepted".to_string());
    }
    let mut construction = city.construction.clone();
    construction.format_version = 99;
    if construction.validate().is_ok() {
        return Err("unsupported construction version accepted".to_string());
    }
    let mut population = city.runtime.clone();
    population.format_version = 99;
    if population.validate().is_ok() {
        return Err("unsupported population version accepted".to_string());
    }
    let mut economy = city.economy.clone();
    economy.tax_per_resident_per_day = 500.0;
    if economy.validate().is_ok() {
        return Err("invalid economy accepted".to_string());
    }
    let mut named = city.named.to_save();
    if let Some(first) = named.citizens.first().cloned() {
        named.citizens.push(first);
    }
    if named.validate().is_ok() {
        return Err("duplicate named identity accepted".to_string());
    }
    Ok(())
}

fn combined_checksum(city: &City) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in city.checksums() {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for index in 0..args.len() {
        if args[index] == name {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = args[index].strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_city_round_trips() {
        let summary = run(2).expect("stress");
        assert_eq!(summary["status"], "ok");
        assert_eq!(summary["placements"], 120);
        assert!(summary["population"].as_u64().expect("population") > 0);
    }
}
