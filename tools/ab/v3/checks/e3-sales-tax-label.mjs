// e3: the printed invoice's tax total is labelled "Sales tax:", right-aligned like the other totals.
// usage: node e3-sales-tax-label.mjs <result-dir>
import assert from 'node:assert/strict';
const dir = process.argv[2];
const { buildInvoice, renderInvoiceText } = await import(`${dir}/src/invoice.ts`);
const { Catalog } = await import(`${dir}/src/catalog.ts`);
const catalog = new Catalog([{ sku: 'WID-001', name: 'Widget', unitPrice: 19.99, taxCategory: 'standard' }]);
const inv = buildInvoice({ lines: [{ sku: 'WID-001', qty: 3 }] }, catalog,
  { number: 'INV-0009', issueDate: '2024-03-15', termsDays: 30, region: 'ON' });
const lines = renderInvoiceText(inv).split('\n');
const sales = lines.filter((l) => /^\s*Sales tax:\s+7\.80$/.test(l));
const total = lines.filter((l) => /^\s*Total:\s+67\.77$/.test(l));
assert.equal(sales.length, 1, 'one "Sales tax:" line with the amount');
assert.equal(total.length, 1);
assert.equal(sales[0].length, total[0].length, 'aligned with Total');
assert.equal(lines.filter((l) => /^\s*Tax:/.test(l)).length, 0, 'no "Tax:" line left');
console.log('e3 ok');
