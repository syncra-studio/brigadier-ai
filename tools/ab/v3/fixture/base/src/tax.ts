import type { TaxCategory } from './catalog.ts';
import { round2 } from './money.ts';

/** Sales tax rates by region and tax category, as fractions (0.13 = 13%). */
export const TAX_RATES: Readonly<Record<string, Readonly<Record<TaxCategory, number>>>> = {
  AB: { standard: 0.05, reduced: 0.05, exempt: 0 },
  BC: { standard: 0.12, reduced: 0.07, exempt: 0 },
  ON: { standard: 0.13, reduced: 0.05, exempt: 0 },
  QC: { standard: 0.15, reduced: 0.05, exempt: 0 },
};

/** Known region codes, sorted. */
export function regions(): string[] {
  return Object.keys(TAX_RATES).sort();
}

export function isRegion(region: string): boolean {
  return Object.hasOwn(TAX_RATES, region);
}

export function taxRate(region: string, category: TaxCategory): number {
  const rates = Object.hasOwn(TAX_RATES, region) ? TAX_RATES[region] : undefined;
  if (!rates) {
    throw new Error(`unknown region: ${region}`);
  }
  return rates[category];
}

/** Tax owed on one invoice line, rounded to cents. */
export function lineTax(amount: number, region: string, category: TaxCategory): number {
  return round2(amount * taxRate(region, category));
}
