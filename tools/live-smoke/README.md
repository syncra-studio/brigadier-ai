# tools/live-smoke: the thread with the real CLIs, once before a merge

The flow tests drive the thread with scripted CLIs, so they never meet what the real Claude and
Codex CLIs do: a background job the CLI kills when its turn ends, a follow-up stuck behind a
long foreground command, a tool name with a server prefix the engine didn't strip. This smoke
runs the integrated tree's own daemon with the real CLIs, headless, and reads every verdict from
the daemon's view.

    tools/live-smoke/run.sh                 # builds brigadierd (debug), then runs smoke.py
    tools/full-checks.sh --live             # the full checks, then this, last

It runs **once, on the integrated tree, before a merge**: part of checking once, never in a
worker's own checks and never in the session's background full checks (plain
`tools/full-checks.sh`, which runs on every landing). It spends tokens: about a dozen Claude turns
and five Codex turns, cheap models at low effort, tiny prompts, about 5 minutes.

## What it does

- Starts its own daemon (this tree's `target/debug/brigadierd`) on `/tmp/live-smoke-<time>-<pid>/data`
  with a clean environment, onboarded, Fable hidden, reviews and Brain enrichment off.
- Two throwaway git projects; sessions C1, C2 and C3 (Claude, `sonnet` low) in the first, X
  (Codex, `gpt-6-luna` low) in the second, all Full access in the project's own checkout. A model the
  CLI doesn't offer falls back to its default (`--claude-model`, `--codex-model`).
- Skipped with a note (exit 0) when a CLI isn't installed or signed in.

## Scenarios and their checks

Each prompt asks for a fresh code, or a command prints one from `/dev/urandom` into the
project's `.smoke/` folder, so "answered" means an assistant message carries it.

1. **Long command** (C1). A foreground `sleep 70 && …` command; once its step runs, a follow-up
   and then a steer. The follow-up isn't Deciding for more than 30 s; the steer goes into the
   running request, its state says it is read when the command ends (`heldBy` on the steered
   request or its queued message, naming the command, then cleared), and it is
   answered; the follow-up is answered; the command's real output reaches the user; every
   request ends.
2. **Background job** (C2, alongside 1). The thread starts `sleep 70 && …` as a background job
   and ends its turn; 5 s later the smoke hibernates the session, as the idle timer would (its
   CLI goes; a refusal is fine too). The turn ended before the job finished; the job finished
   anyway (its output file appeared); its output reaches the user afterwards, told in the
   request's own block; the request ends; and after its CLI went, the next request is answered in
   its own block.
3. **Messages with a wait.** a) C3 → X with X idle; b) X → C1 while C1 runs an unrelated
   command. The sender sent one message, it is replied, a `wait_for_replies` ran and completed,
   exactly one reply came and the wait took it (`taken`), the sender spent exactly one turn
   (`turn_usage`), its answer carries the reply; the receiver got it once and replied once; the
   reply carries its answer, and in b) leaves out the receiver's unrelated output.
4. **Stop during a wait.** X runs its own user's `sleep 30`; C3 messages it and waits, and is
   stopped while its wait runs. The request is stopped, its wait failed, the late reply arrives
   once, after the Stop, and shows once, and no turn or request starts for it.
5. **Final answers.** Every request family (a request and those steered into it) that ended
   done ends on an assistant message that carries one of its codes: never a narration line,
   empty, or `[quiet]`. Stopped requests are checked in 4 instead.

## Report and exit

One PASS/FAIL line per check, then a summary with every failure. Exit 0: all passed (or
skipped); 1: a check failed; 2: the run broke (the daemon didn't start, a 660 s watchdog).
`--keep-report PATH` also writes the results as JSON and keeps the daemon's log beside it.

## What it leaves behind: nothing

Always, after a failure or the watchdog too: Stop and IPC `delete` of its sessions (which
removes the Claude session dirs and Codex threads they made), an orderly daemon shutdown, then
its process group killed. It then checks the exact paths it made: each session's Claude dir
under `~/.claude/projects` (named from the session's own folder), each Codex thread's rollout
(deleted with the CLI's `thread/delete` if the daemon missed it), and removes its `/tmp` root.
Anything it had to remove itself is reported as a cleanup note.
