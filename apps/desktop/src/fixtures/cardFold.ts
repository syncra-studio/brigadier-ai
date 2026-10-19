/**
 * How a folding card's header behaves under real clicks (the Changes and Preview cards'
 * `?drive=fold`): a button in the header keeps its own action and leaves the card open, a click
 * on the header's badge folds it with the closing animation, then opens it with the opening one.
 * Read on the card's body as each starts: its state and the animation it runs ("none" with
 * reduced motion, when the body empties at once).
 */
import { fixtureClick } from "@/fixtures/input";

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

async function until(check: () => boolean, ms = 4000) {
  for (const end = performance.now() + ms; performance.now() < end && !check(); ) await frame();
  return check();
}

export type FoldSeen = {
  afterButton: number;
  folding: { state: string | null; animation: string } | null;
  closedWithinMs: number | null;
  unfolding: { state: string | null; animation: string } | null;
  reopened: number;
};

export async function driveFold(card: HTMLElement, button: HTMLElement, rows: () => number): Promise<FoldSeen> {
  const body = () => card.querySelector<HTMLElement>("[data-slot=card-fold-body]");
  const read = () => {
    const element = body();
    return element ? { state: element.dataset.state ?? null, animation: getComputedStyle(element).animationName } : null;
  };
  const badge = card.querySelector<HTMLElement>("[data-slot=card-fold-header] > :first-child")!;
  await fixtureClick(button);
  await frame();
  const afterButton = rows();
  await fixtureClick(badge);
  await frame();
  const folding = read();
  const start = performance.now();
  // Closed, the body stays as an empty hidden box.
  const closedWithinMs = (await until(() => !!body()?.hidden && rows() === 0)) ? Math.round(performance.now() - start) : null;
  await fixtureClick(badge);
  await frame();
  const unfolding = read();
  await until(() => rows() > 0);
  return { afterButton, folding, closedWithinMs, unfolding, reopened: rows() };
}
