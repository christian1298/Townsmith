# Townsmith content data (v1 draft)

This directory holds the first data-first content contracts from
[[06 - Modding and Resource Packs]] (D-006, proposed). The runtime registry,
mod loader, and save integration are **not implemented yet**; these files are
a validated starting point for that work.

## Layout

```text
content/
  schema/
    material.schema.json   # draft 2020-12
    room.schema.json
    prop.schema.json
  base/
    materials.json         # base: materials, draft balances
    rooms.json             # base: room types
    props.json             # base: props and allowed rooms
```

## Validate

```bash
./scripts/check_content_schema.py
./scripts/check_content_schema.py --self-test
```

The validator checks namespaced IDs, required fields/types, duplicate IDs,
schema file presence, and prop -> room references. The final line is
`CONTENT_SCHEMA files=3 entries=N status=ok`.

## Rules

- IDs are `namespace:path`; the base game uses `base:`.
- `format_version` is `1` for this contract.
- `name_key` is a localization key, not display text.
- `draft: true` marks placeholder balance values. Do not treat them as tuned
  campaign numbers.
- Mods add content; they do not edit these files. Scripting and override rules
  stay in [[06 - Modding and Resource Packs]].
