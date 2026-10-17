# Resume-interrupted-turn probe (#124), 2026-10-18

Can `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1` resume a Claude session that was killed mid-turn any
better than Brigadier's usual resume? Checked against the real CLI.

## How

```
tools/ab/probe_resume_interrupted.sh /tmp/<scratch> KILL|TERM 0|1 ["Continue."]
```

- Claude Code 2.1.302, `claude -p … --model sonnet --output-format stream-json`.
- The turn has three Bash steps. Step 2 is a 25 s Python sleep, because the CLI moves a foreground
  `sleep` to the background and ends the turn early.
- The probe signals the CLI 3 s into step 2, then resumes the session with `--resume`, with the variable set or not.

## What it showed

| Signal | Variable | Resume prompt | Result |
|---|---|---|---|
| SIGKILL | set | "Continue." | Step 2 rerun, step 3 run, `FINISHED`. 3 turns, $0.011 |
| SIGKILL | unset | "Continue." | Says step 2 "was interrupted and its outcome is unknown", reruns it, runs step 3, `FINISHED`. 3 turns, $0.014 |
| SIGTERM | set | "Continue." | Says step 2 "was killed with exit 137", reruns it, runs step 3, `FINISHED`. 3 turns, $0.087 |
| SIGTERM | unset | "Continue." | The same. 3 turns, $0.087 |
| SIGKILL | set | none | The CLI refuses: "No deferred tool marker found in the resumed session … Provide a prompt to continue the conversation." Nothing runs |

- **With a prompt, the variable changes nothing that matters.** Every resume reran the cut-off step and finished
  the turn. With the variable set after a SIGKILL, the model only skipped its remark about the interruption.
- **Without a prompt, the variable doesn't help.** `-p --resume` with no prompt resumes only a session left at a
  deferred tool, not one cut off by a crash.
- **For Brigadier:** a thread whose CLI ends mid-turn fails that request, and Continue sends a message. Each
  same-session continuation also resumes with a message (`CONTINUE_ON_ACCOUNT` for the thread,
  `CONTINUE_SAME_SESSION` for a worker). So the variable would buy nothing, and Brigadier doesn't set it.
