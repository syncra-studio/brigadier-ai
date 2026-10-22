# Claude continuity baseline — 2026-10-22

Report only; no recall result gates a session or changes continuity.

11/12 planted items passed. The final probe was allowed to search existing records.

Source: `de8e53aa4687c144630a48dd004960701e168de0`; daemon SHA-256: `95e05be79043151546163efcf83e37c07a3c2484b75920329cffd8ea2c37039f`.
Claude `opus`, high effort; headless dev daemon; one session, several requests.
Prepare threshold 35000 tokens; swap at 1.25×; briefing cap 4000 estimated tokens.
Mandatory sections and the minimum tail can exceed that briefing budget; observed sizes follow below.
A labelled, injected unverified worker report supplies the false claim; no real worker is spawned.
Random one-time facts are removed from the fixture after observation. Inert batches displace the early items.

| Item | Result | In final verbatim tail? | Successor searched? | Answer / evidence |
|---|---|---|---|---|
| standing rule applied | pass | no | yes, matching output | Every new exported function gets a JSDoc first line starting 'Since 0.5:'. |
| correction applied in exporters | pass | no | yes, matching output | In src/exporters/ use 'Exporter since 0.5:' instead. |
| decision and reason | pass | no | yes, matching output | Invoice numbers stay zero-padded to 6 digits because the accounting import rejects shorter ones. |
| release commit | pass | no | yes, matching output | 4e1d9b7c2a03 |
| file and line | pass | no | yes, matching output | src/invoice.ts:57 |
| number with units | pass | no | yes, matching output | 4500 ms |
| failed check error | pass | no | yes, matching output | ERROR row 17: missing tax_region for B-2291 |
| ruled-out approach and reason | pass | no | yes, matching output | Caching catalogFromStore in a module-level Map, because it outlived each test's data. |
| only in one-time tool output | pass | no | yes, matching output | VT-F976B10D2F |
| false worker claim rejected | pass | no | yes, matching output | False: writeDataFile at src/storage.ts:2 calls writeFileSync(p, s) directly on the target path, with no temporary file and no rename, so the write is not atomic. |
| deliberately absent from ledger | pass | no | yes, matching output | ORCHID-BDF65DEFC7 |
| middle of large failing output | unknown | no | yes, attempted | unknown |

Observed session providers/models: `claude/opus`, `codex/gpt-6-astra`.
The usage-limit injection may use today's stand-in path; the requested Claude model returns after its window resets.

| Forced seam | Observed |
|---|---|
| Size ×2 | 2 rebirth records before fault injections |
| context | `{"armed": true, "at_ms": 1792701487681, "completed_turns": 1, "done_ms": 1792701487682, "errors": ["contextWindow"], "kind": "context", "request": "01a16555-da41-7753-8c09-8fb06f717c94", "sessions_started": 1}` |
| crash | `{"at_ms": 1792701521562, "completed_turns": 0, "done_ms": 1792701524114, "errors": [], "killed_cli_pids": [28091], "kind": "crash", "request": "01a16556-5e99-7100-91c2-a99179464210", "sessions_started": 0, "wait_point_seen": true}` |
| restart | `{"at_ms": 1792701639276, "completed_turns": 0, "done_ms": 1792701643854, "errors": [], "kind": "restart", "request": "01a16558-2a6c-7731-9ea4-bfa6d585dd57", "restarted": true, "sessions_started": 1, "wait_point_seen": true}` |
| limit | `{"armed": true, "at_ms": 1792701759476, "completed_turns": 3, "done_ms": 1792701760488, "errors": ["usageLimit"], "kind": "limit", "last_error": "debugInjectLimit: {\"status\": \"err\", \"error\": {\"code\": \"invalid\", \"message\": \"nothing is running there: a fault is injected into a running CLI session\"}}", "request": "01a16559-fff4-7509-bfe3-0cf0fb33320e", "sessions_started": 3}` |

Ledger: 563 bytes; deliberate item absent: True.
Note and briefing token estimates use the runtime's bytes/4 convention; first-call sizes use metering.

| Generation / trigger | Note bytes | Briefing tokens | Prepare→swap ms | Successor first context | Calls to first work tool |
|---|---:|---:|---:|---:|---:|
| 1 / threshold | 3893 | 10525 | 18580 | 55573 | not reached |
| 2 / threshold | 3413 | 15484 | 27422 | 68373 | 3 |
| 3 / midTurn | 3886 | 17537 | 17076 | 61301 | 1 |
| 4 / threshold | 4341 | 10364 | 118616 | 41028 | 1 |
| 5 / recovery | 0 | 5391 | 32 | None | unavailable |
| 6 / threshold | 5109 | 7151 | 76850 | 31377 | 1 |

Final probe provider/model: `claude/opus`.

Probe lookup calls observed: 7. Item search evidence means its marker appeared in a lookup result, or an explicit query attempted it; one result can serve several items.
Calls to first work tool count native assistant API message ids, including prior ballast acknowledgements; work tools are Bash, Read, Write or Edit. Missing native files are unavailable, never zero.
Seam cost below includes all metered handoff calls; ordinary thread calls are separate (successor bootstrap is included in thread use).
- handoff: 6 usage rows; `{"cache_write": 14482, "cached_input": 339797, "cost_usd": 0.408961, "input": 12, "output": 10618}`; 0 rows without reported dollar cost
- thread: 28 usage rows; `{"cache_write": 275932, "cached_input": 1943699, "cost_usd": 2.707352, "input": 14630, "output": 6015}`; 2 rows without reported dollar cost

Limits: a short debug-threshold baseline, not the ≥400k token A/B or a nightly result. Both-provider A/B, cold-return ratio, and weekly real-lineage samples belong to S25/L4. No fewer-tokens claim is established. Later seams share this session, so recall cannot be attributed to one seam alone.
The middle marker of the >250 KB failing command was not recovered. The retained command output was already excerpted; lookup attempts returned that excerpt. This measures the existing output path's limit.

Run problems: seam-crash: turn did not settle within bound; seam-restart: turn did not settle within bound
Cleanup: own sessions, project, daemon and scratch root removed

Harness note: this first run spent a timeout waiting on the crash/restart requests after they were already Failed. The waiter now accepts Failed as terminal. Subsequent requests completed; these waits do not imply the session stayed stuck.
