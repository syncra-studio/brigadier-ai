#!/usr/bin/env python3
"""The plain Claude Code arm of the long-session head-to-head (`battery/a10long.md`): the user's own
unmodified `claude`, one session driven step by step with `claude -p` and `--resume`, through the
same requests as Brigadier's arm (`a10.py --battery a10long`), in a clone of the same template repo,
on the same root (so the ballast paths in the requests are the same).

Flags as the bench's plain arm (`bench/arms/claude.sh`): the model and effort of Brigadier's thread
(Opus, high), permissions skipped (Brigadier's Full access), only the project's setting sources (so
the user's own `autoCompactEnabled: false` doesn't apply: auto-compaction is on, as shipped), no MCP
servers, no auto-memory, no automatic continuation at a usage limit, no idle compaction, no claude.ai
connectors. Everything else is stock Claude Code, including its auto-compaction (near 967k on a 1M
window).

  plain.py run --root R [--battery a10long] [--n 1] [--tag T] [--expect-account EMAIL]
               [--expect-claude VERSION] [--only r1,...] [--smoke-compact-pct N]
  plain.py score RUN_DIR      (rescored from the run directory alone)

Not sent to plain: `p2` (the lookup probe: plain has no session search). Not planted: the false
`learned` line (Brigadier's daemon plants it into a worker report; plain has none), so that item is
not scored. `--smoke-compact-pct` sets CLAUDE_AUTOCOMPACT_PCT_OVERRIDE to try the plumbing on a
small budget; it is never a measurement. A run is `runs/plain-<battery>-<tag>-<n>/`; the session's
Claude project folder is removed by `run.py cleanup` (it is named after the root's `arm-plain/`)."""
import argparse, importlib.util, json, os, re, shutil, subprocess, sys, time, uuid

HERE = os.path.dirname(os.path.abspath(__file__))


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


a10 = load("a10", os.path.join(HERE, "a10.py"))
v2, v3run, v3lib, log, now_ms = a10.v2, a10.v3run, a10.v3lib, a10.log, a10.now_ms
from v2lib import load_battery  # noqa: E402

SETTINGS = {"autoCompactEnabled": True, "autoMemoryEnabled": False, "autoContinueAtUsageLimit": False,
            "idleCompaction": False, "disableClaudeAiConnectors": True}
FLAGS = ["--model", "opus", "--effort", "high", "--dangerously-skip-permissions", "--setting-sources", "project",
         "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}', "--settings", json.dumps(SETTINGS),
         "--output-format", "stream-json", "--verbose"]
STEP_TIMEOUT_S = 3600


def clean_env(extra=None):
    path = ":".join(p for p in os.environ.get("PATH", "").split(":") if "cmux" not in p)
    env = {k: os.environ[k] for k in ("HOME", "USER", "LOGNAME", "TMPDIR") if k in os.environ}
    env.update(SHELL="/bin/zsh", LANG="en_US.UTF-8", PATH=path)
    env.update(extra or {})
    return env


def read_stream(path):
    out = []
    for line in open(path, errors="replace"):
        if line.startswith("{"):
            try:
                out.append(json.loads(line))
            except ValueError:
                pass
    return out


def step_record(events):
    """From one `claude -p` stream: the answer, the usage of every model call it made, its distinct
    assistant messages, the context of its last call and its compactions."""
    result = next((e for e in reversed(events) if e.get("type") == "result"), {}) or {}
    msgs, context, first = {}, None, None
    for e in events:
        m = e.get("message") or {}
        if e.get("type") == "assistant" and m.get("id") and m.get("usage"):
            msgs[m["id"]] = m["usage"]  # sub-agents' calls too, as Brigadier's count has its workers'
            if not e.get("parent_tool_use_id"):
                u = m["usage"]
                context = sum(u.get(k) or 0 for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))
                first = context if first is None else first
    compactions = [{"trigger": (e.get("compact_metadata") or {}).get("trigger"), "pre_tokens": (e.get("compact_metadata") or {}).get("pre_tokens")}
                   for e in events if e.get("type") == "system" and e.get("subtype") == "compact_boundary"]
    usage = {"input": 0, "cache_read": 0, "cache_write": 0, "output": 0}
    for model in (result.get("modelUsage") or {}).values():
        usage["input"] += model.get("inputTokens") or 0
        usage["cache_read"] += model.get("cacheReadInputTokens") or 0
        usage["cache_write"] += model.get("cacheCreationInputTokens") or 0
        usage["output"] += model.get("outputTokens") or 0
    usage["raw"] = sum(usage.values())
    # `modelUsage` covers every call of the session so far (a resumed session carries its total on),
    # compactions and sub-agents included.
    return {"answer": result.get("result") or "", "is_error": result.get("is_error"), "subtype": result.get("subtype"),
            "num_turns": result.get("num_turns"), "duration_ms": result.get("duration_ms"), "usage_cumulative": usage,
            "models": sorted((result.get("modelUsage") or {}).keys()), "model_calls": len(msgs), "context": context,
            "first_context": first,
            "compactions": compactions}


def cmd_run(a):
    root = v3run.check_root(a.root)
    b = load_battery(a.battery)
    h = b["harness"]
    build = json.load(open(os.path.join(root, "build.json")))
    label = a.tag or build["label"]
    smoke = "-smoke" if a.smoke_compact_pct else ""
    rid_ = f"plain-{b['name']}-{label}{smoke}-{a.n}"
    run = os.path.join(root, "runs", rid_)
    if os.path.exists(os.path.join(run, "score.json")):
        log(rid_, "scored already"); return
    acct, version = a10.account(), a10.claude_version()
    if a.expect_account and acct != a.expect_account:
        sys.exit(f"plain: Claude is signed in as {acct}, not {a.expect_account}")
    if a.expect_claude and not version.startswith(a.expect_claude):
        sys.exit(f"plain: claude --version is {version!r}, not {a.expect_claude}")
    if os.path.exists(run):
        v3run.safe_rmtree(root, run)
    os.makedirs(os.path.join(run, "plain"))
    shutil.copyfile(b["path"], os.path.join(run, "battery.md"))
    ws = os.path.join(root, "arm-plain", "repo")
    if os.path.exists(os.path.dirname(ws)):
        v3run.safe_rmtree(root, os.path.dirname(ws))
    a10.clone(os.path.join(root, "template", "repo"), ws)
    git = ["git", "-C", ws, "-c", "user.name=plain", "-c", "user.email=plain@example.invalid"]
    subprocess.run(git + ["checkout", "-q", "-b", "session"], check=True)
    steps = [s for s in b["steps"] if s["role"] != "lookup" and (not a.only or s["id"] in a.only.split(","))]
    ballast_dir = "ballast-smoke" if a.smoke_compact_pct else "ballast"
    extra = {"CLAUDE_AUTOCOMPACT_PCT_OVERRIDE": str(a.smoke_compact_pct)} if a.smoke_compact_pct else {}
    sid = str(uuid.uuid4())
    start = {"arm": "plain", "battery": b["name"], "label": label, "n": a.n, "build": build, "session": sid, "workspace": ws,
             "flags": FLAGS, "smoke_compact_pct": a.smoke_compact_pct, "only": a.only, "account": acct, "claude_version": version,
             "battery_sha256": v3run.sha256(b["path"]), "ballast": json.load(open(os.path.join(root, ballast_dir, "MANIFEST.json"))),
             "started_ms": now_ms(), "steps": [], "sent_ms": [], "ended_ms": [], "records": {}, "ballast_files": {}, "feed": {}}
    save = lambda: json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
    k = 0
    for i, step in enumerate(steps):
        if step["id"] == h.get("feed", {}).get("copy_before"):
            feed = os.path.join(ws, ".import-feed")
            os.makedirs(feed, exist_ok=True)
            for f in sorted(os.listdir(os.path.join(HERE, "fixture", "feed"))):
                shutil.copyfile(os.path.join(HERE, "fixture", "feed", f), os.path.join(feed, f))
            start["feed"]["copied_ms"] = now_ms()
        text = step["text"]
        if step["role"] == "ballast":
            k += 1
            text, files = a10.ballast_text(text, root, ballast_dir, k, h.get("ballast_files_per_step", 2))
            start["ballast_files"][step["id"]] = files
        out = os.path.join(run, "plain", f"{i + 1:02d}-{step['id']}.jsonl")
        args = ["claude", "-p", text + " " + v3lib.SAFETY, *FLAGS] + (["--session-id", sid] if i == 0 else ["--resume", sid])
        t = now_ms()
        start["steps"].append(step["id"]); start["sent_ms"].append(t)
        if i == 0:
            start["t0_ms"] = t
        save()
        log(rid_, f"{step['id']} sent ({i + 1}/{len(steps)})")
        with open(out, "w") as f, open(out[:-6] + ".err", "w") as err:
            p = subprocess.Popen(args, cwd=ws, env=clean_env(extra), stdout=f, stderr=err, start_new_session=True)
            with open(os.path.join(root, "pids"), "a") as pf:
                pf.write(f"{p.pid}\n")
            try:
                p.wait(STEP_TIMEOUT_S)
            except subprocess.TimeoutExpired:
                v3run.stop([p.pid])
        rec = step_record(read_stream(out))
        rec["rc"] = p.returncode
        start["ended_ms"].append(now_ms()); start["records"][step["id"]] = rec
        save()
        log(rid_, f"{step['id']} ended rc={p.returncode}", f"context {rec['context']}",
            f"compactions {len(rec['compactions'])}" if rec["compactions"] else "")
        if step["id"] == h.get("feed", {}).get("delete_after") and os.path.isdir(os.path.join(ws, ".import-feed")):
            shutil.rmtree(os.path.join(ws, ".import-feed"))
            start["feed"]["deleted_ms"] = now_ms()
        if p.returncode != 0 and rec["subtype"] != "success":
            start["stopped"] = f"{step['id']}: rc {p.returncode}"
            break
    start["ended_run_ms"] = now_ms()
    save()
    # The session's transcripts (and its sub-agents'), the workspace as it was left, the hidden checks.
    keep = v3run.ensure(os.path.join(run, "transcripts", "claude"))
    for src in a10.glob.glob(os.path.join(v3run.HOME, ".claude", "projects", "*", f"{sid}.jsonl")):
        os.link(src, os.path.join(keep, f"{sid}.jsonl"))
    for d in a10.glob.glob(os.path.join(v3run.HOME, ".claude", "projects", "*", sid, "subagents")):
        os.makedirs(os.path.join(keep, sid, "subagents"), exist_ok=True)
        for f in os.listdir(d):
            os.link(os.path.join(d, f), os.path.join(keep, sid, "subagents", f))
    files = subprocess.run(["git", "-C", ws, "ls-files", "-co", "--exclude-standard", "-z"], capture_output=True).stdout
    result = v3run.ensure(os.path.join(run, "result"))
    subprocess.run(["tar", "-C", ws, "--null", "-T", "-", "-cf", os.path.join(run, "result.tar")], input=files, check=True)
    subprocess.run(["tar", "-C", result, "-xf", os.path.join(run, "result.tar")], check=True)
    os.remove(os.path.join(run, "result.tar"))
    a10.run_checks(root, run, h)
    score = score_run(run)
    json.dump(score, open(os.path.join(run, "score.json"), "w"), indent=1, sort_keys=True)
    with open(os.path.join(root, "results.jsonl"), "a") as f:
        f.write(json.dumps({"run": rid_, "root": root, **{k: score[k] for k in ("completed", "valid", "battery", "quality", "compaction_count")}},
                           sort_keys=True) + "\n")
    log(rid_, json.dumps({k: score[k] for k in ("completed", "valid", "compaction_count")}), json.dumps(score["battery"]["by_item"]))


# ---------------------------------------------------------------- score

def score_run(run):
    a10score = load("a10score", os.path.join(HERE, "a10score.py"))
    v2score, v3score = a10score.v2score, a10score.v3score
    start = json.load(open(os.path.join(run, "start.json")))
    b = load_battery(os.path.join(run, "battery.md"))
    h = b["harness"]
    checks = json.load(open(os.path.join(run, "checks.json"))) if os.path.exists(os.path.join(run, "checks.json")) else {}
    order = start["steps"]
    # Each step's record again from its own stream (plain/NN-<step>.jsonl), with the exit code kept.
    recs = {}
    for i, step in enumerate(order):
        path = os.path.join(run, "plain", f"{i + 1:02d}-{step}.jsonl")
        if step in start["records"]:
            recs[step] = {**step_record(read_stream(path)), "rc": start["records"][step].get("rc")} if os.path.exists(path) \
                else start["records"][step]
    sent = dict(zip(order, start["sent_ms"]))
    ended = dict(zip(order, start["ended_ms"]))
    roles = {s["id"]: s["role"] for s in b["steps"]}
    compactions = [{"step": s, "at_ms": ended[s], **c} for s in order for c in recs[s]["compactions"]]
    # A compaction is reported in the stream of the step it happened in: it came before that
    # step's answer, so it counts as between an earlier source and this use.
    seams = lambda lo, hi: sum(1 for c in compactions if lo < c["at_ms"] <= hi)
    need = h.get("acted_after_seams", 0)
    probe = next((s["id"] for s in b["steps"] if s["role"] == "probe"), None)
    completed = probe in recs and all(recs[s]["subtype"] == "success" for s in order) and not start.get("stopped")
    plants = [s["id"] for s in b["steps"] if s["role"] in ("plant", "worker")]
    last_plant = max((sent[p] for p in plants if p in sent), default=0)
    forced = seams(last_plant, ended.get(probe, 0)) if probe in ended else 0
    entries = v2score.probe_entries(recs[probe]["answer"]) if probe in recs else {}
    by_item, detail = {}, {}
    for item in b["items"]:
        if item.get("brief_text"):
            by_item[item["id"]] = "not_applicable"
            detail[item["id"]] = {"what": item["what"], "why": "planted by Brigadier's daemon only"}
            continue
        d = {"what": item["what"], "recall": v2score.recall(item, entries.get(item.get("probe"))),
             "probe_entry": (entries.get(item.get("probe")) or "")[:400], "acted": v2score.acted(item, checks)}
        src = sent.get(item["source"], 0)
        d["seams_before_probe"] = seams(src, ended.get(probe, 0))
        if item.get("acted") and item["acted"] in ended:
            d["seams_before_act"] = seams(src, ended[item["acted"]])
        tested = d["seams_before_probe"] >= 1 and (not item.get("acted") or d.get("seams_before_act", 0) >= need)
        acted_ok = d["acted"] is None or d["acted"]["ok"]
        if not tested:
            status = "not_tested"
        elif d["recall"] == "pass" and acted_ok:
            status = "pass"
        elif d["recall"] == "unknown" and acted_ok:
            status = "unknown"
        else:
            status = "fail"
        by_item[item["id"]], detail[item["id"]] = status, d
    valid = forced >= v2score.SEAMS_WANTED
    asks = {}
    for step, ask_id in (h.get("asks") or {}).items():
        if step not in recs:
            continue
        ask = v3lib.ask_files([ask_id])[0]
        # The context of the ask's first model call (after a compaction at its start, if one came).
        d = {"ask": ask_id, "probe": roles[step], "kind": ask["kind"], "context_at_send": recs[step].get("first_context"),
             "seconds": round((ended[step] - sent[step]) / 1000, 1), "model_calls": {"total": recs[step]["model_calls"]}}
        if ask["rule"].get("rule") == "answer":
            ok, why, info = v3score.score_answer(ask["rule"], recs[step]["answer"])
            d.update(passed=ok, why=why, cited=info["cited"][:6])
        else:
            c = checks.get(step) or {}
            d.update(passed=c.get("rc") == 0, why="hidden check passed" if c.get("rc") == 0 else (c.get("output") or "")[-300:])
        asks[step] = d
    quality = a10score.quality_of(asks)
    # The session's total: the largest cumulative figure a step reported (its last answered step's).
    usage = max((recs[s]["usage_cumulative"] for s in order), key=lambda u: u["raw"],
                default={"input": 0, "cache_read": 0, "cache_write": 0, "output": 0, "raw": 0})
    t0, end = start["t0_ms"], max(start["ended_ms"] or [start["t0_ms"]])
    once = a10score.transcripts_once(run, t0, end)
    working_s = sum(ended[s] - sent[s] for s in order) / 1000
    return {
        "arm": "plain", "label": start["label"], "n": start["n"], "battery_name": start["battery"],
        "account": start["account"], "claude_version": start["claude_version"], "battery_sha256": start["battery_sha256"],
        "started_ms": start["started_ms"], "completed": completed, "valid": valid,
        "battery": {"probe": probe, "seams": {"kind": "compaction", "forced": forced, "wanted": v2score.SEAMS_WANTED},
                    "by_item": by_item, "unknown_instead_of_guess": a10score.losses(by_item, detail)},
        "quality": quality, "asks": asks, "edits": {k: {"rc": v.get("rc"), "result": v.get("result")} for k, v in checks.items()},
        "edit_answers": a10score.edit_answers({k: v for k, v in checks.items() if k in recs}, {s: recs[s]["answer"] for s in recs}),
        "compaction_count": len(compactions), "compactions": compactions,
        "contexts": [[s, recs[s]["context"]] for s in order], "peak_context": max((recs[s]["context"] or 0 for s in order), default=0),
        "usage": {"session": usage, "transcripts_raw_once": once[0], "transcripts_messages": once[1],
                  "hours": round((end - t0) / 3_600_000, 3), "working_s": round(working_s, 1),
                  "raw_per_hour": round(usage["raw"] / ((end - t0) / 3_600_000)) if end > t0 else None},
        "item_details": detail,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run"); r.add_argument("--root", required=True); r.add_argument("--battery", default="a10long")
    r.add_argument("--n", type=int, default=1); r.add_argument("--tag"); r.add_argument("--only")
    r.add_argument("--expect-account"); r.add_argument("--expect-claude"); r.add_argument("--smoke-compact-pct", type=int)
    s = sub.add_parser("score"); s.add_argument("run")
    a = ap.parse_args()
    if a.cmd == "run":
        cmd_run(a)
    else:
        print(json.dumps(score_run(a.run), indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
