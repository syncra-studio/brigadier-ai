import { create } from "zustand";

/**
 * A stop the user just asked for, shown at once while the daemon makes it (#498): its turn (the
 * composer's Stop) and its workers read as stopped until the real state catches up. The stores
 * keep the real state, so a failed stop shows it again as it is.
 */
export type Stop = {
  id: number;
  conversationId: string;
  /** The composer's Stop: the turn, and the answer it worked on, end too. */
  turn: boolean;
  tasks: readonly string[];
  /** When it was pressed: what the stopped workers show as their end. */
  atMs: number;
};

const NO_STOPS: readonly Stop[] = [];

export const useStopping = create<{ stops: readonly Stop[] }>(() => ({ stops: NO_STOPS }));

let next = 1;

export function addStop(stop: Omit<Stop, "id" | "atMs">): Stop {
  const added = { ...stop, id: next++, atMs: Date.now() };
  useStopping.setState(({ stops }) => ({ stops: [...stops, added] }));
  return added;
}

export function dropStop(id: number): void {
  useStopping.setState(({ stops }) => {
    const kept = stops.filter((stop) => stop.id !== id);
    return kept.length === stops.length ? { stops } : { stops: kept.length === 0 ? NO_STOPS : kept };
  });
}

/** Narrows stop `id` to the workers of it the daemon stopped. */
export function keepStopping(id: number, tasks: readonly string[]): void {
  useStopping.setState(({ stops }) => ({
    stops: stops.map((stop) => (stop.id === id ? { ...stop, tasks: stop.tasks.filter((task) => tasks.includes(task)) } : stop)),
  }));
}

/** The composer's Stops of `conversationId` show no more: a new message shows its real state. */
export function dropTurnStops(conversationId: string): void {
  for (const stop of useStopping.getState().stops) {
    if (stop.turn && stop.conversationId === conversationId) dropStop(stop.id);
  }
}

/** The stops of `conversationId`; the same empty list when there are none. */
export function stopsOf(stops: readonly Stop[], conversationId: string): readonly Stop[] {
  return stops.some((stop) => stop.conversationId === conversationId)
    ? stops.filter((stop) => stop.conversationId === conversationId)
    : NO_STOPS;
}

/** Whether the composer's Stop of `conversationId` is under way. */
export function turnStopping(stops: readonly Stop[], conversationId: string | null): boolean {
  return stops.some((stop) => stop.turn && stop.conversationId === conversationId);
}
