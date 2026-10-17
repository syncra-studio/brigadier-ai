# A10: earlier thread rebirth on a 1M window (Phase X, 2026-10-17)

BUILD-PLAN Phase X, research A10. The thread's rebirth at 200k and 300k against today's 400k, on Claude Opus 5.5
(1M window), one real long session per threshold. **Exploratory: n = 1 per threshold.** One run each can't prove a
threshold better; it shows what one session did. Production thresholds are unchanged.

- Harness `tools/ab/v2/a10.py`, battery `tools/ab/v2/battery/a10.md`, at `9bd5b488`. Debug daemon sha256
  `67ed478e…7f4cc32`, with `BRIGADIER_REBIRTH_TOKENS` = the threshold. A release build ignores that variable, and
  the 400k arm is production's 400k/450k exactly. The swap was a quarter above the prepare threshold (250k, 375k,
  450k); the real briefing was used, with no cap.
- One root, template and set of ballast files for all three arms. Account escgamingt.o, Claude Code 2.1.302, one
  identical warm-up before each arm. Run order 400k → 300k → 200k, 11:30–13:55 UTC, run by chore X-1 (Codex w98,
  `msgs/w96-chore-1-report.md`, evidence in `msgs/w96-chore-1-runs/`).
- One session of 50 steps:
  - V2's plants and its first four fillers;
  - an early quality probe at about 80k (V3's q1, q2, e1, e3);
  - six ballast steps of about 40k each (two files read at once);
  - a late quality probe at about 330k in the 400k arm (V3's q3, q4, e2, e4);
  - fifteen more ballast steps, so the 400k arm is reborn twice;
  - V2's last fillers and its two edits, after the seams;
  - the memory-only probe (thread battery) and, from a copy of the state before it, the lookup probe.
- Tokens are the daemon's `turn_usage` for the conversation from t0 to settlement. They cross-check within 0.5%
  against the transcripts counted once per message. `tokens.py`'s `INCOMPLETE` is its double count of the note
  forks' copied history.

| | 400k (today) | 300k | 200k |
|---|---|---|---|
| V2 thread battery (memory only) | 10/10 pass | 10/10 pass | 9/10: the thread-only item said "unknown" (no guess) |
| V2 lookup battery | 10/10 pass | 10/10 pass | 10/10 pass |
| Early quality probe (at ~80k) | 4/4 | 4/4 | 4/4 |
| Late quality probe | 4/4 at 325–334k | 4/4: q3 in the old session at 327k, the rest just after a rebirth (36–52k) | 4/4 at 156–172k |
| Late probe: time / model calls | 93 s / 24 | 60 s / 30 | 64 s / 32 |
| Edits' hidden checks (V2 e1, e2; V3 e1–e4) | 6/6 | 6/6 | 6/6 |
| Rebirths (after step, context) | 2 (b09 414k, b18 405k) | 3 (b07 327k, b14 332k, b21 325k) | 5 (b04 207k, b08 213k, b12 205k, b16 209k, b20 211k) |
| Rebirth time: note ready / prepare to swap / the swap's own wait | 30–40 s / 62–74 s / 1.4–1.5 s | 30–39 s / 61–72 s / 1.3–3.0 s | 29–32 s / 63–93 s / 1.5–2.5 s |
| Briefing tokens | 15k, 20k | 12k, 17k, 19k | 11k–20k |
| Tokens per rebirth (note fork + new session's first cache write) | 483k each | 355k–406k | 272k–290k |
| Peak context | 414k | 332k | 213k |
| **Tokens for the session** (raw; cache reads in brackets) | **21.46M** (20.34M) | **13.85M** (12.72M) | **14.93M** (13.70M) |
| Session length / tokens per hour | 46 min / 28.0M | 44 min / 18.8M | 48 min / 18.5M |
| Loss across a rebirth | none | none | the item only in a tool's output, from memory (found again by search) |

**Recommendation.** In this one session each, rebirth at 300k cost 35% fewer tokens than today's 400k (13.9M
against 21.5M), with the same answers: every planted item recalled after three rebirths, 8/8 quality asks and 6/6
edits. Every call rereads the whole context, so a session kept under 300k pays less on each of its calls, and a rebirth
costs about 400k once. 200k went past the point where that pays: five rebirths at about 280k each cost back
most of the saving (14.9M), and from memory alone it lost the one item that only a tool's output held, though it
said "unknown" and search found it. No arm showed better or worse answers late in the session, so the gain here is
tokens, not quality. My recommendation is to move a Claude thread on a 1M window to rebirth at 300k (swap at 375k,
as run). Because it's n = 1, a second pass of the 300k and 400k arms before the change would be cheap insurance
(about 35M tokens), if you want one. Nothing changes until you say go.
