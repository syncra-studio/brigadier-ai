# Plugins page: research for the redesign

Date: 2026-10-24. Scope: the Settings › Plugins page (skills, MCP servers, plugin bundles) and what it needs underneath so
that an item can be used **in every project or only in some**, and **in every agent CLI**. Phase 1 only: understanding,
research and design. No app code changed.

Related: `docs/STRIP-DOWN-PLAN.md` §9.4 (X1–X9, the bar), G17, Q13; `research/SKILLS-MCP-PROBES.md` (P5: project scope).
Designs: `~/Documents/brigadier-design/settings.pen`, board `Plugins · redesign (pick one)` (`ncpsz`); parts in
`foundations.pen` › `Settings · Plugins` › `Row · Redesign parts (draft)` (`B9XsC5`).

## 1. How it works today

**Reading.** `crates/registry` builds one catalog from each agent's own files:

- Claude Code: `~/.claude.json` (user and per-project `local`), project `.mcp.json`, the skills folders, plugins from
  `installed_plugins.json` plus `enabledPlugins`.
- Codex: `config.toml` (user and project layers), `~/.agents/skills`, `$CODEX_HOME/skills`, plugins.
- Claude Desktop and Cursor: read for the catalog only.

The catalog reaches the page as `IntegrationsView` (`McpServerView`, `SkillView`) from
`core/src/manager/integrations.rs`.

**Switching.** The page's switches are Brigadier's own, not the agents'. They live in the settings stream
(`mcp_servers_off`, `skills_off`, `project_switches`, `servers_everywhere`, `skills_everywhere`;
`core/src/model.rs`), and are applied per session at launch:

- Claude: `--settings` with `permissions.deny` (`mcp__<name>`, `Skill(...)`) and `skillOverrides`
  (`providers/src/claude/mod.rs`).
- Codex: `-c skills.config`, `-c plugins.<id>.mcp_servers.<s>.enabled=false`, `mcp_servers.<n>={enabled:false}`
  (`providers/src/codex/mod.rs`).
- Project exceptions are resolved in TypeScript.

So a switch flipped in the app does nothing in `claude` or `codex` run in a terminal.

**Writing.** Brigadier never writes `mcpServers`, `enabledPlugins`, `.mcp.json` or a project `.codex/config.toml`. It
only writes folder trust (`providers/src/trust.rs`). The page copy says so: "It only reads them".

**Sign-in.** `mcp_auth` probes sign-in state read-only and can run Brigadier's own OAuth into the Keychain. Nothing
consumes that token any more (cross-agent worker sharing went in S16c); X5 removes it.

**Accounts.** Extra Claude account homes start with a fresh `.claude.json` and no `settings.json`, so they miss the
user's MCP servers and plugins (`providers/src/accounts.rs`). This is hole X6.

**Agents.** `ProviderKind {Claude, Codex}` and `registry::Agent {Claude, Codex}`. Skills and MCP logic is written per
agent inline, not behind a trait.

**UI.** `apps/desktop/src/app/settings/integrations/*`:

- `PluginsPage.tsx`: tabs for MCP servers and Skills, a project select, groups.
- `ServerPage`, `SkillPage` and `SignInSection`.
- `live.ts` and `adapter.ts`; the page polls every 5 s.
- IPC: `getIntegrations`, `updateSettings`, and the sign-in calls (`signInMcpServer`, `getMcpSignIn`, cancel,
  reopen, sign out, ask again).

The page's words come from the engine: workers, exceptions, read-only, "Found in", "Not used".

## 2. The gap

What the user asked for, set against today:

| Want | Today |
|---|---|
| Add once, works in every agent | Items live where each agent put them. A Claude-only skill stays Claude-only; nothing on the page can fix it. |
| Pick: every project, or only some | Project items are whatever the agents' own project files say. The app's per-project switches only change Brigadier sessions. |
| Same result in the app and in a terminal | App switches are launch flags, so the terminal disagrees (against G17, terminal == app). |
| Works for 6+ CLIs | Two agents are hard-wired in the model, the registry, the launch flags and the page. |
| Plain, obvious states | Two tabs, one project select, read-only hints, engine words. Nothing says "this needs you". |

The settled mechanism already covers most of this (§9.4): the **Library** (`~/.brigadier/library/`) is a local plugin
marketplace, installed through each CLI's own plugin commands. Every agent then gets the item natively, in the app and
in a terminal, and Brigadier writes no config text itself. None of the Library exists yet (no `library.rs`, no
`brigadier library`).

**The open piece is project scope.**

- Claude: `plugin install --scope local|project` works natively (P5).
- Codex 0.164 rejects `plugin add --scope`, and its native config writer refuses project files (P5). Codex does
  *read* a project `[plugins."x"] enabled = true`, but only in trusted projects.
- Under the native-only rule, Codex cannot get "only in this project" today without either a per-session flag or
  Brigadier writing project TOML. Both are questions for the user (§6).

## 3. What rivals ship (factual)

| Product | Where scope is chosen | Works-in-which-agent | Broken / needs-you | Notes |
|---|---|---|---|---|
| Claude Code `/plugin`, `/mcp` | At install, as three sentences: for you / for everyone on this repo / for you in this repo only. Precedence local > project > user. | Claude only | Separate Errors tab; status words Connected / Needs authentication / Failed + `Issue:` line / Disabled for this project / Pending approval | Shows each plugin's context cost per turn. Removing a shared entry offers "Disable for me" vs "Uninstall for everyone". |
| Codex app and CLI | The app settings are global only; repo config is honoured in trusted projects but can't be changed from the UI (openai/codex#13075, closed as not planned) | Codex only | "Authenticate" on servers that need OAuth | The add form has only name, transport, and command or URL; everything else is in `config.toml`. Skills and plugins need a new session. |
| Claude Desktop connectors | Per account; per chat on/off | Claude only | Sign-in state per connector | Sign-in timing choice: now / when needed / none. Per-tool Always allow / Needs approval / Blocked. |
| Cursor | Global `~/.cursor/mcp.json` vs project `.cursor/mcp.json`; precedence not documented | Cursor only | Status dot; logs in an output panel | The settings label has changed names three times. |
| VS Code | Install vs "Install in Workspace"; Disable asks for a scope: global, workspace or session | Copilot only | Show Output | Five config sources; only the user file syncs. |
| Zed | Settings › AI › MCP Servers | Zed only | Status dot | After install, a sheet lists only the setup that's missing. |
| superset | None: its own skills are installed for you | Copies its curated items into both CLIs' folders | n/a | Writes managed copies into `~/.claude/skills/…` and `~/.agents/skills/`. Curated items only, and it needs an account. |
| t3code | None found | Composer `$` picker | n/a | Same-name skills collide and the wrong one runs (#11161). Plugin skills are missing from the picker. |
| Orca | Workspace picker plus an agent picker at install | An "All agents" filter on the Skills page | "Needs attention" when a copy can't be rewritten | One copy per Claude account, with no label (#22304). The central MCP catalog with per-agent and project scope is still a request (#23906). |
| Emdash | None | Reads `~/.claude.json` and one other global file | n/a | Project and plugin MCPs are missing from the panel even though the agent loads them (#2039). |
| bb | Not documented | Not documented | Not documented | Only a product listing was found. |

Nobody ships **one item, every agent, per project or global, native in the terminal too**. The closest are superset
(curated only, both CLIs) and Orca (an agent picker for skills; MCP is still a request).

## 4. Patterns we take

Written as our own rules. Where an idea came from stays out of the repo.

1. **One choice per item, in plain words: "Use in".** All projects / Only in some projects (pick them) / Off. The
   same words are used at Add time, on the row and on the details page. "All projects except…" is under Advanced.
2. **Every agent by default; show it, don't ask it.** Each row shows a strip of agent marks; a dimmed mark means "not
   in that agent". The details page has one line per agent: Ready / Needs sign-in / Won't start / Not supported, each
   with a reason. Per-agent off lives under Advanced. Designed for six marks, with letter tiles for CLIs that have no
   glyph yet.
3. **A "Needs you" group at the top**, each row with exactly one fix button: Sign in, Use everywhere, Details. It is
   also flagged on the row itself. Healthy items fold to a single line ("Ready in every agent").
4. **Add takes anything.** An `mcp add` line from any CLI, a URL, `mcpServers` JSON, a GitHub link, a skills.sh name or
   a folder (X3). It reads the item back for checking before anything changes, then adds it in the background, agent
   by agent, with progress on the row.
5. **Sign-in is one step per agent, in order.** Each CLI runs its own `mcp login`. The dialog lists every agent's state
   and says which agents can't sign in, and why.
6. **Installed and signed in are separate.** Removing an item says the sign-ins go too. Signing out doesn't remove it.
7. **Status words are fixed:** Ready, Needs sign-in, Won't start, Not supported, Off, Adding. Each carries a reason
   line. Never colour alone.
8. **One disclosure per page.** Advanced holds: the command or address, environment and headers, turn off for one
   agent, where it's saved, and "Share with the team" (saves into the repo).
9. **Changes reach new sessions; running ones keep what they started with.** It is said once, in the Use in menu.
10. **No duplicates, no surprise copies.** "Use everywhere" moves a one-agent skill into the Library, with the steps
    spelled out and Undo (X8). The original is removed only after every agent lists the new copy.

## 5. Pitfalls to avoid

- **The UI shows less than the agent loads** (project and plugin MCPs missing). Our catalog must list every source
  the CLIs read.
- **Surfaces disagree on status** (one place says Connected, another Needs authentication). There is one status
  source per agent: the CLI's own list.
- **Scope errors nobody can reason about** (contradictory "installed in project scope" messages). Use one word set
  and show the result per agent.
- **Same-name collisions and per-account duplicates with no label.** Never dedupe by name silently. Show the source.
  X6 fixes accounts at the root.
- **Hidden context cost.** Enabled items silently tripled a fresh thread's input tokens in one rival. A cost line in
  Advanced is a candidate later, not in this pass.
- **Label churn.** Keep "Plugins" as the page name, and Skills / MCP servers as the kinds.
- **A restart requirement that's easy to miss.** Say "new sessions get it" once, where the choice is made.
- **Trust prompts that are unclear or too frequent.** A project-scoped item in an untrusted folder gets one plain line,
  not a dialog.

## 6. Proposed model and open questions

- **Item:** a skill, an MCP server or a plugin bundle. It has one Use in (All projects / Only in some projects / Off)
  and a status per agent.
- **Mechanism (settled, §9.4):** Library install through each CLI's plugin commands.
  - Off = the CLI's native disable.
  - Sign-in = each CLI's `mcp login`, in sequence.
  - Today's Brigadier-only launch switches (deny rules, `skillOverrides`, `-c` overrides) go, because they break
    terminal == app.
- **Project scope per CLI:**
  - Claude uses `--scope local`, or `project` when "Share with the team" is on.
  - CLIs with no native project write (Codex today) need a decision. See the questions in the phase-1 outline.
- **Adapter trait:** phase 2 puts each CLI's skills/MCP behaviour behind one trait (list, install, disable,
  login, project write, capability flags), so a new CLI is one adapter, not a page change.

## 7. Sources

Official pages fetched 2026-10-24:

- code.claude.com/docs/en/discover-plugins and /mcp
- support.claude.com articles 11176164, 10949351 and 12512180
- the Codex docs on MCP and skills
- cursor.com/docs/mcp and /mcp/install-links
- code.visualstudio.com MCP pages
- zed.dev/docs/ai/mcp
- docs.devin.ai/desktop/cascade/mcp
- docs.superset.sh/skills
- onorca.dev/docs/cli/skills

Issues cited:

- openai/codex #13075, #18498, #24361
- anthropics/claude-code #60260, #102131, #75587
- pingdotgg/t3code #11161
- stablyai/orca #22304, #23906
- generalaction/emdash #2039

The raw notes, with a verified/unverified tag per claim, are kept with the run's worker files, outside the repo.
