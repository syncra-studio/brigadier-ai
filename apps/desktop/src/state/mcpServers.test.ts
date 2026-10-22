import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope, McpServerState, WorkerRow } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";
import { mcpServerGroups } from "@/state/mcpServers";

const updated = (event: Omit<Extract<EventEnvelope["event"], { type: "mcpServersUpdated" }>, "type" | "conversationId" | "boot">, seq: number): EventEnvelope =>
  ({
    stream: "conversation:c",
    streamSeq: seq,
    atMs: seq,
    event: { type: "mcpServersUpdated", conversationId: "c", boot: "b", ...event },
  }) as unknown as EventEnvelope;

const server = (name: string, status: McpServerState["status"]): McpServerState => ({ name, status, tools: [], transport: null });
const row = (id: string, number: number, mcp: McpServerState[]): WorkerRow =>
  ({ id, number, title: id, mcp } as unknown as WorkerRow);

test("each agent's MCP servers read as a group under its name, the session's own first, then the workers' in the order they came", () => {
  const lead = [server("github", "connected"), server("remote", "needs-auth")];
  const rows = [row("t2", 2, [server("pg", "failed")]), row("t3", 3, []), row("t1", 1, [server("docs", "connecting")])];
  const groups = mcpServerGroups(lead, "claude", rows, (each) => `Worker ${each.number}`);
  assert.deepEqual(groups.map((group) => [group.key, group.agentLabel, group.servers.map((each) => `${each.name}:${each.status}`)]), [
    ["lead", "Lead · Claude", ["github:connected", "remote:needs-auth"]],
    ["task:t1", "Worker 1", ["docs:connecting"]],
    ["task:t2", "Worker 2", ["pg:failed"]],
  ]);
  assert.deepEqual(mcpServerGroups([], "codex", [], () => null), []);
  assert.equal(mcpServerGroups([server("x", "connected")], "codex", [], () => null)[0]?.agentLabel, "Lead · Codex");
});

test("an agent's MCP servers on the board follow its updates, and go when it has none", () => {
  const github: McpServerState = { name: "github", status: "connected", tools: [], transport: null };
  let board = applyToBoard(emptyBoard("c"), updated({ taskId: null, provider: "codex", servers: [github], atStart: false }, 1));
  board = applyToBoard(board, updated({ taskId: "t1", provider: "claude", servers: [github], atStart: true }, 2));
  assert.deepEqual(Object.keys(board.mcpServers).toSorted(), ["lead", "task:t1"]);
  board = applyToBoard(board, updated({ taskId: null, provider: "codex", servers: [], atStart: false }, 3));
  assert.deepEqual(Object.keys(board.mcpServers), ["task:t1"]);
  // Nothing to remove: the same board.
  assert.equal(applyToBoard(board, updated({ taskId: null, provider: "codex", servers: [], atStart: false }, 4)), board);
});
