#!/usr/bin/env python3
"""BUILD-PLAN Phase X, research A10: the thread's rebirth at 200k and 300k against today's 400k on a
1M window, in one real long session per threshold (`battery/a10.md`): V2's thread and lookup
batteries, V1's completion and usage through settlement, and a late-session quality probe.

The threshold is the debug daemon's `BRIGADIER_REBIRTH_TOKENS` (a release build ignores it; the
400k arm is production's 400k/450k exactly). Nothing else is overridden: the briefing is the real
one. Every arm of a comparison runs on one root, so one daemon binary, one source commit, one
template and the same ballast files.

  a10.py template --root R [--tag T] [--file-tokens 20000] [--smoke-file-tokens 6000]
                                         (BRIGD, BRIG_SRC from the environment; Claude only)
  a10.py run --root R --threshold 200000 [--n 1] [--tag T] [--expect-account EMAIL]
             [--expect-claude VERSION] [--smoke] [--only r1,r4,...]
  a10.py score RUN_DIR                   (rescored from the run directory alone)
  run.py cleanup --root R [--export DIR] (stops, deletes its Claude folders and the root)

An arm: the template restored, a daemon with the threshold, one warm-up session (V3's), then the
battery's steps in order. After each step settles, a context at or past the threshold waits for its
rebirth to be swapped in before the next step, so every arm is reborn right after the step that
crossed it. Before `p1` (the memory-only probe) the arm is stopped and its data dir and the thread's
Claude transcripts are copied; after `p1` they are put back, so `p2` (the lookup probe) starts from
the same state without `p1`'s answers in what it can search. `p1`'s usage and transcripts are kept
apart (`p1/`).

`--smoke` proves the plumbing on a small budget (small ballast files, a low threshold, `--only`); it
is never a measurement. Nothing here touches the installed app, its daemon or its data."""
import argparse, glob, importlib.util, json, os, random, re, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("v2run", os.path.join(HERE, "run.py"))
v2 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(v2)  # the V2 harness: arm, template, freeze
from v2lib import load_battery  # noqa: E402

v3run, req, log, now_ms = v2.v3run, v2.req, v2.log, v2.now_ms
v3lib = v2.v3lib
WORDS = ("amber basil cedar delta ember fable garnet harbor indigo juniper kestrel lumen meadow nectar "
         "onyx pepper quartz raven saffron tundra umber velvet willow xenon yarrow zephyr").split()
BALLAST_FILES = 44      # two per ballast step, b01-b22
CHARS_PER_TOKEN = 2.07  # these lines, as the Read tool returns them (the smoke measured 2.07)
NOTE_WAIT_S = 1800
NOTE_WRITTEN = "handoff note written"


def check_word(i):
    return f"CHECK-{i:02d}-{random.Random(7000 + i).randrange(16 ** 6):06x}"


def write_ballast(d, count, tokens):
    """`d/01.txt` …: about `tokens` tokens each, its CHECK word on its last line; a MANIFEST of
    their hashes."""
    os.makedirs(d, exist_ok=True)
    manifest = {}
    for i in range(1, count + 1):
        rnd = random.Random(9000 + i)
        lines, size, n = [], 0, 0
        while size < tokens * CHARS_PER_TOKEN:
            n += 1
            line = f"ballast {i:02d} line {n:04d}: " + " ".join(rnd.choice(WORDS) for _ in range(12))
            lines.append(line); size += len(line) + 1
        lines.append(check_word(i))
        path = os.path.join(d, f"{i:02d}.txt")
        open(path, "w").write("\n".join(lines) + "\n")
        manifest[f"{i:02d}.txt"] = {"sha256": v3run.sha256(path), "lines": len(lines), "bytes": os.path.getsize(path)}
    json.dump({"tokens_target": tokens, "files": manifest}, open(os.path.join(d, "MANIFEST.json"), "w"), indent=1, sort_keys=True)


def cmd_template(a):
    v2.template(argparse.Namespace(root=a.root, tag=a.tag, provider="claude"))
    root = v3run.check_root(a.root)
    write_ballast(os.path.join(root, "ballast"), BALLAST_FILES, a.file_tokens)
    write_ballast(os.path.join(root, "ballast-smoke"), BALLAST_FILES, a.smoke_file_tokens)
    log("ballast ready")


# ---------------------------------------------------------------- the thread's context and rebirths

def context_now(data, conv):
    """The thread's last reported context (used, window) and the daemon's thresholds for it."""
    r = req(data, {"method": "listOrchestratorLog", "conversationId": conv, "before": None, "limit": 200})
    page = r["value"]["page"]
    used = window = None
    for e in sorted(page["entries"], key=lambda e: e["streamSeq"]):
        ev = (e["entry"].get("event") if e["entry"].get("type") == "provider" else None) or {}
        if ev.get("type") == "contextSize":
            used, window = ev.get("usedTokens"), ev.get("windowTokens") or window
    return used, window, page.get("thresholds")


def rebirths(data, conv):
    return [e["entry"]["record"] for e in v2.orchestrator_log(data, conv) if e["entry"].get("type") == "rebirth"]


def notes_written(run):
    """How many handoff notes the arm's daemon has written (its log, across restarts)."""
    try:
        return sum(1 for line in open(os.path.join(run, "daemon.log"), errors="replace") if NOTE_WRITTEN in line)
    except OSError:
        return 0


def wait_note(run, arm, conv, deadline):
    """Until the rebirth prepared after this step has its note: the daemon swaps the thread at the
    start of the next turn once the note is ready, so the next step then runs in the new session.
    Returns whether a note is ready."""
    while time.time() < deadline:
        if notes_written(run) > len(rebirths(arm.data, conv)):
            v3run.settle(arm.events, conv, deadline)
            return True
        time.sleep(5)
    return False


# ---------------------------------------------------------------- the copy before p1

def claude_dirs(root):
    """The Claude project folders of the arm's sessions (as `run.py cleanup` finds them)."""
    out = []
    for v in {root, root[len("/private"):] if root.startswith("/private/") else root}:
        slug = re.sub(r"[^A-Za-z0-9]", "-", v)
        assert "-tmp-" in slug
        out += glob.glob(os.path.join(v3run.HOME, ".claude", "projects", glob.escape(slug + "-arm-") + "*"))
    return sorted(set(out))


def clone(src, dst):
    """An APFS clone of the tree `src` at `dst`, its parent made first."""
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    v3run.clone_tree(src, dst)


def snapshot(root, dest):
    """The arm's data dir and its Claude project folders, cloned into `dest` (the arm stopped)."""
    clone(os.path.join(root, "arm", "data"), os.path.join(dest, "data"))
    folders = {}
    for i, d in enumerate(claude_dirs(root)):
        clone(d, os.path.join(dest, "claude", f"{i:02d}"))
        folders[d] = f"{i:02d}"
    json.dump(folders, open(os.path.join(dest, "claude.json"), "w"), indent=1)


def put_back(root, src):
    """The arm as `snapshot` left it: its data dir, and its Claude project folders (folders made
    since are removed)."""
    data = os.path.join(root, "arm", "data")
    v3run.safe_rmtree(root, data)
    clone(os.path.join(src, "data"), data)
    folders = json.load(open(os.path.join(src, "claude.json")))
    for d in claude_dirs(root):
        shutil.rmtree(d)
    for d, name in folders.items():
        clone(os.path.join(src, "claude", name), d)


def relink_transcripts(run):
    """After `put_back`: the run's hard-linked transcripts (the recorder links each once) point at
    the files `p1` grew, so each is linked again to the file put back; one `p1` alone made moves
    to `p1/transcripts/`."""
    keep = os.path.join(run, "transcripts", "claude")
    out = []
    for dirpath, _, names in os.walk(keep):
        for name in names:
            dst = os.path.join(dirpath, name)
            rel = os.path.relpath(dst, keep)
            live = glob.glob(os.path.join(v3run.HOME, ".claude", "projects", "*", glob.escape(rel)))
            if live and os.stat(live[0]).st_ino != os.stat(dst).st_ino:
                os.remove(dst)
                os.link(live[0], dst)
                out.append(rel)
            elif not live:
                moved = os.path.join(run, "p1", "transcripts", rel)
                os.makedirs(os.path.dirname(moved), exist_ok=True)
                os.replace(dst, moved)
                out.append(f"{rel} (p1 only)")
    return out


def keep_p1(root, run, arm_data):
    """`p1`'s own evidence before the copy is put back: the usage store and the thread's transcripts."""
    out = v3run.ensure(os.path.join(run, "p1"))
    v3run.backup(os.path.join(arm_data, "routing.sqlite"), os.path.join(out, "routing.sqlite"))
    for i, d in enumerate(claude_dirs(root)):
        clone(d, os.path.join(out, "claude", f"{i:02d}"))


# ---------------------------------------------------------------- one arm

def account():
    try:
        return (json.load(open(os.path.join(v3run.HOME, ".claude.json"))).get("oauthAccount") or {}).get("emailAddress")
    except (OSError, ValueError):
        return None


def claude_version():
    return subprocess.run(["claude", "--version"], capture_output=True, text=True).stdout.strip()


def ballast_text(text, root, d, k):
    files = [os.path.join(root, d, f"{2 * k - 1:02d}.txt"), os.path.join(root, d, f"{2 * k:02d}.txt")]
    return text.replace("{files}", " and ".join(files)), files


def run_arm(a):
    root = v3run.check_root(a.root)
    b = load_battery("a10")
    h = b["harness"]
    build = json.load(open(os.path.join(root, "build.json")))
    label = a.tag or build["label"]
    smoke = "-smoke" if a.smoke else ""
    rid_ = f"a10-{a.threshold // 1000}k-{label}{smoke}-{a.n}"
    run = os.path.join(root, "runs", rid_)
    if os.path.exists(os.path.join(run, "score.json")):
        log(rid_, "scored already"); return
    acct, version = account(), claude_version()
    if a.expect_account and acct != a.expect_account:
        sys.exit(f"a10: Claude is signed in as {acct}, not {a.expect_account}")
    if a.expect_claude and not version.startswith(a.expect_claude):
        sys.exit(f"a10: claude --version is {version!r}, not {a.expect_claude}")
    if os.path.exists(run):
        v3run.safe_rmtree(root, run)
    os.makedirs(run)
    shutil.copyfile(b["path"], os.path.join(run, "battery.md"))
    steps = [s for s in b["steps"] if not a.only or s["id"] in a.only.split(",")]
    ballast_dir = "ballast-smoke" if a.smoke else "ballast"
    v3run.restore(root, "seeded")
    debug_env = f"BRIGADIER_REBIRTH_TOKENS={a.threshold}"
    conditions = subprocess.run(["bash", "-c", "sysctl -n vm.loadavg; claude --version; codex --version; node --version"],
                                capture_output=True, text=True).stdout.split("\n")
    rec_dir = os.path.join(run, "rec")
    arm = v2.Arm(root, rec_dir, run, debug_env)
    start = {"arm": "brigadier", "battery": b["name"], "seam": "size", "provider": "claude", "n": a.n, "build": build,
             "label": label, "threshold": a.threshold, "smoke": a.smoke, "only": a.only, "debug_env": debug_env,
             "account": acct, "claude_version": version, "conditions_start": conditions, "started_ms": now_ms(),
             "battery_sha256": v3run.sha256(b["path"]), "ballast": json.load(open(os.path.join(root, ballast_dir, "MANIFEST.json"))),
             "steps": [], "requests": [], "sent_ms": [], "ended_ms": [], "contexts": [], "ballast_files": {},
             "seams": [], "feed": {}, "planted_learned": None, "rebirth_waits": []}
    save = lambda: json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
    try:
        time.sleep(5)
        wconv = v2.create_conversation(arm.data, arm.repo, "claude")
        wt, wrid = v3run.send(arm.data, wconv, v3run.WARMUP_TEXT)
        wreq = v3run.wait_request(arm.events, wconv, wrid, time.time() + 600)
        v3run.settle(arm.events, wconv, time.time() + 300)
        start["warmup"] = {"conversation": wconv, "sent_ms": wt, "request": wrid, "state": (wreq or {}).get("state"),
                           "ended_ms": (wreq or {}).get("endedAtMs")}
        log(rid_, "warm-up", (wreq or {}).get("state"))
        conv = v2.create_conversation(arm.data, arm.repo, "claude")
        start["conversation"] = conv
        deadline = time.time() + b["timeout"]
        timed_out, k = False, 0
        for i, step in enumerate(steps):
            if step["id"] == h.get("feed", {}).get("copy_before"):
                ws = v2.workspace(arm.events, conv)
                feed = os.path.join(ws, ".import-feed")
                os.makedirs(feed, exist_ok=True)
                for f in sorted(os.listdir(os.path.join(HERE, "fixture", "feed"))):
                    shutil.copyfile(os.path.join(HERE, "fixture", "feed", f), os.path.join(feed, f))
                start["feed"]["copied_ms"] = now_ms(); start["feed"]["path"] = feed
            if step["id"] == h.get("plant_learned", {}).get("before"):
                r = req(arm.data, {"method": "debugPlantLearned", "conversationId": conv, "text": h["plant_learned"]["text"]})
                start["planted_learned"] = {"status": r.get("status"), "at_ms": now_ms()}
            if step["role"] == "probe" and "p2" in [s["id"] for s in steps]:
                arm.close()
                snapshot(root, os.path.join(root, "pre-probe"))
                arm = v2.Arm(root, rec_dir, run, debug_env)
                start["snapshot_ms"] = now_ms()
                time.sleep(5)
            if step["id"] == h.get("restore_before") and os.path.isdir(os.path.join(root, "pre-probe")):
                arm.close()
                keep_p1(root, run, arm.data)
                put_back(root, os.path.join(root, "pre-probe"))
                start["relinked"] = relink_transcripts(run)
                v3run.safe_rmtree(root, os.path.join(root, "pre-probe"))
                arm = v2.Arm(root, rec_dir, run, debug_env)
                start["restored_ms"] = now_ms()
                time.sleep(5)
            text = step["text"]
            if step["role"] == "ballast":
                k += 1
                text, files = ballast_text(text, root, ballast_dir, k)
                start["ballast_files"][step["id"]] = files
            t, rid = v3run.send(arm.data, conv, text + " " + v3lib.SAFETY)
            if i == 0:
                start["t0_ms"] = t
            start["steps"].append(step["id"]); start["requests"].append(rid); start["sent_ms"].append(t)
            save()
            log(rid_, f"{step['id']} sent ({i + 1}/{len(steps)})")
            r = v3run.wait_request(arm.events, conv, rid, deadline)
            if r is None:
                timed_out = True; break
            start["ended_ms"].append(r.get("endedAtMs"))
            if step["id"] == h.get("feed", {}).get("delete_after") and start["feed"].get("path"):
                v3run.settle(arm.events, conv, deadline)
                shutil.rmtree(start["feed"]["path"])
                start["feed"]["deleted_ms"] = now_ms()
            v3run.settle(arm.events, conv, deadline)
            used, window, thresholds = context_now(arm.data, conv)
            start["contexts"].append({"step": step["id"], "used": used, "window": window, "thresholds": thresholds,
                                      "at_ms": now_ms()})
            log(rid_, f"{step['id']} ended:", r["state"]["type"], f"context {used} of {window}")
            prepare = (thresholds or {}).get("prepareTokens")
            if used and prepare and used >= prepare and step["role"] not in ("probe", "lookup"):
                w0 = now_ms()
                ready = wait_note(run, arm, conv, min(deadline, time.time() + NOTE_WAIT_S))
                start["rebirth_waits"].append({"after": step["id"], "used": used, "waited_ms": now_ms() - w0, "note_ready": ready,
                                               "rebirths_before": len(rebirths(arm.data, conv))})
                log(rid_, "note ready" if ready else "NO NOTE", f"after {step['id']}", f"({(now_ms() - w0) // 1000} s)")
            save()
        settled = False if timed_out else v3run.settle(arm.events, conv, deadline + v3run.SETTLE_MAX_S)
        start["timed_out"] = timed_out; start["settled_quietly"] = settled; start["ended_run_ms"] = now_ms()
        save()
        v2.freeze(root, run, arm, conv)
    finally:
        arm.close()
        if os.path.isdir(os.path.join(root, "pre-probe")):
            v3run.safe_rmtree(root, os.path.join(root, "pre-probe"))
    v3run.backup(os.path.join(arm.data, "routing.sqlite"), v3run.ensure(os.path.join(run, "data")) + "/routing.sqlite")
    run_checks(root, run, h)
    v3run.after(run)
    score = json.loads(subprocess.run([sys.executable, os.path.join(HERE, "a10score.py"), run], check=True,
                                      capture_output=True, text=True).stdout)
    json.dump(score, open(os.path.join(run, "score.json"), "w"), indent=1, sort_keys=True)
    with open(os.path.join(root, "results.jsonl"), "a") as f:
        f.write(json.dumps({"run": rid_, "root": root, **{k: score[k] for k in ("threshold", "completed", "valid", "batteries", "quality", "rebirth_count")}},
                           sort_keys=True) + "\n")
    log(rid_, json.dumps({k: score[k] for k in ("completed", "valid", "rebirth_count")}), json.dumps(score["batteries"])[:400])


def run_checks(root, run, harness):
    """Each edit's hidden check, once, on a copy of the session workspace as the run left it: V2's
    checks under `$CHECKS`, V3's under `$V3CHECKS`."""
    out = {}
    for step, cmd in sorted(harness.get("checks", {}).items()):
        result = os.path.join(run, "result")
        if not os.path.isdir(result):
            out[step] = {"command": cmd, "rc": None, "output": "no workspace was recorded"}
            continue
        cdir = os.path.join(run, "checkdir")
        v3run.clone_tree(result, cdir)
        v3run.clone_tree(os.path.join(root, "nm-cache", "node_modules"), os.path.join(cdir, "node_modules"))
        p = subprocess.run(["bash", "-c", cmd], cwd=cdir, capture_output=True, text=True, timeout=600,
                           env=dict(os.environ, CHECKS=os.path.join(HERE, "checks"), V3CHECKS=os.path.join(v2.V3, "checks")))
        text = p.stdout + p.stderr
        m = re.search(r"^RESULT (\{.*\})$", text, re.M)
        out[step] = {"command": cmd, "rc": p.returncode, "result": json.loads(m.group(1)) if m else None, "output": text[-4000:]}
        v3run.safe_rmtree(root, cdir)
    json.dump(out, open(os.path.join(run, "checks.json"), "w"), indent=1)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("template"); t.add_argument("--root", required=True); t.add_argument("--tag")
    t.add_argument("--file-tokens", type=int, default=20_000); t.add_argument("--smoke-file-tokens", type=int, default=6_000)
    r = sub.add_parser("run"); r.add_argument("--root", required=True); r.add_argument("--threshold", type=int, required=True)
    r.add_argument("--n", type=int, default=1); r.add_argument("--tag"); r.add_argument("--smoke", action="store_true")
    r.add_argument("--only"); r.add_argument("--expect-account"); r.add_argument("--expect-claude")
    s = sub.add_parser("score"); s.add_argument("run")
    a = ap.parse_args()
    if a.cmd == "template":
        cmd_template(a)
    elif a.cmd == "run":
        run_arm(a)
    else:
        print(json.dumps(json.loads(subprocess.run([sys.executable, os.path.join(HERE, "a10score.py"), a.run], check=True,
                                                   capture_output=True, text=True).stdout), indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
