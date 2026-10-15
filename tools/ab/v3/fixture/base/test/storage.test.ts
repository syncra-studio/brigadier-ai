import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, test } from 'node:test';
import { addItem, emptyCart } from '../src/cart.ts';
import { buildInvoice } from '../src/invoice.ts';
import { MemoryStore, parseDataFile, readDataFile, writeDataFile } from '../src/storage.ts';
import { PRODUCTS, sampleCatalog, withTempDir } from './helpers.ts';

function invoice(number: string) {
  return buildInvoice(addItem(emptyCart(), 'WID-001'), sampleCatalog(), {
    number,
    issueDate: '2024-05-01',
    termsDays: 30,
    region: 'ON',
  });
}

describe('MemoryStore', () => {
  test('stores products and lists them by sku', () => {
    const store = new MemoryStore({ products: PRODUCTS });
    assert.equal(store.getProduct('WID-001')?.unitPrice, 19.99);
    assert.equal(store.getProduct('NOPE'), undefined);
    assert.deepEqual(
      store.listProducts().map((p) => p.sku),
      ['BOOK-010', 'CBL-030', 'TEA-200', 'WID-001'],
    );
  });

  test('putProduct replaces a product with the same sku', () => {
    const store = new MemoryStore({ products: PRODUCTS });
    store.putProduct({ ...PRODUCTS[0]!, unitPrice: 21 });
    assert.equal(store.getProduct('WID-001')?.unitPrice, 21);
    assert.equal(store.listProducts().length, PRODUCTS.length);
  });

  test('keeps invoices in the order they were saved', () => {
    const store = new MemoryStore();
    store.saveInvoice(invoice('INV-0002'));
    store.saveInvoice(invoice('INV-0001'));
    assert.deepEqual(
      store.listInvoices().map((i) => i.number),
      ['INV-0002', 'INV-0001'],
    );
    assert.equal(store.getInvoice('INV-0001')?.total, 22.59);
    assert.equal(store.getInvoice('INV-0003'), undefined);
  });

  test('refuses to overwrite an invoice', () => {
    const store = new MemoryStore();
    store.saveInvoice(invoice('INV-0001'));
    assert.throws(() => store.saveInvoice(invoice('INV-0001')), /already exists/);
  });

  test('toData snapshots the store', () => {
    const store = new MemoryStore({ products: PRODUCTS.slice(0, 1), invoices: [invoice('INV-0001')] });
    const data = store.toData();
    assert.equal(data.products.length, 1);
    assert.equal(data.invoices[0]?.number, 'INV-0001');
  });
});

describe('data files', () => {
  test('parseDataFile validates products and defaults missing arrays', () => {
    assert.deepEqual(parseDataFile({}), { products: [], invoices: [] });
    assert.throws(() => parseDataFile({ products: [{ sku: 'x' }] }), /invalid sku/);
    assert.throws(() => parseDataFile({ invoices: [{}] }), /needs a number/);
    assert.throws(() => parseDataFile('nope'), /JSON object/);
  });

  test('round-trips through disk as pretty JSON', () => {
    withTempDir((dir) => {
      const path = join(dir, 'data.json');
      const data = { products: PRODUCTS, invoices: [invoice('INV-0001')] };
      writeDataFile(path, data);
      assert.ok(readFileSync(path, 'utf8').startsWith('{\n  "products": [\n'));
      assert.deepEqual(readDataFile(path), data);
    });
  });

  test('readDataFile reports broken JSON', () => {
    withTempDir((dir) => {
      const path = join(dir, 'data.json');
      writeFileSync(path, '{ not json');
      assert.throws(() => readDataFile(path), SyntaxError);
    });
  });
});
