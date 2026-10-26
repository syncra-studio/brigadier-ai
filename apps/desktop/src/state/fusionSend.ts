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

const armed = new Set<string>();
const listeners = new Set<() => void>();

export function armFusionSend(scope: string, on: boolean): void {
  if (armed.has(scope) === on) return;
  if (on) armed.add(scope);
  else armed.delete(scope);
  for (const listener of listeners) listener();
}

export function takeFusionSend(scope: string): boolean {
  const on = armed.has(scope);
  if (on) armFusionSend(scope, false);
  return on;
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
