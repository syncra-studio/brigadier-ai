# q3: where the invoice number prefix is set
id: q3
kind: question
case: v3
timeout: 900

## Request

```text
Where is the prefix of new invoice numbers set, and what is it?
```

## Scoring

```json
{"rule": "answer", "locations": [{"file": "src/services.ts", "lines": [7, 7]}],
 "facts": [{"name": "INV-", "re": "INV-"}]}
```

## Known answer

`src/services.ts:7`: `export const INVOICE_PREFIX = 'INV-';` (the default of `nextInvoiceNumber`'s `prefix`, :14).
