import type { McpServer } from "@/components/conversation/elements/mcp-server-panel";
import type { McpServerState, ProviderKind, WorkerRow } from "@/ipc/generated";

/** One agent's MCP servers, as the MCP server panel shows them under the agent's name. */
export type McpServerGroup = {
  /** `lead` for the session's own CLI, or `task:<id>` for a worker. */
  key: string;
  /** "Lead · Claude", or the worker's name. */
  agentLabel: string;
  servers: McpServer[];
};

export function shownServer(server: McpServerState): McpServer {
  return {
    id: server.name,
    name: server.name,
    transport: server.transport ?? undefined,
    status: server.status,
    // Each CLI's list is kept current by its own events (S14).
    atStart: false,
    tools: server.tools,
  };
}

/**
 * The conversation's own model, by its vendor's name as sentences say it (`VENDOR_LABELS`, whose
 * module loads components the node tests can't).
 */
const LEAD_LABELS: Record<ProviderKind, string> = {
  claude: "Lead · Claude",
  codex: "Lead · Codex",
};

/**
 * The MCP servers of a session's agents, as their CLIs last said: the session's own first (its
 * thread's `mcp`), then each worker's (its row's `mcp`) under its name, in the order the workers
 * came. An agent with none has no group.
 */
export function mcpServerGroups(
  lead: readonly McpServerState[],
  provider: ProviderKind,
  rows: readonly WorkerRow[],
  name: (row: WorkerRow) => string | null,
): McpServerGroup[] {
  const groups: McpServerGroup[] = [];
  if (lead.length > 0) groups.push({ key: "lead", agentLabel: LEAD_LABELS[provider], servers: lead.map(shownServer) });
  for (const row of rows.toSorted((a, b) => a.number - b.number)) {
    if (row.mcp.length === 0) continue;
    groups.push({ key: `task:${row.id}`, agentLabel: name(row) ?? "Worker", servers: row.mcp.map(shownServer) });
  }
  return groups;
}
