import type { Conversation, Project } from "@/ipc/generated";
import { useApp } from "./store";

/** Keep live catalog changes that arrive while a snapshot read is in flight (#535).
 * Deletions are recorded too, so a late snapshot cannot put removed rows back. */
export function trackCatalogChanges() {
  const projects = new Map<string, Project | undefined>();
  const conversations = new Map<string, Conversation | undefined>();
  const stop = useApp.subscribe((state, before) => {
    changed(state.projects, before.projects, projects);
    changed(state.conversations, before.conversations, conversations);
  });
  return {
    stop,
    merge: (snapshot: { projects: Project[]; conversations: Conversation[] }) => ({
      projects: merge(snapshot.projects, projects),
      conversations: merge(snapshot.conversations, conversations),
    }),
  };
}

function changed<T>(rows: Record<string, T>, before: Record<string, T>, changes: Map<string, T | undefined>) {
  if (rows === before) return;
  for (const id of new Set([...Object.keys(before), ...Object.keys(rows)])) {
    if (rows[id] !== before[id]) changes.set(id, rows[id]);
  }
}

function merge<T extends { id: string }>(snapshot: T[], changes: Map<string, T | undefined>): T[] {
  const rows = new Map(snapshot.map((row) => [row.id, row]));
  for (const [id, row] of changes) {
    if (row) rows.set(id, row);
    else rows.delete(id);
  }
  return [...rows.values()];
}
