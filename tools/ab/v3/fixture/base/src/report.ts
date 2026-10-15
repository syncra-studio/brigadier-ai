import type { Invoice } from './invoice.ts';
import { formatMoney, round2 } from './money.ts';

export interface ReportRow {
  sku: string;
  name: string;
  qty: number;
  revenue: number;
}

/** Highest revenue first; equal revenue falls back to SKU order. */
function compareRows(a: ReportRow, b: ReportRow): number {
  if (a.revenue !== b.revenue) {
    return b.revenue - a.revenue;
  }
  return a.sku < b.sku ? -1 : a.sku > b.sku ? 1 : 0;
}

/**
 * Units sold and revenue (line totals before tax) per SKU across `invoices`,
 * sorted by revenue descending, then SKU ascending. The product name is taken
 * from the first line seen for each SKU.
 */
export function salesReport(invoices: Invoice[]): ReportRow[] {
  const rows: ReportRow[] = [];
  for (const invoice of invoices) {
    for (const line of invoice.lines) {
      let row: ReportRow | undefined;
      for (let i = 0; i < rows.length; i++) {
        if (rows[i].sku === line.sku) {
          row = rows[i];
          break;
        }
      }
      if (row) {
        row.qty += line.qty;
        row.revenue = round2(row.revenue + line.lineTotal);
      } else {
        rows.push({ sku: line.sku, name: line.name, qty: line.qty, revenue: line.lineTotal });
      }
      // Keep the rows ordered as we go so the result is always sorted.
      rows.sort(compareRows);
    }
  }
  return rows;
}

const RULE = '-'.repeat(60);

function reportLine(rank: string, sku: string, name: string, qty: string, revenue: string): string {
  return rank.padStart(4) + '  ' + sku.padEnd(12) + name.padEnd(26) + qty.padStart(6) + revenue.padStart(10);
}

/** Renders report rows as a fixed-width table, ending with a newline. */
export function renderReport(rows: ReportRow[]): string {
  if (rows.length === 0) {
    return 'No sales.\n';
  }
  let out = reportLine('#', 'SKU', 'Product', 'Qty', 'Revenue') + '\n' + RULE + '\n';
  for (const row of rows) {
    const rank = rows.indexOf(row) + 1;
    const name = row.name.length > 25 ? row.name.slice(0, 22) + '...' : row.name;
    out += reportLine(String(rank), row.sku, name, String(row.qty), formatMoney(row.revenue)) + '\n';
  }
  const totalQty = rows.reduce((sum, row) => sum + row.qty, 0);
  const totalRevenue = round2(rows.reduce((sum, row) => sum + row.revenue, 0));
  out += RULE + '\n';
  out += reportLine('', 'TOTAL', '', String(totalQty), formatMoney(totalRevenue)) + '\n';
  return out;
}
