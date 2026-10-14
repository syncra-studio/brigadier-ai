# CLI probes run for this research (2026-10-14)

Claude Code 2.1.299, subscription sign-in (the default account), `claude -p --output-format json --model
claude-opus-5-5 --strict-mcp-config`, in an empty scratch folder. Each row is one call; the usage is from the JSON
result. n = 1 per row, so each is a direction confirmed by a control, not a distribution.

## P1. Which cache lifetime does a `-p` session get on a subscription?

| Call | cache read | cache write | of which 1 h | of which 5 min |
|---|---|---|---|---|
| fresh session, effort medium | 10,938 | 11,456 | 11,456 | 0 |

**Result:** a `-p` session is a "main conversation" and writes with the **1-hour** lifetime, as the prompt-caching
docs say for subscriptions. This confirms the 1-hour basis of Brigadier's cold-cache rebirth (§7 item 3) for
Claude. Brigadier records cache writes but not their lifetime (`ephemeral_1h_input_tokens` /
`ephemeral_5m_input_tokens` are not parsed anywhere under `crates/providers/src`).

## P2. Does an effort change on resume keep the cache (Opus 5.5)?

| Call | cache read | cache write |
|---|---|---|
| A1 fresh, medium | 10,938 | 11,456 |
| A2 resume A1, **low** | 13,031 | 9,402 |
| A3 resume A1, low again | 22,408 | 63 |
| B1 fresh, medium (control) | 22,394 | 0 |
| B2 resume B1, **medium** (control) | 22,408 | 25 |
| C1 fresh, medium, ~112k tokens of filler in the message | 12,565 | 112,313 |
| C2 resume C1, **high** | 115,515 | 9,402 |
| C3 resume C1, high again | 124,892 | 63 |

**Result:**
- A resume at the same effort rewrites nothing worth counting (B2: 25 tokens).
- A resume at a different effort rewrites about **9.4k tokens once, and the same 9.4k whatever the conversation's
  size** (A2 on a ~22k session, C2 on a ~125k session). The conversation itself stays cached (C2 read 115.5k). The
  next call at the new effort is fully cached again (A3, C3).
- So on Opus 5.5 with a subscription, changing effort between turns costs about one 9.4k cache write at the 1-hour
  price (≈ 2 × 9.4k input-equivalents), not a rewrite of the history. This matches the vendor's statement that the
  cache stays intact on 5.5 models, apart from a fixed part that changes with effort.
- Not measured: Codex (its app-server `turn/start` takes `effort`; whether the prefix survives is unknown, see PLAN
  probe list), Sonnet 5.5, Haiku 5.5, or a change of effort inside one turn.

## P3. Shared fixed start across fresh sessions

B1, a fresh session in the same folder with the same flags made within the hour, read its whole 22.4k start from
the cache and wrote 0. This is the same result §7 recorded on 2.1.285, so it still holds on 2.1.299.

Cost of all probes: about 8 small calls and one 112k-token write (the JSON reported $1.00 at API prices for C1).
