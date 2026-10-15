import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { addItem, emptyCart } from '../src/cart.ts';
import {
  catalogFromStore,
  createInvoiceFromCart,
  importProducts,
  invoicesInRange,
  nextInvoiceNumber,
  overdueInvoices,
} from '../src/services.ts';
import { MemoryStore } from '../src/storage.ts';
import { PRODUCTS } from './helpers.ts';

function seededStore(): MemoryStore {
  const store = new MemoryStore({ products: PRODUCTS });
  const cart = addItem(emptyCart(), 'WID-001', 2);
  createInvoiceFromCart(store, cart, { issueDate: '2024-01-10', termsDays: 30, region: 'ON' });
  createInvoiceFromCart(store, cart, { issueDate: '2024-02-10', termsDays: 14, region: 'AB' });
  createInvoiceFromCart(store, cart, { issueDate: '2024-03-10', termsDays: 30, region: 'BC' });
  return store;
}

describe('services', () => {
  test('catalogFromStore builds a catalog of every product', () => {
    const catalog = catalogFromStore(new MemoryStore({ products: PRODUCTS }));
    assert.equal(catalog.size, PRODUCTS.length);
  });

  test('nextInvoiceNumber counts up from the highest number', () => {
    const store = new MemoryStore({ products: PRODUCTS });
    assert.equal(nextInvoiceNumber(store), 'INV-0001');
    const cart = addItem(emptyCart(), 'TEA-200');
    createInvoiceFromCart(store, cart, { issueDate: '2024-01-01', termsDays: 0, region: 'ON', number: 'INV-0009' });
    createInvoiceFromCart(store, cart, { issueDate: '2024-01-01', termsDays: 0, region: 'ON', number: 'CN-0100' });
    assert.equal(nextInvoiceNumber(store), 'INV-0010');
    assert.equal(nextInvoiceNumber(store, 'CN-'), 'CN-0101');
  });

  test('createInvoiceFromCart numbers, saves and returns the invoice', () => {
    const store = seededStore();
    const numbers = store.listInvoices().map((i) => i.number);
    assert.deepEqual(numbers, ['INV-0001', 'INV-0002', 'INV-0003']);
    const second = store.getInvoice('INV-0002');
    assert.equal(second?.dueDate, '2024-02-24');
    assert.equal(second?.total, 41.98);
  });

  test('importProducts upserts', () => {
    const store = new MemoryStore({ products: PRODUCTS });
    const count = importProducts(store, [
      { sku: 'WID-001', name: 'Widget v2', unitPrice: 24.99, taxCategory: 'standard' },
      { sku: 'NEW-1', name: 'Novelty', unitPrice: 1, taxCategory: 'reduced' },
    ]);
    assert.equal(count, 2);
    assert.equal(store.getProduct('WID-001')?.name, 'Widget v2');
    assert.equal(store.listProducts().length, PRODUCTS.length + 1);
  });

  test('invoicesInRange filters by issue date, inclusive', () => {
    const store = seededStore();
    const numbers = (from?: string, to?: string) => invoicesInRange(store, from, to).map((i) => i.number);
    assert.deepEqual(numbers('2024-02-10', '2024-03-10'), ['INV-0002', 'INV-0003']);
    assert.deepEqual(numbers(undefined, '2024-01-31'), ['INV-0001']);
    assert.deepEqual(numbers('2024-03-11'), []);
    assert.deepEqual(numbers(), ['INV-0001', 'INV-0002', 'INV-0003']);
  });

  test('overdueInvoices lists invoices due before the given date', () => {
    const store = seededStore();
    assert.deepEqual(
      overdueInvoices(store, '2024-02-25').map((i) => i.number),
      ['INV-0001', 'INV-0002'],
    );
    assert.deepEqual(overdueInvoices(store, '2024-02-09'), []);
  });
});
