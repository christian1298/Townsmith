//! Pure bridge-level save/load compatibility matrix.
//!
//! `world_save` and `world_load` are `#[func]` methods and need a Godot
//! runtime. The layer helpers they call are pure, so this module exercises the
//! save envelope (placements plus every optional layer) without a Godot
//! runtime:
//!
//! - newer and older world saves,
//! - per-layer checksum roundtrips,
//! - unsupported versions and malformed payloads,
//! - foreign seeds.

use super::*;

const SEED: u64 = 2026;
const FOREIGN_SEED: u64 = SEED + 1;

/// One deterministic fixture containing every optional world-save layer.
struct Layers {
    placements: PlacementStore,
    construction: ConstructionStore,
    population: CityRuntime,
    named: NamedCitizens,
    roads: RoadStore,
    lots: LotStore,
    settlements: SettlementStore,
    economy: Economy,
    utilities: UtilityStore,
    services: ServiceStore,
    industry: IndustryStore,
    research: ResearchStore,
    deltas: DeltaStore,
}

impl Layers {
    fn new() -> Self {
        let placements = PlacementStore::new(SEED);

        let mut construction = ConstructionStore::new(4, 1_000_000);
        construction.add_site(7, 64_224).expect("construction site");
        construction.tick(10.0);

        let mut population = CityRuntime::new(SEED);
        population.set_move_in_per_second(2.0);
        population.tick(2.0, 2, &[(1, 2)]);

        let mut named = NamedCitizens::new(SEED);
        named.sync(&population.households);

        let mut roads = RoadStore::new(SEED);
        roads
            .add(
                &RoadRequest {
                    start: [180, 125],
                    end: [200, 125],
                    width_m: 8,
                },
                |_, _| 40,
            )
            .expect("road");

        let mut lots = LotStore::new(SEED);
        lots.add(
            &LotRequest {
                bounds: [182, 128, 198, 144],
                zone: Zone::Residential,
            },
            |_, _| 40,
            &roads,
        )
        .expect("lot");

        let mut settlements = SettlementStore::new(SEED);
        settlements
            .found(&SettlementRequest {
                name: "Riverbend".to_string(),
                owner: "player".to_string(),
                center: [10, 20],
                radius_m: 64,
            })
            .expect("settlement");

        let mut economy = Economy::new(SEED);
        economy
            .take_loan(40_000.0, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS)
            .expect("loan");
        economy.tick(3600.0, 50, 40, 10);

        let mut utilities = UtilityStore::new(SEED);
        utilities
            .add_site(1, UtilityKind::Water, [10.0, 40.0, 20.0])
            .expect("water site");
        utilities
            .add_site(1, UtilityKind::Power, [10.0, 40.0, 20.0])
            .expect("power site");
        utilities.refresh(&[UtilityConsumer {
            id: 1,
            center: [10.0, 20.0],
            residents: 2,
            workers: 2,
        }]);

        let mut services = ServiceStore::new(SEED);
        services
            .add_site(
                1,
                ServiceKind::School,
                ServiceTier::Improved,
                [10.0, 40.0, 20.0],
            )
            .expect("school site");
        services.refresh(&[ServiceConsumer {
            id: 1,
            center: [10.0, 20.0],
            residents: 2,
            workers: 2,
        }]);

        let mut industry = IndustryStore::new(SEED);
        industry.add_site(1, IndustryKind::Farm).expect("farm");
        industry.add_site(2, IndustryKind::Hunt).expect("hunt");
        industry.tick(core_city::DAY_SECONDS, &[(1, 10)].into_iter().collect(), 20);

        let mut research = ResearchStore::new(SEED);
        research
            .unlock(
                "construction.timber_frame",
                &TechTree::default_tree(),
                Age::Wood,
            )
            .expect("research unlock");

        let mut deltas = DeltaStore::new();
        deltas
            .set_height(WorldChunkCoord::new(0, 0), 169, 125, 42)
            .expect("terrain delta");

        Self {
            placements,
            construction,
            population,
            named,
            roads,
            lots,
            settlements,
            economy,
            utilities,
            services,
            industry,
            research,
            deltas,
        }
    }

    /// Compose the same layer order as `world_save`.
    fn compose(&self) -> String {
        let json = self.placements.to_json_pretty().expect("placements json");
        let json = attach_terrain_deltas(&json, &self.deltas.encode(), self.deltas.checksum())
            .expect("attach terrain deltas");
        let json = attach_construction(&json, &self.construction).expect("attach construction");
        let json = attach_population(&json, &self.population).expect("attach population");
        let json = attach_named_citizens(&json, &self.named.to_save()).expect("attach named");
        let json = attach_roads(&json, &self.roads).expect("attach roads");
        let json = attach_lots(&json, &self.lots).expect("attach lots");
        let json = attach_settlements(&json, &self.settlements).expect("attach settlements");
        let json = attach_economy(&json, &self.economy).expect("attach economy");
        let json = attach_utilities(&json, &self.utilities).expect("attach utilities");
        let json = attach_services(&json, &self.services).expect("attach services");
        let json = attach_industry(&json, &self.industry).expect("attach industry");
        attach_research(&json, &self.research).expect("attach research")
    }
}

/// Build a one-layer save envelope with a replacement format version.
fn versioned_layer(name: &str, store: serde_json::Value, format_version: u16) -> String {
    let mut value = serde_json::json!({ name: store });
    value[name]["format_version"] = serde_json::json!(format_version);
    value.to_string()
}

/// Build a one-layer save envelope with one malformed field value.
fn malformed_layer(
    name: &str,
    store: serde_json::Value,
    field: &str,
    replacement: serde_json::Value,
) -> String {
    let mut value = serde_json::json!({ name: store });
    value[name][field] = replacement;
    value.to_string()
}

fn expect_code<T>(label: &str, result: Result<T, (&'static str, String)>, expected: &str) {
    match result {
        Ok(_) => panic!("{label} unexpectedly parsed"),
        Err((code, message)) => {
            assert_eq!(code, expected, "{label}: {message}");
            assert!(!message.is_empty(), "{label} returned an empty error");
        }
    }
}

#[test]
fn every_optional_layer_round_trips_with_checksums() {
    let layers = Layers::new();
    let json = layers.compose();

    let placements = PlacementStore::from_json(&json).expect("parse placements");
    assert_eq!(placements.checksum(), layers.placements.checksum());

    let deltas = terrain_deltas_from_world_json(&json).expect("parse terrain deltas");
    assert_eq!(deltas.checksum(), layers.deltas.checksum());
    assert_eq!(deltas, layers.deltas);

    let construction = construction_from_world_json(&json)
        .expect("parse construction")
        .expect("construction present");
    assert_eq!(construction.checksum(), layers.construction.checksum());

    let population = population_from_world_json(&json)
        .expect("parse population")
        .expect("population present");
    assert_eq!(population.checksum(), layers.population.checksum());

    let named = named_from_world_json(&json, SEED)
        .expect("parse named citizens")
        .expect("named citizens present");
    assert_eq!(named.checksum(), layers.named.checksum());

    let roads = roads_from_world_json(&json)
        .expect("parse roads")
        .expect("roads present");
    assert_eq!(roads.checksum(), layers.roads.checksum());

    let lots = lots_from_world_json(&json)
        .expect("parse lots")
        .expect("lots present");
    assert_eq!(lots.checksum(), layers.lots.checksum());

    let settlements = settlements_from_world_json(&json)
        .expect("parse settlements")
        .expect("settlements present");
    assert_eq!(settlements.checksum(), layers.settlements.checksum());

    let economy = economy_from_world_json(&json)
        .expect("parse economy")
        .expect("economy present");
    // JSON can normalize an f64's shortest text form, so compare the economy
    // semantically rather than by byte checksum.
    assert!((economy.balance - layers.economy.balance).abs() < 0.001);
    assert!((economy.days_elapsed - layers.economy.days_elapsed).abs() < 0.001);
    assert_eq!(economy.loans.len(), layers.economy.loans.len());
    if let (Some(left), Some(right)) = (economy.loans.first(), layers.economy.loans.first()) {
        assert!((left.balance - right.balance).abs() < 0.001);
        assert!((left.payment_per_day - right.payment_per_day).abs() < 0.001);
    }

    let utilities = utilities_from_world_json(&json)
        .expect("parse utilities")
        .expect("utilities present");
    assert_eq!(utilities.checksum(), layers.utilities.checksum());

    let services = services_from_world_json(&json)
        .expect("parse services")
        .expect("services present");
    assert_eq!(services.checksum(), layers.services.checksum());

    let industry = industry_from_world_json(&json)
        .expect("parse industry")
        .expect("industry present");
    assert_eq!(industry.checksum(), layers.industry.checksum());

    let research = research_from_world_json(&json)
        .expect("parse research")
        .expect("research present");
    assert_eq!(research.checksum(), layers.research.checksum());

    // The composed document carries every layer exactly once.
    let envelope: serde_json::Value = serde_json::from_str(&json).expect("envelope json");
    for key in [
        "construction",
        "population",
        "named_citizens",
        "roads",
        "lots",
        "settlements",
        "economy",
        "utilities",
        "services",
        "industry",
        "research",
        "terrain_deltas",
        "terrain_checksum",
    ] {
        assert!(
            envelope.get(key).is_some(),
            "composed save is missing {key}"
        );
    }
}

#[test]
fn old_saves_close_every_optional_layer() {
    let placements = PlacementStore::new(SEED)
        .to_json_pretty()
        .expect("placements json");
    for (label, json) in [("empty object", "{}"), ("placements only", &placements)] {
        assert!(
            construction_from_world_json(json).expect(label).is_none(),
            "{label}: construction must stay closed"
        );
        assert!(
            population_from_world_json(json).expect(label).is_none(),
            "{label}: population must stay closed"
        );
        assert!(
            named_from_world_json(json, SEED).expect(label).is_none(),
            "{label}: named citizens must stay closed"
        );
        assert!(
            roads_from_world_json(json).expect(label).is_none(),
            "{label}: roads must stay closed"
        );
        assert!(
            lots_from_world_json(json).expect(label).is_none(),
            "{label}: lots must stay closed"
        );
        assert!(
            settlements_from_world_json(json).expect(label).is_none(),
            "{label}: settlements must stay closed"
        );
        assert!(
            economy_from_world_json(json).expect(label).is_none(),
            "{label}: economy must stay closed"
        );
        assert!(
            industry_from_world_json(json).expect(label).is_none(),
            "{label}: industry must stay closed"
        );
        assert!(
            utilities_from_world_json(json).expect(label).is_none(),
            "{label}: utilities must stay closed"
        );
        assert!(
            services_from_world_json(json).expect(label).is_none(),
            "{label}: services must stay closed"
        );
        assert!(
            research_from_world_json(json).expect(label).is_none(),
            "{label}: research must stay closed"
        );
        assert!(
            terrain_deltas_from_world_json(json)
                .expect(label)
                .is_empty(),
            "{label}: terrain deltas must default to empty"
        );
    }
}

#[test]
fn rejection_matrix_reports_the_layer_error_code() {
    let layers = Layers::new();

    let construction = serde_json::to_value(&layers.construction).expect("construction value");
    expect_code(
        "construction version 99",
        construction_from_world_json(&versioned_layer("construction", construction, 99)),
        "invalid_construction",
    );

    let population = serde_json::to_value(&layers.population).expect("population value");
    expect_code(
        "population version 99",
        population_from_world_json(&versioned_layer("population", population, 99)),
        "invalid_population",
    );

    let named = serde_json::to_value(layers.named.to_save()).expect("named value");
    expect_code(
        "named version 99",
        named_from_world_json(&versioned_layer("named_citizens", named, 99), SEED),
        "invalid_named_citizens",
    );

    let roads = serde_json::to_value(&layers.roads).expect("roads value");
    expect_code(
        "roads version 99",
        roads_from_world_json(&versioned_layer("roads", roads, 99)),
        "invalid_roads",
    );

    let lots = serde_json::to_value(&layers.lots).expect("lots value");
    expect_code(
        "lots version 99",
        lots_from_world_json(&versioned_layer("lots", lots, 99)),
        "invalid_lots",
    );

    let settlements = serde_json::to_value(&layers.settlements).expect("settlements value");
    expect_code(
        "settlements version 99",
        settlements_from_world_json(&versioned_layer("settlements", settlements, 99)),
        "invalid_settlements",
    );

    let economy = serde_json::to_value(&layers.economy).expect("economy value");
    expect_code(
        "economy version 99",
        economy_from_world_json(&versioned_layer("economy", economy, 99)),
        "invalid_economy",
    );

    let industry = serde_json::to_value(&layers.industry).expect("industry value");
    expect_code(
        "industry version 99",
        industry_from_world_json(&versioned_layer("industry", industry, 99)),
        "invalid_industry",
    );

    let utilities = serde_json::to_value(&layers.utilities).expect("utilities value");
    expect_code(
        "utilities version 99",
        utilities_from_world_json(&versioned_layer("utilities", utilities, 99)),
        "invalid_utilities",
    );

    let services = serde_json::to_value(&layers.services).expect("services value");
    expect_code(
        "services version 99",
        services_from_world_json(&versioned_layer("services", services, 99)),
        "invalid_services",
    );

    let research = serde_json::to_value(&layers.research).expect("research value");
    expect_code(
        "research version 99",
        research_from_world_json(&versioned_layer("research", research, 99)),
        "invalid_research",
    );

    // Malformed field types must be rejected by the typed layer, not by a
    // panic or by silently defaulting the field.
    let construction = serde_json::to_value(&layers.construction).expect("construction value");
    expect_code(
        "construction sites type",
        construction_from_world_json(&malformed_layer(
            "construction",
            construction,
            "sites",
            serde_json::json!(5),
        )),
        "invalid_construction",
    );

    let population = serde_json::to_value(&layers.population).expect("population value");
    expect_code(
        "population households type",
        population_from_world_json(&malformed_layer(
            "population",
            population,
            "households",
            serde_json::json!(5),
        )),
        "invalid_population",
    );

    let named = serde_json::to_value(layers.named.to_save()).expect("named value");
    expect_code(
        "named citizens type",
        named_from_world_json(
            &malformed_layer("named_citizens", named, "citizens", serde_json::json!(5)),
            SEED,
        ),
        "invalid_named_citizens",
    );

    let roads = serde_json::to_value(&layers.roads).expect("roads value");
    expect_code(
        "roads list type",
        roads_from_world_json(&malformed_layer(
            "roads",
            roads,
            "roads",
            serde_json::json!(5),
        )),
        "invalid_roads",
    );

    let lots = serde_json::to_value(&layers.lots).expect("lots value");
    expect_code(
        "lots list type",
        lots_from_world_json(&malformed_layer("lots", lots, "lots", serde_json::json!(5))),
        "invalid_lots",
    );

    let settlements = serde_json::to_value(&layers.settlements).expect("settlements value");
    expect_code(
        "settlements list type",
        settlements_from_world_json(&malformed_layer(
            "settlements",
            settlements,
            "settlements",
            serde_json::json!(5),
        )),
        "invalid_settlements",
    );

    let economy = serde_json::to_value(&layers.economy).expect("economy value");
    expect_code(
        "economy balance type",
        economy_from_world_json(&malformed_layer(
            "economy",
            economy,
            "balance",
            serde_json::json!("rich"),
        )),
        "invalid_economy",
    );

    let industry = serde_json::to_value(&layers.industry).expect("industry value");
    expect_code(
        "industry sites type",
        industry_from_world_json(&malformed_layer(
            "industry",
            industry,
            "sites",
            serde_json::json!(5),
        )),
        "invalid_industry",
    );

    let utilities = serde_json::to_value(&layers.utilities).expect("utilities value");
    expect_code(
        "utilities water sites type",
        utilities_from_world_json(&malformed_layer(
            "utilities",
            utilities,
            "water_sites",
            serde_json::json!(5),
        )),
        "invalid_utilities",
    );

    let services = serde_json::to_value(&layers.services).expect("services value");
    expect_code(
        "services sites type",
        services_from_world_json(&malformed_layer(
            "services",
            services,
            "sites",
            serde_json::json!(5),
        )),
        "invalid_services",
    );

    let research = serde_json::to_value(&layers.research).expect("research value");
    expect_code(
        "research points type",
        research_from_world_json(&malformed_layer(
            "research",
            research,
            "points",
            serde_json::json!("many"),
        )),
        "invalid_research",
    );

    // Every helper reports malformed JSON as `invalid_json`.
    expect_code(
        "construction json",
        construction_from_world_json("{"),
        "invalid_json",
    );
    expect_code(
        "population json",
        population_from_world_json("{"),
        "invalid_json",
    );
    expect_code(
        "named json",
        named_from_world_json("{", SEED),
        "invalid_json",
    );
    expect_code("roads json", roads_from_world_json("{"), "invalid_json");
    expect_code("lots json", lots_from_world_json("{"), "invalid_json");
    expect_code(
        "settlements json",
        settlements_from_world_json("{"),
        "invalid_json",
    );
    expect_code("economy json", economy_from_world_json("{"), "invalid_json");
    expect_code(
        "industry json",
        industry_from_world_json("{"),
        "invalid_json",
    );
    expect_code(
        "utilities json",
        utilities_from_world_json("{"),
        "invalid_json",
    );
    expect_code(
        "services json",
        services_from_world_json("{"),
        "invalid_json",
    );
    expect_code(
        "research json",
        research_from_world_json("{"),
        "invalid_json",
    );
    expect_code(
        "terrain json",
        terrain_deltas_from_world_json("{"),
        "invalid_json",
    );
}

#[test]
fn seeded_layers_detect_a_foreign_seed() {
    let layers = Layers::new();
    let json = layers.compose();

    // `named_from_world_json` owns the expected seed and rejects a foreign one.
    expect_code(
        "named foreign seed",
        named_from_world_json(&json, FOREIGN_SEED),
        "seed_mismatch",
    );

    // The other helpers return the embedded seed; `world_load` gates it after
    // parsing. A foreign seed must survive parsing so that gate can fire.
    let population = population_from_world_json(&json)
        .expect("population")
        .expect("population present");
    assert_eq!(population.seed, SEED);
    assert_ne!(population.seed, FOREIGN_SEED);

    let research = research_from_world_json(&json)
        .expect("research")
        .expect("research present");
    assert_eq!(research.seed, SEED);
    assert_ne!(research.seed, FOREIGN_SEED);

    let industry = industry_from_world_json(&json)
        .expect("industry")
        .expect("industry present");
    assert_eq!(industry.seed, SEED);
    assert_ne!(industry.seed, FOREIGN_SEED);

    let utilities = utilities_from_world_json(&json)
        .expect("utilities")
        .expect("utilities present");
    assert_eq!(utilities.seed, SEED);
    assert_ne!(utilities.seed, FOREIGN_SEED);

    let services = services_from_world_json(&json)
        .expect("services")
        .expect("services present");
    assert_eq!(services.seed, SEED);
    assert_ne!(services.seed, FOREIGN_SEED);

    // The seed mismatch is detected before the payload validator runs, so a
    // foreign version is not allowed to mask it.
    let named = serde_json::to_value(layers.named.to_save()).expect("named value");
    expect_code(
        "named foreign seed with bad version",
        named_from_world_json(&versioned_layer("named_citizens", named, 99), FOREIGN_SEED),
        "seed_mismatch",
    );
}

/// Parse every layer without requiring a particular outcome.
fn parse_all_layers(json: &str) {
    let _ = PlacementStore::from_json(json);
    let _ = terrain_deltas_from_world_json(json);
    let _ = construction_from_world_json(json);
    let _ = population_from_world_json(json);
    let _ = named_from_world_json(json, SEED);
    let _ = roads_from_world_json(json);
    let _ = lots_from_world_json(json);
    let _ = settlements_from_world_json(json);
    let _ = economy_from_world_json(json);
    let _ = utilities_from_world_json(json);
    let _ = services_from_world_json(json);
    let _ = industry_from_world_json(json);
    let _ = research_from_world_json(json);
}

fn next_u64(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}

#[test]
fn bounded_envelope_mutations_never_panic() {
    const CASES: usize = 200;
    let layers = Layers::new();
    let base: serde_json::Value = serde_json::from_str(&layers.compose()).expect("base json");
    let keys: Vec<String> = base.as_object().expect("object").keys().cloned().collect();
    let mut state = 0x5eed_2026_u64;
    for case in 0..CASES {
        let mut value = base.clone();
        let key = &keys[(next_u64(&mut state) as usize) % keys.len()];
        match next_u64(&mut state) % 6 {
            0 => {
                value.as_object_mut().expect("object").remove(key);
            }
            1 => {
                if let Some(layer) = value.get_mut(key).and_then(|value| value.as_object_mut()) {
                    layer.insert("format_version".to_string(), serde_json::json!(99));
                }
            }
            2 => {
                if let Some(layer) = value.get_mut(key).and_then(|value| value.as_object_mut()) {
                    layer.insert("seed".to_string(), serde_json::json!(FOREIGN_SEED));
                }
            }
            3 => {
                if let Some(layer) = value.get_mut(key).and_then(|value| value.as_object_mut()) {
                    if let Some(field) = layer.keys().next().cloned() {
                        layer.insert(field, serde_json::Value::Null);
                    }
                }
            }
            4 => {
                let text = value.to_string();
                let cut = (next_u64(&mut state) as usize) % text.len().max(1);
                let truncated = &text[..cut.min(text.len())];
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    parse_all_layers(truncated);
                }));
                assert!(outcome.is_ok(), "case {case} panicked on truncated JSON");
                continue;
            }
            _ => {
                if let Some(layer) = value.get_mut(key).and_then(|value| value.as_array_mut()) {
                    layer.clear();
                }
            }
        }
        let text = value.to_string();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parse_all_layers(&text);
        }));
        assert!(outcome.is_ok(), "case {case} panicked on {key}");
    }
}
