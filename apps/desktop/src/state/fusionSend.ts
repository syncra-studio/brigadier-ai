import { useSyncExternalStore } from "react";

import type { Conversation, ProviderOverview, Settings } from "@/ipc/generated";
import { modelAvailable, providerOn } from "@/state/providers";
import { useApp } from "@/state/store";

export const FUSION_HINT = "Needs two ready CLIs";

export function isFusionChat(conversation: Conversation): boolean {
  return conversation.kind === "chat" && !conversation.sideOf && !conversation.tabChat && conversation.lifecycle !== "archived";
}

export function fusionReady(providers: readonly ProviderOverview[], settings: Settings): boolean {
  return providers.filter((overview) => {
    if (!providerOn(settings, overview.provider) || !overview.status?.path || !overview.status.loggedIn) return false;
    const limit = overview.quota?.limit;
    if (limit && (limit.resetsAtMs === null || limit.resetsAtMs > Date.now())) return false;
    const models = overview.models?.models ?? settings.knownModels.filter((model) => model.provider === overview.provider);
    return models.length === 0 || models.some((model) => modelAvailable(settings, { ...model, provider: overview.provider }));
  }).length >= 2;
}

export function useFusionReady(): boolean {
  return useApp((s) => fusionReady(s.providers.view?.providers ?? [], s.settings));
}

const STORAGE_KEY = "brigadier.fusionSend";

function readArmed(): Set<string> {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]");
    return new Set(Array.isArray(stored) ? stored.filter((scope): scope is string => typeof scope === "string") : []);
  } catch {
    return new Set();
  }
}

const armed = readArmed();
const listeners = new Set<() => void>();

function changed(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...armed]));
  } catch {
    // As with drafts, unavailable or full storage leaves the current window working.
  }
  for (const listener of listeners) listener();
}

export function armFusionSend(scope: string, on: boolean): void {
  if (armed.has(scope) === on) return;
  if (on) armed.add(scope);
  else armed.delete(scope);
  changed();
}

/** A deleted Chat no longer keeps its toggle. */
export function forgetFusionSend(scope: string): void {
  armFusionSend(scope, false);
}

/** Fusion stays on for this Chat until its toggle is turned off. */
export function isFusionArmed(scope: string): boolean {
  return armed.has(scope);
}

/** A sent draft becomes this Chat; the next new-chat draft starts with Fusion off. */
export function moveFusionSend(from: string, to: string): void {
  if (from === to || !armed.delete(from)) return;
  armed.add(to);
  changed();
}

export function useFusionArmed(scope: string): boolean {
  return useSyncExternalStore(
    (listener) => { listeners.add(listener); return () => listeners.delete(listener); },
    () => armed.has(scope),
  );
}

/** A typed skill invocation keeps its question in view, with a provider-independent pill. */
export function fusionQuestion(text: string): string | null {
  const match = /^\s*[/$]fusion(?:\s+([\s\S]*))?$/u.exec(text);
  return match ? (match[1] ?? "").trim() : null;
}
