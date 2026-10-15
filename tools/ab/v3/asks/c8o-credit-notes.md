# c8o: credit notes, just over the review line
id: c8o-credit-notes
kind: edit
case: c8
timeout: 1800

## Request

```text
Add credit notes. `credit --invoice <number> --sku <sku> --qty <n> --date <YYYY-MM-DD>` refunds that many units of one line of a saved invoice: it saves a credit note numbered CN-0001, CN-0002, … under `creditNotes` in the data file, at the line's unit price with the tax for those units, and prints it like an invoice, titled CREDIT NOTE and naming the invoice it refers to. A line can't be credited more units than were invoiced, counting earlier credit notes. The report subtracts credited units and revenue. Data files without credit notes still load. Add tests.
```

## Scoring

```json
{"rule": "check", "check": "node \"$CHECKS/c8o-credit-notes.mjs\" \"$PWD\" && npm run check"}
```

## Known answer

`creditNotes` in `DataFile`, the store and `parseDataFile` (`src/storage.ts`), a credit note builder and renderer, a
`credit` command in `src/cli.ts`, and `salesReport` netting credits (`src/report.ts`). About 150–250 changed lines of
code in 4–6 files: at or over the review line (BUILD-PLAN C8).
