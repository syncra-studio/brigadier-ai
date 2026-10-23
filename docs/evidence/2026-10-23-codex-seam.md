# S26: Codex and the shared seam

Checked on 2026-10-23 with codex-cli 0.164.0, a headless development daemon built from this
change, a separate data folder and a throwaway git project. No application was opened or
brought forward.

## Settings and compaction

The adapter reads the selected model's `max_context_window` from Codex's own catalog
(`models_cache.json` in the account's home, refreshed by `model/list`, or the configured
`model_catalog_json`). It passes that maximum as `model_context_window` and nine tenths of
it as `model_auto_compact_token_limit`, for new, resumed and forked threads and terminal
resumes. There is no model-name table or fixed large compaction override. Missing maximum
metadata is an explicit start error, rather than a guessed window.

The unit fixture includes different maxima, 872000 and 272000, which produce limits of
784800 and 244800 respectively. The shared seam still defaults to 200000 tokens; the
bootstrap-growth allowance now stops at 85% of the reported window, below Codex's 90%
compaction point even when the reported window has no reserved portion.

This uses S22's completed window probe, recorded in `docs/STRIP-DOWN-PLAN.md`: on
codex-cli 0.163, the opt-in accepted 503k, 742k and 799k input tokens, capped the requested
1M at the model's 872k maximum, and compacted once past about 90%. S26 did not repeat the
hour-long probe or add hooks to suppress Codex's native compaction.

A recorded rollout contained one native `compacted` marker at
`2026-10-23T08:11:27.390Z` and one completed `ContextCompaction` item at
`2026-10-23T08:11:27.396Z`. Reading the recorded file through the provider produced exactly
one `CompactionEnded` entry. The scrubbed fixture `codex-compacted.jsonl` keeps this ordering
and adds marker-only and item-only cases. These entries use the existing compaction row.

## Live seam

`BRIGADIER_SEAM_TOKENS=60000`, model `gpt-6.1-sol`, low effort, Full access. The first
message contained an inert padding block, a rule forbidding workers and app launches, and
two separate file-writing steps. A successor was to finish anything left and write a
third marker file.

- Conversation: `01a16862-c155-7679-9f4d-b31ea624e8be`.
- Predecessor: `01a16862-c445-7db2-9bd3-6fb4227d521b`.
- Nudge at 86436 tokens, `2026-10-23T10:51:16.519Z`.
- Generation 1 closed at `10:51:37.270Z`, with `record/handoff/1.md` written.
- Successor: `01a16863-4073-7271-87d6-d4dcf09bcede`, started at `10:51:37.623Z`.
- `record/handoff/1-start.md` began with the shared bootstrap and carried `RULE-S26`
  verbatim. The successor's first reported context was 83628 tokens.
- The daemon's thread view returned one closed seam, `carried: true`, with the note,
  bootstrap and successor's first message id. No additional user message started it.
- Passing that recorded view through the app's `cliBlocks` returned two blocks, exactly
  one divider with note and bootstrap paths, and no user bubble for the automatic carry-on.
- `first.txt`, `second.txt` and `successor.txt` held `S26-first`, `S26-second` and
  `S26-successor`. The successor finished the remaining work.

The probe deleted its session and project through IPC, stopped its own daemon and found no
remaining folder trust for its scratch root. The live check tests the shared seam's
divider; the recorded rollout and fixture separately test Codex's forced-compaction divider.

## Checks

- `CARGO_INCREMENTAL=0 cargo test --locked -p brigadier-providers codex:: --lib`: 48 passed.
- `CARGO_INCREMENTAL=0 cargo test --locked -p brigadier-core seam --lib`: 18 passed on the
  original step, 19 after rebasing over S38, including two seams, restart recovery,
  exactly-once messages and the compaction margin.
- `pnpm typecheck`: passed.
- From `apps/desktop`: `node scripts/run-tests.mjs src/app/conversation/cliBlocks.test.ts`:
  10 passed, including the existing compaction row and seam divider checks.
- The ignored `print_a_session_file` diagnostic, pointed at the recorded rollout: one
  passed; exactly one `compacted` entry from the paired native records.
- `CARGO_INCREMENTAL=0 cargo clippy --locked --workspace --all-targets -- -D warnings`:
  passed after staging the built daemon and CLI sidecars required by the desktop manifest.

The bootstrap implementation and standing instructions are unchanged: Codex takes the same
provider-neutral successor bootstrap that S22/S23 already supply. S22b can extend that text
without a Codex-specific path.
