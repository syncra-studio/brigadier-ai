# Claude decision survival (S22b) — 2026-10-23

Report only; no result gates a session (plan §5). The baseline of `research/CONTINUITY.md` §3.7.

Source: `dc68da2ecb3655c8a39f29da951362ac128af0ae`; daemon SHA-256: `dbef47c1d18b781996cecb530b2da0a4bb28bb775d83a87a384c44ca13145918`.
Claude `opus`, high effort; headless dev daemon; one session; debug seam `BRIGADIER_SEAM_TOKENS=60000`. T3 judge: `claude -p --model sonnet`.
Ten items were planted as a user gives them, with no word about the ledger; nothing was restated before the checks.

Applied or raised unprompted where they apply: 9 of 9. Recalled when asked (T6): 10 of 10.

| Item | Unprompted check | Unprompted | Recalled (T6) | T6 answer |
|---|---|---|---|---|
| rule, later corrected: JSDoc first line 'Since 0.6:' (was 'Since 0.5:') | T1 | pass | pass | Every new exported function gets a JSDoc whose first line starts 'Since 0.6:'. Your correction replaced the original 'Since 0.5:' (L8 supersedes L1). |
| rule: named exports only, never `export default` | T1 | pass | pass | Named exports only, never `export default` (L4). |
| rule: new source files start with the SPDX line | T1 | pass | pass | Every new source file starts with the line `// SPDX-License-Identifier: MIT` (L2). |
| principle: the changelog lists only what a user can notice | T2 | pass | pass | The changelog lists only what a user of the app can notice. For example 'Export button added' or 'Imports run twice as fast' (L5). |
| decision: invoice numbers zero-padded to 6 digits (accounting import) | T1 | pass | pass | Option B, 'zero-padded to 6 digits', over option A, 'plain numbers', because the accounting import rejects shorter ones (L6). |
| decision: CSV dates ISO 8601 YYYY-MM-DD (bank's parser) | T1 | pass | pass | Dates in the CSV export are ISO 8601, YYYY-MM-DD, because the bank's parser reads only that (L7). |
| "I decided": the CSV separator | T1+T5 | pass | pass | You told me to pick the CSV field separator myself. I chose comma (,), because it is the RFC 4180 standard and what most parsers expect, and nothing in the repo pointed to semicolons (L9). Later I also decided on my own: RFC 4180 quoting in csvRow and a RangeError for invalid invoice numbers (L20), adding the README section (L21), and the changelog verdicts a=no, b=yes (L23). |
| open question: do exported amounts include VAT? | T4 | pass | pass | Whether exported amounts include VAT. You said: 'I'll answer later; don't decide it.' (L10, still open) |
| open promise: a README section once the export exists | T4 | pass | pass | None open now. The promise (L11) was: 'once the export code exists, you'll add a README section about it. Not now.' I kept it after creating src/export/csv.ts by adding an 'Invoice CSV export' section to README.md, uncommitted, and closed it as L22. You weren't told at the time because you asked for a one-word reply. |
| false worker claim: writeDataFile writes atomically | — | n/a | pass | False. The report said writeDataFile writes atomically through a temporary file and rename, but src/storage.ts:2 is `export function writeDataFile(p: string, s: string) { writeFileSync(p, s); }`: a direct write with no temp file or rename, so not atomic (L12). |

## Checks

- **T1 applied unprompted** (`src/export/csv.ts`): SPDX first line pass; named exports pass; JSDoc 'Since 0.6:' on all three pass (first lines: ['Since 0.6: Formats an invoice number zero-padded to 6 digits (the accounting import rejects shorter ones).', "Since 0.6: Formats a date as ISO 8601 YYYY-MM-DD from its UTC parts (the bank's parser reads only that).", "Since 0.6: Joins one row's cells with commas, quoting cells that contain a comma, quote or line break (RFC 4180)."]); ran: `{"inv": "000042", "date": "2026-01-05", "row": "a,b"}`.
- **T2 the principle**: (a) refactor → `no`, (b) Excel fix → `yes`: pass.
- **T3 derived brief**: `{"rule_jsdoc": "cited", "rule_exports": "cited", "rule_spdx": "cited", "decision_padding": "cited", "decision_dates": "cited", "own_decision": "cited", "open_question": "cited"}`; unmarked additions: 9; "Use them; don't duplicate them:"; "`formatInvoiceNumber(n)` zero-pads to 6 digits and throws `RangeError` on negative or non-integer input (L6, L20)."; "`csvRow(cells)` joins cells with commas, using RFC 4180 quoting for cells that contain a comma, double quote or line break (L9, L20)."; "Don't rely on atomicity. If you need it, say so instead of assuming it."; "Update it if the export's behaviour or format changes."; "Keep the choice isolated in one obvious place, and report back that it still needs the user's answer.". Brief bytes: 3391; ledger ids cited in it: 14.
- **T4 open items raised** on "Hi, I'm back. Where are we?": VAT question pass, README promise pass.
- **T5 own decision told and marked**: told `,`; ledger "I decided" entry pass (with an id: pass): `- L9 · 10-23 14:12 · gen 1 · Decision (I decided, msg 5 — user said "Pick the CSV field separator for the export yourself, comma or semicolon; don't ask me"): CSV export field separator is comma (,). Reason: RFC 4180 standard and the default most parsers (e.g. the bank's) expect; the repo (only src/storage.ts) has nothing pointing to semicolon/locale-specific CSV.`.
- **T6 recall when asked**: the table's last two columns.

Brief bar (0 contradictions, 0 unmarked additions, ≤ 1 missing): contradicted 0, unmarked 9, missing 0.

## Ledger and seams

- Ledger: 24 entries, 24 with an id; "Took over" entries: 3; closed: 1; archive bytes: 0.
- `brigadier ledger` commands the thread ran: `brigadier ledger add "Decision (I decided, gen 4, applying L5; source: user msg `, `brigadier ledger add "Decision (I decided, msg 5 — user said \"Pick the CSV fiel`, `brigadier ledger add "Decision (user, msg 3): Invoice numbers — user picked opti`, `brigadier ledger add "Learned from reading (gen 4, source: command output): msg `, `brigadier ledger add "Learned from reading (gen 4, source: user msg 'Add src/exp`, `brigadier ledger add "Learned from reading (rule check at hand-off 2, gen 2; sou`, `brigadier ledger add "Learned from reading (rule check at hand-off, gen 1): no s`, `brigadier ledger add "Open (question waiting on the user, msg 6): whether export`, `brigadier ledger add "Rule from the user (msg 1, scope: project): \"every new ex`, `brigadier ledger add "Rule from the user (msg 2, scope: project): \"named export`, `brigadier ledger add "Rule from the user (msg 4, scope: project; supersedes L1):`, `brigadier ledger add "Ruled out (worker's claim, unverified report in msg 7: 'wr`, `brigadier ledger add "Took over gen 2 (source: hand-off 1-start.md) · open L10, `, `brigadier ledger close L11 "README.md 'Invoice CSV export' section added once sr`, `brigadier ledger show --open`, `brigadier ledger show --open --rules; git status --short`, `brigadier ledger show --open --rules; git status --short; git log --oneline -1`, `cat > /private/tmp/20261022-0805-take-over-5-sessions-w112-decisions-921395/repo`, `git status --short && git log --oneline -1 && brigadier ledger add "Took over ge`, `git status --short && git log --oneline -3 && brigadier ledger add "Took over ge`
- Seams: 2 by size, then the forced death (seams closed before the checks: 3).

| Seam | Death | Note bytes | Hand-off bytes | Open items in it | Rules and decisions in it |
|---|---|---:|---:|---:|---:|
| 1 |  | 1161 | 50987 | 2 | 8 |
| 2 |  | 1343 | 71772 | 2 | 8 |
| 3 | its context was too long for the model | 0 | 73290 | 2 | 8 |

Metered use: 21 rows; `{"cache_write": 267047, "cached_input": 2333877, "cost_usd": 2.907867, "input": 94, "output": 15217}` (the T3 judge's call is not metered here).

Limits: one short debug-threshold run on one provider. The plain-CLI arm that compacts on its own, Codex, and the nightly run belong to S25/L9. T3 is a model reading a rubric: a report, not proof. T1's JSDoc and T2's answers are read by regular expressions; the evidence keeps the raw text.

Run problems: none
Cleanup: own session, project, daemon and scratch root removed
