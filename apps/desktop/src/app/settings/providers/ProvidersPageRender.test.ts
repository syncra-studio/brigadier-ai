import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer } from "vite";

import type { AccountView } from "@/ipc/generated";
import { renderFixtureLive } from "@/fixtures/headless";

/** Rows' limits with their weekday reset and a countdown's minutes, which the clock moves, left out. */
const day = (rows: { limits: string[] }[]) =>
  rows.map((row) => ({ ...row, limits: row.limits.map((limit) => limit.replace(/Resets (?!in ).+$/, "Resets <day time>").replace(/(Resets in \d+h) \d+m$/, "$1 <min>")) }));

/**
 * The user asked for no "First", "Second", "nearest its weekly limit, 93% used" on the accounts:
 * no order slot on any row, and no row reads as the routing order.
 */
const noOrderLine = (seen: Record<string, unknown>) => {
  assert.equal(seen.orderSlots, 0);
  for (const row of seen.rowTexts as string[]) {
    assert.doesNotMatch(row, /First ·|Second ·|Third ·|nearest its|resets soonest|after the others/);
  }
};

// The real page in Chromium over a faked daemon: the agents with how each stands and its
// busiest limit, a row's switch that doesn't open it, an agent's page (Update when a newer CLI is
// out, Location's Change keeping only a real binary, every limit with when it resets, the models),
// emails masked with one revealed until the page or the agent's is left, the old Usage page's link landing on
// an agent's accounts, and this computer's login added again.
test("Providers lists the agents, and an agent's page has its installation, accounts and models", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=1", "providers-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(seen.title, "Providers");
  // The page's column is the one every Settings page has (#259).
  assert.equal(seen.settingsColumn, true);
  assert.deepEqual(seen.agents, [
    { name: "Claude", state: "Ready · 3 accounts · v2.1.0", meter: "Weekly · in use69% used" },
    { name: "Codex", state: "Ready · 1 account · v2.1.0", meter: "Weekly · in use69% used" },
  ]);
  // "Switch to another agent automatically" is back on the overview (S38, D23): off, and its
  // switch writes the setting.
  assert.deepEqual(seen.fallback, { row: true, on: false });
  assert.equal(seen.fallbackAfter, true);
  // Both are installed: nothing under Not set up, and Add another agent is there.
  assert.equal(seen.notSetUp, 0);
  assert.equal(seen.addAgent, true);
  assert.deepEqual(seen.afterSwitch, { page: false, disabled: ["codex"] });
  // A screen reader's press on a row opens its page and leaves its switch alone (#564).
  assert.deepEqual(seen.rowPress, { tag: "BUTTON", label: "Claude: open its accounts and models", inside: 0, switchInButton: false });
  assert.deepEqual(seen.disabledAfterPress, ["codex"]);
  assert.equal(seen.header, "ClaudeMaxReady · Claude Code by AnthropicEnabledCheck again");
  // Its logo sits before its name, in the design's 44px tile drawn as the overview rows' (#386).
  const rowMark = seen.rowMark as Record<string, string>;
  assert.ok(rowMark, "the overview rows draw a logo");
  assert.deepEqual(seen.claudeMark, { provider: "claude", glyph: true, size: [44, 44], beforeTitle: true, tile: rowMark });
  // One text node, so VoiceOver reads "Max" whole, not "M", "ax" (#580).
  assert.equal(seen.planChipNodes, 1);
  // The meter's name fits beside the percent, not cut to "Weekly · in u…" (#579).
  assert.equal(seen.meterOverflow, false);
  assert.equal(seen.rowOverflow, false);
  assert.equal(seen.cli, "Claude Code CLIv2.1.0 · v2.1.4 is outUpdate");
  assert.equal(seen.updateEnabled, true);
  assert.equal(seen.location, "Location/usr/local/bin/claudeChange");
  const masked = ["ste•••••@•••••.com", "wor•••••@•••••.io", "sid•••••@•••••.org"];
  // Each account is its name over its email (the eye beside the email), else its email over its
  // plan (D2); no order line. The first row is open on every limit, shortest first, with when each
  // resets (the clock's minutes left out); the others show their fullest limit (D7); In use
  // stands where the others have Use (D14).
  assert.deepEqual(day(seen.rows as { limits: string[] }[]), [
    {
      title: masked[0],
      line: "Max plan·This computer's login",
      eye: "title",
      expanded: "true",
      usage: "",
      limits: ["5-hour32% usedResets in 3h", "Weekly69% usedResets <day time>"],
      actions: [`${masked[0]}: more`],
      inUse: true,
    },
    {
      title: masked[1],
      line: "Max plan",
      eye: "title",
      expanded: "false",
      usage: "Weekly93%",
      limits: [],
      actions: ["Use", `${masked[1]}: more`],
      inUse: false,
    },
    {
      title: "Side project",
      line: `${masked[2]}·Max plan`,
      eye: "line",
      expanded: "false",
      usage: "Weekly69%",
      limits: [],
      actions: ["Use", "Side project: more"],
      inUse: false,
    },
  ]);
  // Every account here has a routing place and why, and the page shows neither.
  noOrderLine(seen);
  assert.equal(seen.eyes, 3);
  assert.deepEqual(seen.models, [
    "Opus 5.5opus  ·  defaultFrontier · Best for writing code, reviewing changes and resolving conflicts",
    "Sonnet 5.5sonnetStrong · Best for chat, writing code and research",
    "Fable 5.1claude-fable-5-1Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule",
  ]);
  // Only the row whose eye was pressed, and only until the page is left.
  assert.deepEqual(seen.revealed, [masked[0], "work.person@corp.example.io", "Side project"]);
  // A file that isn't Claude Code is refused with why, and nothing is kept; a real one is kept.
  assert.deepEqual(seen.refused, {
    location: "Location/usr/local/bin/claude/opt/elsewhere/claude isn't Claude Code: it didn't say its versionChange",
    paths: [],
  });
  assert.deepEqual(seen.chosen, {
    location: "Location/opt/homebrew/bin/claudeChange",
    paths: [{ provider: "claude", path: "/opt/homebrew/bin/claude" }],
  });
  assert.equal(seen.cameBack, true);
  const titles = [masked[0], masked[1], "Side project"];
  assert.deepEqual(seen.afterLeaving, titles);
  // The old Usage page's links land on an agent's accounts.
  assert.deepEqual(seen.usageLink, {
    page: { type: "settings", page: "providers" },
    agent: "ClaudeMax",
    accountsInView: true,
  });
  assert.equal(seen.back, true);
  // Going back to the agents hides a revealed email too.
  assert.deepEqual(seen.afterBack, titles);
  // The same account as this computer's login and added here is one row: the added one, saying
  // it is this computer's login, in use when either is (#263).
  assert.deepEqual(seen.twin, [
    { title: "STE•••••@•••••.COM", own: true, inUse: true, menu: true },
    { title: masked[1], own: false, inUse: false, menu: true },
    { title: "Side project", own: false, inUse: false, menu: true },
  ]);
  // In use is the account new work starts on, as switching picks it, not the one picked with Use.
  assert.deepEqual(seen.switched, [false, true, false]);
  // Each agent has its own account switch; Use pins an account and turns it off.
  assert.deepEqual(seen.accountSwitch, [
    { on: true, manual: [] },
    { on: false, manual: ["claude"] },
    { on: true, manual: [] },
  ]);
  assert.deepEqual(seen.afterUse, { on: false, manual: ["claude"] });
  // Every kind of row: this computer's login named "Personal" (its email moves to the line, D2),
  // one named by its email at 93%, one whose usage isn't read yet and one signed out, neither of
  // which opens.
  assert.deepEqual(day(seen.kinds as { limits: string[] }[]), [
    {
      title: "Personal",
      line: `${masked[0]}·Max plan·This computer's login`,
      eye: "line",
      expanded: "true",
      usage: "",
      limits: ["5-hour24% usedResets in 2h <min>", "Weekly41% usedResets <day time>", "Weekly · Fable12% usedResets <day time>"],
      actions: ["Personal: more"],
      inUse: true,
    },
    { title: masked[1], line: "Max plan", eye: "title", expanded: "false", usage: "Weekly93%", limits: [], actions: ["Use", `${masked[1]}: more`], inUse: false },
    { title: "Side project", line: `${masked[2]}·Max plan`, eye: "line", expanded: null, usage: "Usage not read yet", limits: [], actions: ["Use", "Side project: more"], inUse: false },
    { title: "m•••••@•••••.me", line: "Not signed in", eye: "title", expanded: null, usage: "", limits: [], actions: ["Sign in", "m•••••@•••••.me: more"], inUse: false },
  ]);
  assert.deepEqual(seen.kindsOpen, [true, false, false, false]);
  // The eye shows the email and leaves the row open; a press on a closed row's name opens it,
  // the first staying open (several may be).
  assert.deepEqual(seen.afterEye, { open: [true, false, false, false], line: "stephen.golban@example.com·Max plan·This computer's login" });
  assert.deepEqual(seen.twoOpen, { open: [true, true, false, false], expanded: ["true", "true", null, null], limits: ["5-hour61% used", "Weekly93% used"] });
  // The keyboard's toggle is a button of its own, with no control inside it: Enter closes the
  // row, Space opens it again.
  assert.equal(seen.toggleFocused, "account-toggle");
  assert.equal(seen.toggleNested, 0);
  assert.deepEqual(seen.afterEnter, [false, true, false, false]);
  assert.deepEqual(seen.afterSpace, [true, true, false, false]);
  // Use picks an account without toggling its row.
  assert.deepEqual(seen.afterUseClick, { open: [true, true, false, false], manual: ["claude"] });
  // ⋯ (D11, D12): this computer's login Rename only, an added account Rename, Sign in again and
  // Remove, a signed-out one Rename and Remove (Sign in is on the row). None toggles its row.
  assert.deepEqual(seen.menus, [["Rename"], ["Rename", "Sign in again", "Remove"], ["Rename", "Remove"]]);
  assert.deepEqual(seen.afterMenus, [true, true, false, false]);
  // This computer's login renamed, cleared back to its email, and an edit dropped with Escape
  // (which leaves Settings open); the field never toggles its row.
  const open = [true, true, false, false];
  assert.deepEqual(seen.renamed, { field: "Personal", open, names: [{ provider: "claude", name: "Home" }], stillOpen: true });
  assert.deepEqual(seen.cleared, { field: "Personal", open, names: [], stillOpen: true });
  assert.deepEqual(seen.cancelled, { field: "Personal", open, names: [], stillOpen: true });
  // A masked email-like name starts the field empty; leaving it untouched keeps the name.
  assert.deepEqual(seen.untouched, { field: "", open, names: [{ provider: "claude", name: "home@example.net" }], stillOpen: true });
  // Codex's page has its own logo too (#386).
  assert.deepEqual(seen.codexMark, { provider: "codex", glyph: true, size: [44, 44], beforeTitle: true, tile: rowMark });
});

/** An extra Claude account named `name`, signed in as `email`. */
const view = (name: string, email: string): AccountView => ({
  account: { provider: "claude", account: "a" },
  name,
  default: false,
  inUse: false,
  status: { provider: "claude", path: "/c", version: null, loggedIn: true, authMethod: null, plan: null, email, guidance: null, compacts: false },
  quota: null,
  quotaAtMs: null,
  checkedAtMs: null,
  checking: false,
  checkedGeneration: 0,
  error: null,
  place: null,
  reason: null,
});

// A login merged with its added twin keeps the earlier routing place and the places have no gap;
// the rows stay this computer's login first whatever the routing order, and never say it.
test("account rows keep the login first, merge its twin's place without gaps, and show no order", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=order", "providers-result", 30000);
  const seen = JSON.parse(dump) as Record<string, Record<string, unknown>>;
  assert.deepEqual(seen.merged!.rows, ["hostpapa", "golban"]);
  assert.deepEqual(seen.merged!.places, [["hostpapa", 1], ["golban", 2]]);
  noOrderLine(seen.merged!);
  assert.deepEqual(seen.routed!.rows, ["hostpapa", "golban", "Limited", "Signed out"]);
  assert.deepEqual(seen.routed!.places, [["hostpapa", 2], ["golban", 1], ["Limited", null], ["Signed out", null]]);
  noOrderLine(seen.routed!);
});

// Renaming never shows an email the row hides: the editor starts empty for an account named by
// its email unless that row is revealed, and never starts from the signed-in email.
test("an account's name and editor keep its email masked unless its row is revealed", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { accountNames } = await server.ssrLoadModule("/src/app/settings/providers/AccountRows.tsx");
    const named = view("work.person@corp.example.io", "me@example.com");
    assert.deepEqual(accountNames(named, false), { title: "wor•••••@•••••.io", email: "me•••••@•••••.com", editable: "" });
    assert.deepEqual(accountNames(named, true), {
      title: "work.person@corp.example.io",
      email: "me@example.com",
      editable: "work.person@corp.example.io",
    });
    // No name of its own: the title is the email, and the editor starts empty either way.
    const unnamed = view("", "me@example.com");
    assert.deepEqual(accountNames(unnamed, false), { title: "me•••••@•••••.com", email: null, editable: "" });
    assert.equal(accountNames(unnamed, true).editable, "");
    assert.equal(accountNames(view("me@example.com", "me@example.com"), true).editable, "");
    // This computer's login renamed goes by its name too, its email on the line below.
    const own = { ...view("Personal", "me@example.com"), account: { provider: "claude" as const } };
    assert.deepEqual(accountNames(own, false), { title: "Personal", email: "me•••••@•••••.com", editable: "Personal" });
    // A plain name shows and edits as it is.
    assert.deepEqual(accountNames(view("Side project", "s@example.org"), false), {
      title: "Side project",
      email: "s•••••@•••••.org",
      editable: "Side project",
    });
  } finally {
    await server.close();
  }
});

// An agent not on this computer: its Set up is Connect (its own installer, then its own sign-in,
// no terminal): it says how the install goes, a failure plainly with Try again, and once
// installed the agent joins the list, signing in.
test("Providers sets up a missing agent with Connect", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=missing", "providers-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.deepEqual(seen.agents, ["Claude"]);
  assert.deepEqual(seen.before, [{ text: "CodexCodex CLI · not installed", status: "", buttons: ["Set up"] }]);
  assert.deepEqual(seen.installing, [
    { text: "CodexCodex CLI · not installed", status: "Installing Codex… This takes a minute or two.", buttons: ["Set up (off)", "Cancel"] },
  ]);
  assert.deepEqual(seen.failed, [
    { text: "CodexCodex CLI · not installed", status: "Codex couldn't be installed. Try again.", buttons: ["Set up", "Try again"] },
  ]);
  assert.deepEqual(seen.signingIn, [
    { name: "Claude", state: "Ready · 3 accounts · v2.1.0" },
    { name: "Codex", state: "Not signed in" },
  ]);
  assert.equal(seen.notSetUpAfter, 0);
  assert.equal(seen.terminals, 0);
  assert.deepEqual(seen.asked, ["connectAgent codex", "connectAgent codex"]);
});

// On a Linux with neither curl nor wget, the row's Connect line names them and links the
// vendor's install guide, which opens; Try again still asks Connect.
test("Providers names a missing curl or wget and links the install guide", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=missing&nodownloader=1", "providers-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.deepEqual(seen.failed, [
    {
      text: "CodexCodex CLI · not installed",
      status:
        "Installing Codex needs curl or wget, and neither is installed. Install one (for example `sudo apt install curl`) and try again, or install Codex yourself.",
      buttons: ["Set up", "Try again", "Codex's install guide"],
    },
  ]);
  assert.deepEqual(seen.asked, ["connectAgent codex", "openUrl https://github.com/openai/codex", "connectAgent codex"]);
});

// Drawn with every Settings page's parts (#385): its cards are Settings › General's (fill,
// corners, edge), an agent row under the pointer is washed edge to edge with the shared row
// wash, and its switches are General's.
test("Providers draws its cards, rows and switches as Settings › General does", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=look", "providers-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as {
    general: { card: unknown; switch: unknown };
    overview: { cards: unknown[]; switch: unknown; hover: string; wash: string; fillsCard: boolean };
    agent: { cards: unknown[]; switch: unknown };
  };
  assert.ok(seen.general.card, "General has a card");
  assert.equal(seen.overview.cards.length, 3);
  for (const card of [...seen.overview.cards, ...seen.agent.cards]) assert.deepEqual(card, seen.general.card);
  assert.equal(seen.agent.cards.length, 3);
  assert.equal(seen.overview.hover, seen.overview.wash);
  assert.equal(seen.overview.fillsCard, true);
  assert.deepEqual(seen.overview.switch, seen.general.switch);
  assert.deepEqual(seen.agent.switch, seen.general.switch);
});

const signedIn = (checkedAtMs: number) => ({ status: { loggedIn: true }, checkedAtMs });
const signedOut = (checkedAtMs: number) => ({ status: { loggedIn: false }, checkedAtMs });

// An account's sign-in row closes as soon as the account reads signed in from a check made after
// the sign-in started (the daemon checks it the moment its login is stored, before the CLI
// ends); one already signed in when it started waits for the sign-in to end, as before.
test("an account's sign-in is done once it reads signed in since it started", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { signInFinished } = await server.ssrLoadModule("/src/app/settings/providers/AccountRows.tsx");
    const fresh = { startedAtMs: 10_000, wasSignedIn: false, exitedAtMs: null };
    assert.equal(signInFinished(fresh, undefined), false);
    assert.equal(signInFinished(fresh, signedOut(12_000)), false);
    // Still running, checked since it started: done.
    assert.equal(signInFinished(fresh, signedIn(12_000)), true);
    // A check from before it started doesn't count.
    assert.equal(signInFinished(fresh, signedIn(9_000)), false);
    // Signed in when it started ("Sign in again"): only a check after it ended counts.
    const again = { startedAtMs: 10_000, wasSignedIn: true, exitedAtMs: null };
    assert.equal(signInFinished(again, signedIn(12_000)), false);
    assert.equal(signInFinished({ ...again, exitedAtMs: 15_000 }, signedIn(14_000)), true);
    assert.equal(signInFinished({ ...again, exitedAtMs: 15_000 }, signedIn(12_000)), false);
  } finally {
    await server.close();
  }
});
