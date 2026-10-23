#!/usr/bin/env python3
"""Collect bench artifacts and update a report-only regression issue."""
import argparse
from collections import defaultdict
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import write

SETS = {"claude": {"A", "CC", "W"}, "codex": {"B", "CX", "W"}}


def grouped(report):
    groups = defaultdict(list)
    for row in report["runs"]:
        if row["status"] == "complete":
            groups[(row["provider"], row["arm"], row["task"])].append(row)
    return groups


def regressions(current, previous, threshold=.2):
    if not previous or current["fixture_version"] != previous.get("fixture_version") or current["effort"] != previous.get("effort"):
        return []
    now, old = grouped(current), grouped(previous)
    found = []
    for key, rows in now.items():
        prior = old.get(key)
        if not prior:
            continue
        provider, arm, task = key
        label = f"{provider} {arm} {task}"
        # Changing CLI model is a new baseline, not a product regression.
        models = lambda rs: {tuple(r.get("models", [])) for r in rs}
        if models(rows) != models(prior):
            continue
        passed, total = sum(r["grade"]["passed"] for r in rows), len(rows)
        old_passed = sum(r["grade"]["passed"] for r in prior)
        if old_passed == len(prior) and passed < total:
            found.append(f"{label}: pass {old_passed}/{len(prior)} → {passed}/{total}")
        if arm in ("A", "B"):
            continue
        plain_key = (provider, "A" if provider == "claude" else "B", task)
        baseline, old_baseline = now.get(plain_key), old.get(plain_key)
        if not baseline or not old_baseline:
            continue
        if not (models(rows) == models(baseline) == models(old_baseline)):
            continue
        for metric in ("raw_tokens", "wall_s"):
            mean = lambda rs: sum(r[metric] for r in rs) / len(rs)
            if mean(baseline) <= 0 or mean(old_baseline) <= 0 or mean(prior) <= 0:
                continue
            ratio, was = mean(rows) / mean(baseline), mean(prior) / mean(old_baseline)
            if ratio > 1 + threshold and ratio > was * (1 + threshold):
                found.append(f"{label}: {metric} {mean(prior):.2f} → {mean(rows):.2f}; plain {mean(old_baseline):.2f} → {mean(baseline):.2f}; ratio {was:.2f}× → {ratio:.2f}×")
    return found


def collect(inputs, output, previous):
    files = sorted(inputs.glob("bench-*/report.json"))
    if not files:
        raise RuntimeError("No arm-set reports were uploaded")
    reports = [json.loads(path.read_text()) for path in files]
    first = reports[0]
    result = {key: first[key] for key in ("schema_version", "fixture_version", "source", "created_at", "effort", "requested_models", "standing_instruction_bytes")}
    result["runs"] = [row for report in reports for row in report["runs"]]
    for provider, expected in SETS.items():
        actual = {row["arm"] for row in result["runs"] if row["provider"] == provider}
        if actual != expected:
            raise RuntimeError(f"Missing arm-set report for {provider}: expected {expected}, got {actual}")
    if any(report["source"] != first["source"] or report["fixture_version"] != first["fixture_version"] for report in reports):
        raise RuntimeError("Mismatched source or fixtures in arm-set artifacts")
    result["regressions"] = regressions(result, previous)
    result["run_url"] = f"https://github.com/syncra-studio/brigadier-ai/actions/runs/{os.environ.get('GITHUB_RUN_ID', 'local')}"
    write(result, output)
    with (output / "report.md").open("a") as out:
        out.write("\n## Regressions\n\n" + "\n".join(result["regressions"] or ["None measured. First complete observation is the baseline; skipped runs are not passes."]) + "\n")
    if any(row["status"] == "incomplete" for row in result["runs"]):
        raise RuntimeError("An arm-set harness broke; see report.json")
    return result


def issue(report):
    if not report["regressions"]:
        return
    repo, title = "syncra-studio/brigadier-ai", "Bench regression"
    issues = json.loads(subprocess.check_output(["gh", "issue", "list", "-R", repo, "--state", "open", "--search", title + " in:title", "--json", "number,title"], text=True))
    existing = next((item for item in issues if item["title"] == title), None)
    # A single issue, symptoms and measured numbers only; no generated diagnosis.
    with tempfile.TemporaryDirectory(prefix=os.environ.get("BENCH_SCRATCH_PREFIX", "brigadier-bench-" + os.environ.get("GITHUB_RUN_ID", "local") + "-") + "issue-") as tmp:
        body = Path(tmp) / "body.md"
        body.write_text(report["run_url"] + "\n\n" + "\n".join("- " + r for r in report["regressions"]) + "\n")
        command = (["gh", "issue", "edit", str(existing["number"])] if existing else ["gh", "issue", "create", "--title", title])
        subprocess.run(command + ["-R", repo, "--body-file", str(body)], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--previous", type=Path)
    parser.add_argument("--issue", type=Path, help="Only file/update the issue for this existing report")
    args = parser.parse_args()
    if args.issue:
        issue(json.loads(args.issue.read_text()))
    else:
        prior = json.loads(args.previous.read_text()) if args.previous and args.previous.is_file() else None
        collect(args.input, args.output, prior)
