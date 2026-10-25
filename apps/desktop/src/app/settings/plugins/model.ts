import type { AgentItemState, PluginItemView, PluginKind, PluginProjectView, PluginsView } from "@/ipc/generated";

/*
 * What the Plugins page shows, from the daemon's view: the sections of the list for the picked
 * project and tab, each row's line and tags, and how each agent stands, in plain words.
 */

export const KIND_LABEL: Record<PluginKind, string> = { server: "MCP server", skill: "Skill" };
export const TAB_LABEL: Record<PluginKind, string> = { server: "MCP servers", skill: "Skills" };

export type Tone = "muted" | "warning" | "error";
export type Line = { text: string; tone: Tone };

/** One row of the list. */
export type Row = {
  item: PluginItemView;
  /** Its switch: on where the page looks. */
  on: boolean;
  line: Line;
  /** Project tags: where it's on, or off. */
  chips: { text: string; off: boolean }[];
};

type Section = {
  key: "every" | "some" | "own" | "fromEvery" | "fromOther";
  title: string;
  description: string;
  rows: Row[];
};

/** "a", "a and b", "a, b and c". */
export function listNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/** A project's name for its folder; the folder's last part for one the page doesn't list. */
export function projectName(view: PluginsView, path: string): string {
  return view.projects.find((project) => project.path === path)?.name ?? path.split("/").filter(Boolean).pop() ?? path;
}

/** Whether `item` is on in project `project`, else anywhere. */
export function isOn(item: PluginItemView, project: PluginProjectView | null): boolean {
  if (!project) return item.everywhere || item.onIn.length > 0;
  return item.everywhere ? !item.offIn.includes(project.path) : item.onIn.includes(project.path);
}

/** Whether an agent has it at all, on or off (the strip dims the others). */
export const has = (state: AgentItemState) => state.type !== "missing" && state.type !== "notSupported";

/** The row's line when nothing more pressing says otherwise: what it does, else how it runs. */
const about = (item: PluginItemView): Line => ({ text: item.description || item.detail, tone: "muted" });

/** A real problem, which the row shows wherever it sits, on or off. */
function problem(item: PluginItemView): Line | null {
  const attention = item.attention;
  if (attention?.type === "wontStart") return { text: "Won't start. Open it to see why.", tone: "error" };
  if (attention?.type === "signIn") return { text: "Needs sign-in.", tone: "warning" };
  return null;
}

/** Off in every project: on nowhere. */
const nowhere = (item: PluginItemView) => !item.everywhere && item.onIn.length === 0;

/** Whether `path` is one of the user's Brigadier projects (another folder can hold an agent's own copy). */
export const isProject = (view: PluginsView, path: string) => view.projects.some((project) => project.path === path);

/** On in none of the user's projects, though maybe in a folder that isn't one. */
const inNoProject = (view: PluginsView, item: PluginItemView) => !item.everywhere && !item.onIn.some((path) => isProject(view, path));

function matches(item: PluginItemView, query: string): boolean {
  const words = query.trim().toLowerCase();
  return !words || `${item.name} ${item.description} ${item.detail}`.toLowerCase().includes(words);
}

/** The off tags of an item on in every project: one per project, or how many. */
function offChips(view: PluginsView, item: PluginItemView): Row["chips"] {
  if (item.offIn.length > 2) return [{ text: `Off in ${item.offIn.length} projects`, off: true }];
  return item.offIn.map((path) => ({ text: `Off in ${projectName(view, path)}`, off: true }));
}

/** The list's sections for tab `kind`, looking at `project` (null: every project). */
export function sections(view: PluginsView, project: PluginProjectView | null, kind: PluginKind, query: string): Section[] {
  const items = view.items.filter((item) => item.kind === kind && matches(item, query));
  if (!project) {
    return [
      {
        key: "every",
        title: "Global",
        description: "On in every project, and in new ones you add. Turn one off to stop it everywhere.",
        rows: items.filter((item) => item.everywhere || nowhere(item)).map((item) => {
          const on = isOn(item, null);
          return { item, on, line: problem(item) ?? (on ? about(item) : { text: "Off in every project.", tone: "muted" }), chips: on ? offChips(view, item) : [] };
        }),
      },
      {
        key: "some",
        title: "Only in some projects",
        description: "Each one is on only in the projects on its tag. Click the tag to change where.",
        rows: items.filter((item) => !item.everywhere && item.onIn.length > 0).map((item) => ({
          item,
          on: true,
          line: problem(item) ?? about(item),
          chips: item.onIn.map((path) => ({ text: projectName(view, path), off: false })),
        })),
      },
    ];
  }
  const here = project.name;
  return [
    {
      key: "own",
      title: `Only in ${here}`,
      description: "Sessions in your other projects don't see these.",
      rows: items.filter((item) => !item.everywhere && item.onIn.includes(project.path)).map((item) => ({ item, on: true, line: problem(item) ?? about(item), chips: [] })),
    },
    {
      key: "fromEvery",
      title: "Global",
      description: `On in all your projects. Turn one off to skip it in ${here} only.`,
      rows: items.filter((item) => item.everywhere || inNoProject(view, item)).map((item) => {
        const on = isOn(item, project);
        const line: Line = problem(item) ?? (on
          ? about(item)
          : item.everywhere
            ? { text: `Off in ${here}. Still on in your other projects.`, tone: "muted" }
            : nowhere(item)
              ? { text: "Off in every project. Turn it on to use it here.", tone: "muted" }
              : { text: `On in ${listNames(item.onIn.map((path) => projectName(view, path)))} only. Turn it on to use it here.`, tone: "muted" });
        return { item, on, line, chips: [] };
      }),
    },
    {
      key: "fromOther",
      title: "From other projects",
      description: "On in another project. Turn one on to use it here too.",
      rows: items.filter((item) => !item.everywhere && !inNoProject(view, item) && !item.onIn.includes(project.path)).map((item) => ({
        item,
        on: false,
        line: problem(item) ?? { text: `On in ${listNames(item.onIn.map((path) => projectName(view, path)))}.`, tone: "muted" },
        chips: [],
      })),
    },
  ];
}

/**
 * What fixes an item only some agents have: a Library item is added to the agents missing it
 * ("addMissing"); another agent's plugin can't go to the others (null); anything else goes into
 * the Library first ("useEverywhere").
 */
export function spreadOf(item: PluginItemView): "addMissing" | "useEverywhere" | null {
  if (item.key.startsWith("lib:")) return "addMissing";
  if (item.key.startsWith("plugin:")) return null;
  return "useEverywhere";
}

/**
 * Where a Library item goes for the agents missing it: where the others have it, by project id
 * (null: every project). Never wider than that: an item on in no project goes nowhere.
 */
export function placesOf(view: PluginsView, item: PluginItemView): (string | null)[] {
  if (item.everywhere) return [null];
  return item.onIn.flatMap((path) => view.projects.find((project) => project.path === path)?.id ?? []);
}

/** A project's summary in the filter: "2 of its own · 2 turned off". */
export function projectSummary(view: PluginsView, project: PluginProjectView): string {
  const own = view.items.filter((item) => !item.everywhere && item.onIn.includes(project.path)).length;
  const off = view.items.filter((item) => item.everywhere && item.offIn.includes(project.path)).length;
  const parts = [own ? `${own} of its own` : "Nothing of its own"];
  if (off) parts.push(`${off} turned off`);
  return parts.join(" · ");
}

/** Found in other apps' line: "3 in Claude Desktop and Cursor. …". */
export function elsewhereLine(items: PluginItemView[]): string {
  const apps = [...new Set(items.map((item) => item.foundIn).filter((app): app is string => !!app))];
  return `${items.length} in ${listNames(apps)}. Your agents can't use them until you bring them in.`;
}

/** An address without its https:// (ws:// and the like say something, so they stay). */
export const shownDetail = (detail: string) => detail.replace(/^https:\/\//, "");

/** The labels of the agents in `states` whose state is `type`. */
export function agentsIn(view: PluginsView, item: PluginItemView, type: AgentItemState["type"]): string[] {
  return item.agents.filter((entry) => entry.state.type === type).map((entry) => labelOf(view, entry.agent));
}

export function labelOf(view: PluginsView, agent: string): string {
  return view.agents.find((entry) => entry.agent === agent)?.label ?? agent;
}

/** What a won't-start message says is wrong: "unknown type "websocket"". */
export const wontStartReason = (message: string) => message.replace(/^Won't start:\s*/, "").replace(/\.$/, "");
