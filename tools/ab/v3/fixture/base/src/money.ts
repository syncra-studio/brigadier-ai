/**
 * Money helpers.
 *
 * Amounts are plain numbers in the major unit: 12.34 means twelve units and
 * thirty-four cents. Every stored amount is rounded to two decimal places.
 */

/** Rounds to two decimal places; halves round away from zero. */
export function round2(n: number): number {
  if (!Number.isFinite(n)) {
    throw new RangeError(`not a finite amount: ${n}`);
  }
  const sign = n < 0 ? -1 : 1;
  const rounded = Math.round((Math.abs(n) + Number.EPSILON) * 100) / 100;
  return rounded === 0 ? 0 : sign * rounded;
}

/** Formats an amount with exactly two decimals, e.g. 12.3 -> "12.30". */
export function formatMoney(n: number): string {
  return round2(n).toFixed(2);
}

/** Sums amounts and rounds the result to cents. */
export function sumMoney(values: Iterable<number>): number {
  let total = 0;
  for (const v of values) {
    total += v;
  }
  return round2(total);
}

/** True when `n` is a finite amount with no more than two decimals. */
export function isMoney(n: unknown): n is number {
  return typeof n === 'number' && Number.isFinite(n) && Math.abs(round2(n) - n) < 1e-9;
}
