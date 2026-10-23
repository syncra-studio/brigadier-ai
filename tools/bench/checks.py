#!/usr/bin/env python3
"""Focused offline checks: transcript accounting, graders, and non-gating comparisons."""
from copy import deepcopy
import errno
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path
from types import SimpleNamespace
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench
from claude_launch import EXCLUDE_DYNAMIC, launch
from bench import Daemon, cleanup_native, keep, models_file, remove_empty_project, task_prompt, worker_start
from metrics import parse
from economics import call, combine, rates
from tasks import TASKS, fixture, grade
from nightly import collect, issue, regressions

PREFIX = os.environ.get("BENCH_SCRATCH_PREFIX", "brigadier-bench-check-" + os.environ.get("GITHUB_RUN_ID", "local") + "-")


class Accounting(unittest.TestCase):
    def test_claude_final_usage_deduplicates_streaming(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            path = Path(tmp) / "native.jsonl"
            rows = [{"type": "assistant", "message": {"id": "m", "model": "opus", "usage": {
                "input_tokens": 10, "cache_creation_input_tokens": 20, "cache_read_input_tokens": 30, "output_tokens": n},
                "content": [{"type": "tool_use", "id": "tool", "input": {"command": "brigadier ledger add decision ok"}}]}} for n in (1, 4)]
            rows += [{"type": "user", "message": {"content": "task"}},
                     {"type": "user", "message": {"content": [{"type": "tool_result", "content": "not an injection"}]}},
                     {"type": "user", "message": {"content": [{"type": "text", "text": "skill"}]}}]
            path.write_text("\n".join(map(json.dumps, rows)))
            result = parse(path, "claude", "task")
            self.assertEqual(result["usage"], {"input": 10, "cache_write": 20, "cache_read": 30, "output": 4})
            self.assertEqual((result["api_calls"], result["tools"], result["ledger_adds"], result["injected_messages"], result["injected_bytes"]), (1, 1, 1, 1, 5))

    def test_codex_cache_subset_and_repeated_token_event(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            path = Path(tmp) / "rollout.jsonl"
            token = lambda n: {"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": n, "cached_input_tokens": 20, "output_tokens": 5}}}}
            rows = [token(50), token(50), token(80), {"type": "turn_context", "payload": {"model": "default"}},
                    {"type": "response_item", "payload": {"type": "function_call", "call_id": "1", "arguments": json.dumps({"code": 'await tools.exec_command({cmd: "brigadier ledger add decision ok"})'})}}]
            path.write_text("\n".join(map(json.dumps, rows)))
            result = parse(path, "codex", "task")
            self.assertEqual(result["usage"], {"input": 60, "cache_write": 0, "cache_read": 20, "output": 5})
            self.assertEqual(result["api_calls"], 2)
            self.assertEqual(result["ledger_adds"], 1)
            # Shell quoting inside the command string doesn't end it.
            rows[-1]["payload"]["arguments"] = json.dumps({"code": 'await tools.exec_command({cmd: "cd \'/tmp/repo\' && brigadier ledger add decision ok"})'})
            path.write_text("\n".join(map(json.dumps, rows)))
            self.assertEqual(parse(path, "codex", "task")["ledger_adds"], 1)
            path.write_text('{}\n')
            with self.assertRaises(RuntimeError):
                parse(path, "codex", "task")

    def test_role_prices_follow_native_models_and_keep_legacy_units(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            def native(name, model):
                path = root / name
                path.write_text(json.dumps({"type": "assistant", "message": {
                    "id": "m", "model": model, "usage": {"input_tokens": 10,
                    "cache_creation_input_tokens": 20, "cache_read_input_tokens": 30, "output_tokens": 4,
                    "cache_creation": {"ephemeral_1h_input_tokens": 20}}}}))
                return parse(path, "claude", "")
            parent = native("parent.jsonl", "claude-opus-5-5")
            worker = native("worker.jsonl", "claude-sonnet-5-5")
            self.assertAlmostEqual(parent["economics"]["list_price_usd"], .000286)
            self.assertAlmostEqual(worker["economics"]["list_price_usd"], .000143)
            self.assertEqual(parent["cost_units"], worker["cost_units"])
            roles = {"parent": parent["economics"], "worker": worker["economics"]}
            self.assertEqual(roles["parent"]["uncached_input"], 30)
            self.assertEqual(roles["parent"]["cache_hit_ratio"], .5)
            base = dict(provider="claude", status="complete", task="t1", run=1, grade={"passed": True},
                        wall_s=1, raw_tokens=64, cost_units=parent["cost_units"], api_calls=1,
                        usage=parent["usage"], models=parent["models"], brigadier_commands=0,
                        ledger_adds=0, injected_messages=0, injected_bytes=0, worker_usage=None)
            report = dict(source="fixture", fixture_version=1, effort="low", runs=[
                dict(base, arm="A", economics=parent["economics"]), dict(base, arm="W", roles=roles)])
            bench.write(report, root / "report")
            saved = json.loads((root / "report/report.json").read_text())
            self.assertAlmostEqual(saved["aggregate"][1]["list_price_ratio"], 1.5)
            self.assertAlmostEqual(saved["aggregate"][1]["mean_list_price_usd"], .000429)
            self.assertIn("Cache hit ratio", (root / "report/report.md").read_text())
            self.assertIn("claude-sonnet-5-5", (root / "report/report.md").read_text())
            unknown = native("unknown.jsonl", "unpublished-codex-id")
            self.assertIsNone(unknown["economics"]["list_price_usd"])
            report["runs"][1]["roles"]["worker"] = unknown["economics"]
            bench.write(report, root / "report")
            self.assertIsNone(report["aggregate"][1]["mean_list_price_usd"])
            self.assertIsNone(report["aggregate"][1]["list_price_ratio"])
            self.assertIn("n/a", (root / "report/report.md").read_text())

    def test_codex_prices_each_native_model_and_context_tier(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            path = Path(tmp) / "rollout.jsonl"
            rows = []
            for model, total, cached, output, context in (
                    ("gpt-6.1-sol", 100, 20, 5, 100),
                    ("gpt-6-astra", 300100, 100020, 15, 300000)):
                rows += [{"type": "turn_context", "payload": {"model": model}},
                         {"type": "event_msg", "payload": {"type": "token_count", "info": {
                             "total_token_usage": {"input_tokens": total, "cached_input_tokens": cached,
                                                   "output_tokens": output},
                             "last_token_usage": {"input_tokens": context}}}}]
            path.write_text("\n".join(map(json.dumps, rows)))
            result = parse(path, "codex", "")
            self.assertAlmostEqual(result["economics"]["list_price_usd"], 4.200962)
            self.assertEqual(result["economics"]["models"], ["gpt-6-astra", "gpt-6.1-sol"])
            self.assertEqual(result["economics"]["usage"], result["usage"])
            self.assertEqual(rates("claude-opus-5-5-20261001"), rates("claude-opus-5-5"))
            self.assertIsNone(rates("opus"))
            self.assertIsNone(combine([])["cache_hit_ratio"])
            self.assertIsNone(call({"input": 20}, "gpt-6.1-sol")["list_price_usd"])

    def test_native_cleanup_removes_only_the_owned_project(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            projects = root / "projects"
            # The CLI's slug turns every non-alphanumeric character into "-", "_" included.
            own = projects / re.sub(r"[^A-Za-z0-9]", "-", f"/tmp/{root.name}/repo")
            (own / "memory").mkdir(parents=True)
            transcript = own / "explicit-session.jsonl"
            transcript.write_text("{}")
            neighbor = projects / "other-run"
            neighbor.mkdir()
            keep = neighbor / "session.jsonl"
            keep.write_text("{}")
            cleanup_native(transcript, root)
            cleanup_native(transcript, root)
            self.assertFalse(own.exists())
            self.assertTrue(keep.exists())

    def test_only_an_empty_project_folder_of_the_repo_goes(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            home = Path(tmp)
            repo = home / "scratch_1" / "repo"
            folder = home / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(repo))
            (folder / "memory").mkdir(parents=True)
            (folder / "memory" / "note.md").write_text("kept")
            with patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": str(home)}):
                remove_empty_project(repo)
                self.assertTrue((folder / "memory" / "note.md").is_file())
                (folder / "memory" / "note.md").unlink()
                remove_empty_project(repo)
                self.assertFalse(folder.exists())
                remove_empty_project(repo)

    def test_a_worker_transcript_written_after_its_start_is_kept(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            (root / "run").mkdir()
            daemon = Daemon(None, root / "run", root / "kept")
            started = [{"seq": 1, "event": {"type": "cleanupRecorded", "artifact": {"type": "claudeSession", "sessionId": "w"}}}]
            replies = iter([started, [], []])
            daemon.call = lambda method, **_: {"events": next(replies)}
            transcript = root / "w.jsonl"
            with patch.object(bench, "native_files", lambda _p, _s: [transcript] if transcript.exists() else []):
                daemon.events()
                self.assertFalse(daemon.kept)
                # Its CLI writes the file after the session started; a later poll keeps it.
                transcript.write_text("{}")
                daemon.events()
            self.assertIn(str(transcript), daemon.kept)
            keep([saved for _, saved in daemon.kept.values()], daemon.keep_to)
            self.assertTrue((root / "kept" / "claude-w.jsonl").is_file())

    def test_a_transcript_on_another_filesystem_is_copied_with_its_later_writes(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            (root / "run").mkdir()
            daemon = Daemon(None, root / "run")
            transcript = root / "w.jsonl"
            transcript.write_text("{}\n")
            def cross(*_):
                raise OSError(errno.EXDEV, "Cross-device link")
            daemon.call = lambda method, **_: {"events": []}
            with patch.object(bench.os, "link", cross):
                daemon.save_native("claude", transcript)
                transcript.write_text("{}\n{}\n")
                daemon.events()
            self.assertEqual(daemon.kept[str(transcript)][1].read_text(), "{}\n{}\n")


class WorkerModel(unittest.TestCase):
    def test_auto_leaves_model_and_effort_to_the_shipped_advice(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            args = SimpleNamespace(effort="low", worker_model="pinned")
            pinned = task_prompt("t1", "W", "claude", "opus", args)
            self.assertIn("--effort low", pinned)
            self.assertIn("--model opus", pinned)
            self.assertEqual(models_file(root, "pinned").read_text(), "")
            args.worker_model = "auto"
            auto = task_prompt("t1", "W", "claude", "opus", args)
            self.assertIn("brigadier start --agent claude --permission full-access --local.", auto)
            self.assertNotIn("--model opus", auto)
            self.assertNotIn("--effort low", auto)
            self.assertEqual(task_prompt("t1", "A", "claude", "opus", args), task_prompt("t1", "A", "claude", "opus", SimpleNamespace(effort="low", worker_model="pinned")))
            (root / "models.md").unlink()
            # Not made: the daemon writes the shipped default there on first read.
            self.assertFalse(models_file(root, "auto").exists())

    def test_the_start_receipt_says_what_the_worker_got(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            path = Path(tmp) / "parent.jsonl"
            receipt = ("w-1\nStarted worker 1 (claude, claude-sonnet-5-5, medium effort), in this session's checkout, permission full access.\n"
                       "The shipped row \"Mechanical changes and search\" picked its model and effort.\nKeep `brigadier wait` running.\n")
            rows = [{"type": "user", "message": {"content": [{"type": "tool_result", "content": [{"type": "text", "text": receipt}]}]}}]
            path.write_text("\n".join(map(json.dumps, rows)))
            self.assertEqual(worker_start(path), {"started": "claude, claude-sonnet-5-5, medium effort",
                                                  "row": "The shipped row \"Mechanical changes and search\" picked its model and effort."})
            path.write_text("{}\n")
            self.assertIsNone(worker_start(path))


class CacheOptions(unittest.TestCase):
    def test_daemon_selects_an_executable_launcher_only_when_enabled(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            for enabled, stable in ((False, False), (False, True), (True, False), (True, True)):
                root = Path(tmp) / f"dynamic {enabled} stable {stable} with spaces"
                root.mkdir()
                args = SimpleNamespace(daemon="/fake/daemon", worker_model="auto",
                                       claude_flag_exclude_dynamic=enabled, worker_cache_ttl=None, stable_standing=stable)
                daemon = Daemon(args, root)
                daemon.provider = "claude"
                calls = []
                def call(method, **fields):
                    calls.append((method, fields))
                    return {"catalog": {"settings": {}}}
                daemon.call = call
                with patch.dict(os.environ, {"BRIGADIER_BENCH_STABLE_STANDING": "1"}), \
                        patch("bench.subprocess.Popen") as process, patch("bench.shutil.which", return_value="/bin/echo"):
                    process.return_value.poll.return_value = None
                    daemon.start()
                launched_env = process.call_args.kwargs["env"]
                self.assertEqual(launched_env.get("BRIGADIER_BENCH_STABLE_STANDING"), "1" if stable else None)
                settings = calls[-1][1]["settings"]
                if enabled:
                    launcher = Path(settings["agentPaths"][0]["path"])
                    config = json.loads(daemon.launch_config.read_text())
                    config["parent"] = "parent-session"
                    daemon.launch_config.write_text(json.dumps(config))
                    env = dict(os.environ, BRIGADIER_SESSION="parent-session")
                    discovered = subprocess.run([str(launcher), "--version"], env=env, check=True, capture_output=True, text=True)
                    self.assertEqual(discovered.stdout.strip(), "--version")
                    session = subprocess.run([str(launcher), "-p", "task"], env=env, check=True, capture_output=True, text=True)
                    self.assertEqual(session.stdout.strip(), EXCLUDE_DYNAMIC + " -p task")
                else:
                    self.assertNotIn("agentPaths", settings)
                    self.assertIsNone(daemon.launch_config)

    def test_plain_flag_is_opt_in_and_claude_only(self):
        args = SimpleNamespace(effort="low", claude_flag_exclude_dynamic=False)
        baseline = bench.plain_command("claude", "task", "opus", args, "sid")
        self.assertNotIn(EXCLUDE_DYNAMIC, baseline)
        args.claude_flag_exclude_dynamic = True
        changed = bench.plain_command("claude", "task", "opus", args, "sid")
        self.assertEqual(changed, baseline[:-1] + [EXCLUDE_DYNAMIC, "task"])
        self.assertNotIn(EXCLUDE_DYNAMIC, bench.plain_command("codex", "task", None, args, "sid"))

    def test_launcher_distinguishes_parent_worker_and_discovery(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            config = root / "launch.json"
            audit = root / "audit.jsonl"
            config.write_text(json.dumps(dict(binary="/real/claude", parent="parent-session",
                                              exclude_dynamic=True, worker_cache_ttl="5m", audit=str(audit))))
            for session, arguments, role, ttl in (
                    ("parent-session", ["-p", "task"], "parent", "1h"),
                    ("worker-session", ["-p", "task"], "worker", "5m"),
                    ("worker-session", ["--print", "task"], "worker", "5m"),
                    ("worker-session", ["--version"], None, "1h"),
                    (None, ["--version"], None, "1h")):
                with self.subTest(session=session, arguments=arguments):
                    env = {"CLAUDE_CODE_PROMPT_CACHE_TTL": "1h"}
                    if session:
                        env["BRIGADIER_SESSION"] = session
                    with patch.dict(os.environ, env, clear=True), patch("claude_launch.os.execve") as execute:
                        launch(config, arguments)
                    binary, argv, actual_env = execute.call_args.args
                    self.assertEqual(binary, "/real/claude")
                    self.assertEqual(argv, [binary] + ([EXCLUDE_DYNAMIC] if role else []) + arguments)
                    self.assertEqual(actual_env["CLAUDE_CODE_PROMPT_CACHE_TTL"], ttl)
            rows = [json.loads(line) for line in audit.read_text().splitlines()]
            self.assertEqual([r["role"] for r in rows], ["parent", "worker", "worker"])
            self.assertEqual([r["cache_ttl_override"] for r in rows], [None, "5m", "5m"])
            # The independent levers also work separately, without an inherited TTL.
            config.write_text(json.dumps(dict(binary="/real/claude", parent="parent-session",
                                              exclude_dynamic=False, worker_cache_ttl="5m", audit=str(audit))))
            with patch.dict(os.environ, {"BRIGADIER_SESSION": "parent-session"}, clear=True), patch("claude_launch.os.execve") as execute:
                launch(config, ["-p"])
            self.assertNotIn("CLAUDE_CODE_PROMPT_CACHE_TTL", execute.call_args.args[2])
            self.assertEqual(execute.call_args.args[1], ["/real/claude", "-p"])

    def test_launcher_audits_the_actual_standing_text_and_environment(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            config, audit = root / "launch.json", root / "audit.jsonl"
            config.write_text(json.dumps(dict(binary="/real/claude", parent="parent-session",
                                              exclude_dynamic=True, worker_cache_ttl=None, audit=str(audit))))
            standing = "$BRIGADIER_SESSION_DIR $BRIGADIER_LEDGER"
            env = dict(BRIGADIER_SESSION="parent-session", BRIGADIER_SESSION_DIR="/session", BRIGADIER_LEDGER="/session/record/ledger.md")
            with patch.dict(os.environ, env, clear=True), patch("claude_launch.os.execve"):
                launch(config, ["-p", "--append-system-prompt", standing, "task"])
            row = json.loads(audit.read_text())
            self.assertEqual(row["standing_sha256"], hashlib.sha256(standing.encode()).hexdigest())
            self.assertTrue(row["standing_placeholders"])
            self.assertTrue(row["standing_env_present"])

    def test_option_defaults_metadata_and_invalid_ttl(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            base = [sys.executable, "-I", str(bench.HERE / "bench.py"), "--skip", "--arms", "A,W", "--output", tmp]
            subprocess.run(base, check=True, capture_output=True)
            report = json.loads((Path(tmp) / "report.json").read_text())
            self.assertFalse(report["claude_flag_exclude_dynamic"])
            self.assertIsNone(report["worker_cache_ttl"])
            self.assertFalse(report["stable_standing"])
            subprocess.run(base + ["--claude-flag-exclude-dynamic", "--worker-cache-ttl", "5m", "--stable-standing"], check=True, capture_output=True)
            report = json.loads((Path(tmp) / "report.json").read_text())
            self.assertTrue(report["claude_flag_exclude_dynamic"])
            self.assertEqual(report["worker_cache_ttl"], "5m")
            self.assertTrue(report["stable_standing"])
            self.assertIn("stable standing = True", (Path(tmp) / "report.md").read_text())
            bad = subprocess.run(base + ["--worker-cache-ttl", "garbage"], capture_output=True, text=True)
            self.assertEqual(bad.returncode, 2)
            self.assertIn("invalid choice", bad.stderr)


class Graders(unittest.TestCase):
    def test_rejects_incomplete_and_accepts_each_implementation(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            for task in TASKS:
                with self.subTest(task=task):
                    repo = Path(tmp) / task
                    fixture(repo, task)
                    self.assertFalse(grade(repo, task)["passed"])
                    if task == "t1":
                        with (repo / "src/text.js").open("a") as f:
                            f.write(r"export const stripColor = s => s.replace(/\x1b\[[0-9;]*m/g, '');" + "\n")
                        with (repo / "src/index.js").open("a") as f:
                            f.write("export {stripColor} from './text.js';\n")
                    if task == "t2":
                        text = repo / "src/text.js"
                        text.write_text(text.read_text().replace("text.split('\\n')", "text.split(/\\r?\\n/)"))
                    if task == "t3":
                        (repo / "src/text.js").rename(repo / "src/strings.js")
                        (repo / "src/index.js").write_text("export {splitLines,upper} from './strings.js';\n")
                    if task == "t4":
                        (repo / "src/cli.js").write_text("import {upper} from './index.js'; let s=''; for await(const c of process.stdin) s+=c; process.stdout.write(upper(s));\n")
                    if task == "t5":
                        (repo / "src/gradient.js").write_text("export const gradient=(a,b,n)=>Array.from({length:n},(_,i)=>n===1?a:a+(b-a)*i/(n-1));\n")
                        (repo / "src/slug.js").write_text("export const slug=s=>s.toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-|-$/g,'');\n")
                        with (repo / "src/index.js").open("a") as f:
                            f.write("export {gradient} from './gradient.js'; export {slug} from './slug.js';\n")
                    if task != "t2":
                        (repo / "test/new.test.js").write_text("import test from 'node:test'; test('feature',()=>{});\n")
                    result = grade(repo, task)
                    self.assertTrue(result["passed"], result["detail"])
                    if task == "t2":
                        (repo / "test/base.test.js").write_text("import test from 'node:test'; test('removed repro',()=>{});\n")
                        self.assertFalse(grade(repo, task)["passed"])


def sample():
    rows = []
    for provider, arms in (("claude", ("A", "CC", "W")), ("codex", ("B", "CX", "W"))):
        for arm in arms:
            rows.append({"provider": provider, "arm": arm, "task": "t1", "run": 1, "status": "complete", "grade": {"passed": True},
                         "wall_s": 10, "raw_tokens": 100, "models": [provider]})
    return {"fixture_version": 1, "effort": "low", "runs": rows}


class Reports(unittest.TestCase):
    def test_normalized_threshold_and_pass_regression(self):
        old = sample()
        new = deepcopy(old)
        new["runs"][1]["wall_s"] = 13
        new["runs"][1]["raw_tokens"] = 130
        new["runs"][1]["grade"]["passed"] = False
        self.assertEqual(len(regressions(new, old)), 3)
        self.assertFalse(regressions(new, None))
        # Slower whole host with the same wrapper ratio is not a regression.
        for row in new["runs"]:
            row["wall_s"] = 30
            row["raw_tokens"] = 300
            row["grade"]["passed"] = True
        self.assertFalse(regressions(new, old))
        new["runs"][1].update(raw_tokens=1000, models=["changed"])
        self.assertFalse(regressions(new, old))
        # A changed model that fails a task is a new baseline too, plain arms included.
        for index in (0, 1):
            new["runs"][index].update(models=["changed"], grade={"passed": False})
        self.assertFalse(regressions(new, old))

    def test_skips_collect_green_missing_arm_is_harness_failure(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as tmp:
            root = Path(tmp)
            report = dict(sample(), schema_version=1, source="sha", created_at="now", requested_models={}, standing_instruction_bytes=1990)
            for row in report["runs"]:
                row.update(status="skipped", reason="needs the user: secrets")
            for provider in ("claude", "codex"):
                folder = root / f"bench-{provider}"
                folder.mkdir()
                (folder / "report.json").write_text(json.dumps(dict(report, runs=[r for r in report["runs"] if r["provider"] == provider])))
            result = collect(root, root / "out", None)
            self.assertEqual(len(result["runs"]), 6)
            self.assertFalse(result["regressions"])
            self.assertTrue((root / "out/report.md").is_file())
            (root / "bench-codex/report.json").unlink()
            with self.assertRaises(RuntimeError):
                collect(root, root / "out", None)

    def test_regression_only_updates_one_issue(self):
        report = dict(sample(), run_url="https://example.test/run", regressions=["claude CC t1: wall_s 10 → 13"])
        with patch('nightly.subprocess.check_output', return_value='[{"title":"Bench regression","number":42}]'), patch('nightly.subprocess.run') as call:
            issue(report)
            self.assertEqual(call.call_args.args[0][:4], ["gh", "issue", "edit", "42"])
        with patch('nightly.subprocess.run') as call:
            issue(dict(report, regressions=[]))
            call.assert_not_called()


if __name__ == "__main__":
    unittest.main()
