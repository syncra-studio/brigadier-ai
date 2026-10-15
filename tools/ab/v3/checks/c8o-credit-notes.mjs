// c8o: credit notes (C8, a change just over the review line).
// usage: node c8o-credit-notes.mjs <result-dir>   (then `npm run check` in that dir)
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const dir = process.argv[2];
const { run } = await import(`${dir}/src/cli.ts`);
const { lineTax } = await import(`${dir}/src/tax.ts`);
const { round2, formatMoney } = await import(`${dir}/src/money.ts`);
function cli(args) {
  let out = '';
  let err = '';
  const code = run(args, { out: (t) => { out += t; }, err: (t) => { err += t; } });
  return { code, out, err };
}
const tmp = mkdtempSync(join(tmpdir(), 'c8o-'));
const file = join(tmp, 'data.json');
const products = JSON.parse(readFileSync(join(dir, 'examples/data.json'), 'utf8')).products;
writeFileSync(file, JSON.stringify({ products, invoices: [] }, null, 2));
const cart = join(tmp, 'cart.json');
// WID-001 x3 (standard), BOOK-010 x1 (reduced), TEA-200 x2 (exempt).
writeFileSync(cart, JSON.stringify({ lines: [
  { sku: 'WID-001', qty: 3 }, { sku: 'BOOK-010', qty: 1 }, { sku: 'TEA-200', qty: 2 },
] }));
let r = cli(['invoice', '--data', file, '--cart', cart, '--region', 'ON', '--date', '2024-03-15']);
assert.equal(r.code, 0, r.err);
assert.match(r.out, /INV-0001/);

const totalLine = (text) => {
  const m = text.match(/TOTAL\s+(\d+)\s+(-?[\d.]+)/);
  assert.ok(m, `no TOTAL line in:\n${text}`);
  return [Number(m[1]), m[2]];
};
r = cli(['report', '--data', file]);
assert.equal(r.code, 0, r.err);
assert.deepEqual(totalLine(r.out), [6, '111.97']);

const credit = (qty, extra = {}) =>
  cli(['credit', '--data', file, '--invoice', extra.invoice ?? 'INV-0001', '--sku', extra.sku ?? 'WID-001',
       '--qty', String(qty), '--date', '2024-03-20']);
const notes = () => JSON.parse(readFileSync(file, 'utf8')).creditNotes ?? [];

// One unit: numbered, saved, printed with its tax.
r = credit(1);
assert.equal(r.code, 0, r.err);
assert.match(r.out, /CREDIT NOTE/);
assert.match(r.out, /CN-0001/);
assert.match(r.out, /INV-0001/);
const tax = lineTax(round2(19.99 * 1), 'ON', 'standard');
assert.ok(r.out.includes(formatMoney(tax)), `the credited tax ${formatMoney(tax)} is not in:\n${r.out}`);
assert.equal(notes().length, 1);
assert.equal(notes()[0].number, 'CN-0001');

r = credit(1);
assert.equal(r.code, 0, r.err);
assert.match(r.out, /CN-0002/);
assert.deepEqual(notes().map((n) => n.number), ['CN-0001', 'CN-0002']);

// 2 more would credit 4 of 3: refused, counting both earlier notes, and nothing written.
let before = readFileSync(file);
r = credit(2);
assert.notEqual(r.code, 0, 'crediting more units than were invoiced was allowed');
assert.ok(readFileSync(file).equals(before), 'a refused credit changed the data file');
// The last unit is still creditable.
r = credit(1);
assert.equal(r.code, 0, r.err);
assert.match(r.out, /CN-0003/);
before = readFileSync(file);
r = credit(1);
assert.notEqual(r.code, 0, 'a fourth unit of three was credited');
assert.ok(readFileSync(file).equals(before));

// Unknown invoice or SKU.
r = credit(1, { invoice: 'INV-9999' });
assert.notEqual(r.code, 0, 'an unknown invoice was credited');
r = credit(1, { sku: 'GAD-002' });
assert.notEqual(r.code, 0, 'a SKU not on the invoice was credited');
assert.ok(readFileSync(file).equals(before));

// The report subtracts the 3 credited widgets: 6 - 3 units, 111.97 - 59.97.
r = cli(['report', '--data', file]);
assert.equal(r.code, 0, r.err);
assert.deepEqual(totalLine(r.out), [3, '52.00']);

// A data file without credit notes still loads.
const old = join(tmp, 'old.json');
copyFileSync(join(dir, 'examples/data.json'), old);
const raw = JSON.parse(readFileSync(old, 'utf8'));
delete raw.creditNotes;
writeFileSync(old, JSON.stringify(raw, null, 2));
assert.equal(cli(['products', '--data', old]).code, 0);
assert.equal(cli(['report', '--data', old]).code, 0);
console.log('c8o ok');
