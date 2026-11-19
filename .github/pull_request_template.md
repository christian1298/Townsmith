## What changed

<!-- One or two sentences. Link the roadmap milestone, ADR, or question. -->

## Why

<!-- The problem, user impact, or performance reason. -->

## Test plan

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Godot headless import/smoke test
- [ ] Manual playtest (if gameplay/UI changed)
- [ ] Benchmark before/after (if performance-sensitive)

Commands and results:

```text
# paste here
```

## Benchmark results

<!-- Required for rendering, simulation, streaming, or save/load changes.
     Include scene, route, settings, hardware, and JSON/compare output. -->

## Screenshots / GIFs

<!-- Required for visual or UI changes. -->

## Risks and rollback

<!-- What could break? How do we revert? -->

## Docs

- [ ] ADR added/updated if a decision changed
- [ ] `docs/09 - Open Questions.md` updated if a question was answered
- [ ] Affected docs updated, including frontmatter `updated`

## Checklist

- [ ] One logical change; no unrelated refactors
- [ ] Unit and/or integration tests added
- [ ] Tests are deterministic (fixed seeds, no real network, no sleeps)
- [ ] CI is green
- [ ] Self-reviewed the diff
- [ ] No secrets, generated artifacts, or large binaries
- [ ] Worktree/branch will be removed after merge
