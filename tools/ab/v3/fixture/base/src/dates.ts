/**
 * Calendar dates as ISO `YYYY-MM-DD` strings.
 *
 * The arithmetic is done on the year/month/day fields directly so results
 * never depend on the local timezone of the machine running the code.
 */

export interface DateParts {
  year: number;
  month: number;
  day: number;
}

const ISO_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

const DAYS_PER_MONTH = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/** Number of days in `month` (1-12) of `year`. */
export function daysInMonth(year: number, month: number): number {
  if (!Number.isInteger(month) || month < 1 || month > 12) {
    throw new RangeError(`invalid month: ${month}`);
  }
  if (!Number.isInteger(year)) {
    throw new RangeError(`invalid year: ${year}`);
  }
  return DAYS_PER_MONTH[month - 1] as number;
}

/** Parses and validates an ISO date. Throws a RangeError when invalid. */
export function parse(iso: string): DateParts {
  const match = ISO_DATE.exec(iso);
  if (!match) {
    throw new RangeError(`invalid date: ${iso}`);
  }
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (month < 1 || month > 12 || day < 1 || day > daysInMonth(year, month)) {
    throw new RangeError(`invalid date: ${iso}`);
  }
  return { year, month, day };
}

/** Formats date parts as `YYYY-MM-DD`. */
export function format(parts: DateParts): string {
  const y = String(parts.year).padStart(4, '0');
  const m = String(parts.month).padStart(2, '0');
  const d = String(parts.day).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

/** True when `iso` is a valid calendar date. */
export function isValidDate(iso: string): boolean {
  try {
    parse(iso);
    return true;
  } catch {
    return false;
  }
}

/** Compares two ISO dates: -1 if a < b, 0 if equal, 1 if a > b. */
export function compare(a: string, b: string): -1 | 0 | 1 {
  parse(a);
  parse(b);
  if (a === b) {
    return 0;
  }
  return a < b ? -1 : 1;
}

/** Returns the date `n` days after `iso` (or before it, for negative `n`). */
export function addDays(iso: string, n: number): string {
  if (!Number.isInteger(n)) {
    throw new RangeError(`day count must be an integer: ${n}`);
  }
  const { year, month, day } = parse(iso);
  let y = year;
  let m = month;
  let d = day + n;

  if (d > daysInMonth(y, m)) {
    d -= daysInMonth(y, m);
    m += 1;
    if (m > 12) {
      m = 1;
      y += 1;
    }
  } else if (d < 1) {
    m -= 1;
    if (m < 1) {
      m = 12;
      y -= 1;
    }
    d += daysInMonth(y, m);
  }
  return format({ year: y, month: m, day: d });
}

/** The later of two ISO dates. */
export function maxDate(a: string, b: string): string {
  return compare(a, b) >= 0 ? a : b;
}
