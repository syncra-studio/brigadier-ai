import { toolName, unwrapCommand } from "@/app/conversation/activity/words";
import type { FileEntry } from "@/ipc/generated";

/**
 * What the context card reads off a session's own CLI items (`ThreadView.entries`, S14): the pages
 * it fetched or browsed and what it searched, the MCP servers it called, and the commands that
 * move its branch or its pull request. Pure functions of the entries.
 */

function field(input: string | null, key: string): string | null {
  if (!input) return null;
  try {
    const value = (JSON.parse(input) as Record<string, unknown> | null)?.[key];
    return typeof value === "string" ? value : null;
  } catch {
    return null;
  }
}

/** A browser tool's call that opens a page: `browser_navigate`, `navigate_page`, `open_url`. */
const BROWSING = /browser|navigate|open_?url/i;

/**
 * The web as the session's own thread used it, first seen first: the pages it fetched or a
 * browser tool opened (Claude's `WebFetch`, Codex's page open, a browser tool's `url`) and what
 * it searched (Claude's `WebSearch`, Codex's `web_search`), each once.
 */
export function webUse(entries: readonly FileEntry[]): { pages: string[]; searches: string[] } {
  const pages = new Set<string>();
  const searches = new Set<string>();
  for (const { event, parent } of entries) {
    if (parent || event.type !== "toolCall") continue;
    const name = toolName(event.name);
    const url = field(event.input, "url");
    if (name === "WebFetch" && url) pages.add(url);
    else if (name === "WebSearch") {
      const query = field(event.input, "query");
      if (query) searches.add(query);
    } else if (name === "web_search") {
      const action = field(event.input, "action");
      if (action === "openPage" && url) pages.add(url);
      else if (action === null) {
        const query = field(event.input, "query") ?? url;
        if (query) searches.add(query);
      }
    } else if (url && /^https?:\/\//.test(url) && BROWSING.test(name)) pages.add(url);
  }
  return { pages: [...pages], searches: [...searches] };
}

/** The MCP server a tool call went to: Claude's `mcp__<server>__<tool>`, Codex's `<server>/<tool>`. */
export function mcpServerOf(name: string): string | null {
  const claude = /^mcp__(.+?)__/.exec(name)?.[1];
  const server = claude ?? /^([^/\s]+)\/[^/\s]+$/.exec(name)?.[1] ?? null;
  // Codex's subagent calls (`agents/spawn`) are no source.
  if (!server || (!claude && server === "agents")) return null;
  return server;
}

/** How a server's name reads in a tool's name: Claude puts `_` for what a name may not hold. */
export function mcpKey(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");
}

/** The MCP servers the thread called, by the names its tool calls give them, first called first. */
export function usedMcpServers(entries: readonly FileEntry[]): string[] {
  const used = new Set<string>();
  for (const { event, parent } of entries) {
    if (parent || event.type !== "toolCall") continue;
    const server = mcpServerOf(event.name);
    if (server) used.add(server);
  }
  return [...used];
}

/** Shell tools whose call carries its command as `command`. */
const SHELL_TOOLS = new Set(["Bash"]);

/** The thread's commands that finished, by item id: a Codex command, a Claude shell call. */
export function finishedCommands(entries: readonly FileEntry[]): { itemId: string; command: string }[] {
  return entries.flatMap(({ event, parent }) => {
    if (parent) return [];
    if (event.type === "command" && event.status !== "inProgress") return [{ itemId: event.itemId, command: unwrapCommand(event.command) }];
    if (event.type === "toolCall" && event.status !== "inProgress" && SHELL_TOOLS.has(toolName(event.name))) {
      const command = field(event.input, "command");
      return command ? [{ itemId: event.itemId, command: unwrapCommand(command) }] : [];
    }
    return [];
  });
}

/** A command's own commands, one after another: `a && b; c`. */
const parts = (command: string) => command.split(/&&|\|\||;|\n/).map((part) => part.trim());

/** Whether a command may have changed the branch's pull request: a `git push` or a `gh pr …`. */
export function movesPullRequest(command: string): boolean {
  return parts(command).some((part) => /^git\s+push\b/.test(part) || /^gh\s+pr\b/.test(part));
}

/** Whether a command ran git, which may have moved the branch. */
export function runsGit(command: string): boolean {
  return parts(command).some((part) => /^git\b/.test(part));
}

/**
 * What the pull request row reads again on: the finished pushes and `gh pr` commands so far. A
 * new one changes it.
 */
export function pullRequestKey(entries: readonly FileEntry[]): string {
  return finishedCommands(entries).filter((each) => movesPullRequest(each.command)).map((each) => each.itemId).join(" ");
}

/** What the branch row reads again on besides turn ends: the finished git commands so far. */
export function gitCommandsKey(entries: readonly FileEntry[]): string {
  return finishedCommands(entries).filter((each) => runsGit(each.command)).map((each) => each.itemId).join(" ");
}
