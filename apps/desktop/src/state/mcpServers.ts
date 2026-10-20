import { useMemo } from "react";

import { workerName } from "@/app/conversation/rowWords";
import type { McpServer } from "@/components/conversation/elements/mcp-server-panel";
import type { AgentMcpServers, McpServerState, ProviderKind, Task } from "@/ipc/generated";
import { mcpAgentKey, useBoard } from "@/state/board";

/** One agent's MCP servers, as the MCP server panel shows them under the agent's name. */
export type McpServerGroup = {
  /** The agent's key on the board: `lead`, or `task:<id>` for a worker. */
  key: string;
  /** "Lead · Claude", or the worker's name. */
  agentLabel: string;
  /** What the agent's CLI said when it started, not kept current since. */
  atStart: boolean;
  servers: McpServer[];
};

function shownServer(server: McpServerState, atStart: boolean): McpServer {
  return {
    id: server.name,
    name: server.name,
    transport: server.transport ?? undefined,
    status: server.status,
    atStart,
    tools: server.tools,
  };
}

/** The MCP servers of a board's agents, its own model's first, then each worker's in task order. */
export function mcpServerGroups(
  agents: Readonly<Record<string, AgentMcpServers>>,
  tasks: Readonly<Record<string, Task>>,
): McpServerGroup[] {
  const order = (agent: AgentMcpServers) =>
    agent.taskId === null ? -1 : (tasks[agent.taskId]?.number ?? Number.MAX_SAFE_INTEGER);
  return Object.values(agents)
    .filter((agent) => agent.servers.length > 0)
    .toSorted((a, b) => order(a) - order(b) || (a.taskId ?? "").localeCompare(b.taskId ?? ""))
    .map((agent) => {
      const task = agent.taskId === null ? undefined : tasks[agent.taskId];
      return {
        key: mcpAgentKey(agent.taskId),
        agentLabel:
          agent.taskId === null
            ? LEAD_LABELS[agent.provider]
            : task
              ? workerName(tasks, task)
              : "Worker",
        atStart: agent.atStart,
        servers: agent.servers.map((server) => shownServer(server, agent.atStart)),
      };
    });
}

/**
 * The conversation's own model, by its vendor's name as sentences say it (`VENDOR_LABELS`, whose
 * module loads components the node tests can't).
 */
const LEAD_LABELS: Record<ProviderKind, string> = {
  claude: "Lead · Claude",
  codex: "Lead · Codex",
};

const NO_AGENTS: Readonly<Record<string, AgentMcpServers>> = {};
const NO_TASKS: Readonly<Record<string, Task>> = {};

/** The MCP servers of `conversationId`'s agents whose CLI runs now, when its board is the one in view. */
export function useMcpServers(conversationId: string): McpServerGroup[] {
  const agents = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.mcpServers : NO_AGENTS));
  const tasks = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.tasks : NO_TASKS));
  return useMemo(() => mcpServerGroups(agents, tasks), [agents, tasks]);
}
