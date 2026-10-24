#!/usr/bin/env python3
"""Offline dispatch planning and execution over the existing continuity runners."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import subprocess
import sys

import arms

ROOT = Path(__file__).resolve().parents[2]
PROFILES = ("work", "endurance", "recall-short", "recall-long", "updates-short", "updates-long",
            "decisions-short", "decisions-long")
PROVIDERS = ("claude", "codex")
# Planning estimates from PROPOSALS §3, not metered costs or subscription charges.
RATES = {("codex", "short"): (1.3, 1.3), ("codex", "long"): (17, 17),
         ("claude", "short"): (1, 3), ("claude", "long"): (20, 30),
         ("claude", "work"): (20, 30), ("codex", "work"): (20, 30),
         ("claude", "endurance"): (45, 60), ("codex", "endurance"): (20, 20)}


def selected_arms(value):
    values = list(dict.fromkeys(part.strip() for part in value.split(",")))
    if not values or any(value not in arms.ARMS for value in values):
        raise ValueError("unknown arm; use " + ", ".join(arms.ARMS))
    return values


def plan(preset="none", profile="work", requested_arms="plain@200k,native", providers="both", repetitions=1):
    if preset not in ("none", "stage0", "pilot"):
        raise ValueError("unknown preset")
    if profile not in PROFILES or providers not in (*PROVIDERS, "both"):
        raise ValueError("unknown profile or providers")
    if repetitions < 1:
        raise ValueError("repetitions must be positive")
    groups = []

    def add(block, provider, profile, values, count, rate=None):
        kind = profile.rsplit("-", 1)[-1]
        low, high = rate or RATES[provider, kind]
        groups.append({"block": block, "provider": provider, "profile": profile, "arms": values,
                       "repetitions": count, "estimate_per_arm_usd": [low, high]})

    if preset == "stage0":
        # One fixed short recall and updates pair per provider; no long/work/endurance run.
        for provider in PROVIDERS:
            for short in ("recall-short", "updates-short"):
                add("stage0", provider, short, ["seam", "plain@200k"], 1, (1.25, 2.5))
    elif preset == "pilot":
        add("p1", "codex", "updates-short", ["seam", "native-bare", "native", "plain"], 4)
        add("p1", "codex", "recall-long", ["native-bare", "native", "plain"], 2)
        add("p2", "claude", "work", ["plain@200k", "native-bare", "native-full", "seam"], 2)
        for provider in PROVIDERS:
            # §3's P3 table budgets this block at approximately $50 total (24 arm-runs).
            add("p3", provider, "recall-short", ["seam", "seam-repaired"], 6, (50 / 24, 50 / 24))
        add("endurance", "claude", "endurance", ["plain@200k", "native-full"], 1)
        add("endurance", "codex", "endurance", ["plain@200k", "native"], 1)
    else:
        values = selected_arms(requested_arms)
        for provider in PROVIDERS if providers == "both" else [providers]:
            add("custom", provider, profile, values, repetitions)
    cells = []
    for group in groups:
        for repetition in range(group["repetitions"]):
            for arm in group["arms"]:
                safe = arm.replace("@", "-").replace("+", "-")
                cells.append({"id": f"{group['block']}-{group['provider']}-{group['profile']}-{safe}-{repetition + 1}",
                              "block": group["block"], "provider": group["provider"], "profile": group["profile"],
                              "arm": arm, "seed": 1729 + repetition, "repetition": repetition + 1})
    if len(cells) > 256:
        raise ValueError("dispatch exceeds GitHub's 256-job matrix limit")
    estimate = [round(sum(g["estimate_per_arm_usd"][i] * len(g["arms"]) * g["repetitions"] for g in groups), 2)
                for i in (0, 1)]
    return {"preset": preset, "phase": "pilot", "shared_transition_tokens": 200000,
            "groups": groups, "arm_runs": len(cells), "estimated_list_usd": estimate,
            "with_judge_and_reruns_usd": [round(value * 1.3, 2) for value in estimate],
            "matrix": {"include": cells}}


def markdown(planned):
    low, high = planned["estimated_list_usd"]
    uplift = planned["with_judge_and_reruns_usd"]
    lines = ["# Continuity experiment plan", "", f"Preset: **{planned['preset']}**. **{planned['arm_runs']} arm-runs**.",
             f"Estimated list-price equivalent: **${low:.2f}–${high:.2f}** before model calls.",
             f"With assumed judge +10% and reruns +20%: **${uplift[0]:.2f}–${uplift[1]:.2f}**.", "",
             "These are PROPOSALS §3 planning estimates, not measured costs or subscription charges. "
             "P3 uses the table's approximately $50 block estimate. Stage 0 uses its $10–20 budget envelope. "
             "Custom cells reuse the nearest provider/profile estimate; Codex work and Claude legacy long are extrapolations.", "",
             "Presets fix all cells; custom profile/arms/providers/repetitions inputs are ignored for a preset. "
             "P5 output-cap arms are excluded (run2-D17). All comparisons are pilots.", "",
             "| Block | Provider | Profile | Arms | Repetitions | Arm-runs | List USD |",
             "|---|---|---|---|---:|---:|---:|"]
    for group in planned["groups"]:
        count = len(group["arms"]) * group["repetitions"]
        cost = [value * count for value in group["estimate_per_arm_usd"]]
        lines.append(f"| {group['block']} | {group['provider']} | {group['profile']} | {', '.join(group['arms'])} | "
                     f"{group['repetitions']} | {count} | ${cost[0]:.2f}–${cost[1]:.2f} |")
    lines += ["", "Each cell runs one independent lineage. Work/endurance cells with the same repetition share "
              "a seed and task order across arms. Legacy profiles keep their existing randomized plants; their "
              "repetitions are independent and do not produce paired non-inferiority intervals.", "",
              "Raw evidence is never uploaded. CI retains owner-only seven-day bundles in "
              "`$RUNNER_TEMP/continuity-private`; hosted runner teardown removes them sooner. "
              "Use a persistent private runner directory via `BRIGADIER_BATTERY_EVIDENCE_DIR` locally "
              "for post-run diagnosis."]
    return "\n".join(lines) + "\n"


def command(cell, daemon, output):
    provider, profile, arm = cell["provider"], cell["profile"], cell["arm"]
    if provider not in PROVIDERS or profile not in PROFILES or arm not in arms.ARMS:
        raise ValueError("invalid matrix cell")
    common = ["--daemon", str(daemon), "--provider", provider,
              "--timeout", "19800", "--turn-timeout", "240", "--scratch-prefix", "continuity-experiment-" + cell["id"]]
    if profile in ("work", "endurance"):
        return [sys.executable, "-B", str(Path(__file__).with_name("experiment.py")), *common,
                "--profile", profile, "--arms", arm, "--seed", str(cell["seed"]), "--repetitions", "1",
                "--phase", "pilot", "--report", str(output / "result.json")]
    block, size = profile.split("-")
    fillers, ballast = (9, 1200) if size == "short" else (90, 3500)
    script = "battery.py" if block == "recall" else block + ".py"
    return [sys.executable, "-B", str(Path(__file__).with_name(script)), *common,
            "--fair", "--tokens", "200000", "--arm", arm,
            "--min-fillers", str(fillers), "--max-fillers", str(fillers), "--ballast", str(ballast),
            "--report", str(output / "result.md")]


def execute(cell, daemon, output, live):
    output.mkdir(parents=True, exist_ok=True)
    cmd = command(cell, daemon, output)
    run = {"cell": cell, "status": "skipped", "reason": "needs the user: secrets"}
    (output / "run.json").write_text(json.dumps(run, indent=2) + "\n")
    if not live:
        return 0
    # No shell, no credential contents in the plan, and no additional private JSON export.
    completed = subprocess.run(cmd, cwd=ROOT, check=False)
    run.update(status="finished" if completed.returncode == 0 else "incomplete", reason=None,
               exit_code=completed.returncode)
    (output / "run.json").write_text(json.dumps(run, indent=2) + "\n")
    return completed.returncode


def collect(inputs, output):
    from paired import compare, cost_per_pass
    output.mkdir(parents=True, exist_ok=True)
    runs, grouped = [], {}
    for file in sorted(inputs.rglob("run.json")):
        run = json.loads(file.read_text())
        result = file.with_name("result.json")
        run["result"] = json.loads(result.read_text()) if result.is_file() else None
        runs.append(run)
        cell = run["cell"]
        if cell["profile"] in ("work", "endurance") and run["result"]:
            key = (cell["block"], cell["provider"], cell["profile"])
            grouped.setdefault(key, []).extend(run["result"].get("arms", []))
    planned_file = next(inputs.rglob("plan.json"), None)
    planned = json.loads(planned_file.read_text()) if planned_file else None
    observed = {run["cell"]["id"] for run in runs}
    if planned:
        runs += [{"cell": cell, "status": "not_run", "reason": "runner did not upload its report", "result": None}
                 for cell in planned["matrix"]["include"] if cell["id"] not in observed]
    comparisons = [{"block": key[0], "provider": key[1], "profile": key[2],
                    "paired": compare(rows, phase="pilot"),
                    "cost_per_fully_passing_lineage": {arm: cost_per_pass([row for row in rows if row["arm"] == arm])
                                                       for arm in sorted({row["arm"] for row in rows})}}
                   for key, rows in grouped.items()]
    summary = {"phase": "pilot", "plan": planned, "runs": runs, "comparisons": comparisons}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    counts = Counter(run["status"] for run in runs)
    (output / "report.md").write_text("# Continuity experiment results\n\n" +
        f"Arm-runs accounted for: {len(runs)}. Statuses: {dict(counts)}.\n\n" +
        "Per-cell public reports are in the continuity-experiment-cell artifacts. "
        "Paired work/endurance pilot intervals and cost per passing lineage are in summary.json. "
        "Legacy repetitions do not establish paired non-inferiority. Missing credentials and incomplete runs are not passes.\n")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("plan", "run", "collect"))
    parser.add_argument("--preset", default="none")
    parser.add_argument("--profile", default="work")
    parser.add_argument("--arms", default="plain@200k,native")
    parser.add_argument("--providers", default="both")
    parser.add_argument("--repetitions", type=int, default=1)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--daemon", type=Path, default=ROOT / "target/debug/brigadierd")
    parser.add_argument("--cell", help="JSON matrix cell (or CELL environment variable)")
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--input", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    if args.dry_run and args.mode != "plan":
        parser.error("--dry-run is a plan-only path")
    try:
        if args.mode == "plan":
            planned = plan(args.preset, args.profile, args.arms, args.providers, args.repetitions)
            args.output.mkdir(parents=True, exist_ok=True)
            (args.output / "plan.json").write_text(json.dumps(planned, indent=2) + "\n")
            (args.output / "plan.md").write_text(markdown(planned))
            if os.environ.get("GITHUB_OUTPUT"):
                with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                    output.write("matrix=" + json.dumps(planned["matrix"], separators=(",", ":")) + "\n")
            if os.environ.get("GITHUB_STEP_SUMMARY"):
                with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as output:
                    output.write(markdown(planned))
            print(markdown(planned))
            return 0
        if args.mode == "run":
            return execute(json.loads(args.cell or os.environ["CELL"]), args.daemon, args.output, args.live)
        collect(args.input, args.output)
        return 0
    except ValueError as error:
        parser.error(str(error))


if __name__ == "__main__":
    sys.exit(main())
