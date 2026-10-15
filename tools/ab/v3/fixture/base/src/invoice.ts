import type { Cart } from './cart.ts';
import type { Catalog } from './catalog.ts';
import { addDays, parse } from './dates.ts';
import { formatMoney, round2, sumMoney } from './money.ts';
import { lineTax, taxRate } from './tax.ts';

export interface InvoiceLine {
  sku: string;
  name: string;
  qty: number;
  unitPrice: number;
  lineTotal: number;
  tax: number;
}

export interface Invoice {
  number: string;
  issueDate: string;
  dueDate: string;
  region: string;
  lines: InvoiceLine[];
  subtotal: number;
  tax: number;
  total: number;
}

export interface InvoiceOptions {
  number: string;
  issueDate: string;
  /** Payment terms: the invoice is due this many days after the issue date. */
  termsDays: number;
  region: string;
}

export function buildInvoice(cart: Cart, catalog: Catalog, opts: InvoiceOptions): Invoice {
  if (cart.lines.length === 0) {
    throw new Error('cannot invoice an empty cart');
  }
  if (!Number.isInteger(opts.termsDays) || opts.termsDays < 0) {
    throw new RangeError(`invalid payment terms: ${opts.termsDays}`);
  }
  parse(opts.issueDate);
  // Fail early on an unknown region, even for exempt-only carts.
  taxRate(opts.region, 'standard');

  const lines: InvoiceLine[] = cart.lines.map((cartLine) => {
    const product = catalog.require(cartLine.sku);
    const lineTotal = round2(product.unitPrice * cartLine.qty);
    return {
      sku: product.sku,
      name: product.name,
      qty: cartLine.qty,
      unitPrice: product.unitPrice,
      lineTotal,
      tax: lineTax(lineTotal, opts.region, product.taxCategory),
    };
  });

  const subtotal = sumMoney(lines.map((line) => line.lineTotal));
  const tax = sumMoney(lines.map((line) => line.tax));
  return {
    number: opts.number,
    issueDate: opts.issueDate,
    dueDate: addDays(opts.issueDate, opts.termsDays),
    region: opts.region,
    lines,
    subtotal,
    tax,
    total: round2(subtotal + tax),
  };
}

const COL_SKU = 10;
const COL_NAME = 24;
const COL_QTY = 5;
const COL_PRICE = 10;
const COL_AMOUNT = 10;
const WIDTH = COL_SKU + COL_NAME + COL_QTY + COL_PRICE + COL_AMOUNT;

function fit(text: string, width: number): string {
  return text.length < width ? text : `${text.slice(0, width - 4)}...`;
}

/** A totals row: the label right-aligned under the line columns, then the amount. */
function totalLine(label: string, amount: string): string {
  return label.padStart(WIDTH - COL_AMOUNT) + amount.padStart(COL_AMOUNT);
}

/** Renders an invoice as fixed-width plain text, ending with a newline. */
export function renderInvoiceText(inv: Invoice): string {
  const out: string[] = [
    `INVOICE ${inv.number}`,
    `Issue date: ${inv.issueDate}`,
    `Due date:   ${inv.dueDate}`,
    `Region:     ${inv.region}`,
    '',
    'SKU'.padEnd(COL_SKU) +
      'Description'.padEnd(COL_NAME) +
      'Qty'.padStart(COL_QTY) +
      'Price'.padStart(COL_PRICE) +
      'Amount'.padStart(COL_AMOUNT),
    '-'.repeat(WIDTH),
  ];
  for (const line of inv.lines) {
    out.push(
      fit(line.sku, COL_SKU).padEnd(COL_SKU) +
        fit(line.name, COL_NAME).padEnd(COL_NAME) +
        String(line.qty).padStart(COL_QTY) +
        formatMoney(line.unitPrice).padStart(COL_PRICE) +
        formatMoney(line.lineTotal).padStart(COL_AMOUNT),
    );
  }
  out.push('-'.repeat(WIDTH));
  out.push(totalLine('Subtotal:', formatMoney(inv.subtotal)));
  out.push(totalLine('Tax:', formatMoney(inv.tax)));
  out.push(totalLine('Total:', formatMoney(inv.total)));
  return `${out.join('\n')}\n`;
}
