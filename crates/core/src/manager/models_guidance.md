# Model guidance

Advice only. Edit this table to change the advice. User rows with the same Work replace these rows.

| Work | Agent | Model | Effort | Reason |
| --- | --- | --- | --- | --- |
| Architecture and broad changes | claude | opus | high | Strong default for work that needs judgment across a codebase. |
| Bounded implementation | codex | default | high | The CLI default suits a well specified coding task. |
| Mechanical changes and search | claude | sonnet | medium | A smaller model fits a bounded task with clear checks. |
| Independent review | codex | default | high | Codex, a second provider, gives an independent reading. |
| fusion-claude | claude | opus | high | A fusion's Claude consult: the strongest Claude model. |
| fusion-codex | codex | default | high | A fusion's Codex consult: the CLI's own default model. |
