#!/usr/bin/env python3
"""Every "I can't"-like sentence an arm's models wrote to anyone: the assistant text of each
session in the arm's manifest (Claude transcripts with their subagents, Codex rollouts), matched
case-insensitively. Each hit is printed with its session and time for a judgement: it counts as an
"I can't" when the model declined or skipped something a shell could do (THREAD-PLAN §1).
usage: [KEEP=<arm>/transcripts] cant.py <manifest.json> [--json out]"""
import glob, json, os, re, sys

H = os.path.expanduser("~"); KEEP = os.environ.get("KEEP")
man = json.load(open(sys.argv[1])); man = man["sessions"] if isinstance(man, dict) else man
PAT = re.compile(r"[^.\n]*\b(I can(?:no|')t|I can not|I(?:'m| am) (?:not able|unable)|unable to|not able to|"
                 r"can(?:no|')t (?:run|open|start|launch|see|access|check|verify|test|click|drive)|"
                 r"couldn(?:'|’)t (?:run|open|start|launch|verify|test)|no way to)\b[^.\n]*", re.I)


def files(s):
    sid = s["id"]
    if s["provider"] == "claude":
        fs = (glob.glob(f"{KEEP}/claude/{sid}.jsonl") if KEEP else []) or glob.glob(f"{H}/.claude/projects/*/{sid}.jsonl")
        return fs + [g for f in fs for g in glob.glob(f[:-6] + "/subagents/*.jsonl")]
    return (glob.glob(f"{KEEP}/codex/rollout-*{sid}.jsonl") if KEEP else []) or glob.glob(f"{H}/.codex/sessions/*/*/*/rollout-*{sid}.jsonl")


def texts(path, provider):
    for line in open(path, errors="replace"):
        try: r = json.loads(line)
        except ValueError: continue
        if provider == "claude":
            if r.get("type") != "assistant": continue
            for c in (r.get("message") or {}).get("content") or []:
                if isinstance(c, dict) and c.get("type") == "text": yield r.get("timestamp"), c["text"]
        else:
            p = r.get("payload") or {}
            if p.get("type") == "message" and p.get("role") == "assistant":
                for c in p.get("content") or []:
                    if c.get("type") == "output_text": yield r.get("timestamp"), c["text"]


hits = []
for s in man:
    for f in files(s):
        for ts, text in texts(f, s["provider"]):
            for m in PAT.finditer(text):
                hits.append({"session": s["label"], "id": s["id"], "at": ts, "text": m.group(0).strip()[:300]})
for h in hits: print(f"{h['at']} {h['session'][:40]:40} {h['text']}")
print(f"{len(hits)} candidate sentences in {len(man)} sessions")
if "--json" in sys.argv: json.dump(hits, open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1)
