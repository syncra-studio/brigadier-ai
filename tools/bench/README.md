# CLI bench

A report of small coding tasks, separate from CI and release checks. Regressions update one
[Bench regression issue](https://github.com/syncra-studio/brigadier-ai/issues?q=is%3Aissue+%22Bench+regression%22).
No measurement fails a job. Missing transcripts, broken fixtures, cleanup failures or missing
arm reports fail the harness job so missing evidence cannot look like a passing measurement.

Build the headless binaries, then run:

```sh
cargo build --locked -p brigadier-daemon --bin brigadierd -p brigadier-cli --bin brigadier
tools/bench/run.sh --arms A,CC --runs 1
# Small smoke, custom binary/output, and a unique prefix on a shared machine:
tools/bench/run.sh --arms A,CC --tasks t1 --runs 1 \
  --daemon /path/to/target/debug/brigadierd --output /path/to/report \
  --scratch-prefix my-run-my-worker-
```

Requires Python 3.9+, Node, git and existing CLI logins. No fixture download or npm install is
needed. The default is all five tasks, two runs, all arms; each invocation writes `report.md`
and `report.json` to `bench-report/` (choose an output outside your checkout to keep it clean).
`--timeout` bounds each model run (600 seconds by default). A live smoke costs real quota.
`--keep-transcripts DIR` copies each run's native transcripts (parent and workers, full context)
to `DIR/<arm>-<task>-<run>/` before cleanup, for reading what a run saw; keep DIR private.

| Arm | Runs |
|---|---|
| A | Plain `claude -p`, stream-json |
| B | Plain `codex exec --json`, keeping its rollout until measured |
| CC | Headless Brigadier Claude session, New worktree |
| CX | Headless Brigadier Codex session, New worktree |
| W | Headless session delegating all implementation to exactly one worker of the same provider |

W defaults to Claude; use `--worker-provider codex` with `--arms B,CX,W` for the Codex arm set.
The worker uses the parent's checkout (`--local`), retaining the fresh-context and delegation
cost without a second worktree/merge. Its parent still uses New worktree. The grader requires
exactly one metered worker. A, B, CC and CX explicitly forbid workers/subagents.

Claude defaults to `opus`; Codex uses its configured default. Override with `--claude-model`
or `--codex-model`. Every parent and worker gets the same model, `--effort low` and full
permissions. `--effort medium` or `high` is available for a separate baseline.
`--worker-model auto` measures W as a user runs it: the parent stays pinned, its worker gets no
`--model`/`--effort`, and the daemon's models-rules file is the shipped default, so Brigadier's
advice picks them. The worker's start receipt and transcript models are recorded per run. Actual model
identifiers are recorded from native transcripts. Runs execute sequentially in task/run/arm
order; this avoids competing arms on a shared host, but short-task variance remains substantial.

Three opt-in cache experiments change session startup and are report only:
`--claude-flag-exclude-dynamic` passes `--exclude-dynamic-system-prompt-sections` to plain
Claude and to Brigadier parents and workers. It moves cwd/environment/memory/git sections into
the first user message, allowing more of the system prefix to be shared across directories.
`--worker-cache-ttl 5m` (or `1h`) sets `CLAUDE_CODE_PROMPT_CACHE_TTL` for Claude workers only;
parents keep the CLI's default or inherited TTL. These two options are Claude only.
`--stable-standing` sets `BRIGADIER_BENCH_STABLE_STANDING=1` in the isolated daemon only:
its appended standing text keeps the session-folder and ledger environment variables instead
of substituting each session's paths, and adds one sentence about shell expansion. Those
variables are already set for parents and workers. This applies to either wrapped provider;
plain arms are unaffected. All three options are off by default. The isolated daemon selects
a bench-only Claude launcher through `agentPaths`;
no provider or nightly behavior changes. JSON and Markdown record the requested options;
wrapped runs also record `claude_launches` with the actual per-role overrides, standing-text
SHA256, placeholder presence and whether both standing environment variables are set. Confirm TTLs
from native `usage.cache_creation` in `--keep-transcripts`, since settings or other environment
overrides may affect the CLI. These require a CLI supporting the flag and TTL override
(TTL requires v2.1.242+); see [Claude's CLI reference](https://code.claude.com/docs/en/cli-reference)
and [prompt caching](https://code.claude.com/docs/en/prompt-caching).

```sh
tools/bench/run.sh --arms W --runs 1 --worker-model auto \
  --claude-flag-exclude-dynamic --worker-cache-ttl 5m
```

Each run creates a dependency-free ESM repository in its own temporary directory:

| Task | Change | Hidden grader |
|---|---|---|
| t1 | ANSI `stripColor` export and tests | Colored/plain/empty text, public export, added tests |
| t2 | CRLF line-splitting bug from a failing test | CRLF/LF/empty input; reproducer stays byte-identical |
| t3 | Move two helpers across three files | New module, preserved public behavior, old module removed, added tests |
| t4 | Stdin-to-uppercase CLI and a spawn test | Spawn with multiline stdin, exact stdout and exit status, added tests |
| t5 | Independent gradient and slug modules | Inclusive ascending/descending/one-step gradient; slug/empty input, added tests |

Every grader also runs `node --test test/*.test.js`. These are bounded equivalents of the
original five-task manual bench; t4 uses stdin rather than leaving a server running. The grader
lives outside the throwaway repo. `fixture_version` changes when the tasks or rubric change.
The added-test check establishes presence, while hidden assertions establish behavior.

Columns and JSON fields:

- **Pass**: the suite and hidden assertions pass, required files/tests exist, and W used one
  worker. A failed task remains a successful measurement and is eligible for regression reporting.
- **Wall s**: plain CLI launch to exit, or first Brigadier message to settled request; includes
  the session worktree startup. Daemon boot/project registration are excluded.
- **First output s**: first observed model output, reasoning or tool step, sampled at 100 ms.
  CLI handshake/system initialization is excluded; unavailable stays null.
- **Worktree s**: session creation to the checkout becoming usable; zero for plain arms.
- **API calls**: final assistant message ids on Claude; distinct cumulative usage updates on
  Codex (an observable call proxy). **Tools** is the distinct native tool-call count in JSON.
- **Input / cache write / cache read / output**: finalized Claude usage per assistant id (largest
  output wins over streaming duplicates); Codex's last cumulative rollout usage, subtracting
  cached input from total input. Codex does not separately report cache writes, so that field is
  zero, meaning unreported. Reasoning is already inside output and is never added again.
- **Cache economics by role**: `roles.parent`, `roles.worker` and `roles.successor` (when present)
  contain native `usage`, `uncached_input` (`input + cache_write`), `cache_read`,
  `cache_hit_ratio` (`cache_read / (input + cache_write + cache_read)`), `output`, native
  `models`, and `list_price_usd`. Successors aggregate subsequent parent sessions; worker
  usage is read from child transcripts for this table. Legacy totals below still use the daemon
  worker meter. `list_price_usd` on the run sums all roles once.
  [prices.json](prices.json) is the shared USD-per-million standard-rate table, with a source
  URL and verification date on every model. Native cache TTL and per-call context select the
  write rate and context tier. Missing TTL uses short writes, explicitly labelled as an estimate.
  Unknown models have `price_status: "n/a"` and null dollar values in JSON (`n/a` in Markdown),
  never a guessed parent rate. CLI aliases such as `opus` are not native model IDs.
  Old aggregate-only artifacts cannot resolve mixed models or context tiers; those prices stay
  unavailable. A single-model Claude legacy aggregate is labelled as a short-write estimate.
  Estimates exclude regional, service-tier and tool fees and do not measure subscription quota.
- **Aggregate dollars**: mean `list_price_usd` per completed arm and `list_price_ratio` against
  the same provider's plain arm (A/B), beside wall/raw/cost-unit ratios, in Markdown and JSON
  `aggregate`. Any missing price makes the dollar mean and ratio unavailable. Skipped and
  incomplete runs do not enter means. These dollar columns do not change regression rules.
- **Raw tokens**: the sum of those four fields. **Cost units**: price-weighted kilotokens,
  Claude `(input + 1.25*write + .1*read + 5*output)/1000`; Codex
  `(uncached_input + .1*cached + 8*output)/1000`. Compare within a provider, never across providers.
- **W totals**: native parent usage plus the daemon's `turn_usage` rows for worker tasks;
  `parent_usage`, `parent_cost_units`, `worker_usage`, `worker_turns` and `daemon_usage` remain
  separate in JSON. Captured child transcripts supply worker command/injection counts.
- **CLI commands / ledger adds**: occurrences in executed shell tool inputs, including shell
  calls inside Codex code-mode tool source; `brigadier ledger add` is counted separately.
- **Injected msgs / bytes**: transcript-visible textual user/developer context other than the
  typed prompt and tool results, including skill bodies and child-worker instructions. Native
  transcripts do not expose provider-private system prompts. The static standing-instruction
  template size is reported separately as `standing_instruction_bytes`; it is not a token estimate.

Both plain and Brigadier runs use the same native transcript parser. Parent CLI tokens are
cross-checkable against `daemon_usage`. Session transcripts are held only until measured and
then removed; reports contain numbers and bounded grader output, never the full context.
Each daemon has its own data directory and empty models-rules file, never the installed app's
data. Cleanup deletes its sessions/projects through IPC, shuts down its PID, and removes its
scratch directories. Local Codex uses a temporary copy of existing auth, configuration and
skills; trust changes and rollouts cannot alter the real CLI home. Existing Claude settings
and credentials are used, and only this run's explicit transcript ids are removed.

The [Bench workflow](https://github.com/syncra-studio/brigadier-ai/actions/workflows/bench.yml)
runs at 05:30 UTC, after the nightly's 01:30 start, in two independent arm-set jobs. It does not
wait for or modify the nightly. Credentials are existing repository secrets:

- Claude: `CLAUDE_CODE_OAUTH_TOKEN` **or** `ANTHROPIC_API_KEY`.
- Codex: `CODEX_AUTH_JSON` containing the CLI auth JSON.

Missing credentials produce explicit skipped rows without installing CLIs or building the
daemon. Manual dispatch defaults to `live=false`, exercising the same green skip path even
when secrets exist. Set `live=true` to measure the available providers; no login is attempted.

Download the **bench-report** artifact for combined `report.md` and `report.json` (90-day
retention); **bench-claude** and **bench-codex** retain the arm-set reports for 30 days.
The job summaries contain the same tables. The first observation establishes a baseline.
Later runs compare each completed task/provider arm with the previous artifact. A previously
all-passing task becoming a failure is a regression. For CC/CX/W, a raw-token or wall ratio to
its plain arm must exceed 1.20 and worsen by more than 20% relative to the previous ratio.
This detects growing wrapper overhead while controlling for host/provider changes. Missing
observations and changed models/fixture versions/effort establish a new baseline. Skips are
never counted as passes. The issue step only creates or edits **Bench regression**, with the
run link, symptoms and numbers. It cannot fail the workflow or block a release.

Offline checks (no model calls):

```sh
python3 -I tools/bench/checks.py
python3 -I -m py_compile tools/bench/*.py
shellcheck tools/bench/run.sh
actionlint .github/workflows/bench.yml
```
