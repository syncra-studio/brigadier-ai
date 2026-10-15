# q2: where Quebec's standard rate is set
id: q2
kind: question
case: v3
timeout: 900

## Request

```text
Where is Quebec's standard sales tax rate defined, and what is it?
```

## Scoring

```json
{"rule": "answer", "locations": [{"file": "src/tax.ts", "lines": [9, 9]}],
 "facts": [{"name": "15%", "re": "0\\.15|15\\s?%|15 percent"}]}
```

## Known answer

`src/tax.ts:9`: `QC: { standard: 0.15, reduced: 0.05, exempt: 0 }`, so 15%.
