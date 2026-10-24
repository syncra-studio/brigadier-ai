"""A SessionStart(compact) hook for the probe: prints one labelled slice of filler entries."""
import json, sys
k, n, size = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
head = f"[Brigadier · decisions restored · part {k} of {n}]\n"
end = f"\nSLICE-{k}-END"
body, i = "", 0
while len(head + body + end) < size - 60:
    body += f"L{k}-{i:04d} · filler decision text abcdefghij.\n"
    i += 1
print(json.dumps({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": head + body + end}}))
