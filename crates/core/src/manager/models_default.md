# My model rules

Edit or empty this file at any time. Only these rules restrict model choice.
Rules use one directive per line. Model names match a whole family word or an exact id, ignoring case.
Use `*` for every model. Allowlists may be scoped to a provider (`allow claude: opus, sonnet`).
Effort levels: none, minimal, low, medium, high, xhigh, max, ultra.
Guidance rows use the same Work / Agent / Model / Effort / Reason table as the shipped guidance; Work is the override key.
Several rows of one Work form an order: the first one your rules allow takes it, and they replace the shipped rows for that Work.
A rule may name one Work (`forbid for Docs: sonnet`, `allow claude for Docs: opus`, `effort-ceiling for Docs: high`); it then judges only `brigadier start --work Docs`.
Effort rules move a start's effort instead of refusing it: `effort-ceiling: high` brings more down to High, `effort-floor: medium` brings less up to Medium.
An effort rule may name an agent or one model (`effort-ceiling codex: high`, `effort-ceiling opus: medium`) and one Work; a start is refused only when a ceiling and a floor can't both be met.
Words no rule can hold are kept as advice (`advice: prefer Codex when Claude is busy`); sessions read it with their routing, and it restricts nothing.
