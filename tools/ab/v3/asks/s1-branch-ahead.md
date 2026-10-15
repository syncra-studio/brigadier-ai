# s1: how far a branch is ahead of main
id: s1
kind: status
case: v3
timeout: 900

## Request

```text
How many commits is the branch `wip/discounts` ahead of `main`, and which files do they change?
```

## Scoring

```json
{"rule": "answer",
 "facts": [{"name": "2 commits", "re": "\\b(2|two)\\s+(new\\s+|more\\s+)?commits?\\b|ahead\\s+(of\\s+`?main`?\\s+)?by\\s+(2|two)\\b|\\b(2|two)\\s+ahead\\b"},
           {"name": "src/cart.ts", "re": "(src/)?cart\\.ts"},
           {"name": "test/cart.test.ts", "re": "(test/)?cart\\.test\\.ts"}]}
```

## Known answer

`git log main..wip/discounts`: e2d0525 "Add percentOff for cart discounts" (`src/cart.ts`) and 1da5238
"Test percentOff" (`test/cart.test.ts`).
