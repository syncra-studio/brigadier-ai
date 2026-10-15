# q4: where the printed invoice's Description column width is set
id: q4
kind: question
case: v3
timeout: 900

## Request

```text
In the printed invoice, where is the width of the Description column set, and how wide is it?
```

## Scoring

```json
{"rule": "answer", "locations": [{"file": "src/invoice.ts", "lines": [74, 74]}],
 "facts": [{"name": "24 characters", "re": "\\b24\\b"}]}
```

## Known answer

`src/invoice.ts:74`: `const COL_NAME = 24;` (used for the Description column at :98 and :107).
