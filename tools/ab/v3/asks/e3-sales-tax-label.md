# e3: rename the printed "Tax:" total to "Sales tax:"
id: e3
kind: edit
case: v3
timeout: 1200

## Request

```text
In the printed invoice, rename the `Tax:` total line's label to `Sales tax:`.
```

## Scoring

```json
{"rule": "check", "check": "node \"$CHECKS/e3-sales-tax-label.mjs\" \"$PWD\" && npm run check"}
```

## Known answer

`src/invoice.ts:115` `totalLine('Sales tax:', …)`; the expected text in `test/invoice.test.ts:72` follows.
