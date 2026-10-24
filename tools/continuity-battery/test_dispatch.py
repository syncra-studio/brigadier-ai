"""Dispatch matrices and plan-only safety; no CLIs, daemons or model calls."""
from collections import Counter
import contextlib
import io
import json
import os
import re
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import arms
import battery
import updates
import dispatch

PREFIX = "20261024-1334-long-session-memory-w17-test-"


class Presets(unittest.TestCase):
    def test_stage0_is_one_fixed_short_recall_and_updates_pair_per_provider(self):
        planned = dispatch.plan("stage0")
        expected = {(provider, profile, arm, 1729)
                    for provider in ("claude", "codex")
                    for profile in ("recall-short", "updates-short")
                    for arm in ("seam", "plain@200k")}
        actual = {(cell["provider"], cell["profile"], cell["arm"], cell["seed"])
                  for cell in planned["matrix"]["include"]}
        self.assertEqual(actual, expected)
        self.assertEqual(planned["arm_runs"], 8)
        self.assertEqual(planned["estimated_list_usd"], [10, 20])
        self.assertEqual(planned["shared_transition_tokens"], 200000)

    def test_pilot_matches_section3_without_p5(self):
        planned = dispatch.plan("pilot")
        expected = {
            ("p1", "codex", "updates-short"): ({"seam", "native-bare", "native", "plain"}, 4),
            ("p1", "codex", "recall-long"): ({"native-bare", "native", "plain"}, 2),
            ("p2", "claude", "work"): ({"plain@200k", "native-bare", "native-full", "seam"}, 2),
            ("p3", "claude", "recall-short"): ({"seam", "seam-repaired"}, 6),
            ("p3", "codex", "recall-short"): ({"seam", "seam-repaired"}, 6),
            ("endurance", "claude", "endurance"): ({"plain@200k", "native-full"}, 1),
            ("endurance", "codex", "endurance"): ({"plain@200k", "native"}, 1),
        }
        actual = {(g["block"], g["provider"], g["profile"]): (set(g["arms"]), g["repetitions"])
                  for g in planned["groups"]}
        self.assertEqual(actual, expected)
        cells = planned["matrix"]["include"]
        self.assertEqual(planned["arm_runs"], 58)
        self.assertEqual(len({cell["id"] for cell in cells}), 58)
        self.assertEqual(Counter(cell["block"] for cell in cells), {"p1": 22, "p2": 8, "p3": 24, "endurance": 4})
        self.assertEqual(planned["estimated_list_usd"], [462.8, 572.8])
        self.assertEqual(planned["with_judge_and_reruns_usd"], [601.64, 744.64])
        self.assertFalse(any("checkpoint" in cell["arm"] or cell["block"] == "p5" for cell in cells))
        for group in planned["groups"]:
            for repetition in range(1, group["repetitions"] + 1):
                seeds = {cell["seed"] for cell in cells if cell["block"] == group["block"]
                         and cell["provider"] == group["provider"] and cell["profile"] == group["profile"]
                         and cell["repetition"] == repetition}
                self.assertEqual(seeds, {1728 + repetition})

    def test_presets_fix_the_matrix_and_custom_inputs_select_only_requested_cells(self):
        self.assertEqual(dispatch.plan("pilot"), dispatch.plan("pilot", "endurance", "native-full+checkpoint", "claude", 7))
        custom = dispatch.plan(profile="work", requested_arms="native-full+checkpoint, plain@200k,plain@200k",
                               providers="codex", repetitions=3)
        self.assertEqual(custom["arm_runs"], 6)
        self.assertEqual({c["provider"] for c in custom["matrix"]["include"]}, {"codex"})
        self.assertEqual({c["seed"] for c in custom["matrix"]["include"]}, {1729, 1730, 1731})
        self.assertEqual(dispatch.selected_arms(",".join(arms.ARMS)), list(arms.ARMS))
        for kwargs in ({"preset": "p5"}, {"profile": "bad"}, {"providers": "all"}, {"repetitions": 0},
                       {"requested_arms": "native;echo bad"}, {"requested_arms": ""}, {"repetitions": 300}):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                dispatch.plan(**kwargs)

    def test_dry_run_writes_matrix_and_summary_without_launch_or_credentials(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            root = Path(scratch)
            with patch.dict(os.environ, {"GITHUB_OUTPUT": str(root / "outputs"),
                                         "GITHUB_STEP_SUMMARY": str(root / "summary")}, clear=True), \
                    patch("dispatch.subprocess.run", side_effect=AssertionError("no subprocess")), \
                    patch("battery.Battery.setup", side_effect=AssertionError("no daemon")), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(dispatch.main(["plan", "--preset", "pilot", "--dry-run", "--output", str(root / "public")]), 0)
            output = (root / "outputs").read_text().strip().removeprefix("matrix=")
            self.assertEqual(len(json.loads(output)["include"]), 58)
            self.assertIn("$462.80–$572.80", (root / "summary").read_text())
            self.assertIn("58 arm-runs", (root / "summary").read_text())
            self.assertEqual({p.name for p in (root / "public").iterdir()}, {"plan.json", "plan.md"})

    def test_cells_use_the_existing_fair_legacy_scripts_or_experiment_runner(self):
        for cell in dispatch.plan("pilot")["matrix"]["include"]:
            cmd = dispatch.command(cell, Path("/debug/brigadierd"), Path("/public"))
            self.assertNotIn("--evidence", cmd)
            if cell["profile"] in ("work", "endurance"):
                self.assertIn("experiment.py", Path(cmd[2]).name)
                self.assertEqual(cmd[cmd.index("--seed") + 1], str(cell["seed"]))
                self.assertEqual(cmd[cmd.index("--repetitions") + 1], "1")
            else:
                self.assertIn("--fair", cmd)
                self.assertEqual(cmd[cmd.index("--tokens") + 1], "200000")
                expected = "90" if cell["profile"].endswith("long") else "9"
                self.assertEqual(cmd[cmd.index("--min-fillers") + 1], expected)
                self.assertEqual(cmd[cmd.index("--max-fillers") + 1], expected)
                self.assertEqual(Path(cmd[2]).name, "updates.py" if cell["profile"].startswith("updates") else "battery.py")

    def test_missing_credentials_skip_without_subprocess(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch, patch("dispatch.subprocess.run") as launch:
            cell = dispatch.plan("stage0")["matrix"]["include"][0]
            self.assertEqual(dispatch.execute(cell, Path("/debug/brigadierd"), Path(scratch), False), 0)
            launch.assert_not_called()
            self.assertEqual(json.loads(Path(scratch, "run.json").read_text())["status"], "skipped")

    def test_collector_reunites_paired_cells_and_marks_missing_runs(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            root = Path(scratch)
            inputs, output = root / "input", root / "public"
            inputs.mkdir()
            planned = dispatch.plan(profile="work", requested_arms="plain@200k,native", providers="codex", repetitions=2)
            (inputs / "plan.json").write_text(json.dumps(planned))
            for index, cell in enumerate(planned["matrix"]["include"]):
                directory = inputs / cell["id"]
                directory.mkdir()
                row = {"provider": cell["provider"], "profile": "work", "seed": cell["seed"], "lineage": 0,
                       "source": "fixture-source", "daemon_sha256": "fixture-build", "threshold": 200000,
                       "turns": 300, "model": "fixture-model", "arm": cell["arm"], "status": "complete",
                       "run_kind": "continuity", "memory_keys": ["tool_fact"], "recall_apply": 1,
                       "decision_violations": {}, "list_price_usd": 2, "fully_passing": True}
                (directory / "run.json").write_text(json.dumps({"cell": cell, "status": "finished"}))
                (directory / "result.json").write_text(json.dumps({"arms": [row]}))
            private = root / "private"
            private.mkdir()
            (private / "evidence.json").write_text('{"secret": "PRIVATE-FIXTURE"}')
            dispatch.collect(inputs, output)
            summary = json.loads((output / "summary.json").read_text())
            comparison = summary["comparisons"][0]
            self.assertEqual(comparison["paired"][0]["pairs"], 2)
            self.assertIsNone(comparison["paired"][0]["non_inferior"])
            self.assertEqual(comparison["cost_per_fully_passing_lineage"], {"native": 2, "plain@200k": 2})
            self.assertNotIn("PRIVATE-FIXTURE", (output / "summary.json").read_text())
            (directory / "run.json").unlink()
            dispatch.collect(inputs, output)
            summary = json.loads((output / "summary.json").read_text())
            self.assertEqual(len(summary["runs"]), 4)
            self.assertEqual(sum(run["status"] == "not_run" for run in summary["runs"]), 1)

    def test_repaired_seam_keeps_legacy_settling_and_compaction_warning(self):
        for scenario in (battery.Battery, updates.Updates):
            for arm in ("seam", "seam-repaired", "plain"):
                with self.subTest(scenario=scenario.__name__, arm=arm), tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
                    run = SimpleNamespace(repo=Path(scratch), root=Path(scratch), result={},
                        args=SimpleNamespace(arm=arm, fair=True, max_fillers=1, min_fillers=1, ballast=0),
                        expected={"tool_only": "FIXTURE-TICKET", "absent_ledger": "FIXTURE-ITEM", "large_failure": "FIXTURE-DIAGNOSTIC"},
                        setup=Mock(), send=Mock(return_value={"answer": "{}"}), generations=lambda: 0, wait=Mock())
                    scenario.run(run)
                    self.assertEqual(run.wait.call_count, 0 if arm == "plain" else 1)
                    if arm != "plain":
                        self.assertEqual(run.wait.call_args.args[1], 3)
                    result = {"arm": arm, "compactions": 1}
                    battery.prepare_economics(result)
                    self.assertEqual(result["native_compaction_in_seam"], arm != "plain")

    def test_seam_repaired_uses_real_switch_and_counts_seams(self):
        run = object.__new__(arms.CandidateBattery)
        run.args = type("Args", (), {"arm": "seam-repaired"})()
        self.assertEqual(run.configure_environment({})[arms.LONG_SESSION_SWITCH], "seam-repaired")
        with patch("battery.Battery.generations", return_value=3):
            self.assertEqual(run.generations(), 3)


class Workflow(unittest.TestCase):
    def test_nightly_call_and_dry_run_job_separation(self):
        workflow = (dispatch.ROOT / ".github/workflows/continuity-battery.yml").read_text()
        call = workflow.split("  workflow_call:\n", 1)[1].split("  workflow_dispatch:\n", 1)[0]
        self.assertEqual(call, '''    inputs:
      arms:
        description: "Comma-separated arms to run: seam, plain, zed, summary"
        type: string
        default: seam,plain,zed
''')
        for job in ("selection", "provider", "weekly", "report"):
            block = re.split(r"(?m)^  [^ ]", workflow.split(f"  {job}:\n", 1)[1], maxsplit=1)[0]
            self.assertIn("!inputs.dry_run", block)
            self.assertIn("inputs.runner != 'experiment'", block)
        plan_job = workflow.split("  experiment-plan:\n", 1)[1].split("  experiment:\n", 1)[0]
        self.assertNotIn("secrets.", plan_job)
        self.assertIn("plan --dry-run", plan_job)
        experiment_job = workflow.split("  experiment:\n", 1)[1].split("  experiment-report:\n", 1)[0]
        self.assertIn("&& !inputs.dry_run", experiment_job)
        self.assertIn('export BRIGADIER_BATTERY_EVIDENCE_DIR="$RUNNER_TEMP/continuity-private"', experiment_job)
        self.assertIn("path: ${{ runner.temp }}/continuity-experiment-cell-${{ matrix.id }}", experiment_job)
        self.assertNotIn("path: ${{ runner.temp }}/continuity-private", workflow)


if __name__ == "__main__":
    unittest.main()
