# My model rules

Edit or empty this file at any time. Only these rules restrict model choice.
Rules use one directive per line. Model names match a whole family word or an exact id, ignoring case.
Use `*` for every model. Allowlists may be scoped to a provider (`allow claude: opus, sonnet`).
Effort levels: none, minimal, low, medium, high, xhigh, max, ultra.
Guidance rows use the same Work / Agent / Model / Effort / Reason table as the shipped guidance; Work is the override key.
Several rows of one Work form an order: the first one your rules allow takes it, and they replace the shipped rows for that Work.
A rule may name one Work (`forbid for Docs: sonnet`, `allow claude for Docs: opus`, `effort-ceiling for Docs: high`); it then judges only `brigadier start --work Docs`.
