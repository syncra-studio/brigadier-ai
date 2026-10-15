import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { Catalog, isTaxCategory, parseProduct } from '../src/catalog.ts';
import { PRODUCTS, sampleCatalog } from './helpers.ts';

describe('parseProduct', () => {
  test('accepts a valid product', () => {
    const raw = { sku: 'WID-001', name: 'Widget', unitPrice: 19.99, taxCategory: 'standard', extra: true };
    assert.deepEqual(parseProduct(raw), { sku: 'WID-001', name: 'Widget', unitPrice: 19.99, taxCategory: 'standard' });
  });

  test('rejects invalid fields', () => {
    const good = { sku: 'A-1', name: 'A', unitPrice: 1, taxCategory: 'exempt' };
    assert.throws(() => parseProduct(null), /must be an object/);
    assert.throws(() => parseProduct({ ...good, sku: 'lower' }), /invalid sku/);
    assert.throws(() => parseProduct({ ...good, name: '  ' }), /no name/);
    assert.throws(() => parseProduct({ ...good, unitPrice: 1.005 }), /unit price/);
    assert.throws(() => parseProduct({ ...good, unitPrice: -1 }), /unit price/);
    assert.throws(() => parseProduct({ ...good, taxCategory: 'luxury' }), /tax category/);
  });

  test('isTaxCategory', () => {
    assert.ok(isTaxCategory('reduced'));
    assert.ok(!isTaxCategory('Reduced'));
    assert.ok(!isTaxCategory(3));
  });
});

describe('Catalog', () => {
  test('looks products up by sku', () => {
    const catalog = sampleCatalog();
    assert.equal(catalog.size, PRODUCTS.length);
    assert.equal(catalog.get('TEA-200')?.name, 'Loose Leaf Tea 250g');
    assert.equal(catalog.get('NOPE'), undefined);
    assert.ok(catalog.has('WID-001'));
  });

  test('require throws for unknown skus', () => {
    assert.throws(() => sampleCatalog().require('NOPE'), /unknown sku: NOPE/);
  });

  test('lists products sorted by sku', () => {
    assert.deepEqual(
      sampleCatalog()
        .list()
        .map((p) => p.sku),
      ['BOOK-010', 'CBL-030', 'TEA-200', 'WID-001'],
    );
  });

  test('rejects duplicate skus', () => {
    const catalog = new Catalog();
    catalog.add(PRODUCTS[0]!);
    assert.throws(() => catalog.add(PRODUCTS[0]!), /duplicate sku/);
  });
});
