import { createContext, type FC, type ReactNode, useCallback, useContext, useMemo, useState } from "react";

/*
 * What the user opened or closed in a message of the thread, kept while the app runs. The thread
 * mounts only the messages near the view (see `thread.tsx`), so a message scrolled away and back
 * is mounted afresh: a "Show more", a group of steps or a command's output the user opened would
 * close again, and the message would change height under the view. A message's row names the
 * scope (its conversation and message), `KeptScope` a part inside it (a group, a step), and
 * `useKeptState` keeps its value there.
 */

const kept = new Map<string, unknown>();
/** How many times something was kept in each row, by the row's key: what it shows may differ since. */
const changes = new Map<string, number>();

/** The key of message `id`'s row in the thread kept under `threadKey`. */
export function rowKeyOf(threadKey: string | null, id: string): string {
  return `${threadKey ?? ""}:${id}`;
}

/** How many times something was kept in row `rowKey`: it changes as the user opens or closes a part of it. */
export function keptChanges(rowKey: string): number {
  return changes.get(rowKey) ?? 0;
}

/** Where values kept below are filed: the row's key, then each scope's name, and the row's key alone. */
const ScopeContext = createContext<string | null>(null);
const RowContext = createContext<string | null>(null);

/** Files what is kept below under `name`, inside the scope around it. */
export const KeptScope: FC<{ name: string; children: ReactNode }> = ({ name, children }) => {
  const outer = useContext(ScopeContext);
  const scope = useMemo(() => (outer === null ? null : `${outer}/${name}`), [outer, name]);
  return <ScopeContext.Provider value={scope}>{children}</ScopeContext.Provider>;
};

/** The scope of a message's row: `key` names the conversation and the message. */
export const RowScope: FC<{ rowKey: string; children: ReactNode }> = ({ rowKey, children }) => (
  <RowContext.Provider value={rowKey}>
    <ScopeContext.Provider value={rowKey}>{children}</ScopeContext.Provider>
  </RowContext.Provider>
);

/**
 * `useState`, kept under `name` in the scope around it for the app's run, so the message mounted
 * again shows as the user left it. Outside a row it is plain state.
 */
export function useKeptState<T>(name: string, initial: T): [T, (next: T) => void] {
  const scope = useContext(ScopeContext);
  const row = useContext(RowContext);
  const key = scope === null ? null : `${scope}/${name}`;
  const [value, setValue] = useState<T>(() => (key !== null && kept.has(key) ? (kept.get(key) as T) : initial));
  const set = useCallback(
    (next: T) => {
      if (key !== null) kept.set(key, next);
      if (row !== null) changes.set(row, (changes.get(row) ?? 0) + 1);
      setValue(next);
    },
    [key, row],
  );
  return [value, set];
}
