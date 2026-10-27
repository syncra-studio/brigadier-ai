import type { AgentSignInState, CatalogSignIn } from "@/ipc/generated";
import type { PluginItemView, PluginKind, PluginsView } from "./types";

/*
 * What the Plugins page shows, from the daemon's view: each tab's items, each card's line and
 * tag, in plain words.
 */

export const KIND_LABEL: Record<PluginKind, string> = { server: "MCP server", skill: "Skill" };
export const TAB_LABEL: Record<PluginKind, string> = { server: "MCP servers", skill: "Skills" };

export type Tone = "muted" | "warning" | "error";
export type Line = { text: string; tone: Tone };

/** "a", "a and b", "a, b and c". */
export function listNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/** A folder's last part: a folder that isn't a project goes by it. */
export const folderName = (path: string) => path.split("/").filter(Boolean).pop() ?? path;

/** The names of the places an item is kept to: its projects', then its other folders'. */
export function placeNames(view: PluginsView, item: PluginItemView): string[] {
  if (item.scope.type === "allProjects") return [];
  return [
    ...item.scope.projectIds.map((id) => view.projects.find((project) => project.id === id)?.name ?? id),
    ...item.scope.folders.map(folderName),
  ];
}

/** A card's line: its problem, else why it can't be changed, else what it does, else how it runs. */
export function lineOf(item: PluginItemView): Line {
  if (signedIn(item.signIn)) return { text: "", tone: "muted" };
  if (item.signIn?.type === "signedOut") return { text: "Must sign in", tone: "warning" };
  if (item.signIn && item.signIn.type !== "notRequired") return { text: signInLine(item.signIn), tone: item.signIn.type === "cliSetupRequired" || (item.signIn.type === "cliFallback" && item.signIn.agents.length > 0 && !signingIn(item.signIn)) ? "warning" : "muted" };
  if (item.problem) return { text: item.problem.message, tone: "warning" };
  return { text: item.description || item.detail, tone: "muted" };
}

export const signingIn = (state: CatalogSignIn | undefined): boolean => state?.type === "signingIn" ||
  (state?.type === "cliFallback" && state.steps.some((step) => ["waiting", "starting", "inBrowser"].includes(step.state.type)));

export const signedIn = (state: CatalogSignIn | undefined): boolean => state?.type === "signedIn" ||
  (state?.type === "cliFallback" && state.agents.length === 0);

/** Where an item is on, as its tag: "Global", a project's name, "3 projects"; null when it's in none. */
export function chipOf(view: PluginsView, item: PluginItemView): string | null {
  if (item.scope.type === "allProjects") return "Global";
  const names = placeNames(view, item);
  if (names.length === 0) return null;
  return names.length === 1 ? names[0]! : `${names.length} projects`;
}

function matches(item: PluginItemView, query: string): boolean {
  const words = query.trim().toLowerCase();
  return !words || `${item.name} ${item.description} ${item.detail}`.toLowerCase().includes(words);
}

/** Tab `kind`'s items that match `query`, in the daemon's order: the ones with a problem first, then A–Z. */
export const listed = (view: PluginsView, kind: PluginKind, query: string): PluginItemView[] =>
  view.items.filter((item) => item.kind === kind && matches(item, query));

/** An address without its https:// (ws:// and the like say something, so they stay). */
export const shownDetail = (detail: string) => detail.replace(/^https:\/\//, "");

/** What a won't-start message says is wrong: "unknown type "websocket"". */
export const wontStartReason = (message: string) => message.replace(/^Won't start:\s*/i, "").replace(/\.$/, "");

function cliSignInState(state: AgentSignInState): string {
  switch (state.type) {
    case "alreadySignedIn": case "signedIn": return "signed in";
    case "waiting": return "waiting to sign in";
    case "starting": return "starting sign-in";
    case "inBrowser": return "waiting for browser";
    case "cancelled": return "cancelled";
    case "failed": return state.reason;
  }
}

export function signInLine(state: CatalogSignIn): string {
  switch (state.type) {
    case "signedIn": return "Signed in";
    case "signedOut": return "Not signed in";
    case "signingIn": return "Waiting for your browser…";
    case "failed": return state.message;
    case "cliSetupRequired": return `This server only accepts sign-in from ${state.agents.map((agent) => agent === "claude" ? "Claude Code" : "Codex").join(" or ")}: add it in that CLI`;
    case "cliFallback": if (state.steps?.length) return state.steps.map((step) => `${step.agent === "claude" ? "Claude Code" : "Codex"}: ${cliSignInState(step.state)}`).join("; ");
      return `Uses each CLI's sign-in${state.agents.length ? `: ${state.agents.map((agent) => agent === "claude" ? "Claude Code" : agent === "codex" ? "Codex" : agent).join(", ")} still need it` : ""}`;
    case "notRequired": return "";
  }
}
