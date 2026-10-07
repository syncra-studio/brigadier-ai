#!/usr/bin/env python3
"""Brigadier arm tokens from the arm's own data dir: routing.sqlite `turn_usage` rows of the
arm's conversation (its orchestrator and every task) between t0 and the end. Brigadier stores
input without cached input, cache reads (cached_input) and cache writes separately; Codex's
app-server reports cache writes only when it has them, so a Codex cache-write total of 0 is
shown as "not reported". Project-level background work (Brain jobs) and upkeep are listed apart.
A daemon from phase 2 on meters a Codex worker's or thread's child threads (the auto-review,
"guardian") itself: those rows have step 'guardian' and name the child in `child_thread`, listed
as "metered_child_threads" so armtokens.sh doesn't add them again from their rollouts. An older
database has neither column.
usage: brig_tokens.py <arm-dir> <end_ms> [--json out]"""
import collections, json, os, sqlite3, sys

arm = sys.argv[1]; end = int(sys.argv[2])
st = json.load(open(os.path.join(arm, "start.json"))); conv = st["conversation"]; t0 = st["t0_ms"]
db = sqlite3.connect(f"file:{os.path.join(arm, 'data', 'routing.sqlite')}?mode=ro", uri=True)
FIELDS = ("input", "cached_input", "cache_write", "output")
COLUMNS = {row[1] for row in db.execute("PRAGMA table_info(turn_usage)")}
STEP = "step" if "step" in COLUMNS else "NULL"
CHILD = "child_thread" if "child_thread" in COLUMNS else "NULL"


def sums(where, args):
    out = collections.defaultdict(collections.Counter)
    q = f"SELECT provider, model, task_id, {STEP}, {', '.join(FIELDS)} FROM turn_usage WHERE at_ms BETWEEN ? AND ? AND {where}"
    for provider, model, task, step, *vals in db.execute(q, (t0, end, *args)):
        c = collections.Counter(dict(zip(FIELDS, vals)), turns=1)
        out[provider].update(c)
        role = "guardian" if step == "guardian" else "task" if task else "orchestrator"
        out[f"{provider}:{model}:{role}"].update(c)
    return out


def shown(c):
    raw = sum(c[f] for f in FIELDS)
    return {"input_uncached": c["input"], "cache_read": c["cached_input"], "cache_write": c["cache_write"],
            "output": c["output"], "raw": raw, "raw_without_cache_reads": raw - c["cached_input"], "turns": c["turns"]}


arm_rows = sums("conversation_id = ?", (conv,))
background = sums("conversation_id IS NULL", ())
result = {"window_ms": [t0, end], "conversation": conv, "by_provider": {}, "by_model_role": {}, "background": {},
          "metered_child_threads": sorted({row[0] for row in db.execute(
              f"SELECT {CHILD} FROM turn_usage WHERE conversation_id = ? AND {CHILD} IS NOT NULL", (conv,))})}
for key, c in arm_rows.items():
    (result["by_provider"] if ":" not in key else result["by_model_role"])[key] = shown(c)
for key, c in background.items():
    if ":" not in key:
        result["background"][key] = shown(c)
for provider, row in result["by_provider"].items():
    if provider == "codex" and row["cache_write"] == 0:
        row["cache_write"] = "not reported"
total = collections.Counter()
for row in result["by_provider"].values():
    for k in ("raw", "raw_without_cache_reads"):
        total[k] += row[k]
result["total"] = dict(total)
print(json.dumps(result, indent=1))
if "--json" in sys.argv:
    json.dump(result, open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1)
