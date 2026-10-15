import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Catalog, type Product } from '../src/catalog.ts';

export const PRODUCTS: Product[] = [
  { sku: 'WID-001', name: 'Widget', unitPrice: 19.99, taxCategory: 'standard' },
  { sku: 'BOOK-010', name: 'Field Guide to Ledgers', unitPrice: 34.5, taxCategory: 'reduced' },
  { sku: 'TEA-200', name: 'Loose Leaf Tea 250g', unitPrice: 8.75, taxCategory: 'exempt' },
  { sku: 'CBL-030', name: 'USB-C Cable 2m', unitPrice: 12.4, taxCategory: 'standard' },
];

export function sampleCatalog(): Catalog {
  return new Catalog(PRODUCTS);
}

/** Runs `fn` with a fresh temporary directory that is removed afterwards. */
export function withTempDir<T>(fn: (dir: string) => T): T {
  const dir = mkdtempSync(join(tmpdir(), 'tally-test-'));
  try {
    return fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
