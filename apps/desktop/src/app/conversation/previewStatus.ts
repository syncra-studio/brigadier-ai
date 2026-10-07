import type { Preview } from "@/ipc/generated";

/** What the thread header's preview chip shows: what runs, and what its Stop stops. */
export type PreviewChipView = {
  /** "web app", or "2 previews". */
  label: string;
  /** The hover text: each running preview's command and folder. */
  title: string;
  /** The preview its Stop stops; `null` stops every running one. */
  stops: string | null;
};

/** The chip for a session's previews: only while one runs, the newest first in its hover text. */
export function previewChip(previews: Readonly<Record<string, Preview>>): PreviewChipView | null {
  const running = Object.values(previews)
    .filter((preview) => preview.state.type === "running")
    .toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const [newest] = running;
  if (!newest) return null;
  const title = running.map((preview) => `${preview.name}: ${preview.command} (in ${preview.workdir})`).join("\n");
  return running.length === 1
    ? { label: newest.name, title, stops: newest.id }
    : { label: `${running.length} previews`, title, stops: null };
}
