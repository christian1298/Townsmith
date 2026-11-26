//! Deterministic malformed content-identity envelope corpus (D-034/D-035).
//!
//! These tests exercise only the pure [`verify_world_content`] and
//! [`verify_world_content_inspecting`] helpers. They never start Godot, never
//! read a save path, and never touch the live registry, a `PlacementStore`, or
//! the bridge's recorded identity. A tiny [`SimulatedWorld`] mirrors the
//! bridge commit order so every rejection can prove that no state changed.
//!
//! The corpus pins stable error codes and status spellings, not message text.
//! Malformed JSON and invalid paths live in the existing save-path and
//! malformed-JSON tests; this module focuses on the identity envelope.

use crate::content_identity::{
    stamp_world_content, verify_world_content, verify_world_content_inspecting,
    ActiveContentIdentity, ContentIdentityError, ContentIdentityStatus, VerifiedWorldContent,
    WorldContentIdentity, CONTENT_IDENTITY_FORMAT_VERSION,
};
use core_city::{PlacementRequest, PlacementStore};
use serde_json::{json, Value};

const PACK_ID: &str = "example:copper_age";
const PACK_CHECKSUM: u64 = 0xad4c_6458_69a4_11ed;
const REGISTRY_CHECKSUM: u64 = 0x330a_538d_0780_5b50;
const BASE_CHECKSUM: u64 = 0xc2df_0f24_2f5e_78d2;

type Verifier =
    fn(&str, &ActiveContentIdentity) -> Result<VerifiedWorldContent, ContentIdentityError>;

fn base_active() -> ActiveContentIdentity {
    ActiveContentIdentity::base(BASE_CHECKSUM, BASE_CHECKSUM)
}

fn pack_active() -> ActiveContentIdentity {
    ActiveContentIdentity::pack(PACK_ID, PACK_CHECKSUM, REGISTRY_CHECKSUM, BASE_CHECKSUM)
}

fn other_pack_active() -> ActiveContentIdentity {
    ActiveContentIdentity::pack(
        "example:iron_age",
        PACK_CHECKSUM.wrapping_add(1),
        REGISTRY_CHECKSUM.wrapping_add(1),
        BASE_CHECKSUM,
    )
}

fn recorded_identity() -> WorldContentIdentity {
    WorldContentIdentity {
        format_version: CONTENT_IDENTITY_FORMAT_VERSION,
        pack_id: PACK_ID.to_string(),
        pack_checksum: PACK_CHECKSUM,
        registry_checksum: REGISTRY_CHECKSUM,
    }
}

fn world_json() -> String {
    let mut store = PlacementStore::new(2026);
    let request = PlacementRequest {
        blueprint_id: "local:content_fixture".to_string(),
        blueprint_hash: "blake3:content_fixture".to_string(),
        blueprint_path: "/townsmith/content_fixture.tsbp".to_string(),
        name: "Content Fixture".to_string(),
        bounds_m: [8.0, 6.0, 6.0],
        center_x: 128.0,
        center_z: 128.0,
        rotation_deg: 0,
        material_cost: 100.0,
        resident_capacity: 2,
        job_slots: 0,
        solid_voxels: 100,
        structure: None,
    };
    store.place(&request, |_, _| 32).expect("fixture placement");
    store.to_json_pretty().expect("fixture world json")
}

fn world_store() -> PlacementStore {
    PlacementStore::from_json(&world_json()).expect("fixture store")
}

fn world_value() -> Value {
    serde_json::from_str(&world_json()).expect("fixture world value")
}

fn to_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("fixture json")
}

fn identity_json() -> Value {
    json!({
        "format_version": CONTENT_IDENTITY_FORMAT_VERSION,
        "pack_id": PACK_ID,
        "pack_checksum": "ad4c645869a411ed",
        "registry_checksum": "330a538d07805b50",
    })
}

fn identity_with(field: &str, value: Value) -> Value {
    let mut identity = identity_json();
    identity
        .as_object_mut()
        .expect("identity object")
        .insert(field.to_string(), value);
    identity
}

fn identity_without(field: &str) -> Value {
    let mut identity = identity_json();
    identity
        .as_object_mut()
        .expect("identity object")
        .remove(field);
    identity
}

/// A v1-shaped fixture world with an optional top-level `content` member.
fn envelope_value(version: Value, content: Option<Value>) -> String {
    let mut value = world_value();
    value["format_version"] = version;
    match content {
        Some(content) => {
            value["content"] = content;
        }
        None => {
            value
                .as_object_mut()
                .expect("world object")
                .remove("content");
        }
    }
    to_json(&value)
}

fn envelope(version: u64, content: Option<Value>) -> String {
    envelope_value(json!(version), content)
}

fn without_outer_version() -> String {
    let mut value = world_value();
    value
        .as_object_mut()
        .expect("world object")
        .remove("format_version");
    to_json(&value)
}

fn error_code(json: &str, active: &ActiveContentIdentity) -> &'static str {
    verify_world_content(json, active)
        .expect_err("corpus case must be rejected")
        .code
}

fn assert_rejected(json: &str, active: &ActiveContentIdentity, expected: &'static str) {
    let error = verify_world_content(json, active).expect_err("strict verifier must reject");
    assert_eq!(error.code, expected, "strict: {error:?}");
    let error =
        verify_world_content_inspecting(json, active).expect_err("inspecting verifier must reject");
    assert_eq!(error.code, expected, "inspecting: {error:?}");
}

/// Bridge-like state used to prove a failed identity check commits nothing.
struct SimulatedWorld {
    store: PlacementStore,
    recorded: Option<WorldContentIdentity>,
    read_only: bool,
}

impl SimulatedWorld {
    fn new() -> Self {
        Self {
            store: world_store(),
            recorded: Some(recorded_identity()),
            read_only: false,
        }
    }
}

fn strict_load(
    state: &mut SimulatedWorld,
    json: &str,
    active: &ActiveContentIdentity,
) -> Result<ContentIdentityStatus, ContentIdentityError> {
    let verified = verify_world_content(json, active)?;
    let normalized = verified.normalized_json.as_deref().unwrap_or(json);
    let read_only = verified.read_only();
    state.store = PlacementStore::from_json(normalized).expect("fixture store parses");
    state.recorded = verified.identity;
    state.read_only = read_only;
    Ok(verified.status)
}

fn inspecting_load(
    state: &mut SimulatedWorld,
    json: &str,
    active: &ActiveContentIdentity,
) -> Result<ContentIdentityStatus, ContentIdentityError> {
    let verified = verify_world_content_inspecting(json, active)?;
    let normalized = verified.normalized_json.as_deref().unwrap_or(json);
    let read_only = verified.read_only();
    state.store = PlacementStore::from_json(normalized).expect("fixture store parses");
    state.recorded = verified.identity;
    state.read_only = read_only;
    Ok(verified.status)
}

/// Every structural identity failure with the expected stable code.
///
/// The active registry is one matching pack, so these cases fail before any
/// pack/checksum comparison. `content_identity_unsupported` is reserved for
/// outer/content versions above the supported one; malformed or zero versions
/// are `content_identity_invalid`.
fn structural_corpus() -> Vec<(&'static str, String, &'static str)> {
    const UNEXPECTED: &str = "content_identity_unexpected";
    const REQUIRED: &str = "content_identity_required";
    const INVALID: &str = "content_identity_invalid";
    const UNSUPPORTED: &str = "content_identity_unsupported";

    let null = Value::Null;
    vec![
        // Envelope shape.
        (
            "v1-content-object",
            envelope(1, Some(identity_json())),
            UNEXPECTED,
        ),
        (
            "v1-content-null",
            envelope(1, Some(null.clone())),
            UNEXPECTED,
        ),
        ("v1-content-array", envelope(1, Some(json!([]))), UNEXPECTED),
        (
            "v1-content-string",
            envelope(1, Some(json!("identity"))),
            UNEXPECTED,
        ),
        ("v2-without-content", envelope(2, None), REQUIRED),
        ("v2-content-null", envelope(2, Some(null)), INVALID),
        (
            "v2-content-array",
            envelope(2, Some(json!([identity_json()]))),
            INVALID,
        ),
        (
            "v2-content-string",
            envelope(2, Some(json!("identity"))),
            INVALID,
        ),
        ("v2-content-number", envelope(2, Some(json!(1))), INVALID),
        ("v2-content-bool", envelope(2, Some(json!(true))), INVALID),
        // content.format_version.
        (
            "content-missing-version",
            envelope(2, Some(identity_without("format_version"))),
            INVALID,
        ),
        (
            "content-null-version",
            envelope(2, Some(identity_with("format_version", Value::Null))),
            INVALID,
        ),
        (
            "content-zero-version",
            envelope(2, Some(identity_with("format_version", json!(0)))),
            INVALID,
        ),
        (
            "content-v2-version",
            envelope(2, Some(identity_with("format_version", json!(2)))),
            UNSUPPORTED,
        ),
        (
            "content-v3-version",
            envelope(2, Some(identity_with("format_version", json!(3)))),
            UNSUPPORTED,
        ),
        (
            "content-max-version",
            envelope(2, Some(identity_with("format_version", json!(u64::MAX)))),
            UNSUPPORTED,
        ),
        (
            "content-string-version",
            envelope(2, Some(identity_with("format_version", json!("1")))),
            INVALID,
        ),
        (
            "content-float-version",
            envelope(2, Some(identity_with("format_version", json!(1.0)))),
            INVALID,
        ),
        (
            "content-negative-version",
            envelope(2, Some(identity_with("format_version", json!(-1)))),
            INVALID,
        ),
        (
            "content-empty-object",
            envelope(2, Some(json!({}))),
            INVALID,
        ),
        // pack_id.
        (
            "pack-id-missing",
            envelope(2, Some(identity_without("pack_id"))),
            INVALID,
        ),
        (
            "pack-id-null",
            envelope(2, Some(identity_with("pack_id", Value::Null))),
            INVALID,
        ),
        (
            "pack-id-number",
            envelope(2, Some(identity_with("pack_id", json!(42)))),
            INVALID,
        ),
        (
            "pack-id-empty",
            envelope(2, Some(identity_with("pack_id", json!("")))),
            INVALID,
        ),
        (
            "pack-id-base",
            envelope(2, Some(identity_with("pack_id", json!("base")))),
            INVALID,
        ),
        (
            "pack-id-uppercase",
            envelope(
                2,
                Some(identity_with("pack_id", json!("Example:copper_age"))),
            ),
            INVALID,
        ),
        (
            "pack-id-leading-space",
            envelope(
                2,
                Some(identity_with("pack_id", json!(" example:copper_age"))),
            ),
            INVALID,
        ),
        (
            "pack-id-trailing-space",
            envelope(
                2,
                Some(identity_with("pack_id", json!("example:copper_age "))),
            ),
            INVALID,
        ),
        (
            "pack-id-empty-namespace",
            envelope(2, Some(identity_with("pack_id", json!(":copper_age")))),
            INVALID,
        ),
        (
            "pack-id-empty-pack",
            envelope(2, Some(identity_with("pack_id", json!("example:")))),
            INVALID,
        ),
        (
            "pack-id-three-parts",
            envelope(
                2,
                Some(identity_with("pack_id", json!("example:copper:age"))),
            ),
            INVALID,
        ),
        (
            "pack-id-slash",
            envelope(
                2,
                Some(identity_with("pack_id", json!("example/copper_age"))),
            ),
            INVALID,
        ),
        (
            "pack-id-hyphen",
            envelope(
                2,
                Some(identity_with("pack_id", json!("example:copper-age"))),
            ),
            INVALID,
        ),
        (
            "pack-id-unicode",
            envelope(2, Some(identity_with("pack_id", json!("exämple")))),
            INVALID,
        ),
        // pack_checksum.
        (
            "pack-checksum-missing",
            envelope(2, Some(identity_without("pack_checksum"))),
            INVALID,
        ),
        (
            "pack-checksum-null",
            envelope(2, Some(identity_with("pack_checksum", Value::Null))),
            INVALID,
        ),
        (
            "pack-checksum-number",
            envelope(2, Some(identity_with("pack_checksum", json!(1)))),
            INVALID,
        ),
        (
            "pack-checksum-empty",
            envelope(2, Some(identity_with("pack_checksum", json!("")))),
            INVALID,
        ),
        (
            "pack-checksum-short",
            envelope(2, Some(identity_with("pack_checksum", json!("ad4c")))),
            INVALID,
        ),
        (
            "pack-checksum-15-digits",
            envelope(
                2,
                Some(identity_with("pack_checksum", json!("ad4c645869a411e"))),
            ),
            INVALID,
        ),
        (
            "pack-checksum-17-digits",
            envelope(
                2,
                Some(identity_with("pack_checksum", json!("ad4c645869a411ed0"))),
            ),
            INVALID,
        ),
        (
            "pack-checksum-uppercase",
            envelope(
                2,
                Some(identity_with("pack_checksum", json!("AD4C645869A411ED"))),
            ),
            INVALID,
        ),
        (
            "pack-checksum-non-hex",
            envelope(
                2,
                Some(identity_with("pack_checksum", json!("zzzzzzzzzzzzzzzz"))),
            ),
            INVALID,
        ),
        // registry_checksum.
        (
            "registry-checksum-missing",
            envelope(2, Some(identity_without("registry_checksum"))),
            INVALID,
        ),
        (
            "registry-checksum-null",
            envelope(2, Some(identity_with("registry_checksum", Value::Null))),
            INVALID,
        ),
        (
            "registry-checksum-empty",
            envelope(2, Some(identity_with("registry_checksum", json!("")))),
            INVALID,
        ),
        (
            "registry-checksum-short",
            envelope(2, Some(identity_with("registry_checksum", json!("330a")))),
            INVALID,
        ),
        (
            "registry-checksum-15-digits",
            envelope(
                2,
                Some(identity_with("registry_checksum", json!("330a538d07805b5"))),
            ),
            INVALID,
        ),
        (
            "registry-checksum-17-digits",
            envelope(
                2,
                Some(identity_with(
                    "registry_checksum",
                    json!("330a538d07805b500"),
                )),
            ),
            INVALID,
        ),
        (
            "registry-checksum-uppercase",
            envelope(
                2,
                Some(identity_with(
                    "registry_checksum",
                    json!("330A538D07805B50"),
                )),
            ),
            INVALID,
        ),
        (
            "registry-checksum-non-hex",
            envelope(
                2,
                Some(identity_with(
                    "registry_checksum",
                    json!("gggggggggggggggg"),
                )),
            ),
            INVALID,
        ),
        // Unknown content fields are rejected by `deny_unknown_fields`.
        (
            "content-extra-path-field",
            envelope(2, Some(identity_with("path", json!("/tmp/pack")))),
            INVALID,
        ),
        (
            "content-extra-null-field",
            envelope(2, Some(identity_with("future", Value::Null))),
            INVALID,
        ),
        // Outer version.
        ("outer-version-0", envelope(0, None), UNSUPPORTED),
        ("outer-version-3", envelope(3, None), UNSUPPORTED),
        (
            "outer-version-3-with-content",
            envelope(3, Some(identity_json())),
            UNSUPPORTED,
        ),
        (
            "outer-version-string",
            envelope_value(json!("2"), Some(identity_json())),
            INVALID,
        ),
        (
            "outer-version-null",
            envelope_value(Value::Null, Some(identity_json())),
            INVALID,
        ),
        (
            "outer-version-float",
            envelope_value(json!(2.0), Some(identity_json())),
            INVALID,
        ),
        ("outer-version-missing", without_outer_version(), INVALID),
    ]
}

#[test]
fn v1_with_content_is_unexpected_and_v2_without_content_is_required() {
    for (label, content) in [
        ("object", identity_json()),
        ("null", Value::Null),
        ("array", json!([])),
        ("string", json!("identity")),
    ] {
        let json = envelope(1, Some(content));
        assert_rejected(&json, &base_active(), "content_identity_unexpected");
        assert_rejected(&json, &pack_active(), "content_identity_unexpected");
        assert_eq!(
            error_code(&json, &base_active()),
            "content_identity_unexpected",
            "v1 content {label}"
        );
    }
    for active in [base_active(), pack_active(), other_pack_active()] {
        assert_rejected(&envelope(2, None), &active, "content_identity_required");
    }
}

#[test]
fn structural_corpus_has_stable_codes_and_leaves_state_untouched() {
    let verifiers: [(&str, Verifier); 2] = [
        ("strict", verify_world_content),
        ("inspecting", verify_world_content_inspecting),
    ];
    let active = pack_active();
    let before = world_store();
    let cases = structural_corpus();
    assert!(cases.len() >= 30, "the corpus must stay broad");

    for (label, json, expected) in cases {
        for (name, verify) in verifiers {
            let first = verify(&json, &active).expect_err("corpus case must be rejected");
            let second = verify(&json, &active).expect_err("repeat must be rejected");
            assert_eq!(first.code, expected, "{label}/{name}: {first:?}");
            assert_eq!(
                first, second,
                "{label}/{name}: repeated verification must be identical"
            );
        }

        // Mirror `world_load`: a rejection returns before any store or identity
        // commit. The sentinel world must be byte-for-byte untouched.
        let mut state = SimulatedWorld::new();
        let error = strict_load(&mut state, &json, &active).expect_err("strict load must fail");
        assert_eq!(error.code, expected, "{label}/strict load");
        assert_eq!(state.store, before, "{label}: store changed");
        assert_eq!(
            state.recorded,
            Some(recorded_identity()),
            "{label}: recorded identity changed"
        );
        assert!(!state.read_only, "{label}: read-only flag changed");
    }
}

#[test]
fn outer_version_is_checked_before_content_shape() {
    // Unsupported outer versions win before `content` is interpreted.
    for version in [0, 3, 4] {
        assert_rejected(
            &envelope(version, Some(identity_json())),
            &pack_active(),
            "content_identity_unsupported",
        );
        assert_rejected(
            &envelope(version, Some(Value::Null)),
            &pack_active(),
            "content_identity_unsupported",
        );
    }
    // v1 rejects any content member before parsing it.
    assert_rejected(
        &envelope(1, Some(json!({"format_version": 99}))),
        &pack_active(),
        "content_identity_unexpected",
    );
    // v2 parses the content member and reports its structural failure.
    assert_rejected(
        &envelope(2, Some(json!({"format_version": 99}))),
        &pack_active(),
        "content_identity_unsupported",
    );
    assert_rejected(
        &envelope(2, Some(Value::Null)),
        &pack_active(),
        "content_identity_invalid",
    );
}

#[test]
fn exact_identity_matches_and_normalizes_for_both_loads() {
    let baseline = world_store();
    let json = envelope(2, Some(identity_json()));

    let strict = verify_world_content(&json, &pack_active()).expect("strict match");
    assert_eq!(strict.status, ContentIdentityStatus::Match);
    assert_eq!(strict.status_str(), "match");
    assert!(!strict.read_only());
    assert_eq!(strict.format_version, CONTENT_IDENTITY_FORMAT_VERSION);
    assert_eq!(strict.identity.as_ref(), Some(&recorded_identity()));
    let normalized = strict.normalized_json.as_deref().expect("v1 normalization");
    let value: Value = serde_json::from_str(normalized).expect("normalized json");
    assert_eq!(value["format_version"], 1);
    assert!(value.get("content").is_none());
    let store = PlacementStore::from_json(normalized).expect("normalized store");
    assert_eq!(store, baseline);
    assert_eq!(store.checksum(), baseline.checksum());

    let inspecting = verify_world_content_inspecting(&json, &pack_active()).expect("inspect match");
    assert_eq!(inspecting.status, ContentIdentityStatus::Match);
    assert!(!inspecting.read_only());
    assert_eq!(inspecting.identity.as_ref(), strict.identity.as_ref());
    assert_eq!(inspecting.normalized_json, strict.normalized_json);

    // A stamped save produces the same canonical envelope.
    let stamped = stamp_world_content(&world_json(), &pack_active(), None).expect("stamp");
    assert_eq!(stamped.json, json);
}

#[test]
fn different_pack_or_checksums_are_strict_errors_and_read_only_mismatches() {
    let baseline = world_store();
    let cases = [
        (
            "different-pack",
            identity_with("pack_id", json!("example:iron_age")),
            "content_pack_mismatch",
            "content_save_pack_mismatch",
        ),
        (
            "different-pack-checksum",
            identity_with("pack_checksum", json!("ad4c645869a411ee")),
            "content_pack_checksum_mismatch",
            "content_pack_checksum_mismatch",
        ),
        (
            "different-registry-checksum",
            identity_with("registry_checksum", json!("330a538d07805b51")),
            "content_registry_checksum_mismatch",
            "content_registry_checksum_mismatch",
        ),
    ];
    for (label, identity, strict_code, save_code) in cases {
        let json = envelope(2, Some(identity.clone()));
        let error = verify_world_content(&json, &pack_active()).expect_err("strict mismatch");
        assert_eq!(error.code, strict_code, "{label}/strict");

        let inspected =
            verify_world_content_inspecting(&json, &pack_active()).expect("read-only mismatch");
        assert_eq!(
            inspected.status,
            ContentIdentityStatus::Mismatch,
            "{label}/status"
        );
        assert_eq!(inspected.status_str(), "mismatch", "{label}/spelling");
        assert!(inspected.read_only(), "{label}/read-only");
        let recorded: WorldContentIdentity =
            serde_json::from_value(identity).expect("recorded identity");
        assert_eq!(
            inspected.identity.as_ref(),
            Some(&recorded),
            "{label}/identity"
        );
        let normalized = inspected.normalized_json.as_deref().expect("normalized");
        assert_eq!(
            PlacementStore::from_json(normalized).expect("store"),
            baseline,
            "{label}/store"
        );

        // The save side must refuse the same recorded identity rather than
        // writing a different pack or checksum.
        let error = stamp_world_content(&world_json(), &pack_active(), Some(&recorded))
            .expect_err("save must refuse");
        assert_eq!(error.code, save_code, "{label}/save");
    }
}

#[test]
fn base_only_registry_reports_missing_or_pack_missing() {
    let baseline = world_store();
    let json = envelope(2, Some(identity_json()));

    let error = verify_world_content(&json, &base_active()).expect_err("strict missing pack");
    assert_eq!(error.code, "content_pack_missing");

    let inspected = verify_world_content_inspecting(&json, &base_active()).expect("inspect");
    assert_eq!(inspected.status, ContentIdentityStatus::Missing);
    assert_eq!(inspected.status_str(), "missing");
    assert!(inspected.read_only());
    assert_eq!(inspected.identity.as_ref(), Some(&recorded_identity()));
    let normalized = inspected.normalized_json.as_deref().expect("normalized");
    assert_eq!(
        PlacementStore::from_json(normalized).expect("store"),
        baseline
    );

    // A base-only registry still accepts a plain v1 world.
    let legacy = verify_world_content(&world_json(), &base_active()).expect("base v1");
    assert_eq!(legacy.status, ContentIdentityStatus::Absent);
    assert!(!legacy.read_only());
}

#[test]
fn legacy_unbound_and_absent_v1_stay_writable() {
    let json = world_json();
    for active in [base_active(), pack_active(), other_pack_active()] {
        let strict = verify_world_content(&json, &active).expect("v1 strict");
        let inspecting = verify_world_content_inspecting(&json, &active).expect("v1 inspect");
        assert_eq!(strict, inspecting);
        assert!(strict.identity.is_none());
        assert!(strict.normalized_json.is_none());
        assert!(!strict.read_only());
        let expected = if active.packs == 0 {
            ContentIdentityStatus::Absent
        } else {
            ContentIdentityStatus::LegacyUnbound
        };
        assert_eq!(strict.status, expected);
    }

    // A legacy load stays writable: the next save binds the active pack.
    let stamped = stamp_world_content(&json, &pack_active(), None).expect("legacy save");
    assert_eq!(stamped.status_str(), "new");
    assert_eq!(
        serde_json::from_str::<Value>(&stamped.json).expect("stamped")["format_version"],
        2
    );
}

#[test]
fn unknown_content_fields_are_rejected_but_top_level_fields_are_ignored() {
    // Contract: `content` uses `deny_unknown_fields`.
    for (_label, content) in [
        ("path", identity_with("path", json!("/absolute/pack"))),
        ("extra", identity_with("extra", json!(true))),
        ("nested", identity_with("metadata", json!({"name": "x"}))),
    ] {
        assert_rejected(
            &envelope(2, Some(content)),
            &pack_active(),
            "content_identity_invalid",
        );
    }

    // Top-level extras are not part of the identity contract and are ignored.
    let mut value = world_value();
    value["format_version"] = json!(2);
    value["content"] = identity_json();
    value["future_envelope"] = json!({"version": 9, "note": "ignored"});
    let json = to_json(&value);
    let strict = verify_world_content(&json, &pack_active()).expect("match with top-level extra");
    assert_eq!(strict.status, ContentIdentityStatus::Match);
    let normalized = strict.normalized_json.as_deref().expect("normalized");
    let normalized: Value = serde_json::from_str(normalized).expect("normalized json");
    assert_eq!(normalized["format_version"], 1);
    assert!(normalized.get("content").is_none());
    assert_eq!(normalized["future_envelope"]["version"], 9);
    assert_eq!(
        verify_world_content_inspecting(&json, &pack_active())
            .expect("inspect with extra")
            .status,
        ContentIdentityStatus::Match
    );

    // Extras on a base-only v1 world also change nothing.
    let mut value = world_value();
    value["future_envelope"] = json!({"version": 9});
    let json = to_json(&value);
    let base = verify_world_content(&json, &base_active()).expect("base v1 extra");
    assert_eq!(base.status, ContentIdentityStatus::Absent);
    assert!(base.identity.is_none());
}

#[test]
fn status_spellings_and_read_only_flags_are_stable() {
    for (status, spelling, read_only) in [
        (ContentIdentityStatus::Absent, "absent", false),
        (
            ContentIdentityStatus::LegacyUnbound,
            "legacy_unbound",
            false,
        ),
        (ContentIdentityStatus::Match, "match", false),
        (ContentIdentityStatus::Missing, "missing", true),
        (ContentIdentityStatus::Mismatch, "mismatch", true),
    ] {
        assert_eq!(status.as_str(), spelling);
        assert_eq!(status.read_only(), read_only);
    }
}

#[test]
fn multiple_active_packs_are_rejected_before_the_identity_is_read() {
    let mut active = pack_active();
    active.packs = 2;
    for json in [world_json(), envelope(2, Some(identity_json()))] {
        assert_rejected(&json, &active, "content_multiple_packs_unsupported");
    }
}

#[test]
fn read_only_inspection_retains_identity_and_pins_the_save_refusal() {
    let baseline = world_store();
    let json = envelope(2, Some(identity_json()));
    let mut state = SimulatedWorld {
        store: baseline.clone(),
        recorded: None,
        read_only: false,
    };

    let status = inspecting_load(&mut state, &json, &base_active()).expect("read-only load");
    assert_eq!(status, ContentIdentityStatus::Missing);
    assert!(state.read_only);
    assert_eq!(state.recorded, Some(recorded_identity()));
    assert_eq!(state.store, baseline);

    // `world_save_impl` checks this read-only flag first and returns exactly
    // this stable error before it stamps, writes, or parses a package path.
    let refusal = crate::content_read_only_error();
    assert_eq!(refusal.code, "content_read_only");
    assert!(refusal.message.contains("restore the pack"));
    assert!(
        state.read_only,
        "the refusal must leave the world read-only"
    );

    // A strict load with the same pack restores read-write mode.
    let status = strict_load(&mut state, &json, &pack_active()).expect("strict load");
    assert_eq!(status, ContentIdentityStatus::Match);
    assert!(!state.read_only);
}

#[test]
fn strict_rejection_leaves_world_state_and_store_untouched() {
    let baseline = world_store();
    let cases = [
        (
            "uppercase-checksum",
            envelope(
                2,
                Some(identity_with("pack_checksum", json!("AD4C645869A411ED"))),
            ),
            "content_identity_invalid",
        ),
        (
            "v2-no-content",
            envelope(2, None),
            "content_identity_required",
        ),
        (
            "v1-with-content",
            envelope(1, Some(identity_json())),
            "content_identity_unexpected",
        ),
        (
            "outer-v3",
            envelope(3, Some(identity_json())),
            "content_identity_unsupported",
        ),
    ];
    for (label, json, expected) in cases {
        let mut state = SimulatedWorld::new();
        let error = strict_load(&mut state, &json, &pack_active()).expect_err("strict load fails");
        assert_eq!(error.code, expected, "{label}");
        assert_eq!(state.store, baseline, "{label}: store");
        assert_eq!(
            state.recorded,
            Some(recorded_identity()),
            "{label}: identity"
        );
        assert!(!state.read_only, "{label}: read-only");
    }
}
