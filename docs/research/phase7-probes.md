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
the server had no tools. A remote server with no credentials of its own in the config signs in
with OAuth, so the catalog gives it a sign-in state per agent (`signs_in`, `Catalog::sign_in`):
signed in natively for the agent whose config holds it, unknown for the other until Brigadier can
reuse that sign-in (j) or hold its own.

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

## (j) Where each CLI keeps an MCP OAuth sign-in (macOS)

Named (j) because (i) is the rollout probe above. Found from the JavaScript embedded in the
Claude Code 2.1.306 binary, and from the codex-rs source with its key strings checked in the
codex-cli 0.163.0 binary. No Keychain item, credential file or token was read; only whether the
fallback files exist (`~/.claude/.credentials.json`, `~/.codex/.credentials.json` and
`~/.codex/secrets/` don't, so both keep their sign-ins in the Keychain here).

Claude Code:
- One Keychain generic password holds every credential: the claude.ai login, the API key and
  each MCP server. Service `Claude Code-credentials`; with `CLAUDE_CONFIG_DIR` set,
  `Claude Code-credentials-<first 8 hex of sha256(config dir)>`
  (`CLAUDE_SECURESTORAGE_CONFIG_DIR` overrides the hashed folder). Account `$USER`, or
  `claude-code-user` when that isn't `[a-zA-Z0-9._-]+`. Fallback file: `.credentials.json`
  (0600) in the config folder, used only when the Keychain write fails outright.
- Shape (JSON): top-level `claudeAiOauth`, `primaryApiKey`, `mcpOAuth`, `mcpOAuthClientConfig`.
  `mcpOAuth.<key>`, where key is `<name>|<first 16 hex of sha256(JSON {type,url,headers})>`, has
  `serverName`, `serverUrl`, `accessToken`, `refreshToken`, `expiresAt` (ms), `scope`,
  `clientId`, `clientSecret`, `redirectUri`, `issuer`, `stepUpScope`, `discoveryState`.
- The client registration is stored with the token (`clientId`, `clientSecret`, `redirectUri`).
- Refresh: proactively when 300 s or less remain, under a lock file
  `<config dir>/mcp-refresh-<key>.lock` (stale after 60 s), re-reading the store first to adopt
  another process's fresh token; the new token is written back to the same item (keeping the old
  refresh token when the server sends none), under `.storage-write`. `invalid_grant` clears the
  tokens, `invalid_client` the registration too.

Codex:
- `mcp_oauth_credentials_store = auto | file | keyring` (default `auto`: the Keychain, then the
  file when that write fails). Keychain: one generic password per server, service
  `Codex MCP Credentials`, account `<name>|<first 16 hex of sha256(JSON {"type":"http","url",
  "headers":{}})>`. File: `$CODEX_HOME/.credentials.json`.
- Shape: the Keychain value has `server_name`, `url`, `issuer`, `client_id`, `token_response`
  (`access_token`, `token_type`, `expires_in`, `refresh_token`, `scope`) and `expires_at` (ms);
  the file maps the same key to `server_name`, `server_url`, `issuer`, `client_id`,
  `access_token`, `expires_at`, `refresh_token`, `scopes`.
- `client_id` is stored; no `client_secret` (a pre-registered one comes from
  `[mcp_servers.<name>.oauth] client_secret`).
- Refresh: before every MCP request when 30 s or less remain, under
  `$CODEX_HOME/mcp-oauth-locks/<sha256(key)>.lock` (60 s), re-reading the store first and saving
  the new token to the same store before using it; tokens are saved again after each tool call.
  A 401 after that returns "Authentication required" and the call isn't replayed.

The keys compared (`claude_key` and `codex_keys` in `crates/registry/src/sign_ins.rs`, checked
against values computed apart with Node's `JSON.stringify` and Python's `json.dumps`): Claude
hashes `JSON.stringify({type, url, headers: headers || {}})` with the headers as configured;
Codex hashes `{"type":"http","url",...,"headers":{}}` with no headers, in an order that depends
on how the binary was built (serde_json's `preserve_order`), so Brigadier tries both. For a
server without headers the two keys match (`linear|638130d5ab3558f4` for
`https://mcp.linear.app/mcp`); with headers they differ. So one server is matched across configs
by its identity instead: the URL with its scheme and host in lower case, the default port and a
trailing slash dropped (path and query kept), plus its headers other than `Authorization`
(names in lower case, values exact). The same URL for another tenant header is another server.

### What Brigadier does with sign-ins (built)

`crates/core/src/manager/mcp_auth/` and `Catalog::sign_in` in the registry:

- Catalog state per agent (`SignInState`): `native` (the agent's own entry, not known to lack a
  sign-in), `reusableFrom {agent}` (another agent's stored sign-in, read and found usable: an
  unexpired access token or a refresh token), `inBrigadier {sinceMs}`, `needed`, `unknown`
  (not read yet), `notAllowed {agent}` (macOS refused the read) and `unsupported` (sign-ins can't
  be read or kept on this system). Each server also carries `pendingSignIn` (a sign-in in
  Brigadier waiting for the browser) and `copyKey` (a keyed hash of its transport with its values,
  so two copies with other values are two rows; no value can be read from it).
- A worker whose server is served by another store (or by its own config when its own sign-in
  can't) reaches it through Brigadier's proxy on 127.0.0.1: one path per worker and server, which
  takes only that worker's key, checked before the body is read. The proxy adds a fresh access
  token to each request, renews it when it expires within 60 s or once after a 401, and replays
  the request. Neither the key nor a token appears in an answer, a log, an event or a command
  line; Codex's terminal resume names it only through `bearer_token_env_var`. Codex gets the
  proxied entry under the server's own name (the thread's config wins over the user's); Claude,
  which keeps its own entry over a given one of the same name, has its own denied and gets the
  proxied one under a new name.
- A renewal takes the owning CLI's own lock (`mcp-refresh-<key>.lock` or
  `mcp-oauth-locks/<sha256(key)>.lock`), reads its store again (adopting a token the CLI renewed
  meanwhile), refreshes with the client the store or Codex's `[mcp_servers.<name>.oauth]` config
  gives, and writes the new tokens back to that store and backend (Codex's
  `mcp_oauth_credentials_store` is honoured). Claude's item is held as a JSON value and written
  back with only the entry's `accessToken`, `refreshToken` and `expiresAt` changed, then read
  once more. A store that changed under it, a write that doesn't read back, or a refused refresh
  token ends the reuse. Brigadier never creates or deletes an entry in another CLI's store.
- Keychain: a read of Claude's item deserializes only the one `mcpOAuth` entry asked for and
  wipes the bytes at once. Settings' polling never asks (a store that would ask reads as
  `unknown`, and what was read stays known for 30 s); a worker that needs another agent's sign-in
  may show macOS's prompt once. A refusal shows as `notAllowed` and is remembered, so nothing asks
  again, until `askMcpSignInAgain` clears it and reads once more.
- The sign-in in Brigadier (`signInMcpServer`, answering at once with an id; `getMcpSignIn`
  for how it went): discovery from the server's `WWW-Authenticate` resource metadata, dynamic
  client registration, PKCE, a loopback redirect and the default browser, for up to 5 minutes.
  `cancelMcpSignIn` closes the listener and saves nothing, `reopenMcpSignIn` opens the page again
  (its link never leaves the daemon), and `signOutMcpServer` deletes only Brigadier's own
  credential. The result is kept in Brigadier's own Keychain item (service `Brigadier MCP
  sign-ins`), keyed by the server's identity, and serves workers of every agent. Failures are
  plain: "The server didn't accept the sign-in.", "Timed out after 5 minutes. Nothing was saved."
- Off macOS: Claude's `.credentials.json` and Codex's file store are read; Codex's keyring and
  Brigadier's own sign-in aren't, and a server nothing else serves shows `unsupported`.
- Tests use an in-memory Keychain and a fake OAuth server on localhost; none reads the user's
  Keychain or credential files.

## (k) Plugins' skills and servers (claude 2.1.306, codex-cli 0.163.0)

Probed in scratch `CLAUDE_CONFIG_DIR`s and `CODEX_HOME`s, reading Claude's `init` event of
`-p --output-format stream-json` (its `skills`, `plugins`, `mcp_servers` and `tools`) and Codex's
app-server `skills/list` and `mcpServerStatus/list`.

- Claude Code: `installed_plugins.json` records a `project` or `local` install with a
  `projectPath`, its `installPath` in the user's cache. A plugin loads wherever the merged
  `enabledPlugins` (user, then the folder's `.claude/settings.json`, then
  `settings.local.json`) switch it on, the install's scope aside; one none names loads unless
  its manifest says `defaultEnabled: false`. It reads `skills/` and the manifest's `skills`
  entries one level deep (an entry holding `SKILL.md` is one skill), and the plugin's root
  `SKILL.md` only when it has neither. Skills show as `<plugin>:<name>`, servers as
  `plugin:<plugin>:<name>` with tools `mcp__plugin_<plugin>_<name>__…`; `${CLAUDE_PLUGIN_ROOT}`
  is the plugin's folder. A plugin folder in a skills folder loads as `<name>@skills-dir`.
- Switching one off in Claude: `skillOverrides` doesn't reach a plugin's skills (they apply only
  under conditions user settings can't meet) and a `Skill(<plugin>:<name>)` deny rule leaves the
  skill listed, but it keeps Claude from running it; `permissions.deny` `mcp__plugin_<p>_<s>`
  takes the server's tools away.
- Codex: a cached plugin loads only with a `[plugins."<plugin>@<marketplace>"]` table whose
  `enabled` isn't false (bb counts an unlisted one as on; Codex doesn't). The version folder is
  `local`, else the highest by semver, compared as text when either isn't semver
  (`core-plugin-common/src/installed.rs`); without a manifest there, nothing loads. The manifest's
  `skills` (each `./…`) replace `skills/`, each read to any depth. Skills show as
  `<plugin>:<name>` (the manifest's name); servers keep their own names and start in the
  plugin's folder for `cwd: "."`. `skills.config` by path switches a plugin's skill off;
  `-c plugins.<id>.mcp_servers.<name>.enabled=false` its server (an `mcp_servers.<name>` entry
  instead makes a server without a command, which fails the config).

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
  Codex by `mcp_servers.<name>.enabled=false` (c) and `skills.config` (d). A plugin's (k): Claude
  by deny rules `mcp__plugin_<plugin>_<name>` and `Skill(<plugin>:<name>)`, Codex by
  `plugins.<id>.mcp_servers.<name>.enabled=false` and `skills.config`.
