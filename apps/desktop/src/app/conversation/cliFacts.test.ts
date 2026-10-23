import assert from "node:assert/strict";
import { test } from "node:test";

import { gitCommandsKey, mcpKey, mcpServerOf, movesPullRequest, pullRequestKey, usedMcpServers, webUse } from "@/app/conversation/cliFacts";
import type { FileEntry, ItemStatus } from "@/ipc/generated";

const tool = (itemId: string, name: string, input: unknown, status: ItemStatus = "completed"): FileEntry =>
  ({ event: { type: "toolCall", itemId, name, input: JSON.stringify(input), status, output: null } });
const command = (itemId: string, text: string, status: ItemStatus = "completed"): FileEntry =>
  ({ event: { type: "command", itemId, command: text, cwd: null, status, exitCode: 0, output: null, durationMs: null } });

test("a finished git push or gh pr command reads the pull request again; a running one, or any other, does not", () => {
  const before = [command("c1", "npm test")];
  assert.equal(pullRequestKey(before), "");
  // Codex's command, wrapped by its shell and after a cd.
  const pushing = [...before, command("c2", "/bin/zsh -lc 'cd /repo && git push -u origin HEAD'", "inProgress")];
  assert.equal(pullRequestKey(pushing), "");
  const pushed = [...before, command("c2", "/bin/zsh -lc 'cd /repo && git push -u origin HEAD'")];
  assert.notEqual(pullRequestKey(pushed), pullRequestKey(before));
  // Claude's shell call.
  const opened = [...pushed, tool("b1", "Bash", { command: "gh pr create --fill" })];
  assert.notEqual(pullRequestKey(opened), pullRequestKey(pushed));
  assert.equal(pullRequestKey([...opened, tool("b2", "Bash", { command: "git status" })]), pullRequestKey(opened));
  assert.equal(movesPullRequest("git commit -m x && git push"), true);
  assert.equal(movesPullRequest("echo git push"), false);
  assert.equal(movesPullRequest("gh issue list"), false);
});

test("the branch row reads again on finished git commands", () => {
  const entries = [command("c1", "npm test"), tool("b1", "Bash", { command: "git commit -am fix" })];
  assert.equal(gitCommandsKey(entries), "b1");
  assert.equal(gitCommandsKey([...entries, tool("b2", "Bash", { command: "git merge w/1" }, "inProgress")]), "b1");
});

test("the MCP servers used are the ones its tool calls named: Claude's mcp__server__tool, Codex's server/tool", () => {
  const entries = [
    tool("1", "mcp__claude_ai_Linear__search_issues", {}),
    tool("2", "postgres/query", { sql: "select 1" }),
    tool("4", "agents/spawn", {}),
    tool("5", "mcp__claude_ai_Linear__get_issue", {}),
    tool("6", "Read", { file_path: "a.ts" }),
  ];
  assert.deepEqual(usedMcpServers(entries), ["claude_ai_Linear", "postgres"]);
  assert.equal(mcpServerOf("mcp__pencil__batch_design"), "pencil");
  // A server's name as its CLI lists it matches how its tools name it.
  assert.equal(mcpKey("claude.ai Linear"), mcpKey("claude_ai_Linear"));
});

test("pages it fetched or browsed, and what it searched, each once", () => {
  const entries = [
    tool("1", "WebSearch", { query: "express static files" }),
    tool("2", "WebFetch", { url: "https://expressjs.com/en/starter/static-files.html", prompt: "x" }),
    tool("3", "web_search", { action: "openPage", url: "https://developer.mozilla.org/" }),
    tool("4", "web_search", { query: "form validation" }),
    tool("5", "mcp__playwright__browser_navigate", { url: "http://localhost:3000/" }),
    tool("6", "WebFetch", { url: "https://expressjs.com/en/starter/static-files.html" }),
    tool("7", "mcp__linear__get_issue", { url: "https://linear.app/x" }),
  ];
  assert.deepEqual(webUse(entries), {
    pages: ["https://expressjs.com/en/starter/static-files.html", "https://developer.mozilla.org/", "http://localhost:3000/"],
    searches: ["express static files", "form validation"],
  });
});
