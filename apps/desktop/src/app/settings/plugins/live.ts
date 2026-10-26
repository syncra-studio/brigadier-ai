import type { PluginAgentResult, PluginDraft, PluginsAdapter, SignInStep } from "@/app/settings/plugins/adapter";
import { failures } from "@/app/settings/plugins/dialogs";
import { pickFolder, request } from "@/ipc/client";
import type { McpSignInProgress, PluginJobView } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { setFolderTrust } from "@/state/trust";

/** The agents' setup keeps changing while Settings is open; the page is cheap to read again. */
const POLL_MS = 5000;
/** How often a running change is asked how it goes: an in-memory read, so often costs nothing. */
const JOB_MS = 100;
/** How often a sign-in is asked how it goes: each agent's step shows as soon as it happens. */
const SIGN_IN_MS = 100;

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

/** Waits for sign-in `id` to finish, telling `onProgress` each agent's step as it goes; how each agent's went. */
async function signedIn(id: string, onProgress?: (steps: SignInStep[]) => void): Promise<PluginAgentResult[]> {
  for (;;) {
    const { progress } = await request({ method: "getMcpSignInProgress", id });
    onProgress?.(progress.agents.map(({ agent, state }) => ({ agent, state })));
    if (progress.finished) return signInResults(progress);
    await new Promise((resolve) => setTimeout(resolve, SIGN_IN_MS));
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
    read: async () => {
      const { plugins } = await request({ method: "getPlugins" });
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
    switchPlugin: async (key, on) => finished((await request({ method: "switchPlugin", key, on })).job),
    setScope: async (key, projectIds) => finished((await request({ method: "setPluginScope", key, projectIds })).job),
    share: async (key) => finished((await request({ method: "sharePlugin", key })).job),
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
      finished((await request({ method: "addPlugin", text, name, projectIds, ...(values ? { values } : {}) })).job),
    importPlugin: async (key, name, projectIds) => finished((await request({ method: "importPlugin", key, name, projectIds })).job),
    remove: async (key) => finished((await request({ method: "removePlugin", key })).job),
    update: async (key) => finished((await request({ method: "updatePlugin", key })).job),
    details: async (key) => (await request({ method: "getPluginItem", key })).item,
    saveSkill: async (key, path, contents, expected) => {
      const { outcome } = await request({ method: "savePluginSkill", key, path, contents, expected });
      // Refused: the file changed since it was opened. Nothing was written; here is what it holds now.
      if (outcome.type === "changedSinceOpened") return { type: "changed", current: outcome.current };
      const problem = failures(await finished(outcome.job));
      if (problem) throw new Error(problem);
      return { type: "saved" };
    },
    signIn: async (key, serverId, projectId, onProgress) => {
      const { id } = await request({ method: "signInMcpServer", serverId, projectId });
      signIns.set(key, id);
      try {
        return await signedIn(id, onProgress);
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
