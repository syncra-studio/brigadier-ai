#!/usr/bin/env python3
"""Opt-in fair comparisons. --dry-run never starts a daemon, CLI, judge or network call."""
import argparse
import json
from pathlib import Path
import signal
import sys

import arms
import battery
from paired import compare, cost_per_pass
from profiles import DEPTHS, Work, packet
from zed import SkippedArm


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--daemon")
    p.add_argument("--provider", choices=["claude", "codex"], default="claude")
    p.add_argument("--model")
    p.add_argument("--arms", default="plain@200k,native")
    p.add_argument("--profile", choices=["work", "endurance"], default="work")
    p.add_argument("--tokens", type=int, default=200000)
    p.add_argument("--turns", type=int)
    p.add_argument("--seed", type=int, default=1729)
    p.add_argument("--repetitions", type=int, default=1)
    p.add_argument("--phase", choices=["pilot", "confirmatory"], default="pilot")
    p.add_argument("--timeout", type=int, default=24000)
    p.add_argument("--turn-timeout", type=int, default=240)
    p.add_argument("--report", type=Path, required=True)
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--scratch-prefix", default="continuity-experiment")
    return p


def schedule(args):
    """Fixed task order and seeds across arms; depth-triggered disjoint endurance probes."""
    groups = ["A", "B", "C"] if args.profile == "endurance" else ["A"]
    return {"profile": args.profile, "arms": args.arms, "shared_transition_tokens": args.tokens,
            "seeds": [args.seed + index for index in range(args.repetitions)], "phase": args.phase,
            "turns": args.turns, "task_order": list(range(args.turns)), "fact_groups": groups,
            "probe_depths": list(DEPTHS) if args.profile == "endurance" else ["after work"],
            "target_growth_tokens": 2000000 if args.profile == "endurance" else None,
            "growth_tokens_per_request_target": [1000, 3000], "output_tokens_per_request_target": 600,
            "tool_receipt_bytes": len(packet(args.seed, 0)[0].encode()),
            "faults": False, "lookup_access": "identical read-only native archive", "dry_run": args.dry_run}


def summary_row(run):
    battery.prepare_economics(run.result)
    result = run.result
    rows = result.get("items", {})
    memory = {key: value for key, value in rows.items() if value["retention"] == "evicted"}
    violations = {f"{score['group']}.{key}": value for score in result.get("depth_scores", [])
                  for key, value in score["decision_violations"].items()}
    return {"source": result["source"], "daemon_sha256": result["daemon_sha256"],
            "threshold": run.args.tokens, "turns": run.args.turns, "model": run.args.model,
            "provider": run.args.provider, "profile": run.args.profile, "arm": run.args.arm,
            "seed": run.args.seed, "lineage": run.args.lineage,
            "status": result.get("status", "incomplete" if run.errors or not rows else "complete"),
            "reason": result.get("reason"), "errors": run.errors, "run_kind": result["run_kind"],
            "lookup_stats": result.get("lookup_stats"), "roles": result["roles"], "list_price_usd": result["list_price_usd"],
            "native_compaction_in_seam": result["native_compaction_in_seam"],
            "transitions": result.get("seams_before_checks"), "native_compactions": result.get("compactions"),
            "fallback_seams": len(result.get("handoffs", [])), "memory_keys": sorted(memory),
            "retained_items": sum(row["retention"] == "retained" for row in rows.values()),
            "unverified_items": sum(row["retention"] == "unverified" for row in rows.values()),
            "recall_apply": sum(row["verdict"] == "pass" for row in memory.values()) / len(memory) if memory else None,
            "fully_passing": bool(memory) and not any(row["retention"] == "unverified" for row in rows.values()) and all(row["verdict"] == "pass" for row in rows.values()) and
                             all(result.get("work_checks", [])),
            "decision_violations": violations,
            "depth_scores": [{key: value for key, value in score.items() if key not in ("items", "ledger_ids")}
                             for score in result.get("depth_scores", [])]}


def run_one(args):
    transport = arms.transport(args.arm)
    cls = type("WorkRun", (Work, transport), {}) if transport is not battery.Battery else Work
    run = cls(args)
    try:
        run.run()
    except SkippedArm as error:
        run.result.update(status="skipped", reason=str(error))
    except Exception as error:
        run.errors.append(str(error))
    finally:
        try:
            run.capture()
        except Exception as error:
            run.errors.append("capture: " + str(error))
        run.cleanup()
    return summary_row(run)


def write_summary(path, summary):
    path.with_suffix(".json").write_text(json.dumps(summary, indent=2) + "\n")
    lines = ["# Continuity comparison", "", f"Phase: {summary['plan']['phase']}. "
             "Pilot intervals estimate variance; they do not establish non-inferiority.", "",
             "Recall-and-apply includes only facts verified evicted from the compaction payload. "
             "Retained and unverified facts are separate observations.", "",
             "| Arm | Seed | Status | Memory items | Recall-and-apply | Retained | Unverified | Native list-price USD |",
             "|---|---:|---|---:|---:|---:|---:|---:|"]
    for row in summary["arms"]:
        lines.append(f"| {row['arm']} | {row['seed']} | {row['status']} | {len(row['memory_keys'])} | "
                     f"{row['recall_apply']} | {row['retained_items']} | {row['unverified_items']} | "
                     f"{battery.usd(row['list_price_usd'])} |")
        if row.get("reason") or row.get("errors"):
            lines += ["", str(row.get("reason") or row["errors"]), ""]
        if row.get("native_compaction_in_seam"):
            lines += ["", "WARNING: native compaction inside seam arm; transition comparison is confounded.", ""]
        for depth in row["depth_scores"]:
            lines += ["", f"{row['arm']} seed {row['seed']} depth {depth['depth']} ({depth['group']}): "
                      f"memory {depth['memory_items']}; recall-and-apply {depth['recall_apply']}; "
                      f"stale answers {depth['stale_answers']}; dropped ledger ids {depth['dropped_ledger_ids']}.", ""]
    lines += ["", "| Arm | Paired lineages | Mean difference | 90% interval | Decision violations no worse | Non-inferior |",
              "|---|---:|---:|---|---|---|"]
    for row in summary["paired"]:
        lines.append(f"| {row['arm']} | {row['pairs']} | {row['mean_difference']} | {row['interval90']} | "
                     f"{row['no_more_decision_violations']} | {row['non_inferior']} |")
    lines += ["", "Proposed margin: lower 90% bound > -1/12, with no planted decision violated more often. "
              "Only complete, matching seed/order/evicted-item sets enter continuity pairs. "
              "Single-lineage intervals are unavailable.", "",
              "| Arm | Native list-price USD per fully passing lineage |", "|---|---:|"]
    for arm, cost in summary["cost_per_fully_passing_lineage"].items():
        lines.append(f"| {arm} | {battery.usd(cost)} |")
    for row in summary["arms"]:
        lines += ["", f"## {row['arm']} seed {row['seed']}", ""] + battery.role_table(row["roles"])
    path.with_suffix(".md").write_text("\n".join(lines) + "\n")


def main(argv=None):
    p = parser()
    args = p.parse_args(argv)
    args.arms = list(dict.fromkeys(args.arms.split(",")))
    if any(arm not in arms.ARMS for arm in args.arms) or not args.arms:
        p.error("unknown arm")
    if args.tokens < 100000 or args.tokens > 1000000:
        p.error("shared threshold must be 100000..1000000 (Claude CLI's supported range)")
    if args.repetitions < 1:
        p.error("use at least one paired lineage")
    args.turns = args.turns if args.turns is not None else (1400 if args.profile == "endurance" else 300)
    if args.turns < 3:
        p.error("use at least three work turns")
    args.model = args.model or ("opus" if args.provider == "claude" else "gpt-6.1-sol")
    if "fable" in args.model.lower():
        p.error("use a permitted model")
    plan = schedule(args)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    if args.dry_run:
        args.report.write_text(json.dumps(plan, indent=2) + "\n")
        print(json.dumps(plan, indent=2))
        return 0
    if not args.daemon or str(Path(args.daemon).resolve()).startswith("/Applications/"):
        p.error("use a debug daemon from this checkout")
    args.shared_transition, args.min_fillers, args.max_fillers, args.ballast = True, 0, 0, 0
    args.evidence = None
    def terminated(signum, frame):
        raise TimeoutError("experiment supervisor stopped the run")
    signal.signal(signal.SIGTERM, terminated)
    rows = []
    for lineage in range(args.repetitions):
        # A seed fixes the plants and workload; neither includes the arm name.
        for arm in args.arms:
            namespace = argparse.Namespace(**vars(args))
            namespace.arm, namespace.seed, namespace.lineage = arm, args.seed + lineage, lineage
            rows.append(run_one(namespace))
            summary = {"plan": plan, "arms": rows, "paired": compare(rows, phase=args.phase),
                       "cost_per_fully_passing_lineage": {arm: cost_per_pass([row for row in rows if row["arm"] == arm])
                                                         for arm in args.arms}}
            # Checkpoint the public numeric report after each arm in case a later run fails.
            write_summary(args.report, summary)
    return 2 if any(row["status"] == "incomplete" for row in rows) else 0


if __name__ == "__main__":
    sys.exit(main())
