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
Without this option, only the text report remains. Exit 0 means the probe ran,
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

The existing scripts now accept `--provider claude|codex` and `--arm seam|plain|zed`.
`plain.py` changes only the transport: the same plants, fixture, tasks and scorers go
through `claude -p --resume` or `codex exec resume`, with automatic compaction at the
CLI's own default. It installs no seam or standing ledger instructions. The decision
block scores a plain CLI's own decision by its announced choice and applied separator;
only Brigadier's arm is also checked for its ledger marker. No `/compact` is sent.

The workflow runs all three arms for short recall (nine filler turns), long recall (90),
and decision survival (90), with matching filler schedules within each provider.
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
evidence retains the daemon exit code and log tail if its IPC connection disappears. Raw evidence
stays in runner scratch and is deleted; uploaded Markdown contains only the synthetic
battery's data, and `summary.json` contains the numbers. Never upload real transcripts.

Every run uploads a consolidated **continuity-battery** artifact (`summary.json`,
`report.md`, per-arm Markdown), retained for 90 days, plus provider artifacts for 30.
Settings › Reports reads the newest retained artifact on main via the user's existing
`gh` installation/sign-in, and Refresh reads it again. Missing artifacts, read failures,
and skipped live arms stay visible. This checkout had no existing Reports component;
the page reuses `SettingsPage`, `SettingsSection`, `SettingsCard` and `SettingsRow`.

The summary reports native compactions, recall, task checks, all metered token kinds,
reported dollar costs and price-weighted A/B ratios when both arms have reported prices,
decision application/brief/open-item scores, and observed cold-return cost. Codex does
not report dollar prices, so its dollar cost and price-weighted ratio are explicitly
unavailable; no price is guessed from a model name. A cold return is a later uncached
request, excluding the initial cold start; if none is observed, its cost is unavailable.
Cold-return costs never trigger issues. The first artifact is the baseline; matching
completed observations compare to the preceding artifact. Recall/task/decision/token
bar misses, lost recall/application from that baseline, and native Claude compactions
open or update a **Continuity battery regression** issue. Issue-post failure is reported
by Actions but cannot gate sessions or app builds. Missing data is never a pass.

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
