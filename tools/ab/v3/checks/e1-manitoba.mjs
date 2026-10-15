// e1: Manitoba (MB) is in the tax table at 12% standard, 7% reduced, 0% exempt; nothing else changed.
// usage: node e1-manitoba.mjs <result-dir>   (then `npm run check` in that dir)
import assert from 'node:assert/strict';
const dir = process.argv[2];
const tax = await import(`${dir}/src/tax.ts`);
assert.equal(tax.taxRate('MB', 'standard'), 0.12);
assert.equal(tax.taxRate('MB', 'reduced'), 0.07);
assert.equal(tax.taxRate('MB', 'exempt'), 0);
assert.deepEqual(tax.regions(), ['AB', 'BC', 'MB', 'ON', 'QC']);
assert.equal(tax.taxRate('ON', 'standard'), 0.13);
assert.equal(tax.taxRate('QC', 'standard'), 0.15);
assert.equal(tax.taxRate('BC', 'reduced'), 0.07);
console.log('e1 ok');
