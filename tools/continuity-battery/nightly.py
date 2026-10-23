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
FIELDS = ("input", "cached_input", "cache_write", "output", "cost_usd")


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
    return {"usage": totals,
            "compactions": result.get("compactions"),
            "seams": result.get("seams_before_checks", result.get("size_seams_before_faults", 0)),
            "contexts_before_checks": result.get("seams_before_checks"),
            "recall": (sum(row.get("verdict") == "pass" for row in result["items"].values()) / len(result["items"]))
                       if isinstance(result.get("items"), dict) and result["items"]
                       else sum(bool(row.get("recalled")) for row in rows.values()) / len(rows) if rows else None,
            "decision_recall": sum(bool(row.get("recalled")) for row in rows.values()) / len(rows) if rows else None,
            "applied": sum(bool(row["unprompted"]) for row in applicable) if applicable else None,
            "applicable": len(applicable), "planted": len(rows),
            "contradictions": list(items.values()).count("contradicted") if items else None,
            "unmarked_additions": len(judge["unmarked_additions"]) if "unmarked_additions" in judge else None,
            "open_items": sum(bool(opens.get(k)) for k in ("open_question", "open_promise")) if opens else None,
            "cold_return_cost_usd": sum(row["cost_usd"] for row in cold)
                                    if cold and all(row.get("cost_usd") is not None for row in cold) else None,
            "cold_return_observations": len(cold),
            "task_passed": all(result.get("t1", {}).get(k) for k in ("jsdoc", "named", "spdx", "padding", "dates"))
                           if rows else all(row.get("action") is not False for row in result.get("items", {}).values())
                           if isinstance(result.get("items"), dict) and result["items"] else None}


def provider_report(provider, output, daemon, live, selected_arm=None):
    output.mkdir(parents=True, exist_ok=True)
    report = {"provider": provider, "status": "complete", "reason": None, "arms": []}
    if not live:
        report.update(status="skipped", reason="needs the user: secrets")
    else:
        # Same scripts in both arms. Short stays bounded; long crosses 400k of inert input.
        for block, profile, fillers, ballast in (("recall", "short", 9, 1200),
                                                 ("recall", "long", 90, 3500),
                                                 ("decisions", "long", 90, 3500)):
            for arm in ([selected_arm] if selected_arm else ("seam", "plain")):
                name = f"{provider}-{block}-{profile}-{arm}"
                with tempfile.TemporaryDirectory(prefix="continuity-nightly-evidence-") as private:
                    evidence = Path(private) / "evidence.json"
                    command = [sys.executable, str(ROOT / "tools/continuity-battery" /
                               ("decisions.py" if block == "decisions" else "battery.py")),
                               "--daemon", daemon, "--provider", provider, "--arm", arm,
                               "--max-fillers", str(fillers),
                               "--min-fillers", str(fillers), "--ballast", str(ballast),
                               "--timeout", "6000", "--turn-timeout", "240",
                               "--scratch-prefix", f"continuity-nightly-{name}",
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
                        row.update(status="complete" if proc.returncode == 0 and not any(e.startswith(("capture:", "score:")) for e in result.get("errors", [])) else "incomplete",
                                   errors=result.get("errors", []), model=result.get("model"), **metrics(result))
                        if block == "decisions" and any(row.get(k) is None for k in ("applied", "contradictions", "unmarked_additions", "open_items")):
                            row["status"] = "incomplete"
                            row["errors"].append("decision rubric not completely scored")
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
            if row["status"] != "complete":
                continue
            label = f"{provider['provider']} {row['block']} {row['profile']} {row['arm']}"
            if provider["provider"] == "claude" and row["arm"] == "seam" and (row.get("compactions") or 0) > 0:
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
            cost, baseline = row.get("usage", {}).get("cost_usd"), plain.get("usage", {}).get("cost_usd")
            if row["arm"] == "seam" and cost is not None and baseline:
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
                    if row.get("status") == "complete" and row.get(key) is not None and prior.get(key) is not None and row[key] < prior[key]:
                        found.append(f"{p['provider']} {row['block']} {row['profile']} {row['arm']}: {key} below baseline")
    return found


def markdown(summary):
    lines = ["# Continuity nightly", "", "Report only. Regressions open issues; no number gates work.",
             "", f"Run: {summary['run_url']}", f"Source: `{summary['source']}`", ""]
    for provider in summary["providers"]:
        lines += [f"## {provider['provider']}", "", provider["status"] + (": " + provider["reason"] if provider.get("reason") else ""), "",
                  "| Script | Size | Arm | Compactions | Recall | Decisions applied | Brief contradictions / additions | Open items | Task | Cost USD | Cold return USD |",
                  "|---|---|---|---:|---:|---:|---|---:|---|---:|---:|"]
        for row in provider["arms"]:
            get = lambda k: "unavailable" if row.get(k) is None else str(row[k])
            lines.append(f"| {row['block']} | {row['profile']} | {row['arm']} ({row['status']}) | {get('compactions')} | {get('recall')} | {get('applied')} / {row.get('planted', 0)} | {get('contradictions')} / {get('unmarked_additions')} | {get('open_items')} | {get('task_passed')} | {row.get('usage', {}).get('cost_usd')} | {get('cold_return_cost_usd')} |")
        lines += ["", "Price-weighted ratios: " + ", ".join(f"{r['block']}/{r['profile']} {r.get('price_weighted_ratio', 'unavailable')}" for r in provider["arms"] if r["arm"] == "seam"), ""]
    lines += ["Decisions-survive scores require three observed seams or native compactions; fewer contexts are an unqualified observation.",
              "Cold-return cost includes only later uncached requests; no observed return or missing price is unavailable. It never triggers an issue.",
              "Missing reported prices are unavailable, never zero. Short and long use the same tasks and filler schedule in both arms.", "", "## Weekly real lineages", "", json.dumps(summary.get("weekly", {})), "", "## Regressions", ""]
    lines += summary["regressions"] or ["None measured (skipped and incomplete runs are not passes)."]
    return "\n".join(lines) + "\n"


def collect(inputs, output, previous=None):
    output.mkdir(parents=True, exist_ok=True)
    providers = []
    for provider in ("claude", "codex"):
        files = list(inputs.rglob(f"{provider}-*.json"))
        reports = [json.loads(file.read_text()) for file in files]
        providers.append({"provider": provider,
                          "status": "skipped" if reports and all(r["status"] == "skipped" for r in reports)
                                    else "complete" if len(reports) == 2 and all(r["status"] == "complete" for r in reports) else "incomplete",
                          "reason": "; ".join(sorted({r["reason"] for r in reports if r.get("reason")})) or (None if len(reports) == 2 else "runner did not produce both arms"),
                          "arms": [row for report in reports for row in report["arms"]]})
    weekly = next(inputs.rglob("weekly.json"), None)
    summary = {"schema_version": 1, "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
               "source": os.environ.get("GITHUB_SHA", "local"),
               "run_url": f"https://github.com/{os.environ.get('GITHUB_REPOSITORY', 'syncra-studio/brigadier-ai')}/actions/runs/{os.environ.get('GITHUB_RUN_ID', 'local')}",
               "baseline": previous is None, "providers": providers,
               "weekly": json.loads(weekly.read_text()) if weekly else {"status": "skipped", "reason": "weekly sample not due"}}
    summary["regressions"] = regressions(summary, previous)
    (output / "summary.json").write_text(json.dumps(summary, indent=2))
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
    parser.add_argument("--arm", choices=["seam", "plain"])
    parser.add_argument("--daemon", default="target/debug/brigadierd")
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--input", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--previous", type=Path)
    parser.add_argument("--issue", action="store_true")
    args = parser.parse_args()
    if args.mode == "provider":
        provider_report(args.provider, args.output, str(Path(args.daemon).resolve()), args.live, args.arm)
    else:
        summary = collect(args.input, args.output, json.loads(args.previous.read_text()) if args.previous and args.previous.exists() else None)
        if args.issue:
            issue(summary)
