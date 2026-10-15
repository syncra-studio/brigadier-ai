# x1: explain "invalid quantity: 0"
id: x1
kind: error
case: v3
timeout: 900

## Request

```text
I ran `node src/cli.ts invoice --data examples/data.json --cart examples/return.json --region ON --date 2024-03-15` and got `error: invalid quantity: 0`. What does it mean?
```

## Scoring

```json
{"rule": "answer", "locations": [{"file": "src/cart.ts", "lines": [14, 19]}, {"file": "src/cart.ts", "lines": [26, 27]},
                                 {"file": "src/cart.ts", "lines": [65, 80]}],
 "facts": [{"name": "names the check", "re": "assertQty|addItem|parseCart"},
           {"name": "a line needs at least 1", "re": "at least (one|1)|>=\\s*1|≥\\s*1|\\bpositive\\b|1 or more|one or more|less than (one|1)|below (one|1)|\\bqty\\s*<\\s*1|must be (a whole number|an integer)?\\s*(of )?1|minimum (of )?(1|one)|\\bzero\\b[^.\\n]{0,40}not (allowed|accepted|valid)"}]}
```

## Known answer

`examples/return.json` has `{ "sku": "TEA-200", "qty": 0 }`. `parseCart` (`src/cart.ts:65-80`) builds the cart with
`addItem` (:26), whose `assertQty` (:14-19) wants a whole quantity of at least 1.
