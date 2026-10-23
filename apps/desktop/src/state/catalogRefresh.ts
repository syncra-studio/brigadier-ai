import type { Conversation, Project } from "@/ipc/generated";
import { useApp } from "./store";

/** A row's live change during a read: removed, or the fields that changed (all of a new row). */
type Change<T> = { removed: true } | { removed: false; fields: Partial<T> };

/** Set while a catalog snapshot replaces the store, which is not a live change. */
let replacing = false;

/** Keep live catalog changes that arrive while a snapshot read is in flight (#535).
 * Deletions are recorded too, so a late snapshot cannot put removed rows back. Only the
 * fields that changed are kept: a cached row may be stale after a lag, and its other
 * fields come from the snapshot. */
export function trackCatalogChanges() {
  const projects = new Map<string, Change<Project>>();
  const conversations = new Map<string, Change<Conversation>>();
  const stop = useApp.subscribe((state, before) => {
    if (replacing) return;
    changed(state.projects, before.projects, projects);
    changed(state.conversations, before.conversations, conversations);
  });
  return {
    stop,
    merge: (snapshot: { projects: Project[]; conversations: Conversation[] }) => ({
      projects: merge(snapshot.projects, projects, useApp.getState().projects),
      conversations: merge(snapshot.conversations, conversations, useApp.getState().conversations),
    }),
  };
}

/** Replaces the store with a snapshot without other reads taking it for live changes. */
export function replacingCatalog(replace: () => void) {
  replacing = true;
  try {
    replace();
  } finally {
    replacing = false;
  }
}

function changed<T extends object>(rows: Record<string, T>, before: Record<string, T>, changes: Map<string, Change<T>>) {
  if (rows === before) return;
  for (const id of new Set([...Object.keys(before), ...Object.keys(rows)])) {
    const row = rows[id];
    const was = before[id];
    if (row === was) continue;
    if (!row) {
      changes.set(id, { removed: true });
      continue;
    }
    const known = changes.get(id);
    if (!was || known?.removed) {
      changes.set(id, { removed: false, fields: { ...row } });
      continue;
    }
    const fields: Partial<T> = known ? known.fields : {};
    for (const key of Object.keys(row) as (keyof T)[]) {
      if (row[key] !== was[key]) fields[key] = row[key];
    }
    changes.set(id, { removed: false, fields });
  }
}

function merge<T extends { id: string }>(snapshot: T[], changes: Map<string, Change<T>>, live: Record<string, T>): T[] {
  const rows = new Map(snapshot.map((row) => [row.id, row]));
  for (const [id, change] of changes) {
    if (change.removed) {
      rows.delete(id);
      continue;
    }
    const row = rows.get(id);
    // A row the snapshot lacks but the live feed changed stays as the store has it.
    const kept = row ? { ...row, ...change.fields } : live[id];
    if (kept) rows.set(id, kept);
  }
  return [...rows.values()];
}
