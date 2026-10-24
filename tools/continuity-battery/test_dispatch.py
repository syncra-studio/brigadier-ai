"""Dispatch matrices and plan-only safety; no CLIs, daemons or model calls."""
import argparse
from collections import Counter
import contextlib
import io
import json
import os
import re
from pathlib import Path
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import arms
import battery
import codex_transport
import experiment
import updates
import dispatch

PREFIX = "20261024-1334-long-session-memory-w18-test-"


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
            ("p1", "codex", "recall-long"): ({"native-bare@codex-default", "native@codex-default", "plain@codex-default"}, 2),
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

    def test_p1_long_variants_reach_real_native_modes_without_shared_point(self):
        planned = dispatch.plan("pilot")
        cells = [cell for cell in planned["matrix"]["include"] if cell["arm"] in arms.CODEX_DEFAULT_ARMS]
        self.assertEqual(len(cells), 6)
        for cell in cells:
            self.assertEqual((cell["block"], cell["provider"], cell["profile"]), ("p1", "codex", "recall-long"))
            self.assertIn("codex-default", cell["id"])
            cmd = dispatch.command(cell, Path("/debug/brigadierd"), Path("/public"))
            self.assertEqual(cmd[cmd.index("--arm") + 1], cell["arm"])
            args = SimpleNamespace(arm=cell["arm"], provider="codex", fair=True, tokens=200000)
            arms.configure(args, argparse.ArgumentParser(), 43750)
            # Fair access still selects app-server and the common lookup archive.
            self.assertTrue(args.shared_transition)
            vehicle = arms.transport(args.arm)
            run = object.__new__(vehicle)
            run.args = args
            if vehicle is arms.CandidateBattery:
                with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
                    run.root = run.data = Path(scratch)
                    run.processes, run.wait = [], Mock(return_value=True)
                    args.daemon, args.timeout = "/debug/brigadierd", 100
                    with patch("battery.subprocess.Popen") as launch:
                        run.start()
                    env = launch.call_args.kwargs["env"]
                    self.assertNotIn("BRIGADIER_SEAM_TOKENS", env)
                    self.assertEqual(env[arms.LONG_SESSION_SWITCH], arms.CODEX_DEFAULT_ARMS[args.arm])
            else:
                self.assertIs(vehicle, arms.SharedPlainBattery)
                run.repo, run.deadline = Path("/tmp"), time.monotonic() + 10
                run.processes, run.environment = [], lambda: {}
                with patch("codex_transport.subprocess.Popen", return_value=Mock(stdout=io.StringIO(""))) as launch, \
                        patch.object(codex_transport.Client, "call"), patch.object(codex_transport.Client, "write"):
                    codex_transport.Client(run)
                self.assertEqual(launch.call_args.args[0], ["codex", "app-server", "-c", 'model_reasoning_effort="high"'])
                with patch("codex_transport.send", return_value={"answer": "fixture"}) as send:
                    self.assertEqual(run.send("probe", "prompt"), {"answer": "fixture"})
                send.assert_called_once_with(run, "probe", "prompt", None)
            self.assertIn("CLI default (no shared override)", "\n".join(battery.arm_markdown({"arm": args.arm})))
        self.assertIn("native@codex-default", dispatch.markdown(planned))

    def test_other_candidate_and_seam_thresholds_stay_shared(self):
        for arm in ("native-bare", "native", "native-full", "seam-repaired", "seam"):
            run = object.__new__(arms.transport(arm))
            run.args = SimpleNamespace(arm=arm)
            env = run.configure_environment({"BRIGADIER_SEAM_TOKENS": "200000"})
            self.assertEqual(env["BRIGADIER_SEAM_TOKENS"], "200000")
            if arm != "seam":
                self.assertEqual(env[arms.LONG_SESSION_SWITCH], arm)

    def test_codex_default_variants_require_codex_and_work_dry_runs_accept_them(self):
        for arm in arms.CODEX_DEFAULT_ARMS:
            args = SimpleNamespace(arm=arm, provider="claude", fair=True, tokens=200000)
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                arms.configure(args, argparse.ArgumentParser(), 43750)
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            report = Path(scratch) / "plan.json"
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(experiment.main(["--provider", "codex", "--arms", ",".join(arms.CODEX_DEFAULT_ARMS),
                                                  "--dry-run", "--report", str(report)]), 0)
            self.assertEqual(json.loads(report.read_text())["arms"], list(arms.CODEX_DEFAULT_ARMS))
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                experiment.main(["--provider", "claude", "--arms", "native@codex-default",
                                 "--dry-run", "--report", str(report)])

    def test_every_cell_scratch_root_keeps_the_daemon_socket_where_ipc_looks(self):
        # Pilot run 40109608115: the `-codex-default` ids made the socket path 102/107 bytes, so
        # the daemon bound its /tmp fallback and every P1 long native cell timed out at startup.
        cells = [cell for preset in ("stage0", "pilot") for cell in dispatch.plan(preset)["matrix"]["include"]]
        for tmp in (Path("/tmp"), Path("/private/tmp")):
            for cell in cells:
                cmd = dispatch.command(cell, Path("/debug/brigadierd"), Path("/public"))
                prefix = cmd[cmd.index("--scratch-prefix") + 1]
                root = battery.scratch_root(prefix, tmp)
                self.assertLessEqual(len(str(root) + battery.SOCKET_TAIL), battery.MAX_SOCKET_PATH, cell["id"])
                self.assertEqual(root.parent, tmp)
                self.assertTrue(prefix.startswith(root.name[:-7]))
        short = battery.scratch_root("continuity-battery", Path("/tmp"))
        self.assertRegex(short.name, r"^continuity-battery-[0-9a-f]{6}$")

    def test_debug_daemon_answers_on_a_long_cell_scratch_root(self):
        import shutil
        import subprocess
        if not (battery.SRC / "target/debug/brigadierd").is_file():
            self.skipTest("needs a debug daemon")
        prefix = "continuity-experiment-p1-codex-recall-long-native-bare-codex-default-1"
        root = battery.scratch_root(prefix)
        data = root / "data"
        data.mkdir(parents=True)
        env = {k: os.environ[k] for k in ("HOME", "PATH") if k in os.environ}
        proc = subprocess.Popen([str(battery.SRC / "target/debug/brigadierd"), "--foreground", "--data-dir", str(data)],
                                env={**env, "BRIGADIER_DATA_DIR": str(data), "BRIGADIER_IDLE_EXIT_SECS": "30"},
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            ipc, deadline, catalog = battery.smoke.Ipc(str(data)), time.monotonic() + 60, None
            while catalog is None and time.monotonic() < deadline and proc.poll() is None:
                try:
                    catalog = ipc.call({"method": "getCatalog"}, timeout=3)
                except (OSError, RuntimeError, ConnectionError):
                    time.sleep(0.5)
            self.assertIsNotNone(catalog)
        finally:
            proc.terminate()
            proc.wait(15)
            shutil.rmtree(root)

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
