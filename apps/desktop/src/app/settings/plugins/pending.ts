import { invoke } from "@tauri-apps/api/core";

import type { PluginKind, ProviderKind } from "@/ipc/generated";

/*
 * The Plugins page's actions the daemon doesn't list in its request types yet: each is sent as
 * the request it will be, so it works as soon as the daemon answers it, and fails with the
 * daemon's own words until then. Once the requests are generated this file goes and live.ts
 * calls `request` with them.
 */

/** Something read from what the user pasted, before it is added. */
export type PluginDraft = {
  name: string;
  kind: PluginKind;
  /** Its command, address or folder. */
  detail: string;
  /** Reached over the network. */
  remote: boolean;
  /** Signs in with the user's browser. */
  signsIn: boolean;
  /** Why it can't be added, in words for the user. */
  error: string | null;
};

/** How one agent took a change. */
export type PluginAgentResult = {
  agent: ProviderKind;
  ok: boolean;
  /** Why it didn't, in words for the user. */
  message: string | null;
};

async function send<T>(method: string, fields: Record<string, unknown>, answer: string): Promise<T> {
  try {
    const response = await invoke<Record<string, unknown>>("ipc_request", { request: { method, ...fields } });
    return response[answer] as T;
  } catch (cause) {
    const message = typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : String(cause);
    throw new Error(message, { cause });
  }
}

const results = (method: string, fields: Record<string, unknown>) => send<PluginAgentResult[]>(method, fields, "results");

export const pendingRequests = {
  switchPlugin: (key: string, projectId: string | null, on: boolean) => results("switchPlugin", { key, projectId, on }),
  checkPluginAdd: (text: string) => send<PluginDraft[]>("checkPluginAdd", { text }, "drafts"),
  addPlugin: (text: string, name: string | null, projectId: string | null, shared: boolean) =>
    results("addPlugin", { text, name, projectId, shared }),
  useEverywhere: (key: string) => results("useEverywhere", { key }),
  undoUseEverywhere: (key: string) => results("undoUseEverywhere", { key }),
  removePlugin: (key: string) => results("removePlugin", { key }),
  updatePlugin: (key: string) => results("updatePlugin", { key }),
  /** Each agent's own sign-in to the item's server, one after another in the user's browser. */
  signInPlugin: (key: string, projectId: string | null) => results("signInPlugin", { key, projectId }),
  cancelPluginSignIn: (key: string) => results("cancelPluginSignIn", { key }),
};
