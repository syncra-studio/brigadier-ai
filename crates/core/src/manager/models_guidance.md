# Model guidance

Advice only, and the fallback: the automatic rows of each kind of work come from the model ratings (one per agent, best-rated first); these rows stand in for a kind where no model is rated. User rows with the same Work replace them.

| Work | Agent | Model | Effort | Reason |
| --- | --- | --- | --- | --- |
| Architecture and broad changes | claude | opus | high | Strong default for work that needs judgment across a codebase. |
| Bounded implementation | codex | default | high | The CLI default suits a well specified coding task. |
| Mechanical changes and search | claude | sonnet | medium | A smaller model fits a bounded task with clear checks. |
| Adversarial review | codex | default | high | This provider's reviewer; `--reviewing` takes the row on another provider than the author's. |
| Adversarial review | claude | opus | high | This provider's reviewer; `--reviewing` takes the row on another provider than the author's. |
| fusion-claude | claude | opus | high | A fusion's Claude consult: the strongest Claude model. |
| fusion-codex | codex | default | high | A fusion's Codex consult: the CLI's own default model. |
| Computer use | claude | opus | high | Seeing and using apps on screen takes strong judgment. |
| Computer use | codex | default | high | The next choice when the first can't run. |
| Fallback | claude | opus | high | A session that moves to Claude Code: when it is asked to, or every account of its own agent is at its limit. |
| Fallback | codex | default | high | A session that moves to Codex: when it is asked to, or every account of its own agent is at its limit. |
| Conversation titles | codex | luna | low | A small model for short text; use available quota, cost tier and suitability among shipped title choices. |
| Conversation titles | claude | haiku | | A small model for short text; use available quota, cost tier and suitability among shipped title choices. |
| Rule reading | claude | sonnet | medium | Rewrites the rules you type into exact rules and advice. |
| Rule reading | codex | default | medium | The next choice when the first can't run. |
