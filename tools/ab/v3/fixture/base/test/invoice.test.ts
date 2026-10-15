import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { addItem, emptyCart } from '../src/cart.ts';
import { buildInvoice, renderInvoiceText, type InvoiceOptions } from '../src/invoice.ts';
import { sampleCatalog } from './helpers.ts';

const OPTS: InvoiceOptions = { number: 'INV-0007', issueDate: '2024-03-15', termsDays: 30, region: 'ON' };

function sampleCart() {
  return addItem(addItem(addItem(emptyCart(), 'WID-001', 3), 'BOOK-010', 2), 'TEA-200', 2);
}

describe('buildInvoice', () => {
  test('prices, taxes and totals every line', () => {
    const invoice = buildInvoice(sampleCart(), sampleCatalog(), OPTS);
    assert.deepEqual(invoice, {
      number: 'INV-0007',
      issueDate: '2024-03-15',
      dueDate: '2024-04-14',
      region: 'ON',
      lines: [
        { sku: 'WID-001', name: 'Widget', qty: 3, unitPrice: 19.99, lineTotal: 59.97, tax: 7.8 },
        { sku: 'BOOK-010', name: 'Field Guide to Ledgers', qty: 2, unitPrice: 34.5, lineTotal: 69, tax: 3.45 },
        { sku: 'TEA-200', name: 'Loose Leaf Tea 250g', qty: 2, unitPrice: 8.75, lineTotal: 17.5, tax: 0 },
      ],
      subtotal: 146.47,
      tax: 11.25,
      total: 157.72,
    });
  });

  test('due date follows the payment terms', () => {
    const cart = addItem(emptyCart(), 'TEA-200');
    assert.equal(buildInvoice(cart, sampleCatalog(), { ...OPTS, termsDays: 0 }).dueDate, '2024-03-15');
    assert.equal(buildInvoice(cart, sampleCatalog(), { ...OPTS, termsDays: 14 }).dueDate, '2024-03-29');
  });

  test('tax depends on the region', () => {
    const cart = addItem(emptyCart(), 'CBL-030', 2);
    assert.equal(buildInvoice(cart, sampleCatalog(), { ...OPTS, region: 'BC' }).tax, 2.98);
    assert.equal(buildInvoice(cart, sampleCatalog(), { ...OPTS, region: 'AB' }).tax, 1.24);
  });

  test('rejects bad input', () => {
    const cart = sampleCart();
    assert.throws(() => buildInvoice(emptyCart(), sampleCatalog(), OPTS), /empty cart/);
    assert.throws(() => buildInvoice(addItem(cart, 'NOPE'), sampleCatalog(), OPTS), /unknown sku/);
    assert.throws(() => buildInvoice(cart, sampleCatalog(), { ...OPTS, termsDays: -1 }), /payment terms/);
    assert.throws(() => buildInvoice(cart, sampleCatalog(), { ...OPTS, region: 'XX' }), /unknown region/);
    assert.throws(() => buildInvoice(cart, sampleCatalog(), { ...OPTS, issueDate: '2024-02-30' }), /invalid date/);
  });
});

describe('renderInvoiceText', () => {
  test('renders a fixed-width invoice', () => {
    const text = renderInvoiceText(buildInvoice(sampleCart(), sampleCatalog(), OPTS));
    assert.equal(
      text,
      [
        'INVOICE INV-0007',
        'Issue date: 2024-03-15',
        'Due date:   2024-04-14',
        'Region:     ON',
        '',
        'SKU       Description               Qty     Price    Amount',
        '-----------------------------------------------------------',
        'WID-001   Widget                      3     19.99     59.97',
        'BOOK-010  Field Guide to Ledgers      2     34.50     69.00',
        'TEA-200   Loose Leaf Tea 250g         2      8.75     17.50',
        '-----------------------------------------------------------',
        '                                        Subtotal:    146.47',
        '                                             Tax:     11.25',
        '                                           Total:    157.72',
        '',
      ].join('\n'),
    );
  });

  test('shortens long product names', () => {
    const invoice = buildInvoice(addItem(emptyCart(), 'TEA-200'), sampleCatalog(), OPTS);
    invoice.lines[0]!.name = 'An Extremely Long Product Name';
    const line = renderInvoiceText(invoice).split('\n')[7]!;
    assert.equal(line, 'TEA-200   An Extremely Long Pr...     1      8.75      8.75');
  });
});
