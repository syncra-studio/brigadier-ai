import type { Catalog } from './catalog.ts';
import { round2, sumMoney } from './money.ts';

export interface CartLine {
  readonly sku: string;
  readonly qty: number;
}

/** A cart is immutable: every operation returns a new cart. */
export interface Cart {
  readonly lines: readonly CartLine[];
}

function assertQty(qty: number, allowZero = false): void {
  const min = allowZero ? 0 : 1;
  if (!Number.isInteger(qty) || qty < min) {
    throw new RangeError(`invalid quantity: ${qty}`);
  }
}

export function emptyCart(): Cart {
  return { lines: [] };
}

/** Adds `qty` units of `sku`, merging with an existing line for that SKU. */
export function addItem(cart: Cart, sku: string, qty = 1): Cart {
  assertQty(qty);
  const existing = cart.lines.find((line) => line.sku === sku);
  if (!existing) {
    return { ...cart, lines: [...cart.lines, { sku, qty }] };
  }
  return {
    ...cart,
    lines: cart.lines.map((line) => (line.sku === sku ? { sku, qty: line.qty + qty } : line)),
  };
}

export function removeItem(cart: Cart, sku: string): Cart {
  return { ...cart, lines: cart.lines.filter((line) => line.sku !== sku) };
}

/** Sets the quantity of `sku`; zero removes the line, a new SKU is appended. */
export function setQty(cart: Cart, sku: string, qty: number): Cart {
  assertQty(qty, true);
  if (qty === 0) {
    return removeItem(cart, sku);
  }
  if (!cart.lines.some((line) => line.sku === sku)) {
    return { ...cart, lines: [...cart.lines, { sku, qty }] };
  }
  return { ...cart, lines: cart.lines.map((line) => (line.sku === sku ? { sku, qty } : line)) };
}

/** Total number of units in the cart. */
export function cartQuantity(cart: Cart): number {
  return cart.lines.reduce((sum, line) => sum + line.qty, 0);
}

/** Sum of qty x unit price over all lines, rounded to cents. */
export function cartSubtotal(cart: Cart, catalog: Catalog): number {
  return sumMoney(cart.lines.map((line) => round2(catalog.require(line.sku).unitPrice * line.qty)));
}

/** Validates an untrusted value of the form `{ "lines": [{ "sku", "qty" }] }`. */
export function parseCart(raw: unknown): Cart {
  if (typeof raw !== 'object' || raw === null || !Array.isArray((raw as { lines?: unknown }).lines)) {
    throw new TypeError('cart must be an object with a lines array');
  }
  let cart = emptyCart();
  for (const entry of (raw as { lines: unknown[] }).lines) {
    if (typeof entry !== 'object' || entry === null) {
      throw new TypeError('cart line must be an object');
    }
    const { sku, qty } = entry as Record<string, unknown>;
    if (typeof sku !== 'string' || typeof qty !== 'number') {
      throw new TypeError('cart line needs a string sku and a numeric qty');
    }
    cart = addItem(cart, sku, qty);
  }
  return cart;
}

/** A percentage off the subtotal (work in progress: not used by invoices yet). */
export function percentOff(subtotal: number, percent: number): number {
  if (!Number.isInteger(percent) || percent < 0 || percent > 100) {
    throw new RangeError(`invalid percent: ${percent}`);
  }
  return round2(subtotal * (1 - percent / 100));
}
