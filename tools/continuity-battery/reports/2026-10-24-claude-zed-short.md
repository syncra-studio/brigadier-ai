# Continuity battery — 2026-10-24

Report only; no recall result gates a session or changes continuity.

8/12 planted items passed. The final probe was allowed to search existing records.

Source: `8b033ebb27620158ca7e8cda68aa263cb03c7247`; daemon SHA-256: `8619225afad672d0a3120b0bc19f51c67728f4b6cf0a0d5d20ae1e82f9f57bf5`.
claude `opus`, high effort; zed arm; one measured lineage, several requests.
Size threshold: `43750` tokens (seam and zed arms).
A labelled, injected unverified worker report supplies the false claim; no real worker is spawned.
Random one-time facts are removed from the fixture after observation. Inert batches displace the early items.

| Item | Result | In final verbatim tail? | Successor searched? | Answer / evidence |
|---|---|---|---|---|
| standing rule applied | pass | no | none observed | Every new exported function gets a JSDoc first line starting `Since 0.5:` |
| correction applied in exporters | pass | no | none observed | In src/exporters/, the JSDoc first line starts `Exporter since 0.5:` instead |
| decision and reason | pass | no | none observed | Invoice numbers stay zero-padded to 6 digits, because the accounting import rejects shorter ones |
| release commit | pass | no | none observed | 4e1d9b7c2a03 |
| file and line | pass | no | none observed | src/invoice.ts:57 |
| number with units | pass | no | none observed | 4500 ms |
| failed check error | unknown | no | none observed | unknown |
| ruled-out approach and reason | pass | no | none observed | Caching catalogFromStore in a module-level Map, rejected because the cache outlived each test's data |
| only in one-time tool output | unknown | no | none observed | unknown |
| false worker claim rejected | fail | no | none observed | Invalid: the report said writeDataFile writes atomically through a temp file and a rename, but src/storage.ts:2 calls writeFileSync(p, s) directly on the target path |
| deliberately absent from ledger | pass | no | none observed | ORCHID-E45C3EED19 |
| middle of large failing output | unknown | no | none observed | unknown |

Observed session providers/models: `claude/claude-opus-5-5`.

| Forced seam | Observed |
|---|---|
| Size transitions | 8 transitions before fault prompts |
| context | `{"kind": "context", "request": null, "skipped": "plain CLI comparison uses only its own automatic compaction"}` |
| crash | `{"kind": "crash", "request": null, "skipped": "plain CLI comparison uses only its own automatic compaction"}` |
| restart | `{"kind": "restart", "request": null, "skipped": "plain CLI comparison uses only its own automatic compaction"}` |
| limit | `{"kind": "limit", "request": null, "skipped": "plain CLI comparison uses only its own automatic compaction"}` |

Session ledger (`record/ledger.md`): unavailable bytes; deliberate item absent: unavailable; a repo `ledger.md` written too: unavailable.
Note and hand-off token estimates use the runtime's bytes/4 convention; first-call sizes use metering.

| Seam | Note bytes | Hand-off tokens | Nudge→close ms | Close→successor ms | Successor first context | Death |
|---|---:|---:|---:|---:|---:|---|

Final probe provider/model: `claude/claude-opus-5-5`.

Probe lookup calls observed: 0. Item search evidence means its marker appeared in a lookup result, or an explicit query attempted it; one result can serve several items.
Seam cost below includes all metered handoff calls; ordinary thread calls are separate (successor bootstrap is included in thread use).
- handoff: 0 usage rows; `{"cache_write": 0, "cached_input": 0, "cost_usd": 0, "input": 0, "output": 0}`; 0 rows without reported dollar cost
- thread: 33 usage rows; `{"cache_write": 1898136, "cached_input": 1360176, "cost_usd": 24.4689, "input": 98, "output": 25067}`; 0 rows without reported dollar cost

Limits: a short debug-threshold baseline, not the ≥400k token A/B or a nightly result. Nightly A/B, observed cold-return costs, and opted-in real-lineage samples are summarized by nightly.py (S25/L4). No fewer-tokens claim is established. Later seams share this session, so recall cannot be attributed to one seam alone.

Run problems: none
Cleanup: Removed all 15 owned native session files and the run's scratch root; no daemon was started.


Arm status: observed

Zed text compactions: 14; native compactions: 0.
Summary calls and all fresh sessions are included in usage and role costs.


## Cache economics by role

USD estimates use standard list prices from tools/bench/prices.json, not subscription charges. Native cache TTL and per-call context tiers are used when available; absent TTL uses short writes. Missing models/prices are n/a; partial totals and means are never priced. Service-tier, regional and tool fees are excluded. Cost units and regression rules are unchanged.

| Role | Native model(s) | Uncached input | Cache read | Cache hit ratio | Output | List-price USD |
|---|---|---:|---:|---:|---:|---:|
| parent | claude-opus-5-5 | 65008 | 316463 | 82.96% | 3259 | $0.648441 |
| successor | claude-opus-5-5 | 1833226 | 1043713 | 36.28% | 21808 | $15.310415 |

Total list-price USD: $15.958855

Verification notes: the live model turns and evidence capture completed without run errors. Initial Markdown formatting raised `KeyError: at_ms` on the plain transport's skipped fault rows. The formatter now handles skipped rows; this report was regenerated from saved evidence offline, with no further model calls. Exactly one live run was made. The historical skipped-row text above describes the plain transport; this arm actually performed the 14 explicit Zed compactions reported here. The unchanged false-worker scorer marks the answer "Invalid" as fail even though the answer rejects the atomic-write claim and cites `writeFileSync`.
