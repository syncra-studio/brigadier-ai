// p1-c1: a note renders as the invoice's last line, an invoice without one renders exactly as
// before, and test/invoice.test.ts covers both.
// usage: node p1-c1-render.mjs <result-dir> <fixture-base-dir>
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const [dir, base] = process.argv.slice(2);
const { buildInvoice, renderInvoiceText } = await import(`${dir}/src/invoice.ts`);
const before = await import(`${base}/src/invoice.ts`);
const { Catalog } = await import(`${dir}/src/catalog.ts`);
const { addItem, emptyCart } = await import(`${dir}/src/cart.ts`);

const products = JSON.parse(readFileSync(join(dir, 'examples/data.json'), 'utf8')).products;
const catalog = new Catalog(products);
const cart = addItem(addItem(emptyCart(), 'WID-001', 3), 'TEA-200', 2);
const opts = { number: 'INV-0042', issueDate: '2024-03-15', termsDays: 30, region: 'ON' };

// Without a note: the same invoice object and the same text as the base fixture's.
const plain = buildInvoice(cart, catalog, opts);
const old = before.buildInvoice(cart, new (await import(`${base}/src/catalog.ts`)).Catalog(products), opts);
assert.deepEqual(plain, old, 'an invoice without a note changed');
assert.equal(renderInvoiceText(plain), before.renderInvoiceText(old), 'an invoice without a note renders differently');

// With a note: kept on the invoice, and the text's last line.
const noted = buildInvoice(cart, catalog, { ...opts, note: 'Paid by card' });
assert.equal(noted.note, 'Paid by card');
const text = renderInvoiceText(noted);
assert.ok(text.endsWith('\n'), 'the rendered invoice no longer ends with a newline');
assert.equal(text.trimEnd().split('\n').at(-1), 'Note: Paid by card');
// Everything above the note is the plain invoice's text.
assert.equal(text.slice(0, renderInvoiceText(plain).length), renderInvoiceText(plain));

// A test in test/invoice.test.ts covers both cases: it builds or renders with a note and
// checks the `Note:` line.
const suite = readFileSync(join(dir, 'test/invoice.test.ts'), 'utf8');
assert.match(suite, /note\s*:/, 'test/invoice.test.ts never builds an invoice with a note');
assert.match(suite, /Note:/, 'test/invoice.test.ts never checks the Note: line');
console.log('p1-c1 ok');
