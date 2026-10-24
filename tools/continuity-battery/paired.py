"""Paired lineage differences, 90% Student intervals and the proposed D45 bar."""
import math
from statistics import mean, stdev

# Two-sided 90% interval (0.95 Student quantile), df 1..30; normal above 30.
T95 = (6.314, 2.920, 2.353, 2.132, 2.015, 1.943, 1.895, 1.860, 1.833, 1.812,
       1.796, 1.782, 1.771, 1.761, 1.753, 1.746, 1.740, 1.734, 1.729, 1.725,
       1.721, 1.717, 1.714, 1.711, 1.708, 1.706, 1.703, 1.701, 1.699, 1.697)
MARGIN = -1 / 12


def interval90(values):
    if len(values) < 2:
        return None
    # Use df=30 conservatively above the table, rather than under-cover small samples.
    t = T95[min(len(values) - 2, len(T95) - 1)]
    half = t * stdev(values) / math.sqrt(len(values))
    center = mean(values)
    return [center - half, center + half]


def key(row):
    return (row.get("provider"), row.get("profile"), row.get("seed"), row.get("lineage"), row.get("source"), row.get("daemon_sha256"), row.get("threshold"),
            row.get("turns"), row.get("model"))


def compare(rows, baseline="plain@200k", phase="pilot"):
    result = []
    controls = {key(row): row for row in rows if row["arm"] == baseline and row.get("status") == "complete"}
    for arm in sorted({row["arm"] for row in rows} - {baseline}):
        differences, costs, violations, eligible_keys = [], [], {}, []
        pairs = []
        for row in rows:
            control = controls.get(key(row))
            if row["arm"] != arm or row.get("status") != "complete" or not control:
                continue
            if row.get("run_kind") != "continuity" or control.get("run_kind") != "continuity":
                continue
            # Same evicted item set, not a comparison of different retained facts.
            left, right = row.get("memory_keys"), control.get("memory_keys")
            if not left or left != right or row.get("recall_apply") is None or control.get("recall_apply") is None:
                continue
            pairs.append(row)
            eligible_keys.append(key(row))
            differences.append(row["recall_apply"] - control["recall_apply"])
            if row.get("list_price_usd") is not None and control.get("list_price_usd") is not None:
                costs.append(row["list_price_usd"] - control["list_price_usd"])
            for item in set(row.get("decision_violations", {})) | set(control.get("decision_violations", {})):
                violations[item] = violations.get(item, 0) + row.get("decision_violations", {}).get(item, 0) - control.get("decision_violations", {}).get(item, 0)
        interval = interval90(differences)
        no_more_violations = bool(pairs) and all(count <= 0 for count in violations.values())
        observed_bar = interval is not None and interval[0] > MARGIN and no_more_violations
        # Pilot samples estimate variance. They cannot pass the confirmatory bar.
        result.append({"arm": arm, "baseline": baseline, "phase": phase, "pairs": len(pairs),
                       "mean_difference": mean(differences) if differences else None, "interval90": interval,
                       "sample_variance": stdev(differences) ** 2 if len(differences) > 1 else None,
                       "cost_difference_interval90": interval90(costs) if len(costs) == len(pairs) else None,
                       "decision_violation_differences": violations, "no_more_decision_violations": no_more_violations,
                       "observed_bar": observed_bar, "non_inferior": observed_bar if phase == "confirmatory" else None})
    return result


def cost_per_pass(rows):
    complete = [row for row in rows if row.get("status") == "complete"]
    # Include spend on partial/failed attempts, not just successful lineages.
    values = [row.get("list_price_usd") for row in rows if row.get("status") != "skipped"]
    passing = sum(row.get("fully_passing") is True for row in complete)
    return sum(values) / passing if passing and values and all(value is not None for value in values) else None
