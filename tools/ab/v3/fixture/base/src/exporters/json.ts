import type { Invoice } from '../invoice.ts';

/** Pretty-printed JSON array of invoices, ending with a newline. */
export function exportJson(invoices: Invoice[]): string {
  return `${JSON.stringify(invoices, null, 2)}\n`;
}
