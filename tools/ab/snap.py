#!/usr/bin/env python3
"""Keeps every Claude transcript and Codex rollout on this machine that is written after it starts:
hard links in <arm>/snap/{claude,codex}/, checked every 3 s, so audit.py still sees a session a
daemon deleted at task cleanup. Runs from before t0 until it is killed after settlement.
usage: snap.py <arm-dir>"""
import glob, os, sys, time

arm = sys.argv[1]; H = os.path.expanduser("~"); since = time.time() - 5
for d in ("claude", "codex"): os.makedirs(os.path.join(arm, "snap", d), exist_ok=True)
while True:
    pats = [("claude", f"{H}/.claude/projects/*/*.jsonl"), ("claude", f"{H}/.claude/projects/*/*/subagents/*.jsonl"),
            ("codex", f"{H}/.codex/sessions/*/*/*/rollout-*.jsonl")]
    for kind, pat in pats:
        for f in glob.glob(pat):
            try:
                if os.path.getmtime(f) < since: continue
                # The same layout as recorder.py's transcripts/, so tokens.py can read it as KEEP.
                name = os.path.basename(f) if "/subagents/" not in f else os.path.join(
                    os.path.basename(os.path.dirname(os.path.dirname(f))), "subagents", os.path.basename(f))
                dst = os.path.join(arm, "snap", kind, name)
                if not os.path.exists(dst):
                    os.makedirs(os.path.dirname(dst), exist_ok=True); os.link(f, dst)
            except OSError:
                pass
    time.sleep(3)
