import { useApp } from "@/state/store";

/**
 * Whether the composer is the rich field (its text Markdown, shown as formatting) or the plain
 * one, which keeps code, Markdown and links as literal text (Settings › General › Composer).
 * Both hold the same string, so switching remounts the field with the open draft as it is.
 */
let forced: boolean | null = null;

export function useRichComposer(): boolean {
  const plain = useApp((s) => s.settings.plainTextComposer);
  return forced ?? !plain;
}

/** For fixtures and tests: the rich field on or off whatever the setting, or (null) as set. */
export function setRichComposer(on: boolean | null): void {
  forced = on;
}
