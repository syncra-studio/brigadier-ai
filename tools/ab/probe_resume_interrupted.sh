#!/bin/bash
# probe_resume_interrupted.sh <scratch-dir> <KILL|TERM> <0|1> [resume prompt] : whether
# CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1 recovers a Claude turn cut off by a crash (#124).
# One `claude -p` (Sonnet) runs three Bash steps, the second a 25 s Python sleep (the CLI moves a
# foreground `sleep` to the background); the probe signals it while that step runs, then resumes the
# session (`--resume`) with the variable set (1) or not (0), with the prompt given, or none.
# It prints the steps done (steps.txt), each run's tool calls and texts, and the resume's result.
# The scratch dir must not exist; the caller removes it afterwards.
set -uo pipefail
D=${1:?scratch dir}; SIG=${2:?KILL or TERM}; ON=${3:?0 or 1}; PROMPT=${4:-}
case "$D" in /tmp/*|/private/tmp/*) ;; *) echo "probe: the scratch dir must be under /tmp" >&2; exit 2 ;; esac
[ -e "$D" ] && { echo "probe: $D exists" >&2; exit 1; }
mkdir -p "$D" && cd "$D" && git init -q .
ID=$(uuidgen | tr 'A-Z' 'a-z')
TASK='Do these three steps in order, each with its own separate Bash tool call, and do not combine them: 1) run `echo one >> steps.txt`; 2) run `python3 -c "import time; time.sleep(25)"`; 3) run `echo three >> steps.txt`. Then reply with the single word FINISHED.'
FLAGS=(--model sonnet --output-format stream-json --verbose --permission-mode bypassPermissions)
claude -p "$TASK" --session-id "$ID" "${FLAGS[@]}" > first.jsonl 2> first.err &
PID=$!
for _ in $(seq 1 120); do grep -q 'time.sleep(25)' first.jsonl 2>/dev/null && break; sleep 0.5; done
sleep 3
kill "-$SIG" "$PID"; wait "$PID" 2>/dev/null
echo "signalled $SIG; steps done: $(tr '\n' ' ' < steps.txt 2>/dev/null)"
[ "$ON" = 1 ] && export CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1
if [ -n "$PROMPT" ]; then
  claude -p "$PROMPT" --resume "$ID" "${FLAGS[@]}" > second.jsonl 2> second.err
else
  claude -p --resume "$ID" "${FLAGS[@]}" < /dev/null > second.jsonl 2> second.err
fi
echo "resume exit $?; steps done: $(tr '\n' ' ' < steps.txt 2>/dev/null)"
python3 -I - <<'PY'
import json
for name in ("first.jsonl", "second.jsonl"):
    seen, result = [], None
    for line in open(name):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if e.get("type") == "assistant":
            for c in e["message"].get("content", []):
                if c.get("type") == "tool_use":
                    seen.append(c["input"].get("command"))
                elif c.get("type") == "text":
                    seen.append("text: " + c["text"][:100])
        elif e.get("type") == "result":
            result = {k: e.get(k) for k in ("subtype", "is_error", "num_turns", "result", "total_cost_usd")}
    print(name, seen, result)
PY
head -3 second.err
