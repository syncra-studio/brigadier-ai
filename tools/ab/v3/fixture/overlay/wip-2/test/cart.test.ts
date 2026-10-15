import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { addItem, cartQuantity, cartSubtotal, emptyCart, parseCart, percentOff, removeItem, setQty } from '../src/cart.ts';
import { sampleCatalog } from './helpers.ts';

describe('cart operations', () => {
  test('addItem appends and merges lines', () => {
    let cart = addItem(emptyCart(), 'WID-001', 2);
    cart = addItem(cart, 'TEA-200');
    cart = addItem(cart, 'WID-001', 3);
    assert.deepEqual(cart.lines, [
      { sku: 'WID-001', qty: 5 },
      { sku: 'TEA-200', qty: 1 },
    ]);
  });

  test('operations do not modify the original cart', () => {
    const original = addItem(emptyCart(), 'WID-001', 1);
    addItem(original, 'WID-001', 1);
    setQty(original, 'WID-001', 9);
    removeItem(original, 'WID-001');
    assert.deepEqual(original.lines, [{ sku: 'WID-001', qty: 1 }]);
  });

  test('removeItem drops the line', () => {
    const cart = addItem(addItem(emptyCart(), 'WID-001'), 'TEA-200');
    assert.deepEqual(removeItem(cart, 'WID-001').lines, [{ sku: 'TEA-200', qty: 1 }]);
    assert.deepEqual(removeItem(cart, 'NOPE').lines, cart.lines);
  });

  test('setQty updates, appends and removes', () => {
    let cart = addItem(emptyCart(), 'WID-001', 2);
    cart = setQty(cart, 'WID-001', 7);
    cart = setQty(cart, 'CBL-030', 1);
    assert.deepEqual(cart.lines, [
      { sku: 'WID-001', qty: 7 },
      { sku: 'CBL-030', qty: 1 },
    ]);
    assert.deepEqual(setQty(cart, 'WID-001', 0).lines, [{ sku: 'CBL-030', qty: 1 }]);
  });

  test('rejects invalid quantities', () => {
    assert.throws(() => addItem(emptyCart(), 'WID-001', 0), RangeError);
    assert.throws(() => addItem(emptyCart(), 'WID-001', 1.5), RangeError);
    assert.throws(() => setQty(emptyCart(), 'WID-001', -1), RangeError);
  });
});

describe('cart totals', () => {
  test('cartQuantity counts units', () => {
    const cart = addItem(addItem(emptyCart(), 'WID-001', 2), 'TEA-200', 3);
    assert.equal(cartQuantity(cart), 5);
  });

  test('cartSubtotal prices lines from the catalog', () => {
    const cart = addItem(addItem(emptyCart(), 'WID-001', 3), 'CBL-030', 2);
    assert.equal(cartSubtotal(cart, sampleCatalog()), 84.77);
    assert.equal(cartSubtotal(emptyCart(), sampleCatalog()), 0);
  });

  test('cartSubtotal rejects unknown skus', () => {
    assert.throws(() => cartSubtotal(addItem(emptyCart(), 'NOPE'), sampleCatalog()), /unknown sku/);
  });
});

describe('parseCart', () => {
  test('reads and merges lines', () => {
    const cart = parseCart({ lines: [{ sku: 'WID-001', qty: 1 }, { sku: 'TEA-200', qty: 2 }, { sku: 'WID-001', qty: 4 }] });
    assert.deepEqual(cart.lines, [
      { sku: 'WID-001', qty: 5 },
      { sku: 'TEA-200', qty: 2 },
    ]);
  });

  test('rejects malformed input', () => {
    assert.throws(() => parseCart([]), TypeError);
    assert.throws(() => parseCart({ lines: [{ sku: 'WID-001' }] }), TypeError);
    assert.throws(() => parseCart({ lines: ['WID-001'] }), TypeError);
  });
});

describe('percentOff', () => {
  test('takes a whole percentage off', () => {
    assert.equal(percentOff(80, 25), 60);
    assert.throws(() => percentOff(10, 101), /invalid percent/);
  });
});
