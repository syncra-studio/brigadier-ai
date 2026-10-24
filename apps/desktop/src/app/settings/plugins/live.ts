import { usePlugins, type PluginAgentResult, type PluginDraft, type PluginsAdapter } from "@/app/settings/plugins/adapter";
import { pickFolder, request } from "@/ipc/client";
import type { McpSignInProgress, PluginJobView } from "@/ipc/generated";
import { openAddProject } from "@/state/addProject";
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

/** The Plugins page over the daemon. */
export function livePlugins(): PluginsAdapter {
  return {
    read: async (projectId) => {
      const { plugins } = await request({ method: "getPlugins", projectId });
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
    switchPlugin: async (key, projectId, on, agent) =>
      finished((await request({ method: "switchPlugin", key, projectId, on, agent: agent ?? null })).job),
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
        error: null,
      }));
      return { drafts, needsName: false };
    },
    add: async (text, name, projectId, shared) =>
      finished((await request({ method: "addPlugin", text, name, projectId, shared })).job),
    everyAgent: async (key) => {
      const job = await settled((await request({ method: "useEverywhere", key })).job);
      return { results: results(job), key: job.keys[0] ?? null };
    },
    undoEveryAgent: async (key) => finished((await request({ method: "undoUseEverywhere", key })).job),
    remove: async (key) => finished((await request({ method: "removePlugin", key })).job),
    update: async (key) => finished((await request({ method: "updatePlugin", key })).job),
    signIn: async (key, projectId) => {
      const serverId = usePlugins.getState().view.items.find((item) => item.key === key)?.serverId;
      if (!serverId) throw new Error("It has nothing to sign in to.");
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
    addProject: () => openAddProject(),
  };
}
