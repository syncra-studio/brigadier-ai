# x2: explain why 2024-02-29 is rejected
id: x2
kind: error
case: v3
timeout: 900

## Request

```text
`node src/cli.ts invoice --data examples/data.json --cart examples/cart.json --region ON --date 2024-02-29` fails with `error: --date must be a date like 2024-01-31, got: 2024-02-29`. 2024-02-29 is a real date. Why does it fail?
```

## Scoring

```json
{"rule": "answer", "locations": [{"file": "src/dates.ts", "lines": [16, 16]}, {"file": "src/dates.ts", "lines": [19, 27]},
                                 {"file": "src/dates.ts", "lines": [38, 38]}],
 "facts": [{"name": "leap years not handled", "re": "leap"},
           {"name": "February is always 28 days", "re": "\\b28\\b"}]}
```

## Known answer

`daysInMonth` (`src/dates.ts:19-27`) reads the fixed table `DAYS_PER_MONTH` (:16), where February always has 28
days, so `parse` (:38) rejects February 29 even in a leap year.
