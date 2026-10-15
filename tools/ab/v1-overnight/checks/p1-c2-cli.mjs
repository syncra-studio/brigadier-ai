// p1-c2: `invoice --note <text>` prints the note as the invoice's last line and saves it on the
// invoice in the data file; without --note nothing changes. No suite in the fixture covers it.
// usage: node p1-c2-cli.mjs <result-dir>
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const dir = process.argv[2];
const { run } = await import(`${dir}/src/cli.ts`);
function cli(args) {
  let out = '';
  let err = '';
  const code = run(args, { out: (t) => { out += t; }, err: (t) => { err += t; } });
  return { code, out, err };
}
const tmp = mkdtempSync(join(tmpdir(), 'v1-p1-c2-'));
const file = join(tmp, 'data.json');
const products = JSON.parse(readFileSync(join(dir, 'examples/data.json'), 'utf8')).products;
writeFileSync(file, JSON.stringify({ products, invoices: [] }, null, 2));
const cart = join(tmp, 'cart.json');
writeFileSync(cart, JSON.stringify({ lines: [{ sku: 'WID-001', qty: 3 }, { sku: 'TEA-200', qty: 2 }] }));
const invoice = (...extra) =>
  cli(['invoice', '--data', file, '--cart', cart, '--region', 'ON', '--date', '2024-03-15', ...extra]);
const saved = () => JSON.parse(readFileSync(file, 'utf8')).invoices;

let r = invoice('--note', 'Paid by card');
assert.equal(r.code, 0, r.err);
assert.match(r.out, /INV-0001/);
assert.equal(r.out.trimEnd().split('\n').at(-1), 'Note: Paid by card');
assert.equal(saved()[0].note, 'Paid by card', 'the note is not saved on the invoice');

r = invoice();
assert.equal(r.code, 0, r.err);
assert.match(r.out, /INV-0002/);
assert.doesNotMatch(r.out, /Note:/);
assert.equal(saved()[1].note, undefined, 'an invoice without --note was saved with a note');

// The saved note survives a reload: the report and export still read the file.
assert.equal(cli(['report', '--data', file]).code, 0);
r = cli(['export', '--data', file]);
assert.equal(r.code, 0, r.err);
assert.equal(JSON.parse(r.out)[0].note, 'Paid by card');
console.log('p1-c2 ok');
