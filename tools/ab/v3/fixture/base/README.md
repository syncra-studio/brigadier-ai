# tally

A small invoicing and inventory library with a command-line interface.
Products live in a catalog, carts are priced against it, invoices add
regional sales tax and payment terms, and sales reports and exports are
built from saved invoices. Data is a single JSON file: `{ "products": [...], "invoices": [...] }`.

## Layout

- `src/` – library modules (`money`, `dates`, `catalog`, `cart`, `tax`, `invoice`, `storage`, `services`, `report`, `exporters/`) and `cli.ts`
- `test/` – `node:test` suites, one per module
- `examples/` – a sample data file and cart

## Usage

    node src/cli.ts products --data examples/data.json
    node src/cli.ts invoice --data examples/data.json --cart examples/cart.json --region ON --date 2024-03-15 --terms 30
    node src/cli.ts report --data examples/data.json

Requires Node 24+ (TypeScript runs directly via type stripping). Run `npm run check` to type-check and test.
