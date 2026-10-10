import { useSyncExternalStore } from "react";

import { useApp } from "@/state/store";

/**
 * "Read aloud" under an answer, with the system's voices: one answer at a time, in the voice
 * chosen in Settings › Voice (the system's default when none is, or it is gone). Reading
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

/** The voice `uri` names among `voices`; null for the system's default (none chosen, or gone). */
export function voiceFor(uri: string, voices: readonly SpeechSynthesisVoice[]): SpeechSynthesisVoice | null {
  return uri ? (voices.find((voice) => voice.voiceURI === uri) ?? null) : null;
}

/** Reads `text` as answer `id`, stopping whatever was being read. */
export function readAloud(id: string, text: string): void {
  speak(id, spokenText(text));
}

/** Says a line in the chosen voice (Settings › Voice, when one is picked). */
export function sayInVoice(text: string): void {
  speak("sample", text);
}

function speak(id: string, text: string): void {
  speechSynthesis.cancel();
  const utterance = new SpeechSynthesisUtterance(text);
  const voice = voiceFor(useApp.getState().settings.readAloudVoice, speechSynthesis.getVoices());
  if (voice) {
    utterance.voice = voice;
    utterance.lang = voice.lang;
  }
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
