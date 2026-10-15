// c8u: `export --format csv` with CSV quoting, --from/--to and --out (C8, a change just under the review line).
// usage: node c8u-csv-export.mjs <result-dir>   (then `npm run check` in that dir)
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
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

/** RFC 4180 rows: quoted fields may hold commas, doubled quotes and line breaks; rows end in LF or CRLF. */
function parseCsv(text) {
  const rows = [];
  let row = [];
  let field = '';
  let i = 0;
  let quoted = false;
  while (i < text.length) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') { field += '"'; i += 2; continue; }
      if (c === '"') { quoted = false; i++; continue; }
      field += c; i++; continue;
    }
    if (c === '"' && field === '') { quoted = true; i++; continue; }
    if (c === '"') throw new Error(`a bare quote inside an unquoted field at ${i}`);
    if (c === ',') { row.push(field); field = ''; i++; continue; }
    if (c === '\r' && text[i + 1] === '\n') { i++; continue; }
    if (c === '\n') { row.push(field); rows.push(row); row = []; field = ''; i++; continue; }
    field += c; i++;
  }
  assert.ok(!quoted, 'an unclosed quoted field');
  if (field !== '' || row.length > 0) { row.push(field); rows.push(row); }
  return rows;
}

const line = (sku, name, qty, unitPrice, lineTotal, tax) => ({ sku, name, qty, unitPrice, lineTotal, tax });
const invoice = (number, issueDate, region, lines) => ({
  number, issueDate, dueDate: issueDate, region, lines, subtotal: 0, tax: 0, total: 0,
});
const data = {
  products: [],
  invoices: [
    invoice('INV-0001', '2024-01-10', 'ON', [
      line('BOLT-1', 'Bolt, small', 2, 1.75, 3.5, 0.46),
      line('RUL-5', '5" Ruler', 1, 3.5, 3.5, 0.46),
    ]),
    invoice('INV-0002', '2024-02-01', 'QC', [line('TWO-2', 'Two\nlines', 3, 10, 30, 4.5)]),
    invoice('INV-0003', '2024-03-05', 'BC', [line('PLN-3', 'Plain', 1, 7.1, 7.1, 0.85)]),
  ],
};
const tmp = mkdtempSync(join(tmpdir(), 'c8u-'));
const file = join(tmp, 'data.json');
writeFileSync(file, JSON.stringify(data, null, 2));

const HEADER = ['invoice', 'issue_date', 'region', 'sku', 'name', 'qty', 'unit_price', 'line_total', 'tax'];
const ROWS = {
  'INV-0001': [
    ['INV-0001', '2024-01-10', 'ON', 'BOLT-1', 'Bolt, small', '2', '1.75', '3.50', '0.46'],
    ['INV-0001', '2024-01-10', 'ON', 'RUL-5', '5" Ruler', '1', '3.50', '3.50', '0.46'],
  ],
  'INV-0002': [['INV-0002', '2024-02-01', 'QC', 'TWO-2', 'Two\nlines', '3', '10.00', '30.00', '4.50']],
  'INV-0003': [['INV-0003', '2024-03-05', 'BC', 'PLN-3', 'Plain', '1', '7.10', '7.10', '0.85']],
};
const expect = (...numbers) => [HEADER, ...numbers.flatMap((n) => ROWS[n])];

// Every line, in saved order, quoted where it must be.
let r = cli(['export', '--data', file, '--format', 'csv']);
assert.equal(r.code, 0, r.err);
assert.deepEqual(parseCsv(r.out), expect('INV-0001', 'INV-0002', 'INV-0003'));
const whole = r.out;

// --from/--to, both inclusive, as report's range.
r = cli(['export', '--data', file, '--format', 'csv', '--from', '2024-01-10', '--to', '2024-02-01']);
assert.equal(r.code, 0, r.err);
assert.deepEqual(parseCsv(r.out), expect('INV-0001', 'INV-0002'));
r = cli(['export', '--data', file, '--format', 'csv', '--from', '2024-02-01', '--to', '2024-02-01']);
assert.equal(r.code, 0, r.err);
assert.deepEqual(parseCsv(r.out), expect('INV-0002'));
r = cli(['export', '--data', file, '--format', 'csv', '--from', '2024-02-02']);
assert.equal(r.code, 0, r.err);
assert.deepEqual(parseCsv(r.out), expect('INV-0003'));
// The range applies to json too.
r = cli(['export', '--data', file, '--to', '2024-01-31']);
assert.equal(r.code, 0, r.err);
assert.deepEqual(JSON.parse(r.out).map((i) => i.number), ['INV-0001']);
r = cli(['export', '--data', file, '--format', 'csv', '--from', '2024-13-01']);
assert.notEqual(r.code, 0, 'an invalid --from is refused');

// json is unchanged.
r = cli(['export', '--data', file]);
assert.equal(r.code, 0, r.err);
assert.equal(r.out, `${JSON.stringify(data.invoices, null, 2)}\n`);

// --out writes the file, and never overwrites one.
const out = join(tmp, 'out.csv');
r = cli(['export', '--data', file, '--format', 'csv', '--out', out]);
assert.equal(r.code, 0, r.err);
assert.ok(existsSync(out), '--out wrote no file');
assert.equal(readFileSync(out, 'utf8'), whole);
assert.ok(!r.out.includes('invoice,issue_date'), '--out still printed the export');
writeFileSync(out, 'keep me\n');
r = cli(['export', '--data', file, '--format', 'csv', '--out', out]);
assert.notEqual(r.code, 0, 'an existing --out file was overwritten');
assert.equal(readFileSync(out, 'utf8'), 'keep me\n');
console.log('c8u ok');
