import type { Cart } from './cart.ts';
import { Catalog, type Product } from './catalog.ts';
import { compare } from './dates.ts';
import { buildInvoice, type Invoice } from './invoice.ts';
import type { Store } from './storage.ts';

export const INVOICE_PREFIX = 'INV-';

export function catalogFromStore(store: Store): Catalog {
  return new Catalog(store.listProducts());
}

/** The next free invoice number: INV-0001, INV-0002, ... */
export function nextInvoiceNumber(store: Store, prefix = INVOICE_PREFIX): string {
  let highest = 0;
  for (const invoice of store.listInvoices()) {
    if (!invoice.number.startsWith(prefix)) {
      continue;
    }
    const n = Number(invoice.number.slice(prefix.length));
    if (Number.isInteger(n) && n > highest) {
      highest = n;
    }
  }
  return `${prefix}${String(highest + 1).padStart(4, '0')}`;
}

export interface CreateInvoiceOptions {
  issueDate: string;
  termsDays: number;
  region: string;
  /** Defaults to `nextInvoiceNumber(store)`. */
  number?: string;
}

/** Builds an invoice for `cart` from the store's catalog and saves it. */
export function createInvoiceFromCart(store: Store, cart: Cart, opts: CreateInvoiceOptions): Invoice {
  const invoice = buildInvoice(cart, catalogFromStore(store), {
    number: opts.number ?? nextInvoiceNumber(store),
    issueDate: opts.issueDate,
    termsDays: opts.termsDays,
    region: opts.region,
  });
  store.saveInvoice(invoice);
  return invoice;
}

/** Inserts or replaces products; returns how many were written. */
export function importProducts(store: Store, products: readonly Product[]): number {
  for (const product of products) {
    store.putProduct(product);
  }
  return products.length;
}

/** Invoices issued between `from` and `to`, both inclusive; either bound may be omitted. */
export function invoicesInRange(store: Store, from?: string, to?: string): Invoice[] {
  return store.listInvoices().filter((invoice) => {
    if (from !== undefined && compare(invoice.issueDate, from) < 0) {
      return false;
    }
    if (to !== undefined && compare(invoice.issueDate, to) > 0) {
      return false;
    }
    return true;
  });
}

/** Invoices whose due date is strictly before `asOf`. */
export function overdueInvoices(store: Store, asOf: string): Invoice[] {
  return store.listInvoices().filter((invoice) => compare(invoice.dueDate, asOf) < 0);
}
