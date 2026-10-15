# q1: where the default payment terms are set
id: q1
kind: question
case: v3
timeout: 900

## Request

```text
Where is the default payment term for the `invoice` command set when `--terms` isn't given, and what is it?
```

## Scoring

```json
{"rule": "answer", "locations": [{"file": "src/cli.ts", "lines": [64, 64]}],
 "facts": [{"name": "30 days", "re": "\\b30\\b"}]}
```

## Known answer

`src/cli.ts:64`: `const termsText = typeof values.terms === 'string' ? values.terms : '30';`, so 30 days.
