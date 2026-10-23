#!/usr/bin/env python3
"""Compare the wrapped CLIs with isolated headless Brigadier sessions."""
import argparse
from collections import Counter
import datetime as dt
import errno
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import shlex
import subprocess
import sys
import tempfile
import time
import uuid

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
from metrics import FIELDS, cost, parse, records
from economics import NOTE, combine, historical, mean_usd, ratio, role_table, usd
from tasks import TASKS, VERSION, fixture, grade
from claude_launch import EXCLUDE_DYNAMIC

spec = importlib.util.spec_from_file_location("bench_ipc", ROOT / "tools/live-smoke/smoke.py")
ipc_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ipc_module)
ARMS = {"A": "claude", "B": "codex", "CC": "claude", "CX": "codex", "W": "claude"}


def wait(fn, timeout):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = fn()
        if result:
            return result
        time.sleep(.1)
    raise TimeoutError(f"Timed out after {timeout}s")


def stop(proc):
    if proc and proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(10)


def native_files(provider, sid):
    home = Path(os.environ.get("CLAUDE_CONFIG_DIR", Path.home() / ".claude")) if provider == "claude" else Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    pattern = f"projects/*/{sid}.jsonl" if provider == "claude" else f"sessions/*/*/*/rollout-*{sid}.jsonl"
    return list(home.glob(pattern))


def cleanup_native(path, scratch):
    path = Path(path)
    path.unlink(missing_ok=True)
    sidecar = path.with_suffix("")
    if sidecar.is_dir():
        shutil.rmtree(sidecar)
    marker = re.sub(r"[^A-Za-z0-9]", "-", scratch.name)
    if path.parent.parent.name == "projects" and marker in path.parent.name:
        # The CLI also makes memory/ in this invocation's unique project slug.
        # Never remove a shared project folder or a neighboring session's files.
        if path.parent.is_dir():
            shutil.rmtree(path.parent)
    else:
        try:
            path.parent.rmdir()
        except OSError:
            pass


def remove_empty_project(repo):
    """Removes the Claude project folder of `repo` when it holds only an empty memory/: the
    sessions' CLIs make one there and leave it (#574)."""
    home = Path(os.environ.get("CLAUDE_CONFIG_DIR", Path.home() / ".claude"))
    folder = home / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(repo))
    memory = folder / "memory"
    if folder.is_dir() and [p.name for p in folder.iterdir()] == ["memory"] and memory.is_dir() and not any(memory.iterdir()):
        memory.rmdir()
        folder.rmdir()


def keep(paths, dest):
    """Copies the native transcripts to `dest` (--keep-transcripts) before cleanup removes them."""
    if dest:
        dest.mkdir(parents=True, exist_ok=True)
        for path in paths:
            shutil.copyfile(path, dest / Path(path).name)


def models_file(root, worker_model):
    """The daemon's models-rules file: empty when pinned; with `auto`, a path not yet made, so the
    daemon writes the shipped default there and its advice picks the worker's model and effort."""
    path = root / "models.md"
    if worker_model == "pinned":
        path.write_text("")
    return path


def task_prompt(task, arm, provider, model, args):
    prompt = TASKS[task] + " Run node --test test/*.test.js. Do not push or publish."
    if arm != "W":
        return prompt + " Do the work yourself without workers or subagents. Do not commit."
    if args.worker_model == "auto":
        flags = f"--agent {provider} --permission full-access --local"
        pick = " Give no --model or --effort; take the worker's model and effort from Brigadier's advice."
    else:
        flags = f"--agent {provider} --effort {args.effort} --permission full-access --local" + (f" --model {model}" if model else "")
        pick = ""
    return prompt + f" Delegate all implementation and tests to exactly one worker using brigadier start {flags}.{pick} Wait for that worker using brigadier wait, inspect the result and report it. Keep it in this checkout. Do not delegate to any other worker or subagent."


def worker_start(path):
    """The worker `brigadier start`'s receipt in a parent transcript: what it started with
    ("claude, claude-sonnet-5-5, medium effort") and which rules row picked it, if any."""
    def strings(value):
        if isinstance(value, str):
            yield value
        elif isinstance(value, dict):
            for v in value.values():
                yield from strings(v)
        elif isinstance(value, list):
            for v in value:
                yield from strings(v)
    for row in records(path):
        for text in strings(row):
            started = re.search(r"Started worker \S+ \(([^)]*)\)", text)
            if started:
                picked = re.search(r"^(?:Your|The shipped) row .*$", text, re.M)
                return {"started": started.group(1), "row": picked.group(0) if picked else None}
    return None


def plain_command(provider, prompt, model, args, sid):
    command = (["claude", "-p", "--verbose", "--output-format", "stream-json", "--include-partial-messages",
                "--dangerously-skip-permissions", "--effort", args.effort, "--session-id", sid]
               if provider == "claude" else
               ["codex", "exec", "--json", "--dangerously-bypass-approvals-and-sandbox", "-c", f'model_reasoning_effort="{args.effort}"'])
    if model:
        command += ["--model", model]
    if provider == "claude" and args.claude_flag_exclude_dynamic:
        command += [EXCLUDE_DYNAMIC]
    command += [prompt]
    return command


def plain(repo, provider, prompt, model, args, scratch, kept=None):
    sid = str(uuid.uuid4())
    command = plain_command(provider, prompt, model, args, sid)
    stream, err = scratch / "stdout.jsonl", scratch / "stderr.log"
    env = {k: v for k, v in os.environ.items() if not k.startswith("BRIGADIER_") and k != "CLAUDECODE"}
    proc = None
    start = time.monotonic()
    first = None
    try:
        with stream.open("w") as out, err.open("w") as errors:
            proc = subprocess.Popen(command, cwd=repo, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=errors)
            def ended():
                nonlocal sid, first
                for row in records(stream):
                    if row.get("type") == "thread.started":
                        sid = row["thread_id"]
                    if first is None and (row.get("type") in ("assistant", "item.started", "item.completed") or
                                          row.get("event", {}).get("type") in ("content_block_start", "content_block_delta")):
                        first = time.monotonic() - start
                return proc.poll() is not None
            wait(ended, args.timeout)
        wall = time.monotonic() - start
        paths = wait(lambda: native_files(provider, sid), 10)
        metrics = parse(paths[0], provider, prompt)
        return dict(metrics, wall_s=wall, first_output_s=first, worktree_s=0,
                    worker_usage=None, process_exit=proc.returncode), repo
    finally:
        stop(proc)
        keep(native_files(provider, sid), kept)
        for path in native_files(provider, sid):
            cleanup_native(path, scratch)


class Daemon:
    def __init__(self, args, root, kept=None):
        self.args, self.root, self.keep_to = args, root, kept
        self.data = root / "data"
        self.proc = None
        self.ipc = ipc_module.Ipc(str(self.data))
        self.conv = self.project = None
        self.kept = {}
        self.copied = set()
        self.natives = set()
        self.after = 0
        self.launch_config = None
        self.data.mkdir()
        self.saved = root / "native"
        self.saved.mkdir()

    def call(self, method, **fields):
        return self.ipc.call(dict(method=method, **fields))

    def start(self):
        env = {k: v for k, v in os.environ.items() if not k.startswith("BRIGADIER_") and k != "CLAUDECODE"}
        models = models_file(self.root, self.args.worker_model)
        env.update(BRIGADIER_DATA_DIR=str(self.data), BRIGADIER_MODELS_FILE=str(models),
                   TMPDIR=str(self.root), SHELL=shutil.which("zsh") or "/bin/bash")
        with (self.root / "daemon.log").open("w") as out:
            self.proc = subprocess.Popen([self.args.daemon, "--foreground", "--data-dir", str(self.data)], env=env,
                                         stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT)
        def ready():
            if self.proc.poll() is not None:
                raise RuntimeError("Daemon exited: " + (self.root / "daemon.log").read_text()[-2000:])
            try:
                return self.call("getCatalog")
            except (OSError, RuntimeError):
                return None
        catalog = wait(ready, 60)["catalog"]
        settings = catalog["settings"]
        settings.update(onboarded=True, introSeen=True, defaultPermission="full")
        if self.provider == "claude" and (self.args.claude_flag_exclude_dynamic or self.args.worker_cache_ttl):
            binary = shutil.which("claude")
            if not binary:
                raise RuntimeError("Claude binary missing for cache experiment")
            self.launch_config = self.root / "claude-launch.json"
            self.launch_config.write_text(json.dumps(dict(
                binary=binary, parent=None, exclude_dynamic=self.args.claude_flag_exclude_dynamic,
                worker_cache_ttl=self.args.worker_cache_ttl, audit=str(self.root / "claude-launch.jsonl"))))
            launcher = self.root / "claude-bench"
            command = [sys.executable, "-I", str(HERE / "claude_launch.py"), str(self.launch_config)]
            launcher.write_text("#!/bin/sh\nexec " + shlex.join(command) + ' "$@"\n')
            launcher.chmod(0o700)
            settings["agentPaths"] = [{"provider": "claude", "path": str(launcher)}]
        self.call("updateSettings", settings=settings)

    def save_native(self, provider, path):
        path = Path(path)
        if path.is_file() and str(path) not in self.kept:
            dest = self.saved / f"{provider}-{path.name}"
            # Keep before a worker closes and deletes its CLI transcript. A hard link sees its
            # later writes; on another filesystem a copy, refreshed on every poll, stands in.
            try:
                os.link(path, dest)
            except OSError as error:
                if error.errno != errno.EXDEV:
                    raise
                shutil.copyfile(path, dest)
                self.copied.add(str(path))
            self.kept[str(path)] = (provider, dest)

    def events(self):
        while True:
            events = self.call("eventsSince", afterSeq=self.after, limit=500)["events"]
            for env in events:
                self.after = max(self.after, env["seq"])
                e = env.get("event") or {}
                if not isinstance(e, dict):
                    continue
                if e.get("type") == "cleanupRecorded":
                    artifact = e["artifact"]
                    provider = {"claudeSession": "claude", "codexThread": "codex"}.get(artifact.get("type"))
                    if provider:
                        # Recorded when the session starts, often before its CLI writes the file.
                        self.natives.add((provider, artifact.get("sessionId") or artifact.get("threadId")))
                if e.get("type") == "workerEvent":
                    event = e.get("event") or {}
                    if event.get("type") == "sessionStarted" and event.get("file"):
                        self.save_native(self.provider, event["file"])
            for provider, sid in list(self.natives):
                for path in native_files(provider, sid):
                    self.save_native(provider, path)
                    self.natives.discard((provider, sid))
            for original in self.copied:
                if Path(original).is_file():
                    shutil.copyfile(original, self.kept[original][1])
            if len(events) < 500:
                break

    def logs(self):
        entries, before = [], None
        while True:
            page = self.call("listOrchestratorLog", conversationId=self.conv, before=before, limit=500)["page"]
            entries.extend(page["entries"])
            if not page["hasMore"] or not page["entries"]:
                return sorted(entries, key=lambda e: e["streamSeq"])
            before = min(e["streamSeq"] for e in page["entries"])

    def run(self, repo, arm, provider, model, prompt):
        self.provider = provider
        self.start()
        self.project = self.call("addProject", path=str(repo), name="bench", init=False)["project"]["id"]
        self.call("setFolderTrust", id=self.project, path=None, trusted=True)
        setup = {"type": "session", "repo": str(repo), "environment": {"type": "newWorktree", "base": "main", "branch": None},
                 "permission": "full", "orchestrator": {"provider": provider, "model": model, "effort": self.args.effort}, "planMode": False}
        start = time.monotonic()
        self.conv = self.call("createConversation", kind="session", projectId=self.project, title="Bench", setup=setup)["conversation"]["id"]
        if self.launch_config:
            config = json.loads(self.launch_config.read_text())
            config["parent"] = self.conv
            self.launch_config.write_text(json.dumps(config))
        created = start
        start = time.monotonic()
        outcome = self.call("sendMessage", conversationId=self.conv, text=prompt, attachments=[], mentions=[], steer=False)["outcome"]
        worktree = wait(lambda: (self.ipc.view(self.conv)["conversation"].get("setup") or {}).get("environment", {}).get("path"), 60)
        worktree_s = time.monotonic() - created
        msg = outcome.get("message") or {}
        request = msg.get("requestId") or msg.get("id")
        first = None
        def ended():
            nonlocal first
            self.events()
            view = self.ipc.view(self.conv)
            if first is None and (view.get("orchestratorSteps") or any(m["role"] == "assistant" for m in view["messages"]["messages"])):
                first = time.monotonic() - start
            req = next((r for r in view["requests"] if r["id"] == request), None)
            return view if req and req["state"]["type"] not in ("working", "queued", "deciding") and view["run"] in ("idle", "hibernated", "failed") else None
        view = wait(ended, self.args.timeout)
        wall = time.monotonic() - start
        self.events()
        for entry in self.logs():
            e = entry["entry"]
            event = e.get("event") or {}
            if event.get("type") == "sessionStarted" and event.get("file"):
                self.save_native(e.get("provider", provider), event["file"])
        # The first native session belongs to the parent, not its worker.
        parent_files = [e["entry"]["event"]["file"] for e in self.logs()
                        if (e["entry"].get("event") or {}).get("type") == "sessionStarted" and e["entry"]["event"].get("file")]
        if not parent_files:
            raise RuntimeError("No parent native session recorded")
        parent_files = list(dict.fromkeys(parent_files))
        metrics = parse(self.kept[parent_files[0]][1], provider, prompt)
        roles = {"parent": metrics["economics"]}
        successors = []
        for filename in parent_files[1:]:
            other = parse(self.kept[filename][1], self.kept[filename][0], prompt)
            successors.append(other["economics"])
            metrics["usage"] = dict(Counter(metrics["usage"]) + Counter(other["usage"]))
            for key in ("raw_tokens", "cost_units", "brigadier_commands", "ledger_adds", "injected_messages", "injected_bytes", "api_calls", "tools"):
                metrics[key] += other[key]
            metrics["models"] = sorted(set(metrics["models"] + other["models"]))
        import sqlite3
        with sqlite3.connect(f"file:{self.data}/routing.sqlite?mode=ro", uri=True) as db:
            db.row_factory = sqlite3.Row
            rows = [dict(r) for r in db.execute("SELECT * FROM turn_usage WHERE conversation_id=?", (self.conv,))]
        workers = [r for r in rows if r.get("task_id") is not None]
        worker_usage = {"input": sum(r.get("input", 0) for r in workers),
                        "cache_write": sum(r.get("cache_write", 0) for r in workers),
                        "cache_read": sum(r.get("cached_input", 0) for r in workers),
                        "output": sum(r.get("output", 0) for r in workers)}
        # Commands/injections include each captured child, but token totals use the daemon meter
        # for workers so a worker's provider-native stream is never counted twice.
        worker_models = set()
        child_economics = []
        for original, (p, saved) in self.kept.items():
            if original in parent_files:
                continue
            child = parse(saved, p, prompt)
            worker_models.update(child["models"])
            child_economics.append(child["economics"])
            for key in ("brigadier_commands", "ledger_adds", "injected_messages", "injected_bytes", "api_calls", "tools"):
                metrics[key] += child[key]
        if successors:
            roles["successor"] = combine(successors)
        if workers:
            roles["worker"] = combine(child_economics) if child_economics else historical(worker_usage, [])
        metrics["roles"] = roles
        metrics["economics"] = combine(roles.values())
        metrics["parent_usage"] = dict(metrics["usage"])
        metrics["parent_cost_units"] = metrics["cost_units"]
        metrics["usage"] = dict(Counter(metrics["usage"]) + Counter(worker_usage))
        metrics["raw_tokens"] = sum(metrics["usage"].values())
        metrics["cost_units"] += sum(cost({"input": r["input"], "cache_read": r["cached_input"], "cache_write": r["cache_write"], "output": r["output"]}, r["provider"]) for r in workers)
        meter = {"input": sum(r["input"] for r in rows), "cache_read": sum(r["cached_input"] for r in rows),
                 "cache_write": sum(r["cache_write"] for r in rows), "output": sum(r["output"] for r in rows)}
        metrics.update(wall_s=wall, first_output_s=first, worktree_s=worktree_s,
                       claude_launches=records(self.root / "claude-launch.jsonl") if self.launch_config else [],
                       parent_transcripts=[self.kept[f][1].name for f in parent_files],
                       worker_models=sorted(worker_models),
                       worker_start=next(filter(None, (worker_start(self.kept[f][1]) for f in parent_files)), None),
                       worker_usage=worker_usage, worker_turns=len(workers), daemon_usage=meter,
                       delegated=(len({r["task_id"] for r in workers}) == 1) if arm == "W" else None,
                       request_state=next(r["state"]["type"] for r in view["requests"] if r["id"] == request))
        return metrics, Path(worktree)

    def cleanup(self):
        errors = []
        try:
            if self.proc and self.proc.poll() is None:
                if self.conv:
                    self.call("interrupt", conversationId=self.conv)
                    self.call("delete", ids=[self.conv])
                    wait(lambda: self.conv not in {c["id"] for c in self.call("getCatalog")["catalog"]["conversations"]}, 30)
                if self.project:
                    report = self.call("removeProject", id=self.project, deleteBranches=[], keepBrain=False)["report"]
                    errors.extend(report.get("failures", []))
                self.call("shutdown")
                self.proc.wait(15)
        except Exception as error:
            errors.append(str(error))
        finally:
            stop(self.proc)
            keep([saved for _, saved in self.kept.values()], self.keep_to)
            for original in self.kept:
                cleanup_native(original, self.root)
            remove_empty_project(self.root / "repo")
        if errors:
            raise RuntimeError("Cleanup: " + "; ".join(map(str, errors)))


def prepare(report):
    """Enrich both current and kept reports without changing legacy metering or regression fields."""
    for row in report["runs"]:
        if row["status"] != "complete":
            continue
        if "roles" not in row:
            row["roles"] = {"parent": row.get("economics") or historical(
                row.get("parent_usage", row["usage"]), row.get("models", []))}
            if any(row.get("worker_usage", {}).values()):
                row["roles"]["worker"] = historical(row["worker_usage"], row.get("worker_models", []))
        row["economics"] = combine(row["roles"].values())
        row["list_price_usd"] = row["economics"]["list_price_usd"]
    report["aggregate"] = []
    for provider, arm in sorted({(r["provider"], r["arm"]) for r in report["runs"]}):
        rows = [r for r in report["runs"] if (r["provider"], r["arm"], r["status"]) == (provider, arm, "complete")]
        if not rows:
            continue
        plain = [r for r in report["runs"] if r["provider"] == provider and r["arm"] ==
                 ("A" if provider == "claude" else "B") and r["status"] == "complete"]
        amount, baseline = mean_usd(rows), mean_usd(plain)
        avg = lambda key, rs: sum(r[key] for r in rs) / len(rs) if rs else None
        item = {"provider": provider, "arm": arm, "mean_list_price_usd": amount,
                "list_price_ratio": amount / baseline if amount is not None and baseline else None}
        for key in ("cost_units", "raw_tokens", "wall_s"):
            base = avg(key, plain)
            item[key + "_ratio"] = avg(key, rows) / base if base else None
        report["aggregate"].append(item)
    return report


def markdown(report):
    prepare(report)
    lines = ["# CLI bench", "", "Report only; regressions never gate CI or releases.", "",
             f"Source: `{report['source']}`; fixture v{report['fixture_version']}; effort {report['effort']}; worker model {report.get('worker_model', 'pinned')}", "",
             f"Cache experiment: exclude dynamic = {report.get('claude_flag_exclude_dynamic', False)}; worker TTL override = {report.get('worker_cache_ttl') or 'CLI default'}", "",
             "| Arm | Task/run | Status | Pass | Wall s | First output s | Worktree s | API calls | Input / cache write / cache read / output | Cost units | CLI commands | Ledger adds | Injected msgs / bytes |",
             "|---|---|---|---|---:|---:|---:|---:|---|---:|---:|---:|---|"]
    for r in report["runs"]:
        label = r["provider"] + " " + r["arm"]
        if r["status"] != "complete":
            lines.append(f"| {label} | — | {r['status']}: {r.get('reason', '')} | — | — | — | — | — | — | — | — | — | — |")
            continue
        number = lambda k: f"{r[k]:.2f}" if r.get(k) is not None else "unavailable"
        tokens = " / ".join(str(r["usage"].get(k, 0)) for k in FIELDS)
        lines.append(f"| {label} | {r['task']}/{r['run']} | complete | {r['grade']['passed']} | {number('wall_s')} | {number('first_output_s')} | {number('worktree_s')} | {r['api_calls']} | {tokens} | {number('cost_units')} | {r['brigadier_commands']} | {r['ledger_adds']} | {r['injected_messages']} / {r['injected_bytes']} |")
    lines += ["", "## Aggregate", "", "| Arm | Passed/runs | Mean wall s | Mean raw tokens | Mean cost units | Wall / raw / units vs plain | Mean list-price USD | USD vs plain |", "|---|---:|---:|---:|---:|---|---:|---:|"]
    for provider, arm in sorted({(r["provider"], r["arm"]) for r in report["runs"]}):
        rows = [r for r in report["runs"] if r["provider"] == provider and r["arm"] == arm and r["status"] == "complete"]
        if rows:
            avg = lambda k: sum(r[k] for r in rows) / len(rows)
            aggregate = next(a for a in report["aggregate"] if (a["provider"], a["arm"]) == (provider, arm))
            ratios = " / ".join(ratio(aggregate[k + "_ratio"]) for k in ("wall_s", "raw_tokens", "cost_units"))
            lines.append(f"| {provider} {arm} | {sum(r['grade']['passed'] for r in rows)}/{len(rows)} | {avg('wall_s'):.2f} | {avg('raw_tokens'):.0f} | {avg('cost_units'):.2f} | {ratios} | {usd(aggregate['mean_list_price_usd'])} | {ratio(aggregate['list_price_ratio'])} |")
    lines += ["", "## Cache economics by role", "", NOTE, ""]
    for row in report["runs"]:
        if row["status"] != "complete":
            continue
        lines += [f"### {row['provider']} {row['arm']} {row['task']}/{row['run']}", ""]
        lines += role_table(row["roles"])
        lines += ["", "Total list-price USD: " + usd(row["list_price_usd"]), ""]
        notes = row["economics"].get("notes", [])
        if notes:
            lines += ["; ".join(notes), ""]
    workers = [r for r in report["runs"] if r["arm"] == "W" and r["status"] == "complete"]
    if workers:
        lines += ["", "## Workers", "", "| Task/run | Started as | Rules row | Transcript models |", "|---|---|---|---|"]
        for r in workers:
            start = r.get("worker_start") or {}
            lines.append(f"| {r['task']}/{r['run']} | {start.get('started', 'unseen')} | {start.get('row') or '—'} | {', '.join(r.get('worker_models', [])) or 'unseen'} |")
    lines += ["", "W totals include worker turns from turn_usage; parent usage is also retained in JSON.",
              "Injected messages count transcript-visible user/developer text other than the typed prompt and tool results. Provider-private system prompts are unavailable; standing instruction bytes are recorded separately in JSON.", ""]
    return "\n".join(lines)


def write(report, output):
    prepare(report)
    output.mkdir(parents=True, exist_ok=True)
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    (output / "report.md").write_text(markdown(report))


def run_main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--arms", default="A,B,CC,CX,W")
    p.add_argument("--tasks", default=",".join(TASKS))
    p.add_argument("--runs", type=int, default=2)
    p.add_argument("--output", type=Path, default=Path("bench-report"))
    p.add_argument("--daemon", default=str(ROOT / "target/debug/brigadierd"))
    p.add_argument("--claude-model", default="opus")
    p.add_argument("--codex-model", default=None)
    p.add_argument("--worker-provider", choices=["claude", "codex"], default="claude")
    p.add_argument("--effort", choices=["low", "medium", "high"], default="low")
    p.add_argument("--worker-model", choices=["pinned", "auto"], default="pinned",
                   help="auto: W's worker gets no --model/--effort and the daemon has the shipped rules file, so Brigadier's advice picks them")
    p.add_argument("--claude-flag-exclude-dynamic", action="store_true",
                   help="Report-only experiment: move dynamic Claude system sections to the first user message, for parents and workers")
    p.add_argument("--worker-cache-ttl", choices=["5m", "1h"], default=None,
                   help="Report-only experiment: override CLAUDE_CODE_PROMPT_CACHE_TTL for Claude workers only")
    p.add_argument("--timeout", type=int, default=600)
    p.add_argument("--scratch-prefix", default="brigadier-bench-" + os.environ.get("GITHUB_RUN_ID", "local") + "-", help="Unique run/worker prefix for /tmp scratch")
    p.add_argument("--keep-transcripts", type=Path, default=None, help="Copy each run's native transcripts to DIR/<arm>-<task>-<run>/")
    p.add_argument("--skip", action="store_true", help="Write explicit skipped rows without calling a CLI or daemon")
    args = p.parse_args()
    arms, tasks = args.arms.split(","), args.tasks.split(",")
    if not set(arms) <= ARMS.keys() or not set(tasks) <= TASKS.keys() or args.runs < 1:
        p.error("Unknown arm/task, or runs < 1")
    args.daemon = str(Path(args.daemon).resolve())
    report = {"schema_version": 1, "fixture_version": VERSION,
              "source": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "created_at": dt.datetime.now(dt.timezone.utc).isoformat(), "effort": args.effort,
              "requested_models": {"claude": args.claude_model, "codex": args.codex_model},
              "worker_model": args.worker_model,
              "claude_flag_exclude_dynamic": args.claude_flag_exclude_dynamic,
              "worker_cache_ttl": args.worker_cache_ttl,
              "standing_instruction_bytes": (ROOT / "crates/core/src/manager/standing_instructions.md").stat().st_size,
              "runs": []}
    if args.skip:
        report["runs"] = [{"arm": arm, "provider": args.worker_provider if arm == "W" else ARMS[arm], "status": "skipped", "reason": "needs the user: secrets"} for arm in arms]
        write(report, args.output)
        return
    errors = []
    for task in tasks:
        for run in range(1, args.runs + 1):
            for arm in arms:
                provider = args.worker_provider if arm == "W" else ARMS[arm]
                model = args.claude_model if provider == "claude" else args.codex_model
                row = {"arm": arm, "provider": provider, "task": task, "run": run, "status": "incomplete"}
                print(f"{arm} {task}/{run}", flush=True)
                # The prefix is unique to the invoking session on shared hosts.
                with tempfile.TemporaryDirectory(prefix=args.scratch_prefix, dir="/tmp") as tmp:
                    scratch = Path(tmp).resolve()
                    repo = scratch / "repo"
                    daemon = None
                    kept = args.keep_transcripts and args.keep_transcripts.resolve() / f"{arm}-{task}-{run}"
                    try:
                        fixture(repo, task)
                        prompt = task_prompt(task, arm, provider, model, args)
                        if arm in ("A", "B"):
                            result, checkout = plain(repo, provider, prompt, model, args, scratch, kept)
                        else:
                            daemon = Daemon(args, scratch, kept)
                            result, checkout = daemon.run(repo, arm, provider, model, prompt)
                        row.update(result, grade=grade(checkout, task), status="complete")
                        if arm == "W" and not row["delegated"]:
                            row["grade"]["passed"] = False
                            row["grade"]["delegation"] = "Expected exactly one metered worker"
                    except Exception as error:
                        row.update(status="incomplete", reason=str(error))
                        if daemon and daemon.conv:
                            try:
                                row["last_state"] = daemon.ipc.view(daemon.conv)["run"]
                            except Exception:
                                pass
                        errors.append(f"{arm} {task}/{run}: {error}")
                    finally:
                        if daemon:
                            try:
                                daemon.cleanup()
                            except Exception as error:
                                row.update(status="incomplete", reason=str(error))
                                errors.append(str(error))
                    report["runs"].append(row)
                    write(report, args.output)
    if errors:
        raise SystemExit("Harness broke: " + "; ".join(errors))


def main():
    # Existing credentials only. No login, writes to the real CLI home, or shared trust edits.
    prefix = "brigadier-bench-" + os.environ.get("GITHUB_RUN_ID", "local") + "-codex-"
    for i, arg in enumerate(sys.argv[:-1]):
        if arg == "--scratch-prefix":
            prefix = sys.argv[i + 1] + "codex-"
    if "--skip" in sys.argv:
        run_main()
        return
    source = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    with tempfile.TemporaryDirectory(prefix=prefix, dir="/tmp") as tmp:
        home = Path(tmp)
        for name in ("auth.json", "config.toml", "AGENTS.md"):
            if (source / name).is_file():
                shutil.copyfile(source / name, home / name)
                (home / name).chmod(0o600)
        if (source / "skills").is_dir():
            shutil.copytree(source / "skills", home / "skills", symlinks=False)
        old = os.environ.get("CODEX_HOME")
        os.environ["CODEX_HOME"] = str(home)
        try:
            run_main()
        finally:
            if old is None:
                os.environ.pop("CODEX_HOME", None)
            else:
                os.environ["CODEX_HOME"] = old


if __name__ == "__main__":
    main()
