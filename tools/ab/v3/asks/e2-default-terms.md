# e2: default payment terms to 14 days
id: e2
kind: edit
case: v3
timeout: 1200

## Request

```text
Change the default payment terms of the `invoice` command from 30 to 14 days.
```

## Scoring

```json
{"rule": "check", "check": "node \"$CHECKS/e2-default-terms.mjs\" \"$PWD\" && npm run check"}
```

## Known answer

`src/cli.ts:64` default `'14'`; the CLI test that relies on the default is updated.
