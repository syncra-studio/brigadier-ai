# Stop-hook probe (BUILD-PLAN E2, PLAN §5), 2026-10-16

What the Claude worker's `Stop` hook relies on, checked against the real CLI before it ships.

## How

```
tools/ab/probe_stop_hook.sh /tmp/20261015-0226-rebuild-wow-w48-stopprobe
```

- One `claude -p "Reply with the single word ready." --model sonnet --settings <settings.json>`, Claude Code 2.1.301.
- The `--settings` file adds one `Stop` command hook. On the first stop it prints
  `{"decision":"block","reason":"Before you stop: reply with the single word PINEAPPLE on a line of its own."}`.
  On the second it prints nothing, so the turn ends.
- Every stop event the hook gets is logged.

## What it showed

| | first stop | second stop |
|---|---|---|
| `stop_hook_active` | `false` | `true` |
| `background_tasks` key | present (`[]`) | present (`[]`) |

- **The CLI continues with the reason.** The final result is `PINEAPPLE`, with `num_turns` 2 and `is_error` false.
- **`stop_hook_active` is true on the stop that follows a block.** So the hook can block once per stop. Brigadier's
  `stop_decision` returns no block when the flag is set.
- **The event's keys:** `background_tasks`, `cwd`, `effort`, `hook_event_name`, `last_assistant_message`,
  `permission_mode`, `prompt_id`, `session_crons`, `session_id`, `stop_hook_active` and `transcript_path`.
- **Cost:** 2 model calls, about $0.13 in total, 3.1 s.

## Conclusion

The design in `crates/daemon/src/hook.rs` (`stop_answer`) holds as written:

- it blocks with a reason;
- it answers nothing when `stop_hook_active` is set;
- Brigadier's own state decides whether a turn may end (a report, the outline's go-ahead, its own review, a background
  job or an open question).

No change needed.
