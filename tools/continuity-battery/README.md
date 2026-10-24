# Continuity battery v0

From the repository root, with Python 3, Rust and a signed-in Claude Code CLI:

```sh
tools/continuity-battery/run.sh
```

This builds the checkout's debug daemon and CLI, then writes
`tools/continuity-battery/reports/<date>-claude-seam.md`. It uses a separate headless daemon,
scratch data and throwaway git project. It never connects to the installed app. The
model is `opus`, high effort. This spends real subscription tokens. The whole run is
bounded to 25 minutes, each request to 100 seconds, and filler requests to 18.

The script measures the seam, which is always on for sessions; it supplies no new continuity path.
Twelve items are planted before inert filler requests. The final request recalls them,
and a preceding edit applies the standing documentation rule and its correction.
The false claim is a labelled injected worker report, checked against the fixture's
plain `writeFileSync`; no worker or false knowledge node is created. The support
ticket exists only in a one-time command's output after its script is removed. A
second command fails with more than 250 KB of output and a marker in the middle.
One user item is deliberately excluded from the ledger.

`BRIGADIER_SEAM_TOKENS=43750` (the `--tokens` default) sets the debug seam threshold. The script
asks for at least nine filler turns and two closed seams, then injects the existing debug
context-window error, kills its own session's Claude CLI, restarts its own daemon at a
foreground-command wait point, and injects a one-minute usage limit. It waits for that window to
reset before probing Claude. If a forced fault fails to happen, the report says so; a successful
injection alone is not proof of recovery. Repeated answers aren't fed back between faults.

The report reads each seam from the session's `record/seam.json`, its note from
`record/handoff/<n>.md` and what the successor started from from `<n>-start.md`. The battery plants
into, excludes from and measures the session's ledger, `record/ledger.md` (the one the standing
instructions name), not a ledger in the repo.

Overrides pass through `run.sh`, for example `--tokens 75000 --max-fillers 24`.
`--report PATH` chooses the text report. `--evidence /absolute/private/path.json`
keeps detailed input/output, provider events, hand-offs and metering outside git.
The evidence contains the planted answers and is written before scratch cleanup.
A seven-day private bundle is also retained by default (location and expiry below). Exit 0 means the probe ran,
even when recall fails; exit 2 means the run ended before the probe. This is a report,
never a gate. A report-only formatting pass can import `report()` from `battery.py`
and pass it saved JSON evidence; it makes no model calls.

The run deletes its session and project through daemon IPC, shuts down its own
daemon, kills only process groups it created and removes its own scratch root.
Remaining folder trust keeps that root for cleanup through its daemon records and
is reported explicitly. No credentials are entered. A CLI that is missing or signed
out produces an incomplete report, with no attempt to authenticate.

## Decision survival (S22b)

```sh
tools/continuity-battery/run.sh decisions --tokens 60000
```

`decisions.py` measures `research/CONTINUITY.md` §3.7 with the same harness and writes
`reports/<date>-claude-decisions.md`. It plants ten items the way a user gives them, with no word
about the ledger: three rules (one later corrected), a principle with its defining sentence and
two examples, two decisions with reasons, one decision the thread takes itself, an open question,
an open promise and a false worker claim. It forces two seams by size and one death with no note,
then, restating nothing: T4 "where are we?" (both open items raised?), T1 a coding request the
rules and decisions bear on (scored from the file and by running its functions with `node`), T2 a
new case only the principle decides, T3 a worker brief scored by `claude -p --model sonnet` on a
fixed rubric (cited, stated, missing, contradicted, and additions not marked "proposed"), T5 the
thread's own decision told and marked "I decided", and T6 recall when asked. It is a report, never
a gate. The plain-CLI arm and Codex belong to S25 (L9). `decisions.rescore(evidence, report)` scores a
saved run again with no model calls (it reuses the T3 judge's answer).

The original committed reports are a small Claude baseline for S21. The nightly extension
below adds both-provider A/B, a longer comparison and opted-in real-lineage samples (S25/L4). Shared-session
faults measure cumulative survival; they don't isolate each seam's causal effect.
The runtime's token counts and reported dollar costs are measurements, not prices
inferred from a model name.

## Nightly reports (S25, L4/L5/L9)

`nightly.yml` calls `continuity-battery.yml` every night, independently of the app's
unchanged-commit build skip. To run only the battery (no app publication):

```sh
gh workflow run continuity-battery.yml --repo syncra-studio/brigadier-ai --ref main
```

The existing scripts now accept `--provider claude|codex` and `--arm seam|plain|zed|summary`.
`plain.py` changes only the transport: the same plants, fixture, tasks and scorers go
through `claude -p --resume` or `codex exec resume`, with automatic compaction at the
CLI's own default. It installs no seam or standing ledger instructions. The decision
block scores a plain CLI's own decision by its announced choice and applied separator;
only Brigadier's arm is also checked for its ledger marker. No `/compact` is sent.

The workflow supports all four arms for short recall (nine filler turns), long recall (90),
decision survival (90), and updates (short: nine; long: 90), with matching filler schedules within each provider.
Manual dispatches and reusable calls accept an `arms` string (default `seam,plain,zed`), for example
`gh workflow run continuity-battery.yml -R syncra-studio/brigadier-ai --ref main -f arms=seam`.
Only the selected arms run on both providers. Omitted arms are labelled "not run on this
dispatch" in the report and cannot trigger regression issues. Nightly selects its arms with
`jobs.continuity.with.arms` in `nightly.yml`; tonight it includes `summary` on both providers
(D72). Remove `,summary` from that one line after the comparison. An absent selected arm still
makes its provider incomplete. Zed's long recall and decision profiles remain dispatch-only
(`zed_long`); its updates profiles run with the other arms.
The long schedule supplies over 400k inert input tokens at ordinary tokenization;
the metered usage and observed compactions/seams are reported rather than assumed.
The three-context decision bar is unqualified if fewer than three native compactions
or seams happened before checks. A plain CLI with a very large window may not reach
three automatic compactions in this bounded run. CLI defaults are never lowered to
make that number happen. Claude uses `opus`, the rubric judge `sonnet`; Codex uses
`gpt-6.1-sol`; all use high effort.

Only existing repository secrets are consumed: `CLAUDE_CODE_OAUTH_TOKEN` or
`ANTHROPIC_API_KEY` for Claude, `CODEX_AUTH_JSON` for the existing Codex account's
native `auth.json`. No sign-in or credential creation runs. An absent provider's
secret skips its live arms and writes **needs the user: secrets** into the report.
An expired credential produces an incomplete observation, not a pass.
Seam runs follow queued messages through their eventual transcript request IDs before
injecting faults or awaiting completion. Closed Codex transcripts are read from their
exact `archived_sessions/` path when moved there. The harness's debug daemon idle-exit
bound outlives the measurement and cleanup; the normal product idle policy is unchanged.
Run errors appear beside each observation in the consolidated report. Missing transcripts
leave compactions unavailable and preserve the remaining seam/ledger evidence; private
evidence retains the daemon log and owned native paths even if the daemon exits. Private bundles
are retained separately from the public artifact, as described below. Uploaded Markdown contains
only synthetic battery data, and `summary.json` contains the numbers. Never upload real transcripts.

Every run uploads a consolidated **continuity-battery** artifact (`summary.json`,
`report.md`, per-arm Markdown), retained for 90 days, plus provider artifacts for 30.
Settings › Reports reads the newest retained artifact on main via the user's existing
`gh` installation/sign-in, and Refresh reads it again. Missing artifacts, read failures,
and skipped live arms stay visible. This checkout had no existing Reports component;
the page reuses `SettingsPage`, `SettingsSection`, `SettingsCard` and `SettingsRow`.

The summary reports native compactions, recall, task checks and decision scores. Dollar fields
use only deduplicated native economics, with input, cache write, cache read and output reported
separately. The legacy `usage.cost_usd` public field aliases the native list-price total for the
existing Reports UI. Unknown synthetic-event model attribution is explicitly unpriced; missing
prices remain unavailable. Legacy metered dollar estimates remain in private evidence. Native
cold-return dollar attribution is unavailable until per-call role/return attribution exists.
The large-output middle marker is a visibility diagnostic, excluded from recall (11 recall
items in the original script, 29 in updates). The T3 judge gets the full planted instruction set,
including later task constraints. Fault-recovery runs are labelled and excluded from continuity
cost comparisons; native compactions inside seam arms are flagged for either provider. The
first artifact is the baseline; matching completed observations compare to the preceding artifact.
Recall/task/decision/cost bar misses, lost recall/application, and native compactions inside seam
arms can open a regression issue. No number gates sessions or builds; missing data is never a pass.

No real-lineage sampler existed. `lineages.py` is the smallest read-only sampler: it
reads only opted-in `record/lineage.json` files and their listed native files, draws up
to 20 lineages deterministically per ISO week, and lists each native session id and
compaction count. It counts Claude `compact_boundary` and Codex `compacted` records,
not the duplicate notifications. It neither scans home directories nor copies content.
A self-hosted runner that already has access to the opted-in records can run the Sunday
sample by setting repository variables (no credentials):

- `CONTINUITY_LINEAGE_RUNNER`: JSON runner labels, e.g. `["self-hosted", "continuity"]`.
- `CONTINUITY_RECORD_ROOTS`: a JSON array of absolute `record/` folders explicitly opted in.

Without those variables, the weekly artifact records the missing opt-in. Manual runs
also sample once; other weekdays report that the sample is not due. Run locally with
`python3 tools/continuity-battery/lineages.py --record /absolute/session/record --output /tmp/weekly.json`.

Cache economics is captured from native transcripts before cleanup, using the model each
session actually ran. Each recall/decision report now has a numeric JSON companion with the
same basename (`report.md` → `report.json`), retaining only report metadata and economics,
not conversation content. The first native session is `parent`; later sessions aggregate under
`successor`, including provider fallback. The battery's worker claim is injected text, so it has
no actual worker cost. Bench runs with real workers also report a `worker` role.

Each role includes uncached input (`input + cache_write`), cache-read tokens, hit ratio
(`cache_read / (input + cache_write + cache_read)`), output, native models and `list_price_usd`.
The shared [price table](../bench/prices.json) records current standard list prices, source URLs
and verification dates. Native one-hour versus five-minute writes and per-call context tiers
select the rate. Absent TTL uses a labelled short-write estimate; unavailable native models or
prices are `n/a` (null dollars with `price_status: "n/a"` in JSON). Estimates exclude regional,
service-tier and tool fees and are not subscription charges. Legacy evidence without native
model IDs is unpriced; the requested CLI alias is never substituted.

The nightly `report.md` and `report.json` add these role columns, mean list-price dollars per
provider/script/size/arm and a dollar ratio to the matching plain arm. `summary.json` remains
an identical compatibility artifact. A missing price in any completed observation makes that
mean/ratio unavailable. Existing provider-reported `usage.cost_usd`, cold-return costs,
price-weighted ratios and regression rules retain their previous meanings. The T3 rubric judge
remains outside the metered subject run.

Offline verification (no model calls):

```sh
python3 -B -m unittest discover -s tools/continuity-battery -p 'test_*.py' -v
pnpm --filter @brigadier/desktop test src/app/settings/ReportsPageRender.test.ts src/app/settings/pages.test.ts
actionlint .github/workflows/continuity-battery.yml .github/workflows/nightly.yml
```

## Zed text-compaction arm

`--arm zed` runs the same tasks and scorers through the plain CLI on both providers,
using Zed's text-compaction algorithm pinned to
[`9f305aaf88370e8ca39c702242186e3d8f97ecca`](https://github.com/zed-industries/zed/blob/9f305aaf88370e8ca39c702242186e3d8f97ecca/crates/agent/src/thread.rs#L5152-L5222).
At the battery's `--tokens` threshold (latest input, including cache reads/writes,
plus output), it asks the same model for a summary on the old prefix. Claude can
remove tools for that request (`--tools "" --strict-mcp-config`); Codex has no equivalent
CLI switch, so it uses one ordinary plain turn. A fresh session receives the newest
80,000 UTF-8 bytes of actual scenario user messages, then the summary with Zed's
wrapper, then the pending request. Selection is newest-first, but replay is chronological
(`thread.rs:5186–5220` reverses the selection); the oldest partial message retains its
prefix at a character boundary (`thread.rs:5018–5079`). Previous summaries and synthetic
bootstraps are never counted as user turns. The GPL prompt is fetched from its pinned
raw GitHub URL into the run's scratch cache; it is never vendored. Fetch failure writes
a **skipped: could not fetch** report row and cleans up normally.

This measures the open text algorithm, not Delta's product or provider-native compaction.
CLI boundaries fold the retained user messages and summary into one input turn, and the
threshold is checked between CLI turns, rather than between internal tool requests.
Claude's native auto-compaction is disabled for this arm; any native compactions still
observed (including Codex defaults) are reported separately from explicit Zed compactions.
Summary calls and every fresh session are included in per-role cache/list-price dollars.
Nightly aggregates compare seam versus Zed on T1–T6, each decision item's application
and recall, each recall item, overall recall/task success, and reported/list-price dollars. Missing data
is unavailable; decision comparisons with fewer than three transitions are unqualified.

## D49 budget variants (P2 / P3)

Run `tools/continuity-battery/run.sh p2 --report /absolute/report.md` or `run.sh p3 --report /absolute/report.md`.
Both use Claude Sonnet at low effort by default. P2 also requires an existing signed-in Codex CLI;
its target model defaults to `gpt-6.1-sol`, low effort, subject to the user's model rules (D25).
The variants run once each in the Claude seam job, **short profile only** (D49, D53); they add no
long-profile or plain-arm runs. The workflow installs the second CLI only with existing secrets.
Missing credentials are reported; no sign-in is attempted.

P2 plants five random tool-only details in the predecessor's last 40k tokens, grows the reported
context to 60–100k, then requests Claude → Codex through the existing `continueOn` IPC. The first
probe asks for those details without lookups; qualification checks the actual tool events, ledger,
notes and measured context. The bar is ≥4/5 recalled. The same T1–T6 decision block follows,
reporting all ten recalled items and the nine applicable unprompted checks (the false worker claim
has no application check). A run outside the context band or with leaked details is unqualified.

P3 gives a random receipt rule only in an older user message, followed by >32 KB but <80 KB of
new user text and intervening tool reads. After three forced size seams, ten new receipt objects
must apply it (bar ≥9/10). The ledger and notes must not carry the rule, and all three bootstrap
user sections must. Added user content is bounded to ≤12k estimated tokens per seam.

The offline legacy-budget comparison reports retained details/user rules from the same inputs,
with the old 60 KB tail / 32 KB user budget. It measures retention, not a second model's behavior.
Per-role list-price dollars include all observed successor input; a separate first-successor row
prices the native uncached input/cache write. Extra bootstrap tokens and dollars are explicitly
labelled UTF-8/4 estimates at the native model's input (Codex) or cache-write (Claude) rate, using
the observed first-write TTL split when available and a labelled short-write fallback otherwise.
P3's ≤12k overhead bar uses that byte/token estimate; the actual counterfactual token delta is
unavailable without a matched old-budget run. High-entropy text can exceed that approximation.
No incremental causal dollar saving/cost is claimed without a matched live baseline. These bars
are reports only and never create regression issues or gate work. `--evidence` keeps private raw
inputs outside git; report JSON retains numeric scores and economics only.

## Summary handoff and updates (D66)

`--arm summary` asks the running plain CLI to write a handoff at the same configured
`--tokens` threshold as the seam. Like the Zed arm, the CLI transport checks latest
input (including cache) plus output between turns; it cannot intercept a seam's
internal tool-call boundary. The full upstream handoff skill is fetched verbatim at
run time, pinned to
[`5beb35b040e3de97e54fa9e386d7e42d15c295c2`](https://github.com/mattpocock/skills/blob/5beb35b040e3de97e54fa9e386d7e42d15c295c2/skills/productivity/handoff/SKILL.md)
(MIT), into disposable scratch. The only added instruction selects the document's
absolute path inside that run's `/tmp` directory. Fetch failure is a skipped arm.

A fresh CLI starts in the same fixture folder with exactly `Read <file> and continue.`
The pending scenario request follows in a separate resumed turn. The harness adds
no Brigadier seam, ledger, decisions block, user-message replay or verbatim tail.
The old recall script's explicit ledger-creation request is omitted for this arm;
all planted facts, action checks and recall scoring remain the same. The decision
scenario uses the existing CLI rubric, including the thread's own choice without a
Brigadier ledger marker. Claude auto-compaction is disabled for this explicit arm;
any observed native compactions are counted separately. Summary writing, reading
and every fresh successor are included in usage and per-role list-price dollars.

Run the harder scenario with `tools/continuity-battery/run.sh updates --arm plain`
or invoke `updates.py` directly with a debug `--daemon`. It plants 30 randomized
identifiers: 18 facts given once early, eight decisions subsequently replaced twice
(two reverse back to their first choice), three promises left open, and one receipt
in the middle of a >250 KB diagnostic output. The diagnostic fixture deletes itself before printing, so a later source read
cannot recover the receipt from disk; the harness also removes any unexecuted fixture. The first and second decision revisions occur
at one-third and two-thirds of the fixed filler schedule. Every arm uses all nine
short or 90 long filler turns, with 1,200 or 3,500 inert words per turn respectively.
No arm exits early on a transition count. Long inputs displace early facts through
multiple contexts, while CLI native-compaction defaults remain unchanged.

Recall, newest-decision accuracy, and the number of stale-decision mistakes are
reported separately in per-run Markdown/JSON and consolidated nightly reports.
A stale answer counts even when it also lists the current choice; `unknown` loses
recall/newest credit without being a stale mistake. Exact values must occur under
their own keys. A separate "where are we?" turn measures open promises raised before
the recall prompt. The tool-only item also measures provider output excerpting;
its absence does not by itself prove a continuity failure. Scores never gate work.

Both updates sizes run on every selected arm, including Zed even when `zed_long`
is false (that flag still controls only the older long recall/decision profiles).
Nightly selects all four arms. A targeted dispatch can compare the requested three:

```sh
gh workflow run continuity-battery.yml -R syncra-studio/brigadier-ai --ref main -f arms=seam,plain,summary
```


## Opt-in fair work and endurance comparisons

These profiles reuse `Battery`, the existing plain/Zed/summary transports, native accounting and
cleanup. Tonight's existing nightly arm list, profile list, task schedule, thresholds and model
calls are unchanged. The manual experiment dispatch below invokes the new entry point. The original `battery.py`, `decisions.py`
and `updates.py` also accept `--fair`: this opts into a 200k default, fixed filler schedule,
identical archive lookup access and continuity-only probes, with no injected faults. Their
unflagged nightly defaults retain the original thresholds and fault prompts. For the original
short recall pair, use `battery.py --fair --arm seam` and `battery.py --fair --arm plain@200k`
with the same explicit `--tokens`, filler lengths and model. New candidate arm names implicitly
select fair mode on those entry points. The work/endurance profile is the harder memory test;
`--fair` does not move the legacy script's first-message facts.

A plan-only dry run starts no daemon, model, judge or network request:

```sh
python3 -B tools/continuity-battery/experiment.py --profile work \
  --arms plain,plain@200k,seam,native --repetitions 4 --dry-run --report /tmp/work-plan.json
python3 -B tools/continuity-battery/experiment.py --profile endurance \
  --arms plain@200k,native --dry-run --report /tmp/endurance-plan.json
```

A live comparison spends subscription tokens and belongs to the separately authorized paid round:

```sh
tools/continuity-battery/run.sh work --provider claude \
  --arms plain,plain@200k,seam,native --tokens 200000 --repetitions 4 --report /tmp/work.json
```

`plain` retains the CLI default. `plain@200k` uses the selected shared `--tokens` value:
Claude `--autocompact`, Codex `model_auto_compact_token_limit`. The seam/Zed/summary arms use
that same configured threshold. Supported debug thresholds start at 100k, the installed Claude
CLI's minimum. Codex in these opt-in profiles runs through its native app-server; the existing
nightly transport is unchanged. All arms get the same read-only native archive lookup commands;
recall explicitly permits those commands, forbids fixture access and never provides grader data.
The archive returns original native rows, with stable exact item ids. It adds no product memory.

`work` defaults to 300 small import tasks: read a receipt under 2 KB, write a checked JSON artifact,
and explain the calculation in about 400 words (target ~600 output tokens). This targets ~1–3k
context growth per model request; the actual native usage is retained, not assumed. Nonces are
planted in middle user turns and one-time small tool receipts, after the first work task. Later
checks apply a general rule and a scoped exporter reversal, close and replace a promise, raise
an unresolved item without prompting its answer, and finish a draft artifact to an old spec.

`endurance` defaults to at most 1,400 such tasks, targeting approximately 2M tokens of growth
at a 200k transition point. All three disjoint fact sets are planted before the first transition.
A is probed only at depth 3, B only at 6, C only at 10; an exporter decision and promise state
change in the middle. Fewer than ten observed transitions, or a transition before planting
finishes, makes the lineage incomplete. Increasing `--turns` is an explicit later-run choice.
Earlier probes do not ask the later sets. Reports show per-depth memory scores, stale answers
and ledger ids proven absent from the kept context, including post-compaction put-back items.
An opaque payload makes missing ids unavailable, never zero.

Per fact, the scorer inspects native replacement history or Claude's preservation UUIDs and
compaction summary. Visible exact markers are `retained`; proven absent markers are `evicted`;
missing metadata, missing preserved rows, or sealed/encrypted summaries are `unverified`.
Only evicted facts enter memory scores; retained facts are separate, and unverified evidence
cannot establish a memory benefit. A user fact still inside Codex's 64k retention or Claude's
“All user messages” summary is never silently counted as memory. Full payloads remain private.

Seeds and task order are paired across arms. Reports use independent lineage differences with
90% Student intervals, and cost per fully passing lineage includes spend on failed attempts.
Missing prices keep that cost unavailable. Pairing also requires the same build/hash, model,
threshold, workload and evicted-item set. The proposed non-inferiority margin is a lower 90%
bound strictly above -1/12, with no planted decision violated more often. `--phase pilot` is the
default: it reports variance and never establishes non-inferiority. Confirmatory sample size
must be chosen from the pilot before using `--phase confirmatory`. One lineage has no interval.

Candidate arms are `native-bare`, `native`, `native-full`, `restore-only`, `instructions-only`,
`native+checkpoint`, and `native-full+checkpoint`. They start the real daemon with the mode in
`arms.LONG_SESSION_SWITCH` (placeholder environment variable `BRIGADIER_LONG_SESSIONS`, value
`native` for that arm). No compaction, restore or checkpoint logic is imitated in Python.
An older binary lacking the switch name skips cleanly before daemon startup. Phase B must
align the switch/mode values and threshold controls with its real build before Stage 0;
string presence is a preflight, not proof that a particular ablation worked.

## Private evidence and expiry

Every attempted run captures evidence **before** deleting its native session/project:

- Default location: `~/.cache/brigadier-continuity/private/evidence-<timestamp>-<random>/`.
  Set `BRIGADIER_BATTERY_EVIDENCE_DIR` to a persistent private directory on a runner if needed.
- Owner-only permissions: directories 0700, files 0600. Git checkouts and linked export targets
  are refused. Public reports omit this location, transcripts, payloads, checkpoints and answers.
- `evidence.json` contains steps, notes, bootstraps, planted values and scoring. Native JSONL and
  sidecars retain tool outputs; `record/` retains ledger, checkpoints and put-back receipts.
  `call-audit.json` retains per-call native usage/TTL and lookup commands, outputs and matched
  fact keys. A marker match identifies supporting evidence; raw results allow inspection of
  whether the lookup read the intended item. `sources.json` maps private copies to native files.
- Bundles expire after **7 days**. Expired marked bundles are removed at each harness or nightly
  startup. On an otherwise idle persistent host, run `python3 tools/continuity-battery/evidence.py
  purge` periodically; cleanup is invocation-driven, not a background service. A hosted runner
  may destroy its home sooner, so a persistent private mount is required for later diagnosis.
  Neither the public artifact nor this code uploads transcripts or provisions private storage.

The optional `--evidence` legacy export is a caller-owned export in addition to the expiring
bundle; its caller must delete it (the nightly's temporary-directory wrapper already does).
No paid validation or full endurance run is implied by the dry-run commands above.

## Manual experiment dispatch (Stage 0 and pilot)

The existing `workflow_call` from `nightly.yml` and the default manual `runner=legacy`
keep the same arms, credentials, schedules and public artifacts. The experiment path is
opt-in: choose `runner=experiment`, a fixed `preset`, or `dry_run=true`. A preset selects
experiment automatically and overrides `profile`, `arms`, `providers` and `repetitions`.
No workflow is dispatched by these local planning commands:

```sh
python3 -B tools/continuity-battery/dispatch.py plan --preset stage0 --dry-run --output /tmp/stage0-plan
python3 -B tools/continuity-battery/dispatch.py plan --preset pilot --dry-run --output /tmp/pilot-plan
```

Dispatch inputs added alongside the existing `arms` and `zed_long` inputs:

- `runner`: `legacy` (default) or `experiment`.
- `preset`: `none` (custom), `stage0`, or `pilot`.
- `profile`: `work`, `endurance`, `recall-short`, `recall-long`, `updates-short`,
  `updates-long`, `decisions-short`, or `decisions-long`. Legacy scenarios in this path
  run with `--fair --tokens 200000`, nine/90 fillers and 1,200/3,500 ballast words.
- `arms`: the existing comma-separated input also accepts `plain@200k`, `native-bare`,
  `native`, `native-full`, `restore-only`, `instructions-only`, `native+checkpoint`,
  `native-full+checkpoint`, the Codex-only `native-bare@codex-default`,
  `native@codex-default`, `plain@codex-default`, and the test-only `seam-repaired` on the experiment path.
  Its existing default (`seam,plain,zed`) is unchanged.
- `providers`: `claude`, `codex`, or `both` (default).
- `repetitions`: positive integer, default 1. Custom matrices cannot exceed 256 arm-runs.
- `dry_run`: default false. True runs only the offline plan job, without reading provider
  secrets, installing CLIs, building a daemon or calling models/judges. The plan is printed
  in the job summary and uploaded as `continuity-experiment-plan`.

Preset matrices (one arm-run means one arm in one independent lineage):

| Preset/block | Provider | Profile | Arms | Repetitions | Arm-runs |
|---|---|---|---|---:|---:|
| stage0 | both | recall-short and updates-short | seam, plain@200k | 1 | 8 |
| pilot/P1 | Codex | updates-short | seam, native-bare, native, plain | 4 | 16 |
| pilot/P1 | Codex | recall-long | native-bare@codex-default, native@codex-default, plain@codex-default | 2 | 6 |
| pilot/P2 | Claude | work | plain@200k, native-bare, native-full, seam | 2 | 8 |
| pilot/P3 | both | recall-short | seam, seam-repaired | 6 | 24 |
| pilot/endurance | Claude | endurance | plain@200k, native-full | 1 | 2 |
| pilot/endurance | Codex | endurance | plain@200k, native | 1 | 2 |

The P1 Codex long variants use Codex's own compaction point (run2-D18): the
Brigadier arms omit `BRIGADIER_SEAM_TOKENS`, and plain installs no
`model_auto_compact_token_limit`. The suffix appears in cell ids and reports; the
real daemon still receives the base mode (`native-bare` or `native`). Fair lookup
access and the fixed filler schedule remain enabled. All other cells keep their
existing thresholds, including the seam arms' 200k point.

Stage 0 uses the proposal's **$10–20** short-profile budget envelope. The pilot is **58
arm-runs**, **$462.80–572.80** list equivalent; the assumed +10% judge and +20% failed-run
allowance makes it **$601.64–744.64**, before Stage 0. These are planning estimates, not
subscription charges. P3 uses the table's approximate $50 block estimate. No P5 output-cap
arm is included (run2-D17); no checkpoint ablation or confirmatory round is included.
The planner lists all groups, counts and budget assumptions before any model call. Reruns
are an allowance only: the workflow does not automatically spend it retrying paid lineages.

`seam-repaired` sets `BRIGADIER_LONG_SESSIONS=seam-repaired` on the real daemon: size
seams remain as before, native compaction stays off, and only P3's lookup instruction,
tool-evidence pointers and working-state hand-off text are added. It is a test mode, never
a product default. Other candidate arms keep using the same real product switch.

Each cell is bounded to 330 minutes inside a 350-minute job, with at most four concurrent
cells. Work/endurance seeds and task order match across arms for each repetition; the
public collector recombines those rows for the existing paired pilot statistics and cost
per fully passing lineage. The existing legacy scenarios randomize their plants separately:
their repetitions do not provide paired non-inferiority intervals. Short-profile success
validates execution only; it cannot prove the long-profile eviction or endurance bar.

Live cells consume only the workflow's existing credentials and skip with `needs the user:
secrets` when missing. They upload public per-cell reports (`continuity-experiment-cell-*`,
30 days) and a separate public `continuity-experiment` aggregate (90 days). This does not
replace the nightly `continuity-battery` baseline or open regression issues.

CI raw evidence goes to **`$RUNNER_TEMP/continuity-private/evidence-*`**, outside all upload
paths, with w11's owner-only permissions and seven-day expiry manifests. It is **never
uploaded**, including by the collector. Hosted-runner teardown deletes it sooner, so these
hosted runs cannot provide post-run raw transcript diagnosis. To retain that evidence for
seven days, use a persistent private runner and set `BRIGADIER_BATTERY_EVIDENCE_DIR` to a
private persistent directory (and arrange periodic `evidence.py purge` on an idle host).
A separately named GitHub artifact would still be public in this repository and is not
used for raw evidence.
