import type { PluginsAdapter } from "@/app/settings/plugins/adapter";
import { pendingRequests } from "@/app/settings/plugins/pending";
import { pickFolder, request } from "@/ipc/client";
import { openAddProject } from "@/state/addProject";
import { useApp } from "@/state/store";
import { setFolderTrust } from "@/state/trust";

/** The agents' setup keeps changing while Settings is open; the page is cheap to read again. */
const POLL_MS = 5000;

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
    switchPlugin: pendingRequests.switchPlugin,
    checkAdd: pendingRequests.checkPluginAdd,
    add: pendingRequests.addPlugin,
    everyAgent: pendingRequests.useEverywhere,
    undoEveryAgent: pendingRequests.undoUseEverywhere,
    remove: pendingRequests.removePlugin,
    update: pendingRequests.updatePlugin,
    signIn: pendingRequests.signInPlugin,
    cancelSignIn: async (key) => {
      await pendingRequests.cancelPluginSignIn(key);
    },
    pickFolder: () => pickFolder(),
    trustProject: async (projectId) => {
      await setFolderTrust(projectId, null, true);
    },
    addProject: () => openAddProject(),
  };
}
