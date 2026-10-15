import { isMoney } from './money.ts';

export type TaxCategory = 'standard' | 'reduced' | 'exempt';

export const TAX_CATEGORIES: readonly TaxCategory[] = ['standard', 'reduced', 'exempt'];

export interface Product {
  sku: string;
  name: string;
  unitPrice: number;
  taxCategory: TaxCategory;
}

export function isTaxCategory(value: unknown): value is TaxCategory {
  return typeof value === 'string' && (TAX_CATEGORIES as readonly string[]).includes(value);
}

const SKU_PATTERN = /^[A-Z0-9][A-Z0-9-]*$/;

/** Validates an untrusted value (e.g. parsed JSON) as a Product. */
export function parseProduct(raw: unknown): Product {
  if (typeof raw !== 'object' || raw === null) {
    throw new TypeError('product must be an object');
  }
  const r = raw as Record<string, unknown>;
  if (typeof r.sku !== 'string' || !SKU_PATTERN.test(r.sku)) {
    throw new TypeError(`invalid sku: ${String(r.sku)}`);
  }
  if (typeof r.name !== 'string' || r.name.trim() === '') {
    throw new TypeError(`product ${r.sku} has no name`);
  }
  if (!isMoney(r.unitPrice) || r.unitPrice < 0) {
    throw new TypeError(`product ${r.sku} has an invalid unit price`);
  }
  if (!isTaxCategory(r.taxCategory)) {
    throw new TypeError(`product ${r.sku} has an invalid tax category`);
  }
  return { sku: r.sku, name: r.name, unitPrice: r.unitPrice, taxCategory: r.taxCategory };
}

/** An in-memory lookup of products by SKU. */
export class Catalog {
  #bySku = new Map<string, Product>();

  constructor(products: Iterable<Product> = []) {
    for (const product of products) {
      this.add(product);
    }
  }

  add(product: Product): void {
    if (this.#bySku.has(product.sku)) {
      throw new Error(`duplicate sku: ${product.sku}`);
    }
    this.#bySku.set(product.sku, product);
  }

  has(sku: string): boolean {
    return this.#bySku.has(sku);
  }

  get(sku: string): Product | undefined {
    return this.#bySku.get(sku);
  }

  /** Like `get`, but throws when the SKU is unknown. */
  require(sku: string): Product {
    const product = this.#bySku.get(sku);
    if (!product) {
      throw new Error(`unknown sku: ${sku}`);
    }
    return product;
  }

  /** All products, sorted by SKU. */
  list(): Product[] {
    return [...this.#bySku.values()].sort((a, b) => (a.sku < b.sku ? -1 : a.sku > b.sku ? 1 : 0));
  }

  get size(): number {
    return this.#bySku.size;
  }
}
