//! Deterministic malformed-manifest corpus for the D-042 content registry.
//!
//! This is the companion to the inline tests in [`crate::content_registry`].
//! Every case feeds one exact JSON document to the public parser, asserts the
//! stable [`ContentRegistryError::code`], and fails the test if the parser
//! panics. Valid cases pin base-first sorted IDs and reproducible
//! checksum/serialization output.
//!
//! The public parser takes `&str`, so invalid UTF-8 cannot reach it directly;
//! the corpus covers that boundary with a BOM, lone surrogate escapes, and raw
//! control bytes that JSON forbids.

use std::panic::catch_unwind;

use crate::content_registry::{
    is_canonical_content_checksum, is_valid_content_id, parse_content_registry_manifest,
    ContentRegistryEntry, ContentRegistryError, ContentRegistryKind, ContentRegistryManifest,
    MAX_CONTENT_ID_BYTES, MAX_CONTENT_REGISTRY_BYTES, MAX_CONTENT_REGISTRY_ENTRIES,
};
use serde_json::{json, Value};

const HEX_A: &str = concat!(
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
);
const HEX_B: &str = concat!(
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
);
const MIXED_HEX: &str = concat!(
    "0123456789abcdef",
    "0123456789abcdef",
    "0123456789abcdef",
    "0123456789abcdef"
);

fn hash_a() -> String {
    format!("blake3:{HEX_A}")
}

fn hash_b() -> String {
    format!("blake3:{HEX_B}")
}

fn entry(id: &str, kind: &str, checksum: &str) -> Value {
    json!({
        "id": id,
        "kind": kind,
        "checksum": checksum,
    })
}

fn entry_with_namespace(id: &str, kind: &str, namespace: &str, checksum: &str) -> Value {
    json!({
        "id": id,
        "kind": kind,
        "namespace": namespace,
        "checksum": checksum,
    })
}

fn manifest(entries: Vec<Value>) -> String {
    json!({
        "format_version": 1,
        "ids": entries,
    })
    .to_string()
}

fn manifest_with(format_version: Value, ids: Value) -> String {
    json!({
        "format_version": format_version,
        "ids": ids,
    })
    .to_string()
}

/// Hand-built entry for unvalidated-manifest tests.
fn raw_entry(
    id: &str,
    kind: ContentRegistryKind,
    namespace: Option<&str>,
    checksum: &str,
) -> ContentRegistryEntry {
    ContentRegistryEntry {
        id: id.to_string(),
        kind,
        namespace: namespace.map(str::to_string),
        checksum: Some(checksum.to_string()),
    }
}

/// Parse, asserting that the parser does not panic on the way to an error.
fn parse_error(json: &str) -> ContentRegistryError {
    match catch_unwind(|| parse_content_registry_manifest(json)) {
        Ok(Ok(manifest)) => panic!("expected rejection, parsed {manifest:?}"),
        Ok(Err(error)) => error,
        Err(_) => panic!("content-registry parser panicked"),
    }
}

fn assert_invalid(label: &str, json: &str) {
    let error = parse_error(json);
    assert_eq!(
        error.code(),
        "content.invalid_registry",
        "case `{label}` returned {error}"
    );
}

fn assert_invalid_cases(cases: Vec<(&str, String)>) {
    for (label, json) in cases {
        assert_invalid(label, &json);
    }
}

fn assert_unsupported(version: u16) {
    let json = manifest_with(json!(version), json!([]));
    let error = parse_error(&json);
    assert_eq!(error, ContentRegistryError::UnsupportedVersion { version });
    assert_eq!(error.code(), "content.unsupported_registry_version");
}

fn mixed_registry() -> ContentRegistryManifest {
    parse_content_registry_manifest(&manifest(vec![
        entry_with_namespace("mymod", "mod", "mymod", &hash_a()),
        entry_with_namespace("mymod:pack", "resource_pack", "mymod:pack", &hash_b()),
        entry_with_namespace("other", "mod", "alias", &hash_a()),
        entry("zulu", "mod", &hash_b()),
    ]))
    .expect("mixed fixture")
}

// ---------------------------------------------------------------------------
// JSON document shape
// ---------------------------------------------------------------------------

#[test]
fn rejects_empty_and_truncated_documents() {
    assert_invalid_cases(vec![
        ("empty", String::new()),
        ("whitespace-only", " \n\t\r\n".to_string()),
        ("open-brace", "{".to_string()),
        ("open-bracket", "[".to_string()),
        ("truncated-object", r#"{"format_version":1"#.to_string()),
        (
            "truncated-ids-value",
            r#"{"format_version":1,"ids":"#.to_string(),
        ),
        (
            "truncated-ids-array",
            r#"{"format_version":1,"ids":["#.to_string(),
        ),
        (
            "truncated-entry-string",
            r#"{"format_version":1,"ids":[{"id":"mymod"#.to_string(),
        ),
    ]);
}

#[test]
fn rejects_trailing_bytes_after_the_document() {
    let h = hash_a();
    assert_invalid_cases(vec![
        (
            "garbage",
            r#"{"format_version":1,"ids":[]}garbage"#.to_string(),
        ),
        (
            "second-document",
            r#"{"format_version":1,"ids":[]}{"format_version":1,"ids":[]}"#.to_string(),
        ),
        (
            "trailing-comma-object",
            r#"{"format_version":1,"ids":[],}"#.to_string(),
        ),
        (
            "trailing-comma-entry",
            format!(
                r#"{{"format_version":1,"ids":[{{"id":"mymod","kind":"mod","checksum":"{h}",}}]}}"#
            ),
        ),
        (
            "comment",
            r#"{"format_version":1,/* no comments */"ids":[]}"#.to_string(),
        ),
        ("single-quoted", "{'format_version':1,'ids':[]}".to_string()),
        ("nan", r#"{"format_version":NaN,"ids":[]}"#.to_string()),
    ]);
}

#[test]
fn rejects_non_utf8ish_escapes_and_raw_control_bytes() {
    let h = hash_a();
    let with_id = |id_json: &str| {
        format!(
            r#"{{"format_version":1,"ids":[{{"id":{id_json},"kind":"mod","checksum":"{h}"}}]}}"#
        )
    };

    assert_invalid("raw-nul", &with_id("\"my\u{0}mod\""));
    assert_invalid("raw-newline", &with_id("\"my\nmod\""));
    assert_invalid("raw-tab", &with_id("\"my\tmod\""));
    assert_invalid("lone-high-surrogate", &with_id(r#""\ud800""#));
    assert_invalid("lone-low-surrogate", &with_id(r#""\udc00""#));
    assert_invalid("invalid-escape", &with_id(r#""\q""#));

    let bom = format!("\u{feff}{}", manifest(vec![]));
    assert_invalid("bom", &bom);
}

#[test]
fn rejects_wrong_top_level_types_and_null_shapes() {
    assert_invalid_cases(vec![
        ("top-level-array", "[]".to_string()),
        ("top-level-null", "null".to_string()),
        ("top-level-bool", "true".to_string()),
        ("top-level-number", "42".to_string()),
        ("top-level-string", "\"manifest\"".to_string()),
        ("empty-object", "{}".to_string()),
        ("missing-version", r#"{"ids":[]}"#.to_string()),
        ("missing-ids", r#"{"format_version":1}"#.to_string()),
        ("ids-object", r#"{"format_version":1,"ids":{}}"#.to_string()),
        (
            "ids-string",
            r#"{"format_version":1,"ids":"empty"}"#.to_string(),
        ),
        ("ids-number", r#"{"format_version":1,"ids":0}"#.to_string()),
        ("ids-bool", r#"{"format_version":1,"ids":true}"#.to_string()),
        ("ids-null", r#"{"format_version":1,"ids":null}"#.to_string()),
    ]);
}

#[test]
fn rejects_duplicate_object_keys() {
    let h = hash_a();
    assert_invalid_cases(vec![
        (
            "duplicate-format-version",
            r#"{"format_version":1,"format_version":1,"ids":[]}"#.to_string(),
        ),
        (
            "duplicate-ids",
            r#"{"format_version":1,"ids":[],"ids":[]}"#.to_string(),
        ),
        (
            "duplicate-entry-id",
            format!(
                r#"{{"format_version":1,"ids":[{{"id":"one","id":"two","kind":"mod","checksum":"{h}"}}]}}"#
            ),
        ),
        (
            "duplicate-entry-kind",
            format!(
                r#"{{"format_version":1,"ids":[{{"id":"mymod","kind":"mod","kind":"mod","checksum":"{h}"}}]}}"#
            ),
        ),
    ]);
}

#[test]
fn rejects_unknown_fields_at_every_depth() {
    assert_invalid_cases(vec![
        (
            "top-level-scalar",
            json!({"format_version": 1, "ids": [], "extra": true}).to_string(),
        ),
        (
            "top-level-nested",
            json!({
                "format_version": 1,
                "ids": [],
                "metadata": {"a": {"b": {"c": [1, 2, 3]}}},
            })
            .to_string(),
        ),
        ("top-level-null", {
            json!({"format_version": 1, "ids": [], "extra": null}).to_string()
        }),
        ("entry-scalar", {
            let mut value = entry("mymod", "mod", &hash_a());
            value["extra"] = json!(true);
            manifest(vec![value])
        }),
        ("entry-nested", {
            let mut value = entry("mymod", "mod", &hash_a());
            value["extra"] = json!({"a": {"b": {"c": {"d": 1}}}});
            manifest(vec![value])
        }),
        ("entry-null", {
            let mut value = entry("mymod", "mod", &hash_a());
            value["extra"] = Value::Null;
            manifest(vec![value])
        }),
    ]);
}

#[test]
fn rejects_missing_or_null_required_entry_fields() {
    let h = hash_a();
    assert_invalid_cases(vec![
        ("entry-null", manifest(vec![Value::Null])),
        ("entry-array", manifest(vec![json!([])])),
        ("entry-string", manifest(vec![json!("mymod")])),
        ("entry-empty-object", manifest(vec![json!({})])),
        (
            "entry-missing-id",
            manifest(vec![json!({"kind": "mod", "checksum": h})]),
        ),
        (
            "entry-missing-kind",
            manifest(vec![json!({"id": "mymod", "checksum": h})]),
        ),
        (
            "entry-missing-checksum",
            manifest(vec![json!({"id": "mymod", "kind": "mod"})]),
        ),
        (
            "entry-id-null",
            manifest(vec![json!({"id": null, "kind": "mod", "checksum": h})]),
        ),
        (
            "entry-kind-null",
            manifest(vec![json!({"id": "mymod", "kind": null, "checksum": h})]),
        ),
        (
            "entry-checksum-null",
            manifest(vec![
                json!({"id": "mymod", "kind": "mod", "checksum": null}),
            ]),
        ),
        (
            "entry-id-number",
            manifest(vec![json!({"id": 1, "kind": "mod", "checksum": h})]),
        ),
        (
            "entry-kind-number",
            manifest(vec![json!({"id": "mymod", "kind": 1, "checksum": h})]),
        ),
        (
            "entry-checksum-number",
            manifest(vec![json!({"id": "mymod", "kind": "mod", "checksum": 1})]),
        ),
    ]);
}

#[test]
fn accepts_whitespace_around_a_valid_document() {
    let json = format!(
        "\n\t \r\n{}\n ",
        manifest(vec![entry("mymod", "mod", &hash_a())])
    );
    let parsed = parse_content_registry_manifest(&json).expect("whitespace is insignificant");
    assert_eq!(parsed.implicit_ids(), ["base", "mymod"]);
}

// ---------------------------------------------------------------------------
// format_version
// ---------------------------------------------------------------------------

#[test]
fn rejects_unsupported_versions_with_the_stable_code() {
    for version in [0, 2, 3, u16::MAX] {
        assert_unsupported(version);
    }
}

#[test]
fn rejects_wrong_format_version_types() {
    assert_invalid_cases(vec![
        ("missing", r#"{"ids":[]}"#.to_string()),
        ("null", manifest_with(Value::Null, json!([]))),
        ("string", manifest_with(json!("1"), json!([]))),
        ("bool", manifest_with(json!(true), json!([]))),
        ("negative", manifest_with(json!(-1), json!([]))),
        ("fractional", manifest_with(json!(1.5), json!([]))),
        (
            "integral-float",
            r#"{"format_version":1.0,"ids":[]}"#.to_string(),
        ),
        ("exponent", r#"{"format_version":1e0,"ids":[]}"#.to_string()),
        (
            "u16-overflow",
            manifest_with(json!(u32::from(u16::MAX) + 1), json!([])),
        ),
        ("array", manifest_with(json!([1]), json!([]))),
        ("object", manifest_with(json!({"version": 1}), json!([]))),
    ]);
}

// ---------------------------------------------------------------------------
// IDs and namespaces
// ---------------------------------------------------------------------------

#[test]
fn rejects_invalid_ids() {
    let mut ids: Vec<String> = [
        "",
        "MyMod",
        "MOD",
        "myMod",
        "mymod/PACK",
        "mymod/pack",
        "../mymod",
        "/mymod",
        "mymod\\pack",
        "mymod.pack",
        "mymod pack",
        " mymod",
        "mymod ",
        "\tmymod",
        "mymod\n",
        "my\u{0}mod",
        ":",
        "mymod:",
        ":mymod",
        "a:b:c",
        "mymod::pack",
        "mymod:pack:extra",
        "..",
        ".",
        "my+mod",
        "my@mod",
        "möd",
        "mymod:π",
        "🚀",
    ]
    .iter()
    .map(|id| (*id).to_string())
    .collect();
    ids.push("a".repeat(MAX_CONTENT_ID_BYTES + 1));

    for id in &ids {
        let document = manifest(vec![entry(id, "mod", &hash_a())]);
        assert_invalid(&format!("id `{id}`"), &document);
    }
}

#[test]
fn accepts_boundary_and_well_formed_ids() {
    let max = "a".repeat(MAX_CONTENT_ID_BYTES);
    for id in [
        "a",
        "0",
        "9",
        "mymod",
        "my-mod_2",
        "mymod:pack",
        "a-b_c:d-e_f",
        "base:pack",
        max.as_str(),
    ] {
        assert!(is_valid_content_id(id), "`{id}` should be a valid id");
        let parsed = parse_content_registry_manifest(&manifest(vec![entry(id, "mod", &hash_a())]))
            .expect("valid id");
        assert_eq!(parsed.ids[0].id, id);
    }
}

#[test]
fn rejects_explicit_base_claims() {
    let h = hash_a();
    for kind in ["mod", "resource_pack"] {
        let error = parse_error(&manifest(vec![entry("base", kind, &h)]));
        assert_eq!(error.code(), "content.invalid_registry", "id base/{kind}");
        assert!(error.to_string().contains("implicit"));
    }

    let namespace = parse_error(&manifest(vec![entry_with_namespace(
        "mymod", "mod", "base", &h,
    )]));
    assert_eq!(namespace.code(), "content.invalid_registry");
    assert!(namespace.to_string().contains("implicit"));

    // Only the exact token `base` is implicit; `base:pack` is an ordinary ID.
    let child = parse_content_registry_manifest(&manifest(vec![entry("base:pack", "mod", &h)]))
        .expect("base:pack is a normal ID");
    assert_eq!(child.implicit_ids(), ["base", "base:pack"]);
}

#[test]
fn rejects_invalid_namespaces() {
    let overlong = "a".repeat(MAX_CONTENT_ID_BYTES + 1);
    let namespaces = [
        "",
        "MyMod",
        "mymod/../other",
        "mymod:",
        ":mymod",
        "a:b:c",
        "mymod::pack",
        "mymod:pack:extra",
        "mymod.pack",
        " mymod",
        "mymod ",
        "\tmymod",
        "mymod\n",
        "möd",
        overlong.as_str(),
    ];

    for namespace in namespaces {
        let document = manifest(vec![entry_with_namespace(
            "mymod",
            "mod",
            namespace,
            &hash_a(),
        )]);
        assert_invalid(&format!("namespace `{namespace}`"), &document);
    }
}

#[test]
fn namespace_is_optional_for_mods_and_resource_packs() {
    for value in [
        entry("mymod", "mod", &hash_a()),
        entry_with_namespace("mymod", "mod", "mymod", &hash_a()),
        entry("mymod:pack", "resource_pack", &hash_b()),
        entry_with_namespace("mymod:pack", "resource_pack", "mymod:pack", &hash_b()),
    ] {
        let parsed =
            parse_content_registry_manifest(&manifest(vec![value])).expect("namespace optional");
        assert_eq!(parsed.ids.len(), 1);
    }

    let mut null_namespace = entry("mymod", "mod", &hash_a());
    null_namespace["namespace"] = Value::Null;
    let parsed = parse_content_registry_manifest(&manifest(vec![null_namespace]))
        .expect("null namespace means absent");
    assert_eq!(parsed.ids[0].namespace, None);
}

#[test]
fn an_unrelated_namespace_is_accepted_as_an_alias() {
    let parsed = parse_content_registry_manifest(&manifest(vec![entry_with_namespace(
        "mymod",
        "mod",
        "group",
        &hash_a(),
    )]))
    .expect("namespace alias");

    assert_eq!(parsed.ids[0].namespace.as_deref(), Some("group"));
    assert_eq!(parsed.select(&["group"]).expect("alias"), ["base", "mymod"]);
    assert!(parsed.provides("group"));
    assert!(parsed.provides("group:addon"));
    assert_eq!(parsed.implicit_ids(), ["base", "mymod"]);
}

#[test]
fn similar_but_distinct_ids_are_not_duplicates() {
    let parsed = parse_content_registry_manifest(&manifest(vec![
        entry("mymod", "mod", &hash_a()),
        entry("my-mod", "mod", &hash_b()),
        entry("my_mod", "mod", &hash_a()),
        entry("mymod2", "mod", &hash_b()),
    ]))
    .expect("distinct ids");

    assert_eq!(
        parsed.implicit_ids(),
        ["base", "my-mod", "my_mod", "mymod", "mymod2"]
    );
}

#[test]
fn case_variant_ids_fail_as_invalid_ids_before_duplicate_checks() {
    for variant in ["MyMod", "MYMOD", "myMod"] {
        let error = parse_error(&manifest(vec![
            entry("mymod", "mod", &hash_a()),
            entry(variant, "mod", &hash_b()),
        ]));
        assert_eq!(error.code(), "content.invalid_registry", "{variant}");
        assert!(error.to_string().contains("invalid content id"));
    }
}

// ---------------------------------------------------------------------------
// kind
// ---------------------------------------------------------------------------

#[test]
fn rejects_missing_and_invalid_kinds() {
    let h = hash_a();
    let missing = manifest(vec![json!({"id": "mymod", "checksum": h})]);
    assert_invalid("missing-kind", &missing);

    for kind in [
        Value::Null,
        json!(""),
        json!("asset"),
        json!("Mod"),
        json!("MOD"),
        json!("ResourcePack"),
        json!("resource-pack"),
        json!("resource pack"),
        json!("resourcepack"),
        json!(1),
        json!(true),
        json!(["mod"]),
        json!({"name": "mod"}),
    ] {
        let document = manifest(vec![json!({
            "id": "mymod",
            "kind": kind,
            "checksum": h,
        })]);
        assert_invalid(&format!("kind {kind}"), &document);
    }
}

#[test]
fn accepts_both_documented_kinds() {
    for (label, kind, expected) in [
        ("mod", "mod", ContentRegistryKind::Mod),
        (
            "resource_pack",
            "resource_pack",
            ContentRegistryKind::ResourcePack,
        ),
    ] {
        let parsed =
            parse_content_registry_manifest(&manifest(vec![entry("mymod", kind, &hash_a())]))
                .expect(label);
        assert_eq!(parsed.ids[0].kind, expected);
        assert_eq!(parsed.ids[0].kind.as_str(), label);
    }
}

// ---------------------------------------------------------------------------
// checksum
// ---------------------------------------------------------------------------

#[test]
fn rejects_non_canonical_checksums() {
    let lower = "a".repeat(64);
    let invalid = [
        String::new(),
        "blake3:".to_string(),
        format!("blake3:{}", "a".repeat(63)),
        format!("blake3:{}", "a".repeat(65)),
        format!("blake3:{}", "A".repeat(64)),
        lower.clone(),
        format!("sha256:{lower}"),
        format!("BLAKE3:{}", "a".repeat(64)),
        format!("blake3:{}", "g".repeat(64)),
        format!("blake3:{}", "0".repeat(63) + "g"),
        format!(" blake3:{}", "a".repeat(64)),
        format!("blake3:{} ", "a".repeat(64)),
        format!("blake3:{}\n", "a".repeat(64)),
        format!("blake3:{}:extra", "a".repeat(64)),
        format!("blake3:0x{}", "a".repeat(62)),
        "blake3:🔒".to_string(),
    ];

    for checksum in &invalid {
        assert!(!is_canonical_content_checksum(checksum));
        let document = manifest(vec![entry("mymod", "mod", checksum)]);
        assert_invalid(&format!("checksum `{checksum}`"), &document);
    }

    let missing = manifest(vec![json!({"id": "mymod", "kind": "mod"})]);
    let error = parse_error(&missing);
    assert_eq!(error.code(), "content.invalid_registry");
    assert!(error.to_string().contains("missing a checksum"));

    let null = manifest(vec![
        json!({"id": "mymod", "kind": "mod", "checksum": null}),
    ]);
    let error = parse_error(&null);
    assert_eq!(error.code(), "content.invalid_registry");
    assert!(error.to_string().contains("missing a checksum"));
}

#[test]
fn accepts_and_preserves_canonical_checksums() {
    for checksum in [
        hash_a(),
        hash_b(),
        format!("blake3:{}", "0".repeat(64)),
        format!("blake3:{}", "f".repeat(64)),
        format!("blake3:{MIXED_HEX}"),
    ] {
        assert!(is_canonical_content_checksum(&checksum));
        let parsed =
            parse_content_registry_manifest(&manifest(vec![entry("mymod", "mod", &checksum)]))
                .expect("canonical checksum");
        assert_eq!(parsed.ids[0].checksum.as_deref(), Some(checksum.as_str()));
        assert_eq!(
            serde_json::to_string(&parsed).expect("serialize"),
            serde_json::to_string(&parsed).expect("serialize again")
        );
    }
}

#[test]
fn resource_packs_require_a_canonical_checksum() {
    let missing = manifest(vec![json!({
        "id": "mymod:pack",
        "kind": "resource_pack",
    })]);
    assert_invalid("pack-missing-checksum", &missing);

    let malformed = manifest(vec![entry("mymod:pack", "resource_pack", "blake3:short")]);
    assert_invalid("pack-malformed-checksum", &malformed);

    let parsed = parse_content_registry_manifest(&manifest(vec![entry_with_namespace(
        "mymod:pack",
        "resource_pack",
        "mymod:pack",
        &hash_a(),
    )]))
    .expect("pack with checksum");
    assert_eq!(parsed.ids[0].kind, ContentRegistryKind::ResourcePack);
    assert_eq!(parsed.ids[0].checksum.as_deref(), Some(hash_a().as_str()));
}

// ---------------------------------------------------------------------------
// duplicate claims
// ---------------------------------------------------------------------------

#[test]
fn rejects_duplicate_ids_and_namespaces_within_one_kind() {
    let h_a = hash_a();
    let h_b = hash_b();
    let cases = vec![
        (
            "duplicate-id",
            vec![entry("mymod", "mod", &h_a), entry("mymod", "mod", &h_b)],
        ),
        (
            "duplicate-namespace",
            vec![
                entry_with_namespace("one", "mod", "shared", &h_a),
                entry_with_namespace("two", "mod", "shared", &h_b),
            ],
        ),
        (
            "id-collides-with-namespace",
            vec![
                entry("shared", "mod", &h_a),
                entry_with_namespace("other", "mod", "shared", &h_b),
            ],
        ),
        (
            "namespace-collides-with-id",
            vec![
                entry_with_namespace("other", "mod", "shared", &h_a),
                entry("shared", "mod", &h_b),
            ],
        ),
        (
            "three-way",
            vec![
                entry("same", "mod", &h_a),
                entry("same", "mod", &h_b),
                entry("same", "mod", &h_a),
            ],
        ),
    ];

    for (label, entries) in cases {
        let error = parse_error(&manifest(entries));
        assert_eq!(error.code(), "content.invalid_registry", "{label}");
        assert!(
            error.to_string().contains("duplicate"),
            "case `{label}` returned {error}"
        );
    }
}

#[test]
fn rejects_cross_kind_claims_with_kind_mismatch() {
    let same_id = parse_error(&manifest(vec![
        entry("mymod", "mod", &hash_a()),
        entry("mymod", "resource_pack", &hash_b()),
    ]));
    assert_eq!(
        same_id,
        ContentRegistryError::KindMismatch {
            id: "mymod".to_string(),
            expected: ContentRegistryKind::Mod,
            found: ContentRegistryKind::ResourcePack,
        }
    );
    assert_eq!(same_id.code(), "content.kind_mismatch");

    let reverse = parse_error(&manifest(vec![
        entry("mymod", "resource_pack", &hash_a()),
        entry("mymod", "mod", &hash_b()),
    ]));
    assert_eq!(
        reverse,
        ContentRegistryError::KindMismatch {
            id: "mymod".to_string(),
            expected: ContentRegistryKind::ResourcePack,
            found: ContentRegistryKind::Mod,
        }
    );

    let namespace = parse_error(&manifest(vec![
        entry_with_namespace("mymod", "mod", "shared", &hash_a()),
        entry_with_namespace("other", "resource_pack", "shared", &hash_b()),
    ]));
    assert_eq!(
        namespace,
        ContentRegistryError::KindMismatch {
            id: "shared".to_string(),
            expected: ContentRegistryKind::Mod,
            found: ContentRegistryKind::ResourcePack,
        }
    );
}

#[test]
fn accepts_namespace_equal_to_id_and_single_claims() {
    let parsed = parse_content_registry_manifest(&manifest(vec![
        entry_with_namespace("mymod", "mod", "mymod", &hash_a()),
        entry_with_namespace("mymod:pack", "resource_pack", "mymod:pack", &hash_b()),
    ]))
    .expect("namespace may equal id");

    assert_eq!(parsed.ids[0].namespace.as_deref(), Some("mymod"));
    assert_eq!(parsed.ids[1].namespace.as_deref(), Some("mymod:pack"));

    // A namespace that differs from the id adds a second claim but is accepted
    // when nothing else claims it.
    let alias = parse_content_registry_manifest(&manifest(vec![entry_with_namespace(
        "mymod",
        "mod",
        "mymod:extra",
        &hash_a(),
    )]))
    .expect("distinct alias");
    assert_eq!(
        alias.select(&["mymod:extra"]).expect("alias"),
        ["base", "mymod"]
    );
}

// ---------------------------------------------------------------------------
// caps
// ---------------------------------------------------------------------------

#[test]
fn entry_cap_accepts_exactly_the_limit_and_rejects_one_more() {
    let at_cap: Vec<Value> = (0..MAX_CONTENT_REGISTRY_ENTRIES)
        .map(|index| entry(&format!("mod{index}"), "mod", &hash_a()))
        .collect();
    let parsed =
        parse_content_registry_manifest(&manifest(at_cap)).expect("entry cap boundary is valid");
    assert_eq!(parsed.ids.len(), MAX_CONTENT_REGISTRY_ENTRIES);

    let ids = parsed.implicit_ids();
    assert_eq!(ids.len(), MAX_CONTENT_REGISTRY_ENTRIES + 1);
    assert_eq!(ids[0], "base");
    assert!(ids[1..].windows(2).all(|pair| pair[0] < pair[1]));

    let requested: Vec<&str> = parsed.ids.iter().map(|entry| entry.id.as_str()).collect();
    let selected = parsed.select(&requested).expect("select all entries");
    assert_eq!(selected.len(), MAX_CONTENT_REGISTRY_ENTRIES + 1);
    assert!(selected.windows(2).all(|pair| pair[0] < pair[1]));

    let over_cap: Vec<Value> = (0..=MAX_CONTENT_REGISTRY_ENTRIES)
        .map(|index| entry(&format!("mod{index}"), "mod", &hash_a()))
        .collect();
    let error = parse_error(&manifest(over_cap));
    assert_eq!(error.code(), "content.invalid_registry");
    assert!(error.to_string().contains("at most"));
}

#[test]
fn byte_cap_accepts_exactly_one_mebibyte_and_rejects_one_more() {
    let base = manifest(vec![]);
    assert!(base.len() < MAX_CONTENT_REGISTRY_BYTES);

    let mut at_cap = base;
    at_cap.push_str(&" ".repeat(MAX_CONTENT_REGISTRY_BYTES - at_cap.len()));
    assert_eq!(at_cap.len(), MAX_CONTENT_REGISTRY_BYTES);
    parse_content_registry_manifest(&at_cap).expect("exactly 1 MiB is valid");

    let mut over_cap = at_cap;
    over_cap.push(' ');
    let error = parse_error(&over_cap);
    assert_eq!(error.code(), "content.invalid_registry");
    assert!(error.to_string().contains("exceeds"));
}

// ---------------------------------------------------------------------------
// select
// ---------------------------------------------------------------------------

#[test]
fn select_empty_and_base_requests_are_base_only() {
    let registry = mixed_registry();

    assert_eq!(registry.select(&[]).expect("empty request"), ["base"]);
    assert_eq!(registry.select(&["base"]).expect("explicit base"), ["base"]);
    assert_eq!(
        registry
            .select(&["base", " base ", "base"])
            .expect("trimmed base"),
        ["base"]
    );
    assert_eq!(
        registry.select(&["mymod", "base"]).expect("mixed request"),
        ["base", "mymod"]
    );
}

#[test]
fn select_rejects_unknown_tokens() {
    let registry = mixed_registry();

    for token in [
        "missing",
        "missing:pack",
        "mymodx",
        "zulu2",
        "MyMod",
        "base:pack",
        " ",
        "\t",
    ] {
        let error = registry
            .select(&[token])
            .expect_err("unknown selector should fail");
        assert_eq!(error.code(), "content.unknown_id", "token `{token}`");
        assert!(matches!(error, ContentRegistryError::UnknownId { .. }));
    }

    let empty = registry.select(&[""]).expect_err("empty token");
    assert_eq!(empty, ContentRegistryError::UnknownId { id: String::new() });
    let blank = registry.select(&["   "]).expect_err("blank token");
    assert_eq!(blank.code(), "content.unknown_id");
}

#[test]
fn select_is_sorted_deduplicated_and_order_independent() {
    let registry = mixed_registry();

    let first = registry
        .select(&["zulu", "mymod", "other", "zulu", "base", "other"])
        .expect("selection");
    let second = registry
        .select(&["other", "zulu", "base", "mymod"])
        .expect("selection");
    let third = registry
        .select(&["mymod", "zulu", "other"])
        .expect("selection");

    assert_eq!(first, ["base", "mymod", "other", "zulu"]);
    assert_eq!(first, second);
    assert_eq!(first, third);
    assert!(first.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn select_resolves_exact_ids_and_namespace_aliases() {
    let registry = mixed_registry();

    assert_eq!(
        registry.select(&["mymod"]).expect("exact id wins"),
        ["base", "mymod"]
    );
    assert_eq!(
        registry.select(&["mymod:pack"]).expect("exact pack id"),
        ["base", "mymod:pack"]
    );
    assert_eq!(
        registry.select(&["alias"]).expect("explicit namespace"),
        ["base", "other"]
    );
    assert_eq!(
        registry.select(&[" alias "]).expect("trimmed namespace"),
        ["base", "other"]
    );

    let pack_only = parse_content_registry_manifest(&manifest(vec![entry(
        "mymod:pack",
        "resource_pack",
        &hash_a(),
    )]))
    .expect("pack fixture");
    assert_eq!(
        pack_only.select(&["mymod"]).expect("shared namespace"),
        ["base", "mymod:pack"]
    );

    let mod_only =
        parse_content_registry_manifest(&manifest(vec![entry("mymod", "mod", &hash_a())]))
            .expect("mod fixture");
    assert_eq!(
        mod_only
            .select(&["mymod:unlisted"])
            .expect("shared namespace"),
        ["base", "mymod"]
    );

    let alias_only = parse_content_registry_manifest(&manifest(vec![entry_with_namespace(
        "other",
        "mod",
        "alias",
        &hash_a(),
    )]))
    .expect("alias fixture");
    assert_eq!(
        alias_only.select(&["alias:sub"]).expect("namespace prefix"),
        ["base", "other"]
    );
}

#[test]
fn select_exact_id_wins_over_shared_namespace_candidates() {
    let registry = parse_content_registry_manifest(&manifest(vec![
        entry("shared", "mod", &hash_a()),
        entry("shared:one", "mod", &hash_b()),
        entry("shared:two", "mod", &hash_a()),
    ]))
    .expect("fixture");

    assert_eq!(
        registry.select(&["shared"]).expect("exact id"),
        ["base", "shared"]
    );
    assert_eq!(
        registry.select(&["shared:one"]).expect("exact child"),
        ["base", "shared:one"]
    );

    let error = registry
        .select(&["shared:three"])
        .expect_err("two same-kind namespace candidates");
    assert_eq!(error.code(), "content.invalid_registry");
    assert!(error.to_string().contains("more than one"));
}

#[test]
fn select_reports_shared_namespace_ambiguity() {
    let same_kind = parse_content_registry_manifest(&manifest(vec![
        entry("shared:one", "mod", &hash_a()),
        entry("shared:two", "mod", &hash_b()),
    ]))
    .expect("same-kind fixture");
    let error = same_kind
        .select(&["shared"])
        .expect_err("same-kind ambiguity");
    assert_eq!(error.code(), "content.invalid_registry");
    assert!(error.to_string().contains("more than one"));

    let cross_kind = parse_content_registry_manifest(&manifest(vec![
        entry("shared:one", "mod", &hash_a()),
        entry("shared:two", "resource_pack", &hash_b()),
    ]))
    .expect("cross-kind fixture");
    let error = cross_kind
        .select(&["shared"])
        .expect_err("cross-kind ambiguity");
    assert_eq!(
        error,
        ContentRegistryError::KindMismatch {
            id: "shared".to_string(),
            expected: ContentRegistryKind::Mod,
            found: ContentRegistryKind::ResourcePack,
        }
    );
    assert_eq!(error.code(), "content.kind_mismatch");
}

#[test]
fn select_guards_unvalidated_duplicate_manifests() {
    let same_kind = ContentRegistryManifest {
        format_version: 1,
        ids: vec![
            raw_entry("dup", ContentRegistryKind::Mod, None, &hash_a()),
            raw_entry("dup", ContentRegistryKind::Mod, None, &hash_b()),
        ],
    };
    let error = same_kind.select(&["dup"]).expect_err("same-kind duplicate");
    assert_eq!(error.code(), "content.invalid_registry");

    let cross_kind = ContentRegistryManifest {
        format_version: 1,
        ids: vec![
            raw_entry("dup", ContentRegistryKind::Mod, None, &hash_a()),
            raw_entry("dup", ContentRegistryKind::ResourcePack, None, &hash_b()),
        ],
    };
    let error = cross_kind
        .select(&["dup"])
        .expect_err("cross-kind duplicate");
    assert_eq!(
        error,
        ContentRegistryError::KindMismatch {
            id: "dup".to_string(),
            expected: ContentRegistryKind::Mod,
            found: ContentRegistryKind::ResourcePack,
        }
    );
}

#[test]
fn select_does_not_change_the_visible_catalog() {
    let registry = mixed_registry();

    assert_eq!(
        registry.select(&["other"]).expect("subset"),
        ["base", "other"]
    );
    assert!(registry.provides("mymod"));
    assert!(registry.provides("zulu"));
    assert!(registry.select(&["missing"]).is_err());
    assert!(registry.provides("mymod"));
}

// ---------------------------------------------------------------------------
// provides
// ---------------------------------------------------------------------------

#[test]
fn provides_matches_exact_explicit_and_shared_namespaces() {
    let registry = mixed_registry();

    for required in [
        "base",
        " base ",
        "mymod",
        " mymod ",
        "mymod:pack",
        "mymod:other",
        " mymod:other ",
        "other",
        "other:addon",
        "alias",
        "alias:sub",
        "zulu",
    ] {
        assert!(
            registry.provides(required),
            "`{required}` should be provided"
        );
    }
}

#[test]
fn provides_rejects_empty_unrelated_and_case_variant_tokens() {
    let registry = mixed_registry();

    for required in [
        "",
        " ",
        "\t\n",
        "missing",
        "missing:pack",
        "my",
        "mymodx",
        "MyMod",
        "other2",
        "base:pack",
    ] {
        assert!(
            !registry.provides(required),
            "`{required}` should not be provided"
        );
    }
}

// ---------------------------------------------------------------------------
// determinism
// ---------------------------------------------------------------------------

#[test]
fn implicit_ids_are_base_first_with_a_sorted_tail() {
    let first = parse_content_registry_manifest(&manifest(vec![
        entry("zulu", "mod", &hash_a()),
        entry("mike", "mod", &hash_b()),
        entry("tango", "mod", &hash_a()),
    ]))
    .expect("first fixture");
    let second = parse_content_registry_manifest(&manifest(vec![
        entry("tango", "mod", &hash_a()),
        entry("zulu", "mod", &hash_a()),
        entry("mike", "mod", &hash_a()),
    ]))
    .expect("second fixture");

    let ids = first.implicit_ids();
    assert_eq!(ids, ["base", "mike", "tango", "zulu"]);
    assert_eq!(ids, second.implicit_ids());
    assert_eq!(ids[0], "base");
    assert!(ids[1..].windows(2).all(|pair| pair[0] < pair[1]));

    let empty = parse_content_registry_manifest(&manifest(vec![])).expect("empty fixture");
    assert_eq!(empty.implicit_ids(), ["base"]);

    // `base` stays pinned first even when a valid ID sorts before it.
    let early = parse_content_registry_manifest(&manifest(vec![entry("alpha", "mod", &hash_a())]))
        .expect("early id");
    assert_eq!(early.implicit_ids(), ["base", "alpha"]);
    assert!(early
        .select(&["alpha"])
        .expect("select")
        .contains(&"alpha".to_string()));
}

#[test]
fn serialization_and_reparse_are_deterministic() {
    let registry = mixed_registry();
    let h_a = hash_a();
    let h_b = hash_b();

    let first = serde_json::to_string(&registry).expect("serialize");
    let second = serde_json::to_string(&registry).expect("serialize again");
    assert_eq!(first, second);
    assert!(first.contains("\"kind\":\"mod\""));
    assert!(first.contains("\"kind\":\"resource_pack\""));
    assert!(first.contains(&h_a));
    assert!(first.contains(&h_b));

    let reparsed = parse_content_registry_manifest(&first).expect("reparse");
    assert_eq!(registry, reparsed);
    assert_eq!(
        serde_json::to_string(&reparsed).expect("serialize reparse"),
        first
    );
    assert_eq!(reparsed.ids[0].checksum.as_deref(), Some(h_a.as_str()));

    registry.validate().expect("parsed manifest is valid");
    reparsed.validate().expect("reparsed manifest is valid");
}

#[test]
fn repeated_parses_produce_identical_manifests_and_selections() {
    let fixture = manifest(vec![
        entry("zulu", "mod", &hash_b()),
        entry_with_namespace("mymod", "mod", "mymod", &hash_a()),
        entry_with_namespace("other", "resource_pack", "alias", &hash_a()),
    ]);

    let first = parse_content_registry_manifest(&fixture).expect("first parse");
    let second = parse_content_registry_manifest(&fixture).expect("second parse");
    assert_eq!(first, second);

    let request = ["zulu", "alias", "mymod", "zulu", "base"];
    assert_eq!(
        first.select(&request).expect("first selection"),
        second.select(&request).expect("second selection")
    );
    assert_eq!(first.implicit_ids(), ["base", "mymod", "other", "zulu"]);
}
