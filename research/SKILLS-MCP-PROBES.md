# X1: installed CLI plugin probes

2026-10-22, worker w67. Research only for `docs/STRIP-DOWN-PLAN.md` §9.4 X1.
Plan/code baseline: `de8e53aa4687c144630a48dd004960701e168de0`. The supporting
`EXCEL-skills-mcp.md` was read from the Delegator run's `research/` directory;
it was not present in the repository at this baseline.

## Result for X2–X9

The shared local marketplace works. The two MCP formats have materially different
semantics, so the packer must choose deliberately:

| Question | Observed on the installed CLIs | Consequence |
| --- | --- | --- |
| `.mcp.json` variables in Codex | Ordinary `${VAR}`, defaults, `${CLAUDE_PLUGIN_ROOT}` and `${PLUGIN_ROOT}` stayed literal, including in the child process. | X2/X3 must translate configuration; X5's secret wrapper cannot depend on interpolation. |
| Codex command paths | Compatibility `.mcp.json`: absolute commands work; relative paths use the project working directory. Portable `mcp.json`: bare PATH names and contained `./` paths work; plugin root is the default cwd. | Do not apply the portable command rule to the compatibility loader. Use a bare installed wrapper for the common format, or generate format-specific configs. |
| Names | Both CLIs exposed plugin skills as `alpha:same`, `beta:same`, plus user `same`. Codex collapsed same-name plugin MCPs; Claude kept namespaced MCPs with different endpoints. | X2/X8 need explicit collision checks and stable unique server names. Do not promise that every item survives a clash. |
| OAuth | A plugin HTTP server is discovered and marked as needing authentication; both CLIs can begin its own login flow. Claude requires the qualified server name and an interactive terminal. | X5 must use CLI-reported IDs and support a PTY for Claude's headless login flow. Installation alone grants no tools. |
| Project scope | Claude `--scope project` works. Codex project config can enable a plugin, but `plugin add --scope project` is rejected and native config-write RPC refuses project files. | Keep project “Use in both” out of X2/X3 until a native write path is settled; direct project TOML edits would depart from this plan. |
| Extra Claude account | Main-home user/local MCPs and enabled user plugin disappeared in the extra-home layout. | X6 is necessary; sharing only the plugin directory is insufficient. |
| MCP added inside Codex | The add writes the selected thread home; refreshing that home from the base config loses it. Same-home terminal adds persist. | X6 must fix config ownership/writeback, rather than retrying `mcp add`. |
| Next session, no app restart | New plugin MCP and skill appeared in a new Claude process and a new thread on the **same Codex app-server process**. | The next-session bar is feasible. No promise about hot reconfiguration of a running turn. |

These are version-specific observations, not guarantees for later releases.

## Versions, boundaries and sources

Exact version commands and output:

```text
$ claude --version
2.1.308 (Claude Code)
$ codex --version
codex-cli 0.163.0
```

Executables were `/Users/stephen/.local/bin/claude` and
`/Users/stephen/.local/bin/codex`. The following help was inspected: `claude --help`,
`codex --help`, `claude plugin --help`, `claude plugin install --help`,
`claude plugin marketplace add --help`, `claude mcp --help`,
`claude mcp login --help`, `codex plugin --help`, `codex plugin add --help`,
`codex plugin marketplace add --help`, `codex mcp --help`,
`codex mcp login --help`, `codex debug --help`, `codex debug prompt-input --help`,
`codex app-server --help`, and `codex exec --help`.

Current primary documentation was fetched on the probe date:

- [Claude plugin manifest reference](https://code.claude.com/docs/en/plugins-reference): component paths and plugin root substitution.
- [Claude MCP reference](https://code.claude.com/docs/en/mcp): scope, authentication, and endpoint-based plugin deduplication.
- [Codex package reference](https://developers.openai.com/plugins/build/plugins): portable root manifests versus compatibility manifests.
- [Codex command reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli) and [plugin guide](https://learn.chatgpt.com/docs/plugins?surface=cli).
- [Portable manifest schema](https://agent-plugins.org/schemas/1.0.0/plugin.schema.json) and [MCP schema](https://agent-plugins.org/schemas/1.0.0/mcp.schema.json).

All writes and CLI sessions used throwaway homes/projects under
`/tmp/take-over-5-sessions-w67-probes`. macOS also reports this location as
`/private/tmp/take-over-5-sessions-w67-probes`. The documentation worktree was
`/tmp/take-over-5-sessions-w67-worktree`. No real account, credential, service
consent, or user project was used. No model-generated work was requested from a
remote service: Claude's print sessions used a loopback HTTP response fixture;
Codex used startup/discovery RPC without starting a model turn. The Claude fixture
returned `OK`; this verifies CLI initialization and discovery, not model behavior.

## Fixture and command convention

`B` below denotes `/tmp/take-over-5-sessions-w67-probes`; `C` and `X` denote the
absolute executable paths above. Each subprocess had a fresh explicit environment:

```text
PATH=$B/bin:<the launching shell's PATH>
HOME=$B/os-home
CLAUDE_CONFIG_DIR=$B/claude
CODEX_HOME=$B/codex
TERM=dumb
NO_COLOR=1
W67_SENTINEL=expanded-value
CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
DISABLE_AUTOUPDATER=1
```

No inherited authentication variables were passed. The `env` changes and exact
CLI argv are preserved in the command ledger below. `project-a` was the default
cwd; `project-b` was the independent control. Both were initialized with
`/usr/bin/git init -q <absolute-project-path>` before project-scope checks.

The marketplace at `$B/market/.claude-plugin/marketplace.json` had this shape;
entries were added for each later probe:

```json
{
  "name": "w67-local",
  "owner": {"name": "w67"},
  "plugins": [{"name": "alpha", "source": "./alpha", "description": "w67 probe"}]
}
```

Each compatibility plugin had `.claude-plugin/plugin.json`:

```json
{
  "name": "alpha", "version": "0.1.0", "description": "w67 probe",
  "skills": "./skills/", "mcpServers": "./.mcp.json"
}
```

`skills/same/SKILL.md` contained frontmatter `name: same`, a distinct description
such as `alpha skill marker`, and one instruction to return that plugin's marker.
The native-manifest repeat added `.codex-plugin/plugin.json` with the same fields.
The MCP fixture `server.py` logged its argv, cwd and selected synthetic env vars
to `$B/launches.jsonl`, then implemented newline-delimited JSON-RPC:

```python
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    method = req.get('method')
    if method == 'initialize':
        result = {
            'protocolVersion': req['params']['protocolVersion'],
            'capabilities': {'tools': {}},
            'serverInfo': {'name': 'w67-fixture', 'version': '0.1.0'}
        }
    elif method == 'tools/list':
        result = {'tools': [{
            'name': 'probe', 'description': 'w67 fixture ' + ' '.join(sys.argv[1:]),
            'inputSchema': {'type': 'object', 'properties': {}}
        }]}
    elif method == 'tools/call':
        result = {'content': [{'type': 'text', 'text': 'w67 fixture called'}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
```

`$B/bin/w67-probe` was executable and ran `/usr/bin/python3
$B/market/alpha/server.py "$@"`. Bundled `launch` scripts ran their own adjacent
`server.py` via `/usr/bin/python3`. No packages were downloaded.

Session-start protocol was taken from the installed binary, using:

```sh
codex app-server generate-json-schema --out /tmp/take-over-5-sessions-w67-probes/schema
codex app-server --stdio
```

The RPC harness sent these exact parameter shapes, retaining each returned thread
ID for the status request:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"w67-probes","version":"0.1.0"},"capabilities":{"experimentalApi":true}}}
{"jsonrpc":"2.0","method":"initialized"}
{"jsonrpc":"2.0","id":2,"method":"thread/start","params":{"cwd":"/tmp/take-over-5-sessions-w67-probes/project-a","ephemeral":true,"approvalPolicy":"never","sandbox":"read-only","model":"gpt-6.1-sol","config":{"model_reasoning_effort":"low"}}}
```

Then `mcpServerStatus/list` with `{"threadId": <returned ID>}` and `skills/list`
with `{"cwds":[<project path>]}`. `project-b` replaced the cwd for controls.
Processes were terminated by their own saved PID in `finally` blocks.

Claude session command, with `ANTHROPIC_BASE_URL=http://127.0.0.1:<fixture-port>`
and the synthetic local-only `ANTHROPIC_API_KEY=w67-local-only`:

```sh
claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
```

The `system/init` event's `mcp_servers`, `skills`, and `plugins` fields are the
evidence. An exploratory `--bare --tools ''` run suppressed custom tools/skills;
it was discarded and repeated normally. It is not evidence of missing plugins.

## P1. Compatibility `.mcp.json` variables

Installed `alpha@w67-local` in both CLIs using their native marketplace and install
commands (ledger entries 1–7). At version 0.1.0 the server used `command: python3`
and `args: ["${CLAUDE_PLUGIN_ROOT}/server.py", "${W67_SENTINEL}",
"${W67_MISSING:-fallback}"]`. Codex listed it literally and its startup handshake
failed; the fixture never launched.

At version 0.1.1 the server used the known executable `w67-probe` and this config:

```json
{
  "mcpServers": {
    "env": {
      "command": "w67-probe",
      "args": ["${W67_SENTINEL}", "${CLAUDE_PLUGIN_ROOT}", "${PLUGIN_ROOT}", "${W67_MISSING:-fallback}"],
      "env": {
        "W67_VALUE": "${W67_SENTINEL}", "W67_DEFAULT": "${W67_MISSING:-fallback}",
        "W67_ROOT": "${CLAUDE_PLUGIN_ROOT}", "W67_PLUGIN_ROOT": "${PLUGIN_ROOT}"
      }
    }
  }
}
```

After `claude plugin marketplace update w67-local`, `claude plugin update
alpha@w67-local`, and `codex plugin add alpha@w67-local`, a real Codex thread
connected and discovered `probe`. Its child argv still contained all four literal
strings. Child env contained literal `W67_VALUE=${W67_SENTINEL}` and
`W67_DEFAULT=${W67_MISSING:-fallback}`; the two root env values also stayed literal.
The parent's sentinel was absent in the child unless explicitly configured.

Claude `mcp list` reported:

```text
plugin:alpha:env: w67-probe expanded-value /tmp/take-over-5-sessions-w67-probes/market/alpha ${PLUGIN_ROOT} fallback - ✔ Connected
```

Its child log confirmed expanded ordinary/default/Claude-root values and a literal
`${PLUGIN_ROOT}`. **Conclusion:** Codex's listing was not merely hiding runtime
interpolation; no such interpolation occurred through this compatibility loader.
X2/X3 must not copy Claude env templates unmodified. Portable root handling is P2.

## P2. Codex command-path rule and PATH

`alpha` 0.2.0 used the compatibility manifest and `.mcp.json`. Reinstalled as
0.2.1 with an additional native `.codex-plugin/plugin.json`; the outcomes matched.
Each row was a distinct MCP server and was inspected at real thread startup:

| Command in config | Observed result |
| --- | --- |
| `w67-probe` | Connected through the fixture's PATH entry. |
| `/usr/bin/python3` with an absolute fixture script argument | Connected; child cwd was the project. |
| `./launch`, present only in plugin | `MCP startup failed: No such file or directory (os error 2)` |
| `scripts/launch`, present only in plugin | Same failure. |
| `../alpha/launch` | Same failure in this fixture. |
| `${CLAUDE_PLUGIN_ROOT}/launch` | Same failure; literal executable token. |
| `w67-not-on-path` | Same failure. |

Created `$B/project-a/launch` and `$B/project-a/scripts/launch`, each reporting
`project-relative` in the tool description. Without reinstalling, the next thread
connected through both relative commands and discovered that marker. This proves
the compatibility paths were project-relative, not plugin-relative.

The binary-string clue was checked with:

```sh
strings /Users/stephen/.local/bin/codex | rg -o '.{0,140}command must.{0,200}' | head -15
```

It specifically said `Agent Plugins stdio command must be a bare executable name
or a contained ` plus the `./` path requirement. That rule belongs to the portable
format. A second marketplace fixture tested **root** `plugin.json` declaring
`https://agent-plugins.org/schemas/1.0.0/plugin.schema.json` and root `mcp.json`
declaring the corresponding MCP schema. Each server included `"type":"stdio"`.
There was one plugin per command to isolate config rejection:

| Portable plugin | `command` | Listing / real startup |
| --- | --- | --- |
| `port-bare` | `w67-probe` | Listed and connected; default cwd is installed plugin root. |
| `port-rel` | `./launch` | Rewritten to absolute path inside plugin cache; connected. |
| `port-abs` | `/usr/bin/python3` | Install exited 0; server absent from `mcp list` and runtime inventory. |
| `port-parent` | `../alpha/launch` | Install exited 0; server absent. |
| `port-var` | `${PLUGIN_ROOT}/launch` | Install exited 0; server absent. |
| `port-noprefix` | `scripts/launch` | Install exited 0; server absent. |

Portable args/env expanded `${PLUGIN_ROOT}` to the installed cache root and
exported `PLUGIN_ROOT` and `PLUGIN_DATA`; `${CLAUDE_PLUGIN_ROOT}`, `${W67_SENTINEL}`,
and `${W67_MISSING:-fallback}` remained literal. Portable `./launch` was resolved
from the plugin, even though no launcher existed in `project-b`.

**Conclusion for X2/X5/X9:** format matters. A bare wrapper on PATH works for the
tested formats. A bundled launcher needs `${CLAUDE_PLUGIN_ROOT}` in Claude,
an explicitly resolved path/cwd for Codex compatibility, or portable `./launch`.
An install exit code of 0 does not prove its MCP survived config validation;
verify inventory and `tools/list`. No dependence on undocumented search order.

## P3. Skill and server name clashes

Installed `alpha` then `beta`, both 0.3.0, each with `skills/same/SKILL.md` and
an MCP named `shared`; commands were `w67-probe alpha` and `w67-probe beta`.

- Claude listed `plugin:alpha:shared` and `plugin:beta:shared`, both connected.
- Codex listed only `shared`, with `args: ["alpha"]`; startup status had
  `pluginId: "alpha@w67-local"` and the alpha tool marker. Beta's MCP was hidden.
- Codex `skills/list` exposed **`alpha:same` and `beta:same`**. This corrects the
  earlier research's bare-name assumption. Prompt-input also exposed the plugin
  skills. Claude's actual session init likewise contained both qualified names.

Added user `shared` through both CLIs, with different args `user`. Added user skill
`same` under each CLI's native skill root **inside the throwaway home**.

- Codex runtime selected the user MCP: `pluginId: null`, marker `user`.
- Claude kept both plugin endpoints and user `shared`: name equality alone did
  not hide the plugin servers.
- Both CLIs exposed user `same` alongside `alpha:same` and `beta:same`.

Added Claude user `same-endpoint` with exactly `w67-probe alpha`. Claude then
removed `plugin:alpha:shared` from listing and session init, retaining
`plugin:beta:shared`, user `shared`, and `same-endpoint`. This is endpoint
deduplication; fields were not merged.

A further Codex user skill with frontmatter `name: alpha:same` and description
`user qualified skill marker` appeared in `skills/list` **alongside** the plugin's
own `alpha:same` entry. Both entries were enabled. No invocation-winner claim is
made from this inventory result.

**Conclusion for X2/X8:** user MCP wins in the Codex case tested; two plugin MCPs
do not coexist under one bare server name. The alpha/beta winner is an observation,
not a stable ordering contract. Namespaced plugin skills ordinarily coexist,
but an explicitly qualified user skill can still clash. Claude's MCP rule is
endpoint-based across plugin/user sources, so “user entry wins by name” is too broad.

## P4. OAuth plugin server, first use and login

`oauth@w67-local` bundled HTTP server `oauth-probe`, pointing only at a loopback
fixture. The fixture returned HTTP 401 with
`WWW-Authenticate: Bearer resource_metadata="<base>/.well-known/oauth-protected-resource"`.
That metadata advertised `<base>/mcp`, authorization server `<base>`, and scope
`probe`. Authorization metadata advertised `/authorize`, `/token`, `/register`,
authorization-code flow and S256 PKCE. `/register` returned synthetic client ID
`w67-local-client`; no `/authorize` consent or token exchange was completed.

Observed before login:

```text
Codex mcp list --json: "auth_status": "not_logged_in"
Codex thread startup: "runtimeStatus": "authenticationRequired", "authStatus": "notLoggedIn", "tools": {}
Claude mcp list: plugin:oauth:oauth-probe: http://127.0.0.1:53127/mcp (HTTP) - ! Needs authentication
Claude system/init: {"name":"plugin:oauth:oauth-probe","status":"needs-auth","source":"plugin"}
```

The exact login commands were:

```sh
codex mcp login oauth-probe --no-browser
claude mcp login oauth-probe --no-browser
claude mcp login plugin:oauth:oauth-probe --no-browser
```

Codex printed `Authorize the MCP server by opening this URL in your browser:`
and a loopback `/authorize` URL containing S256 challenge, callback URI, state,
scope and resource. It waited for a callback; the saved process PID was terminated.

Claude's bare name exited 1:

```text
No MCP server named "oauth-probe". Configured servers: plugin:oauth:oauth-probe
```

Qualified login with piped stdin exited 1 and said authentication cannot complete
because stdin is not a terminal. Repeated **the same command** in a Python
`pty.openpty()` slave. It printed:

```text
Starting authentication for "plugin:oauth:oauth-probe"…
Visit this URL to authorize:
Waiting for authorization… (^C to cancel)
Or paste the redirect URL here:
```

Its URL likewise contained synthetic client ID, challenge, callback, state,
scope and resource. No browser was opened. That PID was terminated before consent.
Fixture requests confirmed discovery/registration; no token request occurred.

**Conclusion for X5/X7:** plugin OAuth discovery and login initiation work without
copying the server into user MCP config. Use the CLI-reported server identity;
do not blindly reuse the bare Codex name for Claude. Claude `--no-browser` still
needs a terminal. Completed consent, token storage, refresh and post-login tool
use remain for X5/X9; a real service's consent needs the user. This probe did not
sign in to a new service or claim end-to-end authenticated success.

## P5. A plugin for one project

Used independent `CLAUDE_CONFIG_DIR=$B/scope-claude` and
`CODEX_HOME=$B/scope-codex`, with `project-a` and `project-b` controls.

Claude commands from project A:

```sh
claude plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
claude plugin install gamma@w67-local --scope project
```

Install exited 0, with `scope: project` in its message. The CLI created project
`.claude/settings.json` enabling `gamma@w67-local`, and recorded a `projectPath`
in the throwaway home's plugin installation registry. Actual session init in A
contained connected `plugin:gamma:fresh` and `gamma:new-item`; B had neither.

Codex:

```sh
codex plugin add gamma@w67-local --scope project
```

Exited 2: `error: unexpected argument '--scope' found`. A normal `plugin add`
installed it globally. As a **fixture-only manual configuration experiment**,
set its user-home config to `enabled = false`, trusted both project paths in that
home, and wrote A's `.codex/config.toml`:

```toml
[plugins."gamma@w67-local"]
enabled = true
```

A's `codex mcp list --json` and real thread contained connected `fresh` plus the
gamma skills; B's listing and real thread inventory were empty. This proves the
reader supports project gating, not that the install command writes that scope.

Tested the installed app-server's native writer twice:

```json
{"method":"config/value/write","params":{"filePath":"/tmp/take-over-5-sessions-w67-probes/project-a/.codex/config.toml","keyPath":"plugins.\"gamma@w67-local\".enabled","value":false,"mergeStrategy":"upsert"}}
```

The repeat used `value: true`. Both returned error code `-32600`,
`config_write_error_code: "configLayerReadonly"`, and
`Only writes to the user config are allowed`.

**Conclusion for X2/X3/X8:** project gating is possible in both readers, but the
tested Codex native writers do not provide the equivalent project install.
Keep project moves out of scope under the plan's native-only write rule.

## P6. The two holes, live in throwaway home layouts

These are live CLI reproductions of the app's home preparation semantics, not a
launch of the GUI or a check of the human's actual signed-in accounts. The exact
source at the baseline was inspected: `crates/providers/src/accounts.rs`
(`shared`, `prepare_home`) and `crates/providers/src/codex/home.rs`
(`ThreadHome::prepare`, `with_kept_trust`). No app code was changed or built.

### P6a. Extra Claude account loses main-home configuration

In `$B/hole-claude-main`, used native commands to add user `hole-user`, local
`hole-local` for project B, and install user plugin `alpha@w67-local`.
Constructed `$B/hole-claude-extra` exactly as the relevant home-sharing algorithm:
its `.claude.json` started as `{"hasCompletedOnboarding":true}`; the listed
shared paths (`projects`, `CLAUDE.md`, `agents`, `commands`, `skills`, `plugins`)
were symlinked where present. Neither `.claude.json` nor `settings.json` was shared.

`claude mcp list`, run with the main config directory, connected
`plugin:alpha:shared`, `hole-user`, and `hole-local`. The same command with the
extra config directory said:

```text
No MCP servers configured. Use `claude mcp add` to add a server.
```

Actual print-session initialization, using the same local response fixture:

```text
main mcp_servers: plugin:alpha:shared connected, hole-user connected, hole-local connected
extra mcp_servers: []
main skills: includes alpha:same
extra skills: does not include alpha:same
```

**Conclusion for X6:** sharing a plugins directory does not share enablement or
user/local server definitions. This fixture proves the missing-config behavior
without touching real account homes. It does not independently test every setting
such as `skillOverrides` or real multi-account switching.

### P6b. An MCP added inside a Codex thread does not survive config refresh

Created `$B/hole-codex-base/config.toml` with `[features] hooks = true`; copied it
to `$B/hole-codex-thread/config.toml`, reproducing the fresh-copy part of
`ThreadHome::prepare`. Ran with the thread `CODEX_HOME`:

```sh
codex mcp add hole-session -- w67-probe hole-session
codex mcp list --json
```

Output: `Added global MCP server 'hole-session'.`; listing showed that entry.
The base home's listing remained `[]`. Repeated the launch-time copy of the base
config into the thread home, then ran:

```sh
codex mcp list --json
codex mcp get hole-session
```

Output: `[]`, then exit 1 with `Error: No MCP server named 'hole-session' found.`
The source's retained hook-trust merge carries hook trust, not added MCP tables.
Control: `codex mcp add hole-terminal -- w67-probe hole-terminal` with the base
home persisted across two independent `codex mcp list --json` processes.

**Conclusion for X6:** this is the app's config-copy lifecycle, not a failure of
ordinary terminal `codex mcp add`. The reproduction explicitly performs that copy;
it does not claim the CLI itself deletes entries at startup. X6 needs a live app
regression after fixing ownership/writeback.

## P7. Added item appears next session without app restart

Held **one** `codex app-server --stdio` process open. Started an old thread and
confirmed its `shared` MCP inventory. Started a normal Claude print session:
no `gamma` plugin/skill/server appeared.

Created new plugin `gamma` 0.1.0 with MCP `fresh` (`w67-probe gamma`) and skill
`new-item` (`gamma new-item marker`), updated the marketplace, then ran:

```sh
claude plugin marketplace update w67-local
claude plugin install gamma@w67-local
codex plugin add gamma@w67-local
```

Without restarting the held Codex process, sent another `thread/start` and its
`mcpServerStatus/list`. The new thread exposed `fresh` with
`pluginId: "gamma@w67-local"`, `runtimeStatus: "connected"`, and the gamma tool
marker. `skills/list` exposed enabled `gamma:new-item`. A fresh independent
app-server process also found both. The next Claude print session's actual init
contained connected `plugin:gamma:fresh` and skill `gamma:new-item`.

Updating existing alpha versions also produced Claude's literal
`Restart to apply changes.` message; each subsequent process loaded the installed
version. No desktop app was restarted during any probe.

**Caveat:** querying inventory for the old Codex thread after installation also
listed `fresh`, with `runtimeStatus: null`; discovery can enumerate a newly
configured server without proving that old thread has an active connection.
This research proves next-session discovery and does not assert that already
running turns either hot-load or freeze every item.

**Conclusion for X2/X7/X9:** native installation can satisfy the next-session bar,
including the important persistent Codex app-server case. Keep X6's app-home
refresh problem separate from that native CLI behavior.

## Cleanup and remaining checks

All fixture CLI processes were waited for; persistent app-server and pending
login processes were terminated only by their saved PIDs. Loopback fixture
servers were shut down and sockets closed. No generic process-name kill was used.

The throwaway root was removed after extracting this document and the worker
report. This removed homes `os-home`, `claude`, `codex`, `scope-claude`,
`scope-codex`, `oauth-claude`, `oauth-codex`, `portable-codex`, `hole-claude-main`,
`hole-claude-extra`, `hole-codex-base`, and `hole-codex-thread`; marketplace source,
plugin caches/data, project trust entries, both projects' `.claude`/`.codex` config,
logs, generated schemas, Python fixtures, and scratch output went with it.
The short-lived documentation worktree and branch were removed after landing.

The post-probe audit found **zero** `take-over-5-sessions-w67` markers in the real
`~/.codex/config.toml`, `~/.claude.json`, or `~/.claude/.claude.json`, and zero
matching directories under real `~/.claude/projects`. No real-home trust or plugin
entries needed removal; no existing vendor marketplace was changed. The schema
generation command was read-only with respect to configuration. No backups of
user files were made.

Still needed later: real OAuth consent and token refresh (X5/X9), actual GUI
account switching and config-writeback regression checks after X6, a native
Codex project-write decision, and future-version collision/format regression
coverage. These do not block this bounded research step. No app test suite or
build was run for this documentation-only change.

## Exact CLI command ledger

The following is the recorded probe command ledger, in execution order.
`cwd=` is the subprocess working directory; `env-delta=` is a JSON map overriding
the explicit base environment above. It records argv using shell quoting; it is
not a script to run blindly after cleanup. `PTY` means the command's stdin/stdout
were attached to `pty.openpty()` rather than pipes. The RPC commands and fixture
edits used between these calls are detailed per probe above. Volatile ports are
the ports actually assigned for those local runs.

```text
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin install alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin add alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex debug prompt-input probe
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin details alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin marketplace update w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin update alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin add alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude mcp list
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin marketplace update w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin update alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin add alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin add alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin marketplace update w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin update alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin install beta@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin add alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin add beta@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude mcp list
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex debug prompt-input probe
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex mcp add shared -- w67-probe user
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude mcp add --scope user shared -- w67-probe user
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude mcp list
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude mcp add --scope user same-endpoint -- w67-probe alpha
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude mcp list
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:52880", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"} /Users/stephen/.local/bin/claude --bare -p --model opus --effort low --output-format stream-json --verbose --tools '' --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:52921", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:52974", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin marketplace update w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/claude plugin install gamma@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /Users/stephen/.local/bin/codex plugin add gamma@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:52974", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /usr/bin/git init -q /tmp/take-over-5-sessions-w67-probes/project-a
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={} /usr/bin/git init -q /tmp/take-over-5-sessions-w67-probes/project-b
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/scope-claude"} /Users/stephen/.local/bin/claude plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/scope-claude"} /Users/stephen/.local/bin/claude plugin install gamma@w67-local --scope project
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:53028", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/scope-claude"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:53028", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/scope-claude"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/scope-codex"} /Users/stephen/.local/bin/codex plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/scope-codex"} /Users/stephen/.local/bin/codex plugin add gamma@w67-local --scope project
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/scope-codex"} /Users/stephen/.local/bin/codex plugin add gamma@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/scope-codex"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/scope-codex"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude plugin install oauth@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/oauth-codex"} /Users/stephen/.local/bin/codex plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/oauth-codex"} /Users/stephen/.local/bin/codex plugin add oauth@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/oauth-codex"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-a env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude mcp list
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:53127", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/oauth-codex"} /Users/stephen/.local/bin/codex mcp login oauth-probe --no-browser
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude mcp login oauth-probe --no-browser
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude mcp login plugin:oauth:oauth-probe --no-browser
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-main"} /Users/stephen/.local/bin/claude mcp add --scope user hole-user -- w67-probe hole-user
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-main"} /Users/stephen/.local/bin/claude mcp add --scope local hole-local -- w67-probe hole-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-main"} /Users/stephen/.local/bin/claude plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-main"} /Users/stephen/.local/bin/claude plugin install alpha@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-main"} /Users/stephen/.local/bin/claude mcp list
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-extra"} /Users/stephen/.local/bin/claude mcp list
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:53233", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-main"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"ANTHROPIC_BASE_URL": "http://127.0.0.1:53233", "ANTHROPIC_API_KEY": "w67-local-only", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/hole-claude-extra"} /Users/stephen/.local/bin/claude -p --model opus --effort low --output-format stream-json --verbose --no-session-persistence 'Reply OK.'
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-thread"} /Users/stephen/.local/bin/codex mcp add hole-session -- w67-probe hole-session
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-thread"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-base"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-thread"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-thread"} /Users/stephen/.local/bin/codex mcp get hole-session
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-base"} /Users/stephen/.local/bin/codex mcp add hole-terminal -- w67-probe hole-terminal
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-base"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/hole-codex-base"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex plugin marketplace add /tmp/take-over-5-sessions-w67-probes/market
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex plugin add port-abs@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex plugin add port-rel@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex plugin add port-parent@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex plugin add port-var@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex plugin add port-bare@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex plugin add port-noprefix@w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CODEX_HOME": "/tmp/take-over-5-sessions-w67-probes/portable-codex"} /Users/stephen/.local/bin/codex mcp list --json
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude plugin marketplace update w67-local
cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude plugin update oauth@w67-local
PTY cwd=/tmp/take-over-5-sessions-w67-probes/project-b env-delta={"CLAUDE_CONFIG_DIR": "/tmp/take-over-5-sessions-w67-probes/oauth-claude"} /Users/stephen/.local/bin/claude mcp login plugin:oauth:oauth-probe --no-browser
```
