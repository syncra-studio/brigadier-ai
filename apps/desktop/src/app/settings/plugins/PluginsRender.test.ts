import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type RowSeen = { name: string; line: string; tone: string | null; off: boolean; lit: string[]; chips: string[]; switch: string | null; button: string | null };
type SectionSeen = { title: string; description: string; count: string; rows: RowSeen[] };
type DialogSeen = { slot: string; width: number; title: string; description: string; text: string; agents: string[]; buttons: string[]; radios: string[] };
type Seen = {
  settingsColumn: boolean;
  title: string;
  description: string;
  sections: SectionSeen[];
  tabs: string[];
  filter: string;
  folds: { text: string; open: boolean; rows: RowSeen[] }[];
  agentRows: string[];
  projectSwitches: string[];
  banner: string;
  empty: string;
  emptyRow: string;
  removeRow: string;
  menu: { rows: string[]; current: string } | null;
  dialog: DialogSeen | null;
  toasts: string[];
  overflow: boolean;
};

const AGENTS = ["claude", "codex", "gemini", "cursor", "opencode", "amp"];
const section = (seen: Seen, title: string) => {
  const found = seen.sections.find((entry) => entry.title === title);
  assert.ok(found, `no section ${title}: ${seen.sections.map((entry) => entry.title).join(", ")}`);
  return found;
};
const names = (rows: RowSeen[]) => rows.map((row) => row.name);
const row = (rows: RowSeen[], name: string) => rows.find((entry) => entry.name === name)!;

// The page in Chromium over a faked daemon, through every state the Plugins board draws.
test("plugins: every drawn list, details and dialog state, and what the switches and dialogs ask for", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "plugins.html?drive=1", "plugins-result", 40000)) as Record<string, unknown> & Record<string, Seen>;
  assert.equal(seen.error, undefined);
  for (const state of ["L1", "L2", "L3", "L4", "L5", "L6", "L7", "D1", "D2", "D3"]) {
    assert.equal(seen[state]!.settingsColumn, true, `${state}: the column every Settings page uses`);
    assert.equal(seen[state]!.overflow, false, `${state}: nothing runs off the page`);
  }

  // L1: All projects, MCP servers.
  const l1 = seen.L1!;
  assert.equal(l1.title, "Plugins");
  assert.equal(l1.description, "Skills and MCP servers your agents can use. Pick a project to choose what's on there.");
  assert.deepEqual(l1.tabs, ["MCP servers7 *", "Skills6"]);
  assert.equal(l1.filter, "All projects");
  assert.deepEqual(l1.sections.map((entry) => `${entry.title} ${entry.count}`), ["Needs attention 3", "Every project 6", "Only in some projects 1"]);
  const attention = section(l1, "Needs attention").rows;
  assert.deepEqual(attention.map((entry) => [entry.name, entry.line, entry.tone, entry.button]), [
    ["linear", "Codex needs you to sign in.", "warning", "Sign in"],
    ["figma-dev", 'Won\'t start: unknown type "websocket".', "error", "Details"],
    ["grill-me", "Only Claude Code has it.", "muted", "Use everywhere"],
  ]);
  // The strip is built from the view's agents: a mark per agent, the ones with the problem dimmed.
  assert.deepEqual(row(attention, "linear").lit, ["claude", "gemini", "cursor", "opencode"]);
  assert.deepEqual(row(attention, "figma-dev").lit, []);
  assert.deepEqual(row(attention, "grill-me").lit, ["claude"]);
  const every = section(l1, "Every project");
  assert.equal(every.description, "On in all your projects. Turn one off to stop it everywhere.");
  assert.deepEqual(names(every.rows), ["pencil", "railway", "node_repl", "linear", "figma-dev", "playwright"]);
  assert.deepEqual(row(every.rows, "pencil").lit, AGENTS);
  assert.deepEqual(row(every.rows, "railway").chips, ["Off in steady-app"]);
  assert.deepEqual(row(every.rows, "node_repl").chips, ["Off in brigadier-ai"]);
  assert.deepEqual([row(every.rows, "figma-dev").line, row(every.rows, "figma-dev").tone], ["Won't start. See Needs attention.", "error"]);
  assert.deepEqual(row(every.rows, "playwright"), { ...row(every.rows, "playwright"), line: "Off in every project.", off: true, switch: "false", lit: [] });
  const some = section(l1, "Only in some projects");
  assert.equal(some.description, "Each one is on only in the projects shown.");
  assert.deepEqual([some.rows[0]!.name, some.rows[0]!.chips, some.rows[0]!.switch], ["postgres", ["brigadier-ai"], "true"]);
  // D4: Found in other apps, folded until opened.
  assert.deepEqual(l1.folds, [{ text: "Found in other apps3 in Claude Desktop and Cursor. Your agents can't use them until you bring them in.", open: false, rows: [] }]);

  // L2: All projects, Skills.
  const l2 = seen.L2!;
  assert.deepEqual(l2.tabs, ["MCP servers7", "Skills6 *"]);
  assert.equal(section(l2, "Needs attention").rows.length, 3, "Needs attention shows both tabs' items");
  const skills = section(l2, "Every project").rows;
  assert.deepEqual(names(skills), ["higgsfield", "impeccable", "hallmark", "use-railway", "grill-me"]);
  assert.match(l2.sections.flatMap((entry) => entry.rows).find((entry) => entry.name === "higgsfield")!.line, /Images, video/);
  assert.deepEqual([row(skills, "grill-me").line, row(skills, "grill-me").tone], ["Only Claude Code has it. See Needs attention.", "warning"]);
  assert.deepEqual(names(section(l2, "Only in some projects").rows), ["delegator"]);

  // L3: one project.
  const l3 = seen.L3!;
  assert.equal(l3.filter, "brigadier-ai");
  assert.deepEqual(l3.sections.map((entry) => entry.title), ["Needs attention", "Only in brigadier-ai", "From every project"]);
  assert.equal(section(l3, "Only in brigadier-ai").description, "Sessions in your other projects don't see these.");
  assert.deepEqual(names(section(l3, "Only in brigadier-ai").rows), ["postgres"]);
  const fromEvery = section(l3, "From every project");
  assert.equal(fromEvery.description, "Turn one off to skip it in brigadier-ai only.");
  assert.deepEqual(fromEvery.rows.map((entry) => entry.switch), ["true", "true", "false", "true", "true", "false"]);
  assert.equal(row(fromEvery.rows, "node_repl").line, "Off in brigadier-ai. Still on in your other projects.");
  assert.equal(row(fromEvery.rows, "playwright").line, "Off in every project. Turn it on to use it here.");

  // L4: a project with nothing of its own.
  const l4 = seen.L4!;
  assert.equal(l4.emptyRow, "Nothing only for steady-app yet. Add one here, or turn one on below.Add to steady-app");
  assert.equal(row(section(l4, "From every project").rows, "railway").line, "Off in steady-app. Still on in your other projects.");
  const other = section(l4, "From other projects");
  assert.equal(other.description, "On in another project. Turn one on to use it here too.");
  assert.deepEqual([other.rows[0]!.name, other.rows[0]!.line, other.rows[0]!.switch, other.rows[0]!.off], ["postgres", "On in brigadier-ai.", "false", true]);

  // L5: the project filter open; picking a project closes it and shows that project.
  assert.deepEqual(seen.L5!.menu, {
    rows: ["All projectsWhat every project gets", "brigadier-ai2 of its own · 2 turned off", "steady-appNothing of its own · 2 turned off", "landingNothing of its own", "Add project…"],
    current: "All projectsWhat every project gets",
  });
  assert.equal(seen.picked!.menu, null);
  assert.equal(seen.picked!.filter, "steady-app");

  // L6: Found in other apps open.
  const fold = seen.L6!.folds[0]!;
  assert.equal(fold.open, true);
  assert.deepEqual(fold.rows.map((entry) => [entry.name, entry.button, entry.lit.length]), [
    ["filesystem", "Use everywhere", 0],
    ["brave-search", "Use everywhere", 0],
    ["context7", "Use everywhere", 0],
  ]);

  // L7: nothing added yet.
  assert.deepEqual(seen.L7!.tabs, ["MCP servers0 *", "Skills0"]);
  assert.equal(seen.L7!.empty, "No MCP servers yetAdd one once and every agent can use it, in all your projects or just one.AddWhat can I add?");
  assert.equal(seen.L7!.sections.length, 0);

  // D1: needs sign-in.
  const d1 = seen.D1!;
  assert.equal(d1.title, "linear");
  assert.equal(d1.description, "MCP server · mcp.linear.app/mcp");
  assert.equal(d1.banner, "Codex needs you to sign in to linear. Until then, Codex sessions can't use it.Sign in");
  assert.deepEqual(d1.sections.map((entry) => entry.title), ["Agents", "Projects"]);
  assert.deepEqual(d1.agentRows, [
    "Claude CodeSigned in.Ready",
    "CodexWaiting for you to sign in.Needs sign-in",
    "GGemini CLISigned in.Ready",
    "CCursor AgentSigned in.Ready",
    "OOpenCodeSigned in.Ready",
    "AAmpAmp can't sign in to servers on the web yet.Not supported",
  ]);
  assert.deepEqual(d1.projectSwitches, [
    "Every projectAlso turns on in projects you add later. true",
    "brigadier-ai~/Code/brigadier-ai true",
    "steady-app~/Code/steady-app true",
    "landing~/Code/landing true",
  ]);
  assert.deepEqual(d1.folds.map((entry) => [entry.text, entry.open]), [["AdvancedAddress, sign-in details, where it's saved.", false]]);
  assert.equal(d1.removeRow, "Remove linearRemoves it from every agent and every project.");

  // D2: won't start, with Try again.
  const d2 = seen.D2!;
  assert.equal(d2.description, "MCP server · ws://localhost:3845/mcp");
  assert.equal(d2.banner, 'figma-dev won\'t start: unknown type "websocket". No agent can use it until the setup is fixed.Try again');
  assert.equal(d2.agentRows.length, 6);
  for (const agentRow of d2.agentRows) assert.match(agentRow, /Couldn't start it\.Won't start$/);

  // D3: only in one project, the Projects card's switches.
  const d3 = seen.D3!;
  assert.equal(section(d3, "Projects").description, "Where it's on. It's only in brigadier-ai now; turn on Every project to use it everywhere.");
  assert.deepEqual(d3.projectSwitches, [
    "Every projectTurn on to use it in all your projects. false",
    "brigadier-ai~/Code/brigadier-ai true",
    "steady-app~/Code/steady-app false",
    "landing~/Code/landing false",
  ]);
  for (const agentRow of d3.agentRows) assert.match(agentRow, /Ready in brigadier-ai\.Ready$/);
  assert.deepEqual(seen.detailCalls, ["switchPlugin lib:postgres p-brigadier off", "switchPlugin lib:postgres everywhere on"]);

  // Every dialog is the app's dialog at Project settings' width.
  for (const state of ["G1", "G2", "G3", "G4", "G5", "G6", "G7"]) assert.equal(seen[state]!.dialog?.width, 560, state);

  // G1: Add, paste.
  const g1 = seen.G1!.dialog!;
  assert.deepEqual([g1.slot, g1.title, g1.description], ["add", "Add a skill or MCP server", "Paste what you have. Brigadier works out the rest and adds it to every agent."]);
  assert.match(g1.text, /Works with an mcp add line from any agent, a server URL, mcpServers JSON, a GitHub link, a skills\.sh name or a skill folder\./);
  assert.deepEqual(g1.buttons, ["Close", "Choose folder…", "Cancel", "Continue"]);
  assert.match(seen.badPaste!.dialog!.text, /That doesn't look like an MCP server or a skill\./);

  // G2: Add, check, every project (All projects picked).
  const g2 = seen.G2!.dialog!;
  assert.deepEqual([g2.title, g2.description], ["Add sentry", "An MCP server from mcp.sentry.dev. It signs in with your browser."]);
  assert.match(g2.text, /sentryMCP serverhttps:\/\/mcp\.sentry\.dev\/mcp/);
  assert.deepEqual(g2.radios, [
    "Every projectOn in all your projects. You can turn it off in one later. *",
    "Only in one projectSessions in other projects won't see it.",
  ]);
  assert.match(g2.text, /All 6/);
  assert.match(g2.text, /Advancedname, share with the team/);
  assert.deepEqual(g2.buttons.slice(-3), ["Back", "Cancel", "Add"]);

  // G3: Add inside a project defaults to that project.
  const g3 = seen.G3!.dialog!;
  assert.equal(g3.title, "Add sentry to brigadier-ai");
  assert.equal(g3.radios[1], "Only in one projectSessions in other projects won't see it. *");

  // G4: Adding, each agent in turn; then each agent's result.
  const g4 = seen.G4!.dialog!;
  assert.equal(g4.title, "Adding sentry");
  assert.equal(g4.description, "Each agent adds it its own way, so it works in a terminal too.");
  assert.deepEqual(g4.agents, ["Claude CodeAddedDone", "CodexAddedDone", "GGemini CLIAdding…", "CCursor AgentWaiting", "OOpenCodeWaiting", "AAmpWaiting"]);
  assert.deepEqual(g4.buttons, ["Close", "Hide"]);
  assert.deepEqual(seen.added!.dialog!.agents.map((entry) => entry.endsWith("AddedDone")), [true, true, true, true, true, true]);
  assert.deepEqual(seen.addCalls, ["addPlugin - everywhere just me", "addPlugin - everywhere just me"]);

  // G5: Sign in, each agent's own sign-in in turn.
  const g5 = seen.G5!.dialog!;
  assert.equal(g5.title, "Sign in to linear");
  assert.equal(g5.description, "Each agent keeps its own sign-in, so your browser asks once per agent. After the first one it usually just confirms.");
  assert.deepEqual(g5.agents, [
    "Claude CodeSigned inSigned in",
    "CodexWaiting for your browser…",
    "GGemini CLIOpens your browser after CodexNext",
    "CCursor AgentWaitingNext",
    "OOpenCodeWaitingNext",
    "AAmpAmp can't sign in to servers on the web yet.Not supported",
  ]);
  assert.deepEqual(g5.buttons, ["Close", "Open browser again", "Cancel"]);

  // G6: Use everywhere, then Undo from the toast.
  const g6 = seen.G6!.dialog!;
  assert.equal(g6.title, "Use grill-me in every agent?");
  assert.equal(g6.description, "Right now only Claude Code has it. Brigadier gives it to every agent, the same way each agent adds its own skills. You can undo this.");
  assert.match(g6.text, /Keeps a copy in Brigadier's libraryGives it to all 6 agentsRemoves Claude Code's old copy, only after every agent has the new one\./);
  assert.deepEqual(g6.buttons, ["Close", "Cancel", "Use everywhere"]);
  // A switch that can't be kept to a project asks first, then flips the Library's copy.
  assert.equal(seen.notSwitchable!.dialog!.title, "Use grill-me in every agent?");
  assert.deepEqual(seen.useEverywhereCalls, ["useEverywhere skill:grill-me", "switchPlugin lib:grill-me p-brigadier off"]);
  assert.deepEqual(seen.afterUseEverywhere!.toasts, ["grill-me is in every agent nowUndo"]);
  assert.equal((seen.undoCalls as unknown as string[]).at(-1), "undoUseEverywhere skill:grill-me");

  // G7: Remove, from the details page; confirming goes back to the list.
  const g7 = seen.G7!.dialog!;
  assert.equal(g7.title, "Remove linear?");
  assert.equal(g7.description, "It goes from all 6 agents and every project, and each agent forgets its sign-in. Sessions already running keep it until they end.");
  assert.deepEqual(g7.buttons, ["Close", "Cancel", "Remove"]);
  assert.equal(seen.removed!.title, "Plugins");
  assert.ok(!seen.removed!.sections.flatMap((entry) => names(entry.rows)).includes("linear"));

  // The switch means what the filter says: everywhere, or only this project.
  assert.deepEqual(seen.switchCalls, ["switchPlugin server:railway everywhere off", "switchPlugin server:pencil p-brigadier off", "switchPlugin server:playwright p-brigadier on"]);
  assert.equal(row(section(seen.offEverywhere!, "Every project").rows, "railway").line, "Off in every project.");
  assert.deepEqual(names(section(seen.inProject!, "Only in brigadier-ai").rows), ["playwright", "postgres"]);

  // A project's note shows on its agent's line, with its one fix.
  assert.match(seen.noted!.agentRows[1]!, /^CodexCodex only reads this project's settings once you trust the project\.Trust landing/);
  assert.deepEqual(seen.trustCalls, ["trustProject p-landing"]);

  // Search keeps the rows that match; an empty section goes.
  assert.deepEqual(seen.searched!.sections.map((entry) => [entry.title, names(entry.rows)]), [["Needs attention", ["linear", "figma-dev", "grill-me"]], ["Every project", ["railway"]]]);
});
