# e1: add Manitoba to the tax table
id: e1
kind: edit
case: v3
timeout: 1200

## Request

```text
Add Manitoba (MB) to the sales tax table: 12% standard, 7% reduced, 0% exempt.
```

## Scoring

```json
{"rule": "check", "check": "node \"$CHECKS/e1-manitoba.mjs\" \"$PWD\" && npm run check"}
```

## Known answer

`src/tax.ts` gains `MB: { standard: 0.12, reduced: 0.07, exempt: 0 }`; `test/tax.test.ts`'s region list
becomes `['AB', 'BC', 'MB', 'ON', 'QC']`. The check runs on the session workspace as the run left it.
