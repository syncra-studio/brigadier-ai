import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Radio = { title: string; line: string; checked: string };
type Summary = { title: string; line: string; button: string };
type Exception = { name: string; path: string; badge: string };
type Where = { labels: string[]; description: string; radios: Radio[]; summary: Summary | null; exceptions: Exception[]; chips: Record<string, string[]>; groups: { title: string; description: string; aside: string }[]; lines: string[][]; tabs: string[]; select: { setting: string; value: string; washed: boolean } | null; note: string };
type Row = { title: string; line: string; end: string };
type Switch = { label: string; checked: string; disabled: boolean };
type State = { title: string; description: string; sections: string[]; text: string; switches: Switch[]; overflow: boolean; signIn: { title: string; line: string; rows: Row[] } | null; where: Where; settings?: unknown };
type Picker = { name: string; placeholder: string; label: string; projects: { name: string; path: string; state: string; disabled: boolean }[]; footer: string };

const PENCIL = "ClaudeUser:pencil";
const API = "ClaudeProjectFile:steady-app:steady-api";
const POSTGRES = "ClaudeProjectFile:postgres";
const PLAYWRIGHT = "Desktop:playwright";
const GRILLING = "user:grilling:3";
const DEFAULT = "The switch above is the default. A trusted project can make an exception for itself.";
const TURN_OFF = "Turn off in a project…";
const EVERY: Summary = { title: "Every project", line: "On by default, so sessions in every trusted project get it.", button: TURN_OFF };
const EXCEPT_BRIGADIER: Summary = { title: "Every project except brigadier-ai", line: "On by default. brigadier-ai turns it off for itself.", button: TURN_OFF };
const exception = (name: string, badge: string, path = `~/Code/${name}`): Exception => ({ name, path, badge });
// The Where it's used section of a page.
const shared = (label: string, summary: Summary, exceptions: Exception[] = []) => ({ labels: [label], description: DEFAULT, radios: [], summary, exceptions });
const stays = (project: string, on: boolean) => ({
  labels: [`${on ? "On" : "Off"} in ${project}`],
  description: "It comes from a project's own config.",
  radios: [
    { title: `Only in ${project}`, line: `Its config lives in this project's .mcp.json, so only sessions in ${project} get it.`, checked: "true" },
    { title: "Every project", line: `Use it like one of your own: on everywhere, with exceptions per project. Brigadier keeps reading it from ${project}'s .mcp.json.`, checked: "false" },
  ],
  summary: null,
  exceptions: [],
});
const everywhere = (project: string, summary: Summary, exceptions: Exception[] = []) => ({
  labels: ["On by default"],
  description: `It comes from ${project}'s own config.`,
  radios: [
    { title: `Only in ${project}`, line: "Its config lives in that project's .mcp.json. Picking this again drops the exceptions below.", checked: "false" },
    { title: "Every project", line: `Used like one of your own: on by default everywhere, with exceptions. Brigadier keeps reading it from ${project}'s .mcp.json.`, checked: "true" },
  ],
  summary,
  exceptions,
});
// The settings after each action: every snapshot starts from the fixture's, where steady-api is used everywhere,
// brigadier-ai turns it off, steady-app turns pencil off and both turn grilling on.
const entry = ([serversOn, serversOff]: string[][], projectId: string) => ({ projectId, serversOn, serversOff, skillsOn: [GRILLING], skillsOff: [] });
const settings = (mcpServersOff: string[], brigadier: string[][], steady: string[][], serversEverywhere: string[]) => ({ mcpServersOff, skillsOff: [GRILLING], projectSwitches: [entry(brigadier, "brigadier-ai"), entry(steady, "steady-app")], serversEverywhere, skillsEverywhere: [] });
const VIEW_NOTE = (project: string) => `Showing what sessions in ${project} get. A switch here changes only ${project}: it adds or removes this project's exception. The default stays as it is.`;
const projectGroups = (project: string, own: number, changed: number, defaults: number, unused: number) => [
  { title: `From ${project}'s own config`, description: "Its .mcp.json, .codex/config.toml and .cursor/mcp.json", aside: `${own} server${own === 1 ? "" : "s"}` },
  { title: `Changed for ${project}`, description: "Its exceptions to your defaults", aside: `${changed} server${changed === 1 ? "" : "s"}` },
  { title: "Your defaults it uses", description: "On in every project, and not changed here", aside: `${defaults} servers` },
  { title: `Not used in ${project}`, description: "Off by default, couldn't be read, or another project's own", aside: `${unused} servers` },
];
const line = {
  figma: ["figma-dev", "Couldn't read it: unknown type “websocket”", "Not used"],
  linear: ["linear", "https://mcp.linear.app/mcp", "true"],
  nodeRepl: ["node_repl", "npx -y @openai/node-repl-mcp", "true"],
  pencil: ["pencil", "/Applications/Pencil.app/Contents/MacOS/pencil mcp", "true"],
  playwright: ["playwright", "npx @playwright/mcp@latest", "false"],
  railway: ["railway", "npx -y @railway/mcp-server", "true"],
  api: ["steady-api", "node ./tools/steady-api-mcp/index.js", "true"],
  postgres: ["postgres", "npx -y @modelcontextprotocol/server-postgres postgresql://localhost/brigadier_dev", "true"],
  db: ["steady-db", "npx -y @supabase/mcp-server --project-ref steady", "true"],
};
const off = (row: string[]) => [row[0]!, row[1]!, "false"];
const PLAYWRIGHT_UNUSED = ["playwright", "Off by default · switch on to use it here", "false"];

test("plugins where it's used: every screen and component state on the board, and the settings each action writes", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "integrations.html?drive=where-used", "integrations-result", 30000)) as Record<string, unknown> & Record<string, State>;
  assert.equal(seen.error, undefined);
  for (const [state, page] of Object.entries(seen)) if (page && typeof page === "object" && "overflow" in page) assert.equal(page.overflow, false, state);
  const detail = (state: string) => {
    const { labels, description, radios, summary, exceptions } = seen[state]!.where;
    return { labels, description, radios, summary, exceptions };
  };
  const list = (state: string) => {
    const { chips, groups, lines, tabs, select, note } = seen[state]!.where;
    return { chips, groups, lines, tabs, select, note };
  };

  // The list: chips say where a server or skill is used, when that isn't simply everywhere or nowhere.
  assert.deepEqual(seen["where-servers"]!.sections, ["Yours", "brigadier-ai", "steady-app"]);
  assert.deepEqual(list("where-servers"), {
    chips: { [`server:${PENCIL}`]: ["Off in steady-app"], [`server:${API}`]: ["From steady-app", "Off in brigadier-ai"] },
    groups: [
      { title: "Yours", description: "Claude Code, Codex, Claude Desktop and Cursor — on in every project unless a project says otherwise", aside: "Switches apply to new and resumed sessions" },
      { title: "brigadier-ai", description: "~/Code/brigadier-ai · its own config, only in this project", aside: "1 server" },
      { title: "steady-app", description: "~/Code/steady-app · its own config, only in this project", aside: "1 server" },
    ],
    lines: [line.figma, line.linear, line.nodeRepl, line.pencil, line.playwright, line.railway, line.api, line.postgres, line.db],
    tabs: ["MCP servers9", "Skills19"],
    select: { setting: "project:all", value: "All projects", washed: false },
    note: "",
  });
  assert.deepEqual(list("where-skills").chips, { [`skill:${GRILLING}`]: ["On in 2 projects"] });
  assert.deepEqual(list("where-skills").groups.map((group) => [group.title, group.description, group.aside]), [
    ["Yours", "Every worker gets every skill, whichever agent it came from", "Switches apply to new and resumed sessions"],
    ["brigadier-ai", "~/Code/brigadier-ai · its own config, only in this project", "2 skills"],
  ]);
  assert.equal(seen.chipTip, "On in brigadier-ai and steady-app");

  // A server or skill page: on or off by default, with each project's exception.
  for (const state of ["where-railway", "where-pencil", "where-postgres", "where-steady-api", "where-playwright", "where-untrusted", "where-runs-from"]) assert.deepEqual(seen[state]!.sections, ["Where it's used", "What it is", "Found in", "Workers"], state);
  assert.deepEqual(seen["where-grilling"]!.sections, ["Where it's used", "Folders", "Workers"]);
  assert.deepEqual(detail("where-railway"), shared("On by default", EVERY));
  assert.deepEqual(detail("where-pencil"), shared("On by default", { ...EXCEPT_BRIGADIER, title: "Every project except steady-app", line: "On by default. steady-app turns it off for itself." }, [exception("steady-app", "Off here")]));
  assert.equal(seen.removeTip, "Remove exception");
  assert.deepEqual(detail("where-grilling"), shared("Off by default", { title: "Only in brigadier-ai and steady-app", line: "Off by default. These two projects turn it on for themselves.", button: "Turn on in a project…" }, [exception("brigadier-ai", "On here"), exception("steady-app", "On here")]));
  assert.deepEqual(detail("where-playwright"), shared("Off by default", { title: "No project", line: "Off by default, and no project turns it on.", button: "Turn on in a project…" }));
  // An exception kept for a project that's no longer trusted.
  assert.deepEqual(detail("where-untrusted"), shared("On by default", { title: "Every project except steady-app and old-landing", line: "On by default. These two projects turn it off for themselves.", button: TURN_OFF }, [exception("steady-app", "Off here"), exception("old-landing", "Off here", "No longer trusted · kept, but not read")]));

  // A project's own server: only in its project, or used in every project.
  assert.equal(seen["where-postgres"]!.description, "Local command · from brigadier-ai's own .mcp.json");
  assert.deepEqual(detail("where-postgres"), stays("brigadier-ai", true));
  assert.equal(seen["where-steady-api"]!.description, "Local command · from steady-app's own .mcp.json · used in every project");
  assert.deepEqual(detail("where-steady-api"), everywhere("steady-app", EXCEPT_BRIGADIER, [exception("brigadier-ai", "Off here")]));
  // Runs from shows the server's own working folder, also when it isn't the project's root.
  assert.match(seen["where-runs-from"]!.text, /Runs from~\/Code\/steady-app\/tools\/steady-api-mcp, wherever the session is/);
  assert.match(seen["where-steady-api"]!.text, /Runs from~\/Code\/steady-app, wherever the session is/);

  // The project view: what sessions in a project get, in four groups.
  assert.deepEqual(list("where-brigadier"), {
    chips: { [`server:${API}`]: ["From steady-app", "Off here"] },
    groups: projectGroups("brigadier-ai", 1, 1, 4, 3),
    lines: [line.postgres, off(line.api), line.linear, line.nodeRepl, line.pencil, line.railway, line.figma, PLAYWRIGHT_UNUSED, ["steady-db", "Only in steady-app · from its own .mcp.json", "steady-app only"]],
    tabs: ["MCP servers5", "Skills19"],
    select: { setting: "project:brigadier-ai", value: "brigadier-ai", washed: true },
    note: VIEW_NOTE("brigadier-ai"),
  });
  assert.deepEqual(seen["where-steady-app"]!.sections, ["From steady-app's own config", "Changed for steady-app", "Your defaults it uses", "Not used in steady-app"]);
  assert.deepEqual(list("where-steady-app"), {
    chips: { [`server:${PENCIL}`]: ["Off here"] },
    groups: projectGroups("steady-app", 2, 1, 3, 3),
    lines: [line.api, line.db, off(line.pencil), line.linear, line.nodeRepl, line.railway, line.figma, PLAYWRIGHT_UNUSED, ["postgres", "Only in brigadier-ai · from its own .mcp.json", "brigadier-ai only"]],
    tabs: ["MCP servers5", "Skills17"],
    select: { setting: "project:steady-app", value: "steady-app", washed: true },
    note: VIEW_NOTE("steady-app"),
  });
  // Skills: no group of unused ones when every skill is used.
  const skillsView = list("where-brigadier-skills");
  assert.deepEqual(skillsView.groups.map((group) => [group.title, group.description, group.aside]), [
    ["From brigadier-ai's own config", "Its .claude/skills and .agents/skills", "2 skills"],
    ["Changed for brigadier-ai", "Its exceptions to your defaults", "1 skill"],
    ["Your defaults it uses", "On in every project, and not changed here", "16 skills"],
  ]);
  assert.deepEqual(skillsView.chips, { [`skill:${GRILLING}`]: ["On here"] });
  assert.deepEqual(skillsView.lines.slice(0, 3).map((row) => [row[0], row[2]]), [["delegator", "true"], ["release-notes", "true"], ["grilling", "true"]]);

  // The sign-in status follows the page's project: off in brigadier-ai, linear doesn't claim workers can use it there.
  assert.deepEqual(seen["where-linear-off"]!.sections, ["Where it's used", "Sign-in", "What it is", "Found in"]);
  assert.deepEqual(detail("where-linear-off"), shared("On by default", EXCEPT_BRIGADIER, [exception("brigadier-ai", "Off here")]));
  assert.deepEqual([seen["where-linear-off"]!.signIn!.title, seen["where-linear-off"]!.signIn!.line], ["Off", "Brigadier doesn't give it to workers."]);
  assert.deepEqual([seen["where-linear-all"]!.signIn!.title, seen["where-linear-all"]!.signIn!.line], ["Every worker can use it", "Signed in through Claude Code. Brigadier reuses that sign-in for Codex workers, so every worker can use it."]);
  assert.deepEqual(seen["where-linear-off"]!.signIn!.rows, seen["where-linear-all"]!.signIn!.rows);

  // The picker lists every project; ones that can't take the exception say why.
  const picker = seen.picker as unknown as Picker;
  const footer = "Brigadier only reads trusted projects. Trust one from its menu in the sidebar.";
  assert.deepEqual(picker, {
    name: "Turn pencil off in…",
    placeholder: "Find a project",
    label: "Turn pencil off in…",
    projects: [
      { name: "brigadier-ai", path: "~/Code/brigadier-ai", state: "", disabled: false },
      { name: "northwind-site", path: "~/Code/clients/northwind-site", state: "", disabled: false },
      { name: "steady-app", path: "~/Code/steady-app", state: "Already off here", disabled: true },
      { name: "scratch", path: "~/scratch", state: "Not trusted", disabled: true },
      { name: "old-landing", path: "~/Code/old-landing", state: "Not trusted", disabled: true },
    ],
    footer,
  });
  assert.deepEqual(seen.pickerFound, { ...picker, projects: [picker.projects[1]] });

  // Each action's settings. Picking brigadier-ai turns pencil off there; removing steady-app's exception drops its id.
  const base = settings([PLAYWRIGHT], [[], [API]], [[], [PENCIL]], [API]);
  assert.deepEqual(detail("picked"), shared("On by default", { title: "Every project except brigadier-ai and steady-app", line: "On by default. These two projects turn it off for themselves.", button: TURN_OFF }, [exception("brigadier-ai", "Off here"), exception("steady-app", "Off here")]));
  assert.deepEqual(seen.picked!.settings, settings([PLAYWRIGHT], [[], [API, PENCIL]], [[], [PENCIL]], [API]));
  assert.deepEqual(detail("removed"), shared("On by default", EXCEPT_BRIGADIER, [exception("brigadier-ai", "Off here")]));
  assert.deepEqual(seen.removed!.settings, settings([PLAYWRIGHT], [[], [API, PENCIL]], [[], []], [API]));
  // A project's own server: every project, then only its project again; then the same while it's off in its project.
  assert.deepEqual(detail("postgresEverywhere"), everywhere("brigadier-ai", EVERY));
  assert.deepEqual(seen.postgresEverywhere!.settings, settings([PLAYWRIGHT], [[], [API]], [[], [PENCIL]], [API, POSTGRES]));
  assert.deepEqual(detail("postgresOnly"), stays("brigadier-ai", true));
  assert.deepEqual(seen.postgresOnly!.settings, base);
  assert.deepEqual(detail("postgresOff"), stays("brigadier-ai", false));
  assert.equal(seen.postgresOff!.switches[0]!.checked, "false");
  assert.deepEqual(seen.postgresOff!.settings, settings([PLAYWRIGHT, POSTGRES], [[], [API]], [[], [PENCIL]], [API]));
  // Off in its own project when used everywhere: that stays as brigadier-ai's exception, and it's on by default elsewhere.
  assert.deepEqual(detail("postgresOffEverywhere"), everywhere("brigadier-ai", EXCEPT_BRIGADIER, [exception("brigadier-ai", "Off here")]));
  assert.deepEqual(seen.postgresOffEverywhere!.settings, settings([PLAYWRIGHT], [[], [API, POSTGRES]], [[], [PENCIL]], [API, POSTGRES]));
  assert.deepEqual(detail("postgresOffOnly"), stays("brigadier-ai", false));
  assert.deepEqual(seen.postgresOffOnly!.settings, settings([PLAYWRIGHT, POSTGRES], [[], [API]], [[], [PENCIL]], [API]));
  // Only in steady-app again drops brigadier-ai's exception for steady-api.
  assert.deepEqual(detail("steadyOnly"), stays("steady-app", true));
  assert.deepEqual(seen.steadyOnly!.settings, settings([PLAYWRIGHT], [[], []], [[], [PENCIL]], []));
  // The project view's switches change only brigadier-ai's exceptions; its own server's switch is its on or off there.
  assert.deepEqual(list("viewSwitched").chips, { [`server:${PLAYWRIGHT}`]: ["On here"], [`server:${API}`]: ["From steady-app"] });
  assert.deepEqual(list("viewSwitched").groups, projectGroups("brigadier-ai", 1, 1, 5, 2));
  assert.deepEqual(seen.viewSwitched!.settings, settings([PLAYWRIGHT], [[PLAYWRIGHT], []], [[], [PENCIL]], [API]));
  assert.deepEqual(seen.viewOwn, settings([PLAYWRIGHT, POSTGRES], [[PLAYWRIGHT], []], [[], [PENCIL]], [API]));
  assert.deepEqual(seen.edits, [
    `inProject server ${PENCIL} brigadier-ai false`,
    `inProject server ${PENCIL} steady-app null`,
    `everywhere server ${POSTGRES} brigadier-ai true`,
    `everywhere server ${POSTGRES} brigadier-ai false`,
    `everywhere server ${POSTGRES} brigadier-ai true`,
    `everywhere server ${POSTGRES} brigadier-ai false`,
    `everywhere server ${API} steady-app false`,
    `inProject server ${PLAYWRIGHT} brigadier-ai true`,
    `inProject server ${API} brigadier-ai true`,
  ]);
  assert.deepEqual(seen.writes, [{ kind: "server", id: POSTGRES, enabled: false }, { kind: "server", id: POSTGRES, enabled: false }]);

  // Settings search finds each server with where it lives, and the project, which opens its view.
  assert.deepEqual(seen.search, ["steady-apiPlugins › MCP servers · from steady-app", "steady-dbPlugins › MCP servers · steady-app", "steady-appPlugins · project view"]);
  assert.deepEqual(seen.searchOpened!.where.select, { setting: "project:steady-app", value: "steady-app", washed: true });
});
