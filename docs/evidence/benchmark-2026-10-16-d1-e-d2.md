# Benchmark: D1 + E + D2 ship gate (2026-10-16)

The one combined bench for D1 (never forgets, without the output store), E (never hallucinates) and D2 (the
output store and compaction), against the plain rows (one provider CLI, no Brigadier) recorded for phase C.

- **T1–T6:** build `63dde77c` (revision 63), n=2 per task, T4 n=4. Tag `degate63`.
- **T7L:** build `5d63bc50` (revision 62), n=3. Tag `degate`. Revision 63 only moves `run_check`'s commit onto the
  first check expected to pass. T7L passed every gate at 5d63bc50, so it wasn't rerun. The two builds' rows are
  never pooled.
- **Setup:** Claude Code 2.1.301 (Opus 5.5) and Codex CLI 0.163.0 (gpt-6.1-sol), high effort.
- **Machine:** no quiet hold (Q54). Every T4 arm started at 1-minute load ≤ 4 with no build running.

**Token gate (Q52):** the total minus Codex review tokens and the turns spent on review findings. Background
indexing and the blind judges are excluded. Limits: T1–T6 ≤ 1.15× plain, T7L ≤ 1.0× plain.

## T1–T6 at 63dde77c

| Task | Plain median | Brigadier gate statistic | Gate ratio | Full ratio | Hidden tests | Wall s (plain) |
|---|---:|---:|---:|---:|---|---:|
| T1 | 132,347 | 121,181.5 | **0.92×** | 0.92× | 2/2 pass | 47.8 (46.3) |
| T2 | 1,072,833 | 683,921 | **0.64×** | 0.80× | 2/2 pass | 196.2 (215.2) |
| T3 | 1,905,810.5 | 1,136,694.5 | **0.60×** | 0.93× | 2/2 pass | 420.4 (483.5) |
| T4 | 201,944.5 | 212,945.5 | **1.05×** | 1.05× | 4/4 pass | 68.7 (68.55) |
| T5 | 1,086,460 | 595,349.5 | **0.55×** | 1.03× | 2/2 pass | 292.1 (343.5) |
| T6 | 256,807 | 225,573 | **0.88×** | 0.88× | 2/2 pass | 91.6 (139.0) |

- **The T4 statistic** is the mean of the middle two of its four runs: 171,970, 211,657, 214,234 and 258,913 tokens.
  The earlier T4 at 5d63bc50 was 1.54×: it ran the check, then the same check again with the commit. Revision 63
  puts the commit on the first check expected to pass. Three of the four T4 runs now call `run_check` once; one
  still ran a second, cached check.
- **T4 wall time** is 68.7 s against plain's 68.55 s (1.002×), down from 106.3 s. The four runs took 64.6, 64.9,
  72.5 and 97.2 s.

## T7L at 5d63bc50

| | Plain | Brigadier |
|---|---:|---:|
| Gate statistic (median of 3) | 5,260,538.5 | 3,704,746 (**0.70×**) |
| Full ratio | | 0.88× |
| Requested features (F1–F24) | 24/24 | 24/24 in each run |
| Hidden-test groups | 26/26 | 26/26 in each run |
| CLI compactions (thread and workers) | | 0 in every run |
| Wall s (median) | 1159 | 1002.3 |

Every standing constraint holds in every final result, by its hidden tests. The judges' medium findings on
constraints don't show a lost constraint.

## High findings

The blind judges' high findings were checked against each run's own eval. None is a real defect beyond plain's.

| Run | Finding | Verdict |
|---|---|---|
| T2-1 | Cart operations drop the discount | False: the hidden test `cart operations keep the code` passes |
| T3-1, T3-2 | Exact cent rounding changes some invoice amounts (13.70 QC, 80.30 AB) | The task itself requires halves rounded away from zero in integer cents. Plain's three T3 runs get the same finding (13.70 QC, 80.30 AB, 33.50 QC) |
| T3-2 | Decimal parser accepts `1\n` | False: JS `$` without the `m` flag doesn't match before a final newline, and the run's rejection test passes |
| T5-1, T5-2 | New test fails strict TypeScript | False: `tsc` exits 0 with no errors |
| T5 (5d63bc50) | Three highs | False: `tsc` exits 0 with no errors, strict over src and test |

## The final answer audit (E3)

All 14 T1–T6 final answers at 63dde77c pass `tools/ab/answer_audit.py` (exit 0, no missing labels, no unchecked
claims), as do all 15 at 5d63bc50.

## Evidence

Result rows `bench/results/*-brigadier-degate63-*.json` and `*-brigadier-degate-*.json`, kept outside the repo.
The full reports are in the run directory (w65 chores 4 and 10).
