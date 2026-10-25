import type { PluginAgentResult, PluginDraft, PluginsAdapter } from "@/app/settings/plugins/adapter";
import { failures } from "@/app/settings/plugins/dialogs";
import type { NextRequest, PluginItemDetails, PluginsView } from "@/app/settings/plugins/types";
import { pickFolder, request } from "@/ipc/client";
import type { McpSignInProgress, PluginJobView } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { setFolderTrust } from "@/state/trust";

/** The agents' setup keeps changing while Settings is open; the page is cheap to read again. */
const POLL_MS = 5000;
/** How often a running change is asked how it goes. */
const JOB_MS = 300;

/** Waits for change `id` to finish. */
async function settled(id: string): Promise<PluginJobView> {
  for (;;) {
    const { job } = await request({ method: "getPluginJob", id });
    if (job.finished) return job;
    await new Promise((resolve) => setTimeout(resolve, JOB_MS));
  }
}

/** Waits for change `id` to finish; each agent's part, or why it stopped. */
const finished = async (id: string): Promise<PluginAgentResult[]> => results(await settled(id));

function results(job: PluginJobView): PluginAgentResult[] {
  if (job.error) throw new Error(job.error);
  // What the user should know although it worked goes with the first agent that did it.
  const first = job.agents.findIndex(({ step }) => step.type === "done");
  return job.agents.map(({ agent, step }, index) => ({
    agent,
    ok: step.type !== "failed",
    message: step.type === "failed" || step.type === "skipped" ? step.reason || null : index === first ? job.warning : null,
  }));
}

/** The sign-in going on for each item, by its key: what Cancel stops. */
const signIns = new Map<string, string>();

/** Waits for sign-in `id` to finish; how each agent's went. */
async function signedIn(id: string): Promise<PluginAgentResult[]> {
  for (;;) {
    const { progress } = await request({ method: "getMcpSignInProgress", id });
    if (progress.finished) return signInResults(progress);
    await new Promise((resolve) => setTimeout(resolve, JOB_MS));
  }
}

function signInResults(progress: McpSignInProgress): PluginAgentResult[] {
  return progress.agents.map(({ agent, state }) => ({
    agent,
    ok: state.type !== "failed" && state.type !== "cancelled",
    message: state.type === "failed" ? state.reason : state.type === "cancelled" ? "Cancelled." : null,
  }));
}

/**
 * The shared-plugins requests, until the generated `Request` and `Response` have them: each
 * answers as the protocol says (`{ plugins }`, `{ item }`, else `{ job }`). Goes when ts-rs
 * generates them; the calls below then go through `request` as they are.
 */
const next = request as unknown as {
  (call: Extract<NextRequest, { method: "getPlugins" }>): Promise<{ plugins: PluginsView | null }>;
  (call: Extract<NextRequest, { method: "getPluginItem" }>): Promise<{ item: PluginItemDetails }>;
  (call: Exclude<NextRequest, { method: "getPlugins" | "getPluginItem" }>): Promise<{ job: string }>;
};

/** The Plugins page over the daemon. */
export function livePlugins(): PluginsAdapter {
  return {
    read: async () => {
      const { plugins } = await next({ method: "getPlugins" });
      // An answer without the page (an older daemon, a test's stand-in) keeps the last one.
      if (!plugins) throw new Error("Brigadier didn't send the plugins.");
      return plugins;
    },
    subscribe: (onChange) => {
      const timer = setInterval(onChange, POLL_MS);
      const unsubscribe = useApp.subscribe((state, previous) => {
        if (state.projects !== previous.projects) onChange();
      });
      return () => {
        clearInterval(timer);
        unsubscribe();
      };
    },
    switchPlugin: async (key, on) => finished((await next({ method: "switchPlugin", key, on })).job),
    setScope: async (key, projectIds) => finished((await next({ method: "setPluginScope", key, projectIds })).job),
    share: async (key) => finished((await next({ method: "sharePlugin", key })).job),
    checkAdd: async (text, name) => {
      const { check } = await request({ method: "checkPluginAdd", text, name });
      if (check.needsName && !name?.trim()) return { drafts: [], needsName: true };
      if (check.error) throw new Error(check.error);
      const drafts = check.items.map((item): PluginDraft => ({
        name: item.name,
        kind: item.kind,
        detail: item.detail,
        remote: item.remote,
        signsIn: item.remote && item.secrets.length === 0,
        secrets: item.secrets,
        error: null,
      }));
      return { drafts, needsName: false };
    },
    add: async (text, name, projectIds, values) =>
      finished((await next({ method: "addPlugin", text, name, projectIds, ...(values ? { values } : {}) })).job),
    remove: async (key) => finished((await next({ method: "removePlugin", key })).job),
    update: async (key) => finished((await next({ method: "updatePlugin", key })).job),
    details: async (key) => (await next({ method: "getPluginItem", key })).item,
    saveSkill: async (key, path, contents, expected) => {
      const job = await settled((await next({ method: "savePluginSkill", key, path, contents, expected })).job);
      // A refusal because the file changed since it was opened: its own answer, not a failure.
      if (job.error && /changed since/i.test(job.error)) return "changed";
      const problem = failures(results(job));
      if (problem) throw new Error(problem);
      return "saved";
    },
    signIn: async (key, serverId, projectId) => {
      const { id } = await request({ method: "signInMcpServer", serverId, projectId });
      signIns.set(key, id);
      try {
        return await signedIn(id);
      } finally {
        if (signIns.get(key) === id) signIns.delete(key);
      }
    },
    cancelSignIn: async (key) => {
      const id = signIns.get(key);
      if (id) await request({ method: "cancelMcpSignIn", id });
    },
    pickFolder: () => pickFolder(),
    trustProject: async (projectId) => {
      await setFolderTrust(projectId, null, true);
    },
  };
}
