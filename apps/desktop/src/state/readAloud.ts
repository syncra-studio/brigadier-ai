import { useSyncExternalStore } from "react";

/**
 * "Read aloud" under an answer, in the system's default voice: one answer at a time. Reading
 * another stops the first.
 */

const listeners = new Set<() => void>();
let current: { id: string; utterance: SpeechSynthesisUtterance } | null = null;

function changed(): void {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** What is read of an answer's Markdown: its words, not its marks, links or code fences. */
export function spokenText(markdown: string): string {
  return markdown
    .replace(/```[^\n]*\n([\s\S]*?)```/g, "$1")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(#{1,6}|>|[-*+]|\d+\.)\s+/gm, "")
    .replace(/[*_~`|]/g, "")
    .trim();
}

/** Reads `text` as answer `id`, stopping whatever was being read. */
export function readAloud(id: string, text: string): void {
  speak(id, spokenText(text));
}

function speak(id: string, text: string): void {
  speechSynthesis.cancel();
  // No voice is set: the system's default voice speaks.
  const utterance = new SpeechSynthesisUtterance(text);
  const ended = () => {
    if (current?.utterance !== utterance) return;
    current = null;
    changed();
  };
  utterance.addEventListener("end", ended);
  utterance.addEventListener("error", ended);
  current = { id, utterance };
  speechSynthesis.speak(utterance);
  changed();
}

export function stopReading(): void {
  current = null;
  speechSynthesis.cancel();
  changed();
}

/** Whether answer `id` is being read now. */
export function useReading(id: string): boolean {
  return useSyncExternalStore(subscribe, () => current?.id === id);
}
