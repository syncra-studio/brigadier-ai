# Phase 7 probes: sharing MCP servers and skills (2026-10-21)

Run against claude 2.1.306 and codex-cli 0.163.0 on macOS, in scratch homes under the task's
test folder: a Claude `HOME` holding only a `.claude.json` with one stdio server and one skill in
`.claude/skills` (its login read through a link to the user's keychain folder, never written),
and a `CODEX_HOME` holding a `config.toml` with one stdio server, one skill in `skills/` and a
link to the user's `auth.json`. The user's own configs were only read. The test servers were a
stdio server whose one tool returns `PROBE_WORD` from its environment and its working folder,
and a streamable-HTTP server on 127.0.0.1 whose tool returns the `Authorization` and `X-Probe`
headers it was sent. Codex ran outside the sandbox (its API isn't reachable through it).

| Probe | Result |
| --- | --- |
| (a) Claude `--settings` deny `mcp__<name>` hides a user-scope server | Holds |
| (b) Claude: excluding a native skill per session | Holds, by name: `skillOverrides` |
| (c) Codex `mcp_servers.<name>.enabled=false` drops a config.toml server | Holds |
| (d) Codex: disabling a skill by path | Holds: `skills.config=[{path, enabled=false}]` |
| (e) Codex streamable HTTP wholly by `-c` | Holds (`url`, `bearer_token_env_var`, `env_http_headers`) |
| (f) A Claude-signed-in OAuth remote in Codex without `codex mcp login` | Doesn't work (as expected) |
| (g) Claude `--mcp-config`: stdio `cwd`, `${VAR}` in env and HTTP headers | `cwd` ignored; `${VAR}` works in both |
| (h) Codex `env_vars` renaming a variable | No: a source is only `local` or `remote` |
| (i) Codex writes a thread's config (env values) to its rollouts or databases | No |

## (a) Claude: denying a user-scope server

`claude -p … --setting-sources user --settings '{"permissions":{"deny":["mcp__echo"]}}'`: the
model listed no `mcp__` tools; without the rule it listed `mcp__echo__secret_word`. A switched-off
server Claude has natively (user, local or project scope) is kept from a thread or worker by a
deny rule `mcp__<name>` in Brigadier's `--settings`, which win over the user's.

## (b) Claude: excluding a native skill

Claude 2.1.306 has a `skillOverrides` setting: "Per-skill listing overrides keyed by skill name.
"name-only" lists the skill without its description; "user-invocable-only" hides it from the
model but keeps /name; "off" hides it from both." Flag settings (`--settings`) are read before the
user's. `--settings '{"skillOverrides":{"probe-skill":"off"}}'` took `probe-skill` out of the
model's skill list; every other skill stayed.

It is keyed by name, not path: switching off one of two Claude-native skills with the same name
hides both for Claude. The catalog says so (`switch_scope: name` for Claude-native skills).

## (c) Codex: disabling a configured server

`codex exec -c mcp_servers.echo.enabled=false …`: the model had no `echo` tool ("NO TOOL");
without it the call returned `word=codexcar`. Brigadier already uses the same key in a thread's
config for sealed sessions; per thread it works the same.

## (d) Codex: disabling a skill by path

`-c 'skills.config=[{path="<CODEX_HOME>/skills/codex-probe-skill/SKILL.md", enabled=false}]'`
took the skill out of the instructions' skill list; without it the skill was listed. The key
replaces the user's own `skills.config` array, so Brigadier passes the user's entries along with
its own.

## (e) Codex: an HTTP server given wholly by `-c`

`-c 'mcp_servers.h1={url="http://127.0.0.1:47811/mcp", bearer_token_env_var="TOK_ALIAS",
env_http_headers={"X-Probe"="HDR_ALIAS"}}'` with `TOK_ALIAS=tok123 HDR_ALIAS=hdr456` in the
process environment: the tool returned `auth=Bearer tok123 probe=hdr456`. The header values come
from variables Brigadier names, so an alias needs nothing more.

## (f) An OAuth remote signed in for Claude, in Codex

The user's Claude project config has `atlassian` (`https://mcp.atlassian.com/v2/mcp`, no
headers: Claude signs in with OAuth). Given to Codex by `-c` with its URL only, Codex logged
`AuthRequired(AuthRequiredError { www_authenticate_header: "Bearer resource_metadata=…" })` and
the server had no tools. A remote server with no credentials of its own in the config is marked
`needs_sign_in` in the catalog: the other agent needs its own sign-in (`codex mcp login <name>`,
or `/mcp` in Claude).

## (g) Claude: `cwd`, and `${VAR}` in env and headers

- A stdio server in `--mcp-config` with `"cwd": "<another folder>"` still ran in the session's
  folder (`cwd=…/probe/proj`): Claude ignores `cwd`. A server that needs its own folder is
  started through a small wrapper that changes folder first (`/bin/sh -c 'cd "$0" && exec
  "$@"'` on Unix; on Windows its command and arguments are made absolute instead).
- `"env": {"PROBE_WORD": "${BRIG_ALIAS_1}"}` with `BRIG_ALIAS_1=banana` in Claude's environment:
  the server saw `PROBE_WORD=banana`. So each injected value goes into Claude's environment under
  an alias of its own, and two servers can give the same name different values.
- An HTTP server with `"headers": {"Authorization": "Bearer ${TOK_ALIAS}", "X-Probe":
  "${HDR_ALIAS}"}`: the server saw both values.

## (h) Codex: renaming a variable through `env_vars`

`env_vars=[{name="PROBE_WORD", source="ALIAS_X"}]` is refused: "unsupported env_vars source
`ALIAS_X`; expected `local` or `remote`". `env_vars` passes a variable only under its own name.

## (i) Does Codex keep a thread's config on disk?

A thread started over `codex app-server` with `config.mcp_servers.e4.env = {PROBE_WORD =
"word2", OTHER = "SECRETNOECHO555"}`, one turn calling the server's tool: `grep -rla
SECRETNOECHO555` over the whole scratch `CODEX_HOME` (rollouts under `sessions/`, every
`*.sqlite`, `-wal` and `-shm`, logs) found nothing. The value the tool itself returned is in the
rollout, as any tool output is.

## What Brigadier does with them

- Codex app-server sessions keep a server's env and header values in the thread's config, sent
  over the app-server's stdin, which (i) shows stays off disk and off any command line. Each
  server has its own `env` there, so names never collide.
- `codex resume` in a terminal takes the config as `-c` arguments, which any process can read: no
  value goes there. Every server's env becomes `env_vars` plus a variable in the terminal's
  environment; since (h) rules out renaming, a stdio server whose names would collide, or whose
  value isn't the terminal's own, starts through `/bin/sh -c` that sets each name from its alias.
  HTTP headers use `env_http_headers` and `bearer_token_env_var` with aliases (e). On Windows,
  where there is no such shell, a server that needs a wrapper is left out of the terminal resume.
- Claude gets every value in its environment under a per-server alias, named as `${ALIAS}` in its
  `--mcp-config` (g).
- Switching off: Claude by `permissions.deny` `mcp__<name>` (a) and `skillOverrides` `off` (b);
  Codex by `mcp_servers.<name>.enabled=false` (c) and `skills.config` (d).
