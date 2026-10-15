import { readFileSync, writeFileSync } from 'node:fs';
import { parseProduct, type Product } from './catalog.ts';
import type { Invoice } from './invoice.ts';

/** Persistence for products and invoices. */
export interface Store {
  getProduct(sku: string): Product | undefined;
  /** Inserts a product or replaces the one with the same SKU. */
  putProduct(product: Product): void;
  /** All products, sorted by SKU. */
  listProducts(): Product[];
  /** Saves a new invoice. Throws if an invoice with that number exists. */
  saveInvoice(invoice: Invoice): void;
  getInvoice(number: string): Invoice | undefined;
  /** All invoices, in the order they were saved. */
  listInvoices(): Invoice[];
}

/** The on-disk shape of a tally data file. */
export interface DataFile {
  products: Product[];
  invoices: Invoice[];
}

export class MemoryStore implements Store {
  #products = new Map<string, Product>();
  #invoices = new Map<string, Invoice>();

  constructor(data: Partial<DataFile> = {}) {
    for (const product of data.products ?? []) {
      this.putProduct(product);
    }
    for (const invoice of data.invoices ?? []) {
      this.saveInvoice(invoice);
    }
  }

  getProduct(sku: string): Product | undefined {
    return this.#products.get(sku);
  }

  putProduct(product: Product): void {
    this.#products.set(product.sku, product);
  }

  listProducts(): Product[] {
    return [...this.#products.values()].sort((a, b) => (a.sku < b.sku ? -1 : a.sku > b.sku ? 1 : 0));
  }

  saveInvoice(invoice: Invoice): void {
    if (this.#invoices.has(invoice.number)) {
      throw new Error(`invoice ${invoice.number} already exists`);
    }
    this.#invoices.set(invoice.number, invoice);
  }

  getInvoice(number: string): Invoice | undefined {
    return this.#invoices.get(number);
  }

  listInvoices(): Invoice[] {
    return [...this.#invoices.values()];
  }

  /** Snapshot of the store in data-file shape. */
  toData(): DataFile {
    return { products: this.listProducts(), invoices: this.listInvoices() };
  }
}

/** Validates an untrusted value (parsed JSON) as a data file. */
export function parseDataFile(raw: unknown): DataFile {
  if (typeof raw !== 'object' || raw === null) {
    throw new TypeError('data file must contain a JSON object');
  }
  const r = raw as Record<string, unknown>;
  const products = r.products ?? [];
  const invoices = r.invoices ?? [];
  if (!Array.isArray(products) || !Array.isArray(invoices)) {
    throw new TypeError('data file: products and invoices must be arrays');
  }
  for (const invoice of invoices) {
    if (typeof invoice !== 'object' || invoice === null || typeof invoice.number !== 'string') {
      throw new TypeError('data file: every invoice needs a number');
    }
  }
  return { products: products.map(parseProduct), invoices: invoices as Invoice[] };
}

export function readDataFile(path: string): DataFile {
  return parseDataFile(JSON.parse(readFileSync(path, 'utf8')));
}

export function writeDataFile(path: string, data: DataFile): void {
  writeFileSync(path, `${JSON.stringify(data, null, 2)}\n`);
}
