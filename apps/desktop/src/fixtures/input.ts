/** A user's input a live fixture asks for (see `renderFixtureLive`). */
export type FixtureInput =
  | { type: "wheel"; x: number; y: number; deltaX?: number; deltaY?: number }
  | { type: "viewport"; width: number; height: number };

/**
 * In a live fixture's page: has the test's browser turn the wheel or resize the window, as a
 * user would, and resolves once it did.
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
