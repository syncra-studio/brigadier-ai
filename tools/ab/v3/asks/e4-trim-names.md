# e4: trim product names
id: e4
kind: edit
case: v3
timeout: 1200

## Request

```text
Make `parseProduct` trim surrounding whitespace from product names, so " Widget " is stored as "Widget".
```

## Scoring

```json
{"rule": "check", "check": "node \"$CHECKS/e4-trim-names.mjs\" \"$PWD\" && npm run check"}
```

## Known answer

`src/catalog.ts:38` stores `r.name.trim()` (the blank-name check at :29 already trims).
