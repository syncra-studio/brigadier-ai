import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { isRegion, lineTax, regions, taxRate } from '../src/tax.ts';

describe('tax', () => {
  test('lists regions in order', () => {
    assert.deepEqual(regions(), ['AB', 'BC', 'ON', 'QC']);
    assert.ok(isRegion('ON'));
    assert.ok(!isRegion('XX'));
    assert.ok(!isRegion('toString'));
  });

  test('looks up rates by region and category', () => {
    assert.equal(taxRate('ON', 'standard'), 0.13);
    assert.equal(taxRate('BC', 'reduced'), 0.07);
    assert.equal(taxRate('QC', 'exempt'), 0);
  });

  test('rejects unknown regions', () => {
    assert.throws(() => taxRate('XX', 'standard'), /unknown region: XX/);
  });

  test('lineTax rounds each line to cents', () => {
    assert.equal(lineTax(59.97, 'ON', 'standard'), 7.8);
    assert.equal(lineTax(69, 'ON', 'reduced'), 3.45);
    assert.equal(lineTax(24.8, 'BC', 'standard'), 2.98);
    assert.equal(lineTax(17.5, 'AB', 'exempt'), 0);
  });
});
