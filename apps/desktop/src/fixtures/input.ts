/** A user's input a live fixture asks for (see `renderFixtureLive`). */
export type FixtureInput =
  | { type: "wheel"; x: number; y: number; deltaX?: number; deltaY?: number }
  | { type: "viewport"; width: number; height: number }
  /** A real key press (down, then up) to whatever has focus, as the keyboard sends it. */
  | { type: "key"; key: string }
  /** A real click (pointer down, then up) at a point in the page. */
  | { type: "click"; x: number; y: number }
  /** The pointer moved to a point, without a click. */
  | { type: "pointer"; x: number; y: number };

/** A real click at the middle of `element`, as the user's pointer would give it. */
export function fixtureClick(element: Element): Promise<void> {
  const rect = element.getBoundingClientRect();
  return fixtureInput({ type: "click", x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 });
}

/**
 * In a live fixture's page: has the test's browser turn the wheel, resize the window, press a
 * key or click, as a user would, and resolves once it did.
 */
export function fixtureInput(input: FixtureInput): Promise<void> {
  const page = window as unknown as {
    fixtureInput?: (payload: string) => void;
    fixtureInputDone?: (id: number) => void;
    fixtureInputWaiting?: Map<number, () => void>;
  };
  const waiting = (page.fixtureInputWaiting ??= new Map());
  page.fixtureInputDone ??= (id) => {
    waiting.get(id)?.();
    waiting.delete(id);
  };
  const id = waiting.size + Date.now();
  return new Promise((resolve) => {
    waiting.set(id, resolve);
    if (!page.fixtureInput) throw new Error("fixtureInput runs only under renderFixtureLive");
    page.fixtureInput(JSON.stringify({ ...input, id }));
  });
}
