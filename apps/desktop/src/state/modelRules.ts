import { useEffect } from "react";
import { create } from "zustand";

import { request } from "@/ipc/client";
import type { GuidanceRow, ModelRule, ModelRules } from "@/ipc/generated";
import { useApp } from "@/state/store";

/**
 * Settings › Routing's data: the user's rules file (`~/.brigadier/models.md`) as the daemon
 * reads it, read again each time the page opens so a hand edit shows, and its saves.
 */
export const useModelRules = create<{ view: ModelRules | null; error: string | null }>()(() => ({
  view: null,
  error: null,
}));

const errorText = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

/** Reads the rules file. */
export async function loadModelRules(): Promise<void> {
  try {
    const { rules } = await request({ method: "getModelRules" });
    useModelRules.setState({ view: rules, error: null });
  } catch (cause) {
    useModelRules.setState({ error: errorText(cause) });
  }
}

/** Reads the rules file ahead of the page, with the daemon there. */
export function prefetchModelRules(): void {
  if (useApp.getState().connection.status === "connected") void loadModelRules();
}

/** Reads the rules file whenever the page opens (and on reconnecting); `once`, only while none was read yet. */
export function useModelRulesRead(once = false): void {
  const connected = useApp((s) => s.connection.status === "connected");
  useEffect(() => {
    if (connected && !(once && useModelRules.getState().view !== null)) void loadModelRules();
  }, [connected, once]);
}

/** Writes the rows and rules to the file; what the daemon read back replaces the view. */
export async function saveModelRules(file: ModelRules, rows: readonly GuidanceRow[], rules: readonly ModelRule[]): Promise<void> {
  const { rules: next } = await request({ method: "setModelRules", revision: file.revision, rows: [...rows], rules: [...rules] });
  useModelRules.setState({ view: next, error: null });
}

/** Has a model rewrite rules typed in the user's own words into exact rules and advice notes; nothing is saved. */
export async function readModelRules(text: string): Promise<ModelRule[]> {
  const { rules } = await request({ method: "readModelRules", text });
  return rules;
}
