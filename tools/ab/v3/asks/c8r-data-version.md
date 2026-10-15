# c8r: a data-file version, small and risky by nature
id: c8r-data-version
kind: edit
case: c8
timeout: 1800

## Request

```text
Data files should carry a format version. Save `"version": 2` as the first key of every data file we write, keep reading files written before (they have no version: treat them as version 1), and refuse a file from a newer version than we know with an error that names its version. Careful: this touches every customer's saved data.
```

## Scoring

```json
{"rule": "check", "check": "node \"$CHECKS/c8r-data-version.mjs\" \"$PWD\" && npm run check"}
```

## Known answer

`src/storage.ts`: `writeDataFile` writes `{ version: 2, products, invoices }`, `parseDataFile` accepts a missing
version or 1–2 and throws on a higher one naming it. About 10–25 changed lines of code in 1–2 files: far under the
line, so only a risky mark starts a review (BUILD-PLAN C8).
