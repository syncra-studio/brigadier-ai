import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type { Invoice, InvoiceLine } from '../src/invoice.ts';
import { renderReport, salesReport } from '../src/report.ts';

function line(sku: string, name: string, qty: number, unitPrice: number): InvoiceLine {
  const lineTotal = Math.round(qty * unitPrice * 100) / 100;
  return { sku, name, qty, unitPrice, lineTotal, tax: 0 };
}

function invoice(number: string, lines: InvoiceLine[]): Invoice {
  const subtotal = lines.reduce((sum, l) => sum + l.lineTotal, 0);
  return { number, issueDate: '2024-01-01', dueDate: '2024-01-31', region: 'AB', lines, subtotal, tax: 0, total: subtotal };
}

const INVOICES: Invoice[] = [
  invoice('INV-0001', [line('WID-001', 'Widget', 3, 19.99), line('TEA-200', 'Loose Leaf Tea 250g', 2, 8.75)]),
  invoice('INV-0002', [line('CBL-030', 'USB-C Cable 2m', 1, 12.4), line('WID-001', 'Widget (renamed)', 1, 19.99)]),
  invoice('INV-0003', [line('BOOK-010', 'Field Guide to Ledgers', 2, 34.5), line('STK-005', 'Sticker Pack', 4, 3.1)]),
];

describe('salesReport', () => {
  test('sums units and revenue per sku, highest revenue first', () => {
    assert.deepEqual(salesReport(INVOICES), [
      { sku: 'WID-001', name: 'Widget', qty: 4, revenue: 79.96 },
      { sku: 'BOOK-010', name: 'Field Guide to Ledgers', qty: 2, revenue: 69 },
      { sku: 'TEA-200', name: 'Loose Leaf Tea 250g', qty: 2, revenue: 17.5 },
      { sku: 'CBL-030', name: 'USB-C Cable 2m', qty: 1, revenue: 12.4 },
      { sku: 'STK-005', name: 'Sticker Pack', qty: 4, revenue: 12.4 },
    ]);
  });

  test('is empty without invoices', () => {
    assert.deepEqual(salesReport([]), []);
  });

  test('does not modify the invoices', () => {
    const before = JSON.stringify(INVOICES);
    salesReport(INVOICES);
    assert.equal(JSON.stringify(INVOICES), before);
  });
});

describe('renderReport', () => {
  test('renders a ranked table with totals', () => {
    assert.equal(
      renderReport(salesReport(INVOICES)),
      [
        '   #  SKU         Product                      Qty   Revenue',
        '------------------------------------------------------------',
        '   1  WID-001     Widget                         4     79.96',
        '   2  BOOK-010    Field Guide to Ledgers         2     69.00',
        '   3  TEA-200     Loose Leaf Tea 250g            2     17.50',
        '   4  CBL-030     USB-C Cable 2m                 1     12.40',
        '   5  STK-005     Sticker Pack                   4     12.40',
        '------------------------------------------------------------',
        '      TOTAL                                     13    191.26',
        '',
      ].join('\n'),
    );
  });

  test('shortens long names', () => {
    const rows = [{ sku: 'X-1', name: 'A Remarkably Long Product Name', qty: 1, revenue: 1 }];
    assert.match(renderReport(rows), /A Remarkably Long Prod\.\.\. /);
  });

  test('says so when there is nothing to report', () => {
    assert.equal(renderReport([]), 'No sales.\n');
  });
});
