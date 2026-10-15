// c8r: data files carry a format version (C8, a small change risky by nature).
// usage: node c8r-data-version.mjs <result-dir>   (then `npm run check` in that dir)
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
const tmp = mkdtempSync(join(tmpdir(), 'c8r-'));
const example = JSON.parse(readFileSync(join(dir, 'examples/data.json'), 'utf8'));
delete example.version;
const cart = join(tmp, 'cart.json');
writeFileSync(cart, JSON.stringify({ lines: [{ sku: 'WID-001', qty: 1 }] }));
const write = (name, value) => {
  const path = join(tmp, name);
  writeFileSync(path, JSON.stringify(value, null, 2));
  return path;
};

// A file from before versions loads, and writing it adds version 2 first, keeping everything.
const old = write('old.json', example);
let r = cli(['products', '--data', old]);
assert.equal(r.code, 0, r.err);
r = cli(['invoice', '--data', old, '--cart', cart, '--region', 'ON', '--date', '2024-03-15']);
assert.equal(r.code, 0, r.err);
const saved = JSON.parse(readFileSync(old, 'utf8'));
assert.equal(Object.keys(saved)[0], 'version', 'version is not the first key');
assert.equal(saved.version, 2);
assert.deepEqual(saved.products, example.products);
assert.deepEqual(saved.invoices.slice(0, example.invoices.length), example.invoices);
assert.equal(saved.invoices.length, example.invoices.length + 1);
// The version 2 file it wrote reads back.
assert.equal(cli(['report', '--data', old]).code, 0);
assert.equal(cli(['products', '--data', old]).code, 0);

// An explicit version 1 loads.
const v1 = write('v1.json', { version: 1, ...example });
assert.equal(cli(['products', '--data', v1]).code, 0);

// A newer version is refused, naming it, and left as it was.
const v3 = write('v3.json', { version: 3, ...example });
const before = readFileSync(v3);
r = cli(['products', '--data', v3]);
assert.notEqual(r.code, 0, 'a version 3 file was read');
assert.match(r.err, /\b3\b/, `the error doesn't name version 3: ${r.err}`);
r = cli(['invoice', '--data', v3, '--cart', cart, '--region', 'ON', '--date', '2024-03-16']);
assert.notEqual(r.code, 0, 'a version 3 file was written');
assert.ok(readFileSync(v3).equals(before), 'a version 3 file was changed');
console.log('c8r ok');
