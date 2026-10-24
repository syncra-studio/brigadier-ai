#!/usr/bin/env python3
"""Run and summarize the existing battery. Numbers become reports and issues, never holds."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / "tools/bench"))
from economics import combine, historical, mean_usd, ratio, usd
from battery import NOTE, role_table
from scoring import recall_rows
FIELDS = ("input", "cached_input", "cache_write", "output", "cost_usd")
ARMS = ("seam", "plain", "zed", "summary")
PROFILES = (("recall", "short", 9, 1200), ("recall", "long", 90, 3500),
            ("decisions", "long", 90, 3500), ("updates", "short", 9, 1200),
            ("updates", "long", 90, 3500))


def parse_arms(value):
    arms = tuple(dict.fromkeys(arm.strip() for arm in value.split(",")))
    if not arms or any(arm not in ARMS for arm in arms):
        raise argparse.ArgumentTypeError("arms must be a comma-separated list of seam, plain, zed, summary")
    return arms


def decision_checks(result):
    """Scenario checks, without ledger markers for CLI arms."""
    if not result.get("rows"):
        items = result.get("items")
        if not isinstance(items, dict):
            return {}
        checks = {f"recall.{key}": row.get("verdict") == "pass" for key, row in recall_rows(items).items()}
        checks.update({f"apply.{key}": row["action"] for key, row in items.items() if row.get("action") is not None})
        if result.get("block") == "updates":
            checks.update({f"newest.{key}": row["newest"] for key, row in items.items() if row.get("newest") is not None})
            checks.update({f"no_stale.{key}": not row["stale"] for key, row in items.items() if row.get("stale") is not None})
        return checks
    t1, t3 = result.get("t1", {}), result.get("t3", {})
    brief = list(t3.get("items", {}).values())
    checks = {
        "T1": all(t1.get(k) for k in ("jsdoc", "named", "spdx", "padding", "dates")) if t1 else None,
        "T2": result.get("t2_scored", {}).get("pass"),
        "T3": (brief.count("contradicted") == 0 and brief.count("missing") <= 1 and
               not t3["unmarked_additions"]) if brief and "unmarked_additions" in t3 else None,
        "T4": all(result["t4_scored"].get(k) for k in ("open_question", "open_promise"))
              if result.get("t4_scored") else None,
        "T5": result["rows"].get("own_decision", {}).get("unprompted"),
        "T6": sum(bool(r.get("recalled")) for r in result["rows"].values()) / len(result["rows"]),
    }
    for key, row in result["rows"].items():
        checks[f"{key}.applied"] = row.get("unprompted")
        checks[f"{key}.recalled"] = row.get("recalled")
    return checks


def metrics(result):
    usage = result.get("usage", [])
    totals = {key: sum(row.get(key) or 0 for row in usage)
              if any(row.get(key) is not None for row in usage) else None for key in FIELDS}
    if not usage or any(row.get("cost_usd") is None for row in usage):
        totals["cost_usd"] = None
    rows = result.get("rows", {})
    applicable = [row for row in rows.values() if row.get("unprompted") is not None]
    judge = result.get("t3", {})
    items = judge.get("items", {})
    opens = result.get("t4_scored", {})
    # A true cold return is a later request with no cache hit, not the initial cold start.
    # The plain arm writes `kind`; the seam arm's rows are the daemon's own, with `step`.
    cold = [row for row in usage[1:] if (row.get("kind") or row.get("step")) == "thread"
            and row.get("cached_input") == 0]
    roles = result.get("roles") or {"parent": historical(
        {"input": totals["input"], "cache_write": totals["cache_write"],
         "cache_read": totals["cached_input"], "output": totals["output"]}, [])}
    native_usd = combine(roles.values())["list_price_usd"]
    # Keep the old UI field as an alias, never a second legacy dollar source.
    totals["cost_usd"] = native_usd
    return {"usage": totals, "roles": roles, "list_price_usd": native_usd,
            "run_kind": "fault-recovery" if any(not s.get("skipped") for s in result.get("seams", [])) else "continuity",
            "native_compaction_in_seam": result.get("arm") == "seam" and (result.get("compactions") or 0) > 0,
            "compactions": result.get("compactions"), "zed_compactions": result.get("zed_compactions"),
            "checks": decision_checks(result),
            "summary_compactions": result.get("summary_compactions"),
            "update_scores": result.get("update_scores"),
            "budget_scores": result.get("budget_scores"),
            "first_successor_write": result.get("first_successor_write"),
            "extra_bootstrap_estimate": result.get("extra_bootstrap_estimate"),
            "seams": result.get("seams_before_checks", result.get("size_seams_before_faults", 0)),
            "contexts_before_checks": result.get("seams_before_checks"),
            "recall": (sum(row.get("verdict") == "pass" for row in recall_rows(result["items"]).values()) / len(recall_rows(result["items"])))
                       if isinstance(result.get("items"), dict) and recall_rows(result["items"])
                       else sum(bool(row.get("recalled")) for row in rows.values()) / len(rows) if rows else None,
            "decision_recall": sum(bool(row.get("recalled")) for row in rows.values()) / len(rows) if rows else None,
            "applied": sum(bool(row["unprompted"]) for row in applicable) if applicable else None,
            "applicable": len(applicable), "planted": len(rows),
            "contradictions": list(items.values()).count("contradicted") if items else None,
            "unmarked_additions": len(judge["unmarked_additions"]) if "unmarked_additions" in judge else None,
            "open_items": sum(bool(opens.get(k)) for k in ("open_question", "open_promise")) if opens else None,
            "cold_return_cost_usd": None,  # Native per-call cold-return attribution is not yet available.
            "cold_return_observations": len(cold),
            "task_passed": None if result.get("block") == "updates" else (
                all(result.get("t1", {}).get(k) for k in ("jsdoc", "named", "spdx", "padding", "dates"))
                if rows else all(row.get("action") is not False for row in result.get("items", {}).values())
                if isinstance(result.get("items"), dict) and result["items"] else None)}


def provider_report(provider, output, daemon, live, selected_arm=None, zed_long=False):
    from evidence import purge
    purge()
    output.mkdir(parents=True, exist_ok=True)
    report = {"provider": provider, "status": "complete", "reason": None, "arms": []}
    if not live:
        report.update(status="skipped", reason="needs the user: secrets")
    else:
        # Same scripts in every arm. Short stays bounded; long crosses 400k of inert input.
        profiles = list(PROFILES)
        if provider == "claude" and selected_arm in (None, "seam"):
            profiles += [("p2", "short", 18, 0), ("p3", "short", 16, 0)]
        for block, profile, fillers, ballast in profiles:
            for arm in (["seam"] if block in ("p2", "p3") else [selected_arm] if selected_arm else ARMS):
                if arm == "zed" and not zed_long and block != "updates" and (block, profile) != ("recall", "short"):
                    report["arms"].append({"block": block, "profile": profile, "arm": arm,
                                            "status": "skipped",
                                            "reason": "zed: short only tonight (long on dispatch)"})
                    continue
                name = f"{provider}-{block}-{profile}-{arm}"
                with tempfile.TemporaryDirectory(prefix="continuity-nightly-evidence-") as private:
                    evidence = Path(private) / "evidence.json"
                    command = [sys.executable, str(ROOT / "tools/continuity-battery" /
                               ("decisions.py" if block == "decisions" else "updates.py" if block == "updates" else "battery.py")),
                               "--daemon", daemon, "--provider", provider, "--arm", arm,
                               "--max-fillers", str(fillers),
                               "--min-fillers", str(fillers), "--ballast", str(ballast),
                               "--timeout", "6000", "--turn-timeout", "240",
                               "--scratch-prefix", f"continuity-nightly-{name}",
                               "--report", str(output / f"{name}.md"), "--evidence", str(evidence)]
                    if block in ("p2", "p3"):
                        command = [sys.executable, str(ROOT / "tools/continuity-battery/budgets.py"),
                                   "--variant", block, "--daemon", daemon, "--max-fillers", str(fillers),
                                   "--scratch-prefix", f"continuity-nightly-d49-{block}",
                                   "--report", str(output / f"{name}.md"), "--evidence", str(evidence)]
                    row = {"block": block, "profile": profile, "arm": arm}
                    try:
                        with subprocess.Popen(command, cwd=ROOT) as proc:
                            try:
                                proc.wait(timeout=6300)
                            except subprocess.TimeoutExpired:
                                proc.terminate()
                                try:
                                    proc.wait(timeout=60)
                                except subprocess.TimeoutExpired:
                                    proc.kill()
                                    proc.wait()
                        result = json.loads(evidence.read_text())
                        row.update(status="skipped" if result.get("status") == "skipped" else
                                   "complete" if proc.returncode == 0 and not any(e.startswith(("capture:", "score:", "capture/score:")) for e in result.get("errors", [])) else "incomplete",
                                   reason=result.get("reason"),
                                   errors=result.get("errors", []), model=result.get("model"), **metrics(result))
                        if row["status"] == "complete" and block in ("p2", "p3") and row.get("budget_scores") is None:
                            row["status"] = "incomplete"
                            row["errors"].append("budget variant not scored")
                        if row["status"] == "complete" and block == "decisions" and any(row.get(k) is None for k in ("applied", "contradictions", "unmarked_additions", "open_items")):
                            row["status"] = "incomplete"
                            row["errors"].append("decision rubric not completely scored")
                        if row["status"] == "complete" and block == "updates" and row.get("update_scores") is None:
                            row["status"] = "incomplete"
                            row["errors"].append("updates rubric not completely scored")
                    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
                        row.update(status="incomplete", errors=[str(error)])
                    report["arms"].append(row)
        if any(row["status"] != "complete" for row in report["arms"]):
            report["status"] = "incomplete"
    (output / f"{provider}-{selected_arm or 'both'}.json").write_text(json.dumps(report, indent=2))


def regressions(summary, previous=None):
    found = []
    for provider in summary["providers"]:
        for row in provider["arms"]:
            if row["status"] != "complete" or row["block"] in ("p2", "p3"):
                continue
            label = f"{provider['provider']} {row['block']} {row['profile']} {row['arm']}"
            if row["arm"] == "seam" and (row.get("compactions") or 0) > 0:
                found.append(label + ": native compaction observed")
            if row.get("recall") is not None and row["recall"] < .9:
                found.append(label + ": recall below 90%")
            if row.get("task_passed") is False:
                found.append(label + ": task check failed")
            if row["block"] == "decisions":
                contexts = row.get("contexts_before_checks")
                if contexts is None:
                    contexts = row.get("seams", 0) if row["arm"] == "seam" else row.get("compactions", 0)
                if (contexts or 0) >= 3 and ((row.get("applied") or 0) < 9 or row.get("contradictions") != 0
                        or row.get("unmarked_additions") != 0 or row.get("open_items") != 2):
                    found.append(label + ": decisions-survive bar missed")
            plain = next((p for p in provider["arms"] if p["arm"] == "plain" and p["block"] == row["block"]
                          and p["profile"] == row["profile"] and p["status"] == "complete"), {})
            cost, baseline = row.get("list_price_usd"), plain.get("list_price_usd")
            same_run_kind = row.get("run_kind", "continuity") == plain.get("run_kind", "continuity")
            if row["arm"] == "seam" and same_run_kind and cost is not None and baseline:
                row["price_weighted_ratio"] = cost / baseline
                if cost / baseline > (1.01 if row["profile"] == "short" else .95):
                    found.append(label + ": price-weighted token bar missed")
    for session in summary.get("weekly", {}).get("sessions", []):
        if session.get("provider") == "claude" and (session.get("compactions") or 0) > 0:
            found.append(f"Weekly real lineage {session['session_id']}: native Claude compaction observed")
    # First run is the baseline. Compare only matching, completed observations thereafter.
    if previous:
        old = {(p["provider"], r["block"], r["profile"], r["arm"]): r for p in previous.get("providers", [])
               for r in p.get("arms", []) if r.get("status") == "complete"}
        for p in summary["providers"]:
            for row in p["arms"]:
                prior = old.get((p["provider"], row["block"], row["profile"], row["arm"]), {})
                for key in ("recall", "applied"):
                    if row["block"] not in ("p2", "p3") and row.get("status") == "complete" and row.get(key) is not None and prior.get(key) is not None and row[key] < prior[key]:
                        found.append(f"{p['provider']} {row['block']} {row['profile']} {row['arm']}: {key} below baseline")
    return found


def prepare(summary):
    aggregates = []
    for provider in summary["providers"]:
        for row in provider["arms"]:
            if row.get("status") == "complete" and "list_price_usd" not in row:
                usage = row.get("usage", {})
                row["roles"] = row.get("roles") or {"parent": historical({
                    "input": usage.get("input"), "cache_write": usage.get("cache_write"),
                    "cache_read": usage.get("cached_input"), "output": usage.get("output")}, [])}
                row["list_price_usd"] = combine(row["roles"].values())["list_price_usd"]
        keys = sorted({(r["block"], r["profile"], r["arm"], r.get("run_kind", "continuity")) for r in provider["arms"]
                       if r["status"] == "complete"})
        for block, profile, arm, run_kind in keys:
            rows = [r for r in provider["arms"] if (r["block"], r["profile"], r["arm"], r["status"], r.get("run_kind", "continuity")) ==
                    (block, profile, arm, "complete", run_kind)]
            plain = [r for r in provider["arms"] if (r["block"], r["profile"], r["arm"], r["status"], r.get("run_kind", "continuity")) ==
                     (block, profile, "plain", "complete", run_kind)]
            amount, baseline = mean_usd(rows), mean_usd(plain)
            aggregates.append({"provider": provider["provider"], "block": block, "profile": profile,
                               "arm": arm, "run_kind": run_kind, "mean_list_price_usd": amount,
                               "list_price_ratio": amount / baseline if amount is not None and baseline else None})
    summary["aggregate"] = aggregates
    comparisons = []
    def mean(values):
        return sum(values) / len(values) if values and all(v is not None for v in values) else None
    for provider in summary["providers"]:
        for block, profile in sorted({(r["block"], r["profile"]) for r in provider["arms"]}):
            pair = {arm: [r for r in provider["arms"] if r["arm"] == arm and r["block"] == block
                          and r["profile"] == profile and r["status"] == "complete"
                          and r.get("run_kind", "continuity") == "continuity"] for arm in ("seam", "zed")}
            check_keys = sorted({key for rows in pair.values() for row in rows for key in row.get("checks", {})})
            for key in ["recall", "task_passed", *check_keys, "list_price_usd"]:
                def values(arm):
                    return [r.get(key) if key in ("recall", "task_passed", "list_price_usd") else
                            r.get("checks", {}).get(key) for r in pair[arm]]
                left, right = mean(values("seam")), mean(values("zed"))
                dollars = key == "list_price_usd"
                qualified = bool(pair["seam"] and pair["zed"]) and (block != "decisions" or all(
                    r.get("contexts_before_checks") is not None and r["contexts_before_checks"] >= 3
                    for rows in pair.values() for r in rows))
                comparisons.append({"provider": provider["provider"], "block": block, "profile": profile,
                                    "check": key, "seam": left, "zed": right,
                                    "seam_meets_zed": None if left is None or right is None else
                                                      left <= right if dollars else left >= right,
                                    "qualified": qualified,
                                    "dollar_ratio": left / right if dollars and left is not None and right else None})
    summary["seam_vs_zed"] = comparisons
    return summary


def markdown(summary):
    prepare(summary)
    lines = ["# Continuity nightly", "", "Report only. Regressions open issues; no number gates work.",
             "", f"Run: {summary['run_url']}", f"Source: `{summary['source']}`", ""]
    for provider in summary["providers"]:
        lines += [f"## {provider['provider']}", "", provider["status"] + (": " + provider["reason"] if provider.get("reason") else ""), "",
                  "| Script | Size | Arm | Native compactions | Zed compactions | Summary handoffs | Recall | Decisions applied | Brief contradictions / additions | Open items | Task | Native list-price USD |",
                  "|---|---|---|---:|---:|---:|---:|---:|---|---:|---|---:|"]
        for row in provider["arms"]:
            get = lambda k: "unavailable" if row.get(k) is None else str(row[k])
            lines.append(f"| {row['block']} | {row['profile']} | {row['arm']} ({row['status']}) | {get('compactions')} | {get('zed_compactions')} | {get('summary_compactions')} | {get('recall')} | {get('applied')} / {row.get('planted', 0)} | {get('contradictions')} / {get('unmarked_additions')} | {get('open_items')} | {get('task_passed')} | {usd(row.get('list_price_usd'))} |")
        for row in provider["arms"]:
            if row.get("update_scores") is not None:
                scores = row["update_scores"]
                lines += ["", f"Updates/{row['profile']} {row['arm']}: recall {scores['recall']}; "
                          f"newest decision wins {scores['newest_decision_wins']}; "
                          f"stale-decision mistakes {scores['stale_decision_mistakes']}/{scores['decision_count']}; "
                          f"open promises raised {scores['open_promises_raised']}/{scores['open_promise_count']}.", ""]
            if row.get("budget_scores") is not None:
                lines += ["", f"D49 {row['block']}/{row['profile']}: `{json.dumps(row['budget_scores'], sort_keys=True)}`; "
                          f"first successor input/write `{json.dumps(row.get('first_successor_write'))}`; "
                          f"extra bootstrap estimate `{json.dumps(row.get('extra_bootstrap_estimate'))}`.", ""]
            if row.get("native_compaction_in_seam"):
                lines += ["", "WARNING: native compaction occurred inside the seam arm; transition comparison is confounded."]
            if row.get("run_kind") == "fault-recovery":
                lines += ["", "Fault-recovery run: excluded from paired continuity comparisons."]
            if row.get("reason"):
                lines += ["", f"{row['block']}/{row['profile']} {row['arm']}: " + row["reason"], ""]
            if row.get("errors"):
                lines += ["", f"{row['block']}/{row['profile']} {row['arm']} run problems: " +
                          "; ".join(row["errors"]), ""]
        lines += ["", "Price-weighted ratios: " + ", ".join(f"{r['block']}/{r['profile']} {r.get('price_weighted_ratio', 'unavailable')}" for r in provider["arms"] if r["arm"] == "seam"), ""]
    lines += ["## List-price aggregates", "", NOTE, "",
              "| Provider | Script | Size | Arm | Mean list-price USD | USD vs plain |",
              "|---|---|---|---|---:|---:|"]
    for row in summary["aggregate"]:
        lines.append(f"| {row['provider']} | {row['block']} | {row['profile']} | {row['arm']} ({row['run_kind']}) | "
                     f"{usd(row['mean_list_price_usd'])} | {ratio(row['list_price_ratio'])} |")
    lines += ["", "## Seam vs Zed", "",
              "Means of completed matching observations; higher survival is better, lower dollars is better. "
              "Decision comparisons need three observed transitions in both arms.", "",
              "| Provider | Script | Size | Check | Seam | Zed | Seam meets Zed | Qualified | Dollar ratio |",
              "|---|---|---|---|---:|---:|---|---|---:|"]
    for row in summary["seam_vs_zed"]:
        value = lambda key: "unavailable" if row[key] is None else str(row[key])
        lines.append(f"| {row['provider']} | {row['block']} | {row['profile']} | {row['check']} | "
                     f"{value('seam')} | {value('zed')} | {value('seam_meets_zed')} | {row['qualified']} | {ratio(row['dollar_ratio'])} |")
    for provider in summary["providers"]:
        for row in provider["arms"]:
            if row["status"] == "complete":
                lines += ["", f"### {provider['provider']} {row['block']}/{row['profile']} {row['arm']}", ""]
                lines += role_table(row.get("roles", {}))
    lines += ["", "Decisions-survive scores require three observed seams or native compactions; fewer contexts are an unqualified observation. Explicit Zed compactions count as transitions.",
              "Cold-return cost includes only later uncached requests; no observed return or missing price is unavailable. It never triggers an issue.",
              "Missing reported prices are unavailable, never zero. Short and long use the same tasks and filler schedule in all arms.", "", "## Weekly real lineages", "", json.dumps(summary.get("weekly", {})), "", "## Regressions", ""]
    lines += summary["regressions"] or ["None measured (skipped and incomplete runs are not passes)."]
    return "\n".join(lines) + "\n"


def collect(inputs, output, previous=None, selected_arms=ARMS):
    output.mkdir(parents=True, exist_ok=True)
    providers = []
    for provider in ("claude", "codex"):
        files = [p for p in inputs.rglob(f"{provider}-*.json") if p.name in
                 {f"{provider}-{arm}.json" for arm in (*selected_arms, "both")}]
        reports = [json.loads(file.read_text()) for file in files]
        rows = [row for report in reports for row in report["arms"] if row["arm"] in selected_arms]
        observed = {row["arm"] for row in rows}
        complete = all(arm in observed for arm in selected_arms) and all(r["status"] == "complete" for r in reports)
        covered = {arm for arm in selected_arms if any(
            file.name in (f"{provider}-{arm}.json", f"{provider}-both.json") for file in files)}
        rows += [{"block": block, "profile": profile, "arm": arm, "status": "not_run",
                  "reason": "not run on this dispatch"}
                 for arm in ARMS if arm not in selected_arms
                 for block, profile, _, _ in PROFILES]
        providers.append({"provider": provider,
                          "status": "skipped" if covered == set(selected_arms) and all(r["status"] == "skipped" for r in reports)
                                    else "complete" if complete else "incomplete",
                          "reason": "; ".join(sorted({r["reason"] for r in reports if r.get("reason")})) or (None if complete else "runner did not complete selected arms"),
                          "arms": rows})
    weekly = next(inputs.rglob("weekly.json"), None)
    summary = {"schema_version": 1, "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
               "source": os.environ.get("GITHUB_SHA", "local"),
               "run_url": f"https://github.com/{os.environ.get('GITHUB_REPOSITORY', 'syncra-studio/brigadier-ai')}/actions/runs/{os.environ.get('GITHUB_RUN_ID', 'local')}",
               "baseline": previous is None, "providers": providers,
               "weekly": json.loads(weekly.read_text()) if weekly else {"status": "skipped", "reason": "weekly sample not due"}}
    summary["regressions"] = regressions(summary, previous)
    prepare(summary)
    (output / "summary.json").write_text(json.dumps(summary, indent=2))
    (output / "report.json").write_text(json.dumps(summary, indent=2))
    (output / "report.md").write_text(markdown(summary))
    for path in inputs.rglob("*.md"):
        shutil.copyfile(path, output / path.name)
    return summary


def issue(summary):
    if not summary["regressions"]:
        return
    repo = "syncra-studio/brigadier-ai"
    title = "Continuity battery regression"
    found = subprocess.run(["gh", "issue", "list", "-R", repo, "--state", "open", "--search", title + " in:title", "--json", "number,title"], capture_output=True, text=True, check=True)
    existing = next((i for i in json.loads(found.stdout) if i["title"] == title), None)
    # Exact body through a file; no shell expansion of provider output.
    with tempfile.TemporaryDirectory(prefix="continuity-issue-") as scratch:
        body = Path(scratch) / "body.md"
        body.write_text(markdown(summary))
        command = (["gh", "issue", "comment", str(existing["number"])] if existing else ["gh", "issue", "create", "--title", title])
        subprocess.run(command + ["-R", repo, "--body-file", str(body)], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["provider", "collect"])
    parser.add_argument("--provider", choices=["claude", "codex"])
    parser.add_argument("--arm", choices=ARMS)
    parser.add_argument("--arms", type=parse_arms, default=ARMS, help="comma-separated arms expected in the collected report")
    parser.add_argument("--daemon", default="target/debug/brigadierd")
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--zed-long", action="store_true", help="run the zed arm's long recall + decisions profiles")
    parser.add_argument("--input", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--previous", type=Path)
    parser.add_argument("--issue", action="store_true")
    args = parser.parse_args()
    if args.mode == "provider":
        provider_report(args.provider, args.output, str(Path(args.daemon).resolve()), args.live, args.arm, args.zed_long)
    else:
        summary = collect(args.input, args.output, json.loads(args.previous.read_text()) if args.previous and args.previous.exists() else None,
                          selected_arms=args.arms)
        if args.issue:
            issue(summary)
