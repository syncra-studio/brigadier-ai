# c8u: a csv export, just under the review line
id: c8u-csv-export
kind: edit
case: c8
timeout: 1800

## Request

```text
Add a csv export format. `export --format csv` writes a header row, then one row per invoice line, with the columns invoice,issue_date,region,sku,name,qty,unit_price,line_total,tax. Money has two decimals. A value that holds a comma, a double quote or a line break is quoted the CSV way (quotes doubled). Let export also take `--from`/`--to` dates the way report does, and an `--out <file>` option that writes the file instead of printing it, refusing to overwrite a file that's already there. Add tests.
```

## Scoring

```json
{"rule": "check", "check": "node \"$CHECKS/c8u-csv-export.mjs\" \"$PWD\" && npm run check"}
```

## Known answer

A csv exporter next to `src/exporters/json.ts`, registered in `src/exporters/index.ts`; `src/cli.ts` filters with
`invoicesInRange` and handles `--out` (exclusive create). About 60–120 changed lines of code in 2–4 files: under the
review line (BUILD-PLAN C8). The check parses the CSV, so quoting every field is accepted.
