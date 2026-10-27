import { usePlugins, type PluginDraft, type PluginsAdapter } from "./adapter";
import { pickFolder, request } from "@/ipc/client";
import type { CatalogSignIn, CatalogView } from "@/ipc/generated";
import { useApp } from "@/state/store";
import type { PluginProjectView, PluginsView } from "./types";
import { signingIn } from "./model";

const agentName = (row: CatalogView["rows"][number]) => {
  const agents = new Set(row.definitions.map((definition) => definition.agent));
  return agents.has("claude") && agents.has("codex") ? "Claude Code + Codex" : agents.has("claude") ? "Claude Code" : "Codex";
};

/** The catalog's durable choices, rendered with the shared Settings grid. */
export function catalogPage(catalog: CatalogView, projects: PluginProjectView[]): PluginsView {
  const offers = catalog.rows.filter((row) => row.definitions.every((definition) => definition.agent === null));
  const rows = catalog.rows.filter((row) => !offers.includes(row));
  const names = rows.map((row) => {
    if (rows.filter((other) => other.kind === row.kind && other.name === row.name).length === 1) return row.name;
    const first = row.definitions[0];
    const library = row.definitions.some((definition) => /\/library\/(skills|servers|plugins)\//.test(definition.file));
    const source = projects.find((project) => project.path === first?.project)?.name ?? first?.plugin ?? (library ? "Library" : agentName(row));
    return `${row.name} · ${source}`;
  });
  const qualified = names.map((name, index) => names.filter((other, position) => other === name && rows[position]!.kind === rows[index]!.kind).length > 1 ? `${name} · ${agentName(rows[index]!)}` : name);
  const readableNames = qualified.map((name, index) => {
    const duplicates = qualified.flatMap((other, position) => other === name && rows[position]!.kind === rows[index]!.kind ? [position] : []);
    return duplicates.length > 1 ? `${name} (${duplicates.indexOf(index) + 1})` : name;
  });
  return {
    projects,
    elsewhere: offers.map((row) => {
      const definition = row.definitions[0]!;
      return { key: row.id, name: row.name, detail: definition.endpoint ?? definition.file, url: definition.transport === "http" ? definition.endpoint : null,
        foundIn: definition.file.includes("cursor") ? "Cursor" : "Claude Desktop", file: definition.file };
    }),
    items: rows.map((row, index) => {
      const first = row.definitions[0];
      const paths = row.choice.scope === "global" ? null : row.choice.scope;
      const projectIds = projects.filter((project) => paths?.includes(project.path)).map((project) => project.id);
      const files = [...new Set(row.definitions.map((definition) => definition.file))];
      const library = files.some((file) => /\/library\/(skills|servers|plugins)\//.test(file));
      return {
        key: row.id, kind: row.kind, name: readableNames[index]!, description: "", detail: first?.endpoint ?? first?.file.replace(/\/SKILL\.md$/, "") ?? "",
        url: first?.transport === "http" ? first.endpoint : null, icon: null,
        on: row.choice.on,
        scope: paths ? { type: "someProjects" as const, projectIds, folders: paths.filter((path) => !projects.some((project) => project.path === path)) } : { type: "allProjects" as const },
        differentPerAgent: row.differentPerAgent, signIn: row.signIn,
        problem: null, locked: null, removal: library ? "everything" as const : "hide" as const, canShare: false, skillCount: row.kind === "skill" ? 1 : 0, files,
      };
    }).toSorted((a, b) => a.name.localeCompare(b.name)),
  };
}

function currentProjects(): PluginProjectView[] {
  return Object.values(useApp.getState().projects).flatMap((project) => project.repos.filter((repo) => project.trust.some((trust) => trust.path === repo.path && trust.trusted)).map((repo) => ({
    id: repo.path, name: project.repos.length > 1 ? `${project.name} / ${repo.name}` : project.name, path: repo.path, notes: [],
  })));
}

/** Mutations answer after saving. Every completion is followed by a fresh catalog read. */
export function livePlugins(): PluginsAdapter {
  let view: PluginsView | null = usePlugins.getState().hasLoaded ? usePlugins.getState().view : null;
  let changed: ((view?: PluginsView) => void) | null = null;
  let polling = false;
  let revision = 0;
  let reads = 0;
  let catalogVersion: string | null = null;
  let checkingCatalog = false;
  const states = new Map<string, { revision: number; state: CatalogSignIn }>();
  const mutations = new Map<string, Promise<void>>();
  const update = (rowId: string, signIn: CatalogSignIn) => {
    states.set(rowId, { revision: ++revision, state: signIn });
    if (!view) return;
    view = { ...view, items: view.items.map((item) => item.key === rowId ? { ...item, signIn } : item) };
    changed?.(view);
  };
  // Serialize start/cancel/sign-out requests per row. A late start reply must not undo Cancel.
  const mutate = async (rowId: string, optimistic: CatalogSignIn, action: () => Promise<CatalogSignIn>) => {
    update(rowId, optimistic);
    const before = states.get(rowId);
    const operation = (mutations.get(rowId) ?? Promise.resolve()).catch(() => {}).then(async () => {
      let next: CatalogSignIn;
      try { next = await action(); }
      catch (cause) {
        if (states.get(rowId) === before) update(rowId, { type: "failed", message: cause instanceof Error ? cause.message : String(cause) });
        throw cause;
      }
      if (states.get(rowId) === before) update(rowId, next);
      if (next.type === "failed") throw new Error(next.message);
    });
    mutations.set(rowId, operation);
    try { await operation; }
    finally { if (mutations.get(rowId) === operation) mutations.delete(rowId); }
    return [];
  };
  const keepNewerStatuses = (catalog: CatalogView, before: number) => {
    for (const row of catalog.rows) {
      const latest = states.get(row.id);
      const collectingSteps = row.signIn.type === "cliFallback" && row.signIn.agents.length > 0 && !row.signIn.steps.length;
      if (latest && (latest.revision > before || mutations.has(row.id) || (collectingSteps && signingIn(latest.state)))) row.signIn = latest.state;
    }
  };
  const poll = async () => {
    if (polling || !view || !changed) return;
    polling = true;
    try {
      await Promise.all(view.items.filter((item) => signingIn(item.signIn) && !mutations.has(item.key)).map(async (item) => {
        const before = states.get(item.key);
        try {
          const { state } = await request({ method: "getPluginSignInStatus", rowId: item.key });
          if (changed && states.get(item.key) === before) {
            // The native CLI flow can take a moment to publish its first pair of steps.
            if (state.type !== "cliFallback" || state.steps.length || !state.agents.length) update(item.key, state);
          }
        } catch { /* A transient IPC failure must not stop following the pending flow. */ }
      }));
    } finally { polling = false; }
  };
  return {
    read: async () => {
      const read = ++reads;
      const before = revision;
      const { catalog } = await request({ method: "getPluginCatalog" });
      catalogVersion = catalog.version;
      keepNewerStatuses(catalog, before);
      // Show the cached daemon snapshot before waiting for independent sign-in status reads.
      const page = catalogPage(catalog, currentProjects());
      const shown = { ...page, items: page.items.map((item) => {
        const latest = states.get(item.key);
        return latest ? { ...item, signIn: latest.state } : item;
      }) };
      if (read === reads) { view = shown; changed?.(shown); }
      await Promise.all(catalog.rows.map(async (row) => {
        if (row.kind === "server" && row.signIn.type !== "notRequired" && row.definitions.some((definition) => definition.agent !== null)) {
          try {
            row.signIn = (await request({ method: "getPluginSignInStatus", rowId: row.id })).state;
          } catch (cause) {
            const pending = states.get(row.id)?.state ?? view?.items.find((item) => item.key === row.id)?.signIn;
            row.signIn = pending && signingIn(pending) ? pending : { type: "failed", message: cause instanceof Error ? cause.message : String(cause) };
          }
        }
      }));
      keepNewerStatuses(catalog, before);
      const fresh = catalogPage(catalog, currentProjects());
      if (read === reads) view = fresh;
      return fresh;
    },
    subscribe: (onChange) => {
      changed = onChange;
      const timer = setInterval(() => void poll(), 500);
      const catalogTimer = setInterval(() => {
        if (checkingCatalog || mutations.size) return;
        checkingCatalog = true;
        void request({ method: "getPluginCatalog" }).then(({ catalog }) => {
          if (changed && catalog.version !== catalogVersion) onChange();
        }).catch(() => {}).finally(() => { checkingCatalog = false; });
      }, 1000);
      const focused = () => onChange();
      window.addEventListener("focus", focused);
      const unsubscribe = useApp.subscribe((state, previous) => {
        if (state.projects !== previous.projects) onChange();
      });
      return () => { changed = null; clearInterval(timer); clearInterval(catalogTimer); window.removeEventListener("focus", focused); unsubscribe(); };
    },
    switchPlugin: async (rowId, on) => { await request({ method: "setPluginChoice", rowId, on }); return []; },
    setScope: async (rowId, paths) => { await request({ method: "setPluginChoiceScope", rowId, scope: paths ?? "global" }); return []; },
    checkAdd: async (text, name) => {
      const { check } = await request({ method: "checkPluginAdd", text, name });
      if (check.needsName && !name?.trim()) return { drafts: [], needsName: true };
      if (check.error) throw new Error(check.error);
      const drafts = check.items.map((item): PluginDraft => ({ name: item.name, kind: item.kind, detail: item.detail, remote: item.remote,
        signsIn: item.remote && item.secrets.length === 0, secrets: item.secrets, error: null }));
      return { drafts, needsName: false };
    },
    add: async (text, name, projectIds, values) => { await request({ method: "addPlugin", text, name, projectIds, ...(values ? { values } : {}) }); return []; },
    importPlugin: async (key, name, projectIds) => { await request({ method: "importPlugin", key, name, projectIds }); return []; },
    remove: async (key) => { await request({ method: "removePlugin", key }); return []; },
    update: async () => { throw new Error("Remove and add the library item to update it."); },
    details: async (key) => (await request({ method: "getPluginItem", key })).item,
    saveSkill: async (key, path, contents, expected) => {
      const { outcome } = await request({ method: "savePluginSkill", key, path, contents, expected });
      return outcome.type === "changedSinceOpened" ? { type: "changed", current: outcome.current } : { type: "saved" };
    },
    signIn: async (rowId) => {
      const fallback = view?.items.find((item) => item.key === rowId)?.signIn?.type === "cliFallback";
      return mutate(rowId, { type: "signingIn", flowId: "" }, async () => {
        let { state } = await request({ method: "startPluginSignIn", rowId });
        if (!fallback && state.type === "cliFallback" && state.agents.length) {
          state = (await request({ method: "startPluginSignIn", rowId })).state;
        }
        if (state.type === "cliFallback" && state.agents.length && !state.steps.length) {
          return { ...state, steps: state.agents.map((agent) => ({ agent, server: rowId, state: { type: "starting" as const } })) };
        }
        return state;
      });
    },
    signOut: (rowId) => mutate(rowId, { type: "signedOut" }, async () => {
      await request({ method: "signOutPlugin", rowId });
      return { type: "signedOut" };
    }),
    pickFolder,
    trustProject: async () => {},
  };
}
