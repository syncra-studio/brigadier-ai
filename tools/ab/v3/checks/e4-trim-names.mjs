// e4: parseProduct trims surrounding whitespace from names; blank names are still rejected.
// usage: node e4-trim-names.mjs <result-dir>
import assert from 'node:assert/strict';
const dir = process.argv[2];
const { parseProduct } = await import(`${dir}/src/catalog.ts`);
const good = { sku: 'A-1', name: '  Widget  ', unitPrice: 1, taxCategory: 'exempt' };
assert.equal(parseProduct(good).name, 'Widget');
assert.equal(parseProduct({ ...good, name: 'Gadget Pro' }).name, 'Gadget Pro');
assert.equal(parseProduct({ ...good, name: '\tTea 250g\n' }).name, 'Tea 250g');
assert.throws(() => parseProduct({ ...good, name: '   ' }), /no name/);
console.log('e4 ok');
