# Versioned CLI recordings

D9(c): the version directories hold the raw production-adapter exchange, the scrubbed native
session file, and the thread views the real Rust replayer and S9/S10 reader produce. Their
`index.json` maps each scenario to its sources; several scenarios intentionally share one tiny
run. Existing recordings are referenced in place, without copying or relabelling them.

The original Codex plan/question run was ephemeral. Its protocol recording is reused; the
`nativeRecording` entry identifies the small persisted counterpart used for the separate
live/native ID and reload checks. These are different real sessions and have different IDs.

D25/D29: current-version limit coverage is explicitly **missing**. Never consume quota to
reach a limit. The recorder preserves an opportunistic real rejection and marks its source in
the index. The old derived limit envelopes remain at their original paths: they do not carry
a CLI version, so the root index records that provenance rather than inventing a version.
Historical recordings retain the versions in their original headers. The background-watch
reference in core is attributed to 2.1.308, not the installed 2.1.310.

Record only gaps for an installed version (Haiku / gpt-6-luna, low effort, tiny prompts):

```sh
tools/record-cli-fixtures.sh claude
tools/record-cli-fixtures.sh codex
```

A new version gets a fresh index. A complete index returns without starting an adapter or
making a model request. Native compaction runs against the tiny history; no context filling.
Raw recordings capture the interrupt and native resume requests too. Sessions are closed and
their artifacts removed after exporting the scrubbed native file. If cleanup fails, the
scratch directory keeps `cleanup.json`; retry without a model call using
`cargo run -p brigadier-providers --example record_fixtures -- --cleanup <provider> <scratch>`.

Inspect changes before regenerating the presentation snapshots:

```sh
UPDATE_FIXTURES=1 cargo test -p brigadier-core --lib version_fixture_tests
cd apps/desktop
UPDATE_FIXTURES=1 pnpm test src/app/conversation/versionFixtures.test.ts
```

Without the environment variable, both commands compare checked-in snapshots. They are
included in ordinary `cargo test` / `pnpm test` and need no installed CLI or credentials.
The Rust check uses the production replayer, native readers and thread-view builder, checks
kept/fresh reads and live/native duplicate suppression. The Node test draws every live and
persisted view through `cliBlocks`, comparing the resulting replies, steps and turn states.

`usage.json` reports the real usage of the new captures. Cumulative reports must not be
summed, including across resume. Claude's persisted `cost-state` supplies its session cost;
Codex supplies token totals and no dollar cost. Reused recordings incur no new model usage.
