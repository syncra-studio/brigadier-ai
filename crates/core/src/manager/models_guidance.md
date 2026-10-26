# Model guidance

Advice only. Edit this table to change the advice. User rows with the same Work replace these rows.

| Work | Agent | Model | Effort | Reason |
| --- | --- | --- | --- | --- |
| Architecture and broad changes | claude | opus | high | Strong default for work that needs judgment across a codebase. |
| Bounded implementation | codex | default | high | The CLI default suits a well specified coding task. |
| Mechanical changes and search | claude | sonnet | medium | A smaller model fits a bounded task with clear checks. |
| Independent review | codex | default | high | This provider's reviewer; `--reviewing` takes the row on another provider than the author's. |
| Independent review | claude | opus | high | This provider's reviewer; `--reviewing` takes the row on another provider than the author's. |
| fusion-claude | claude | opus | high | A fusion's Claude consult: the strongest Claude model. |
| fusion-codex | codex | default | high | A fusion's Codex consult: the CLI's own default model. |
| Fallback | claude | opus | high | A session that moves to Claude Code: when it is asked to, or every account of its own agent is at its limit. |
| Fallback | codex | default | high | A session that moves to Codex: when it is asked to, or every account of its own agent is at its limit. |
| Conversation titles | codex | luna | low | A small model for short text; use available quota, cost tier and suitability among shipped title choices. |
| Conversation titles | claude | haiku | | A small model for short text; use available quota, cost tier and suitability among shipped title choices. |
