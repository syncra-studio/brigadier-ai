import { SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { Badge } from "@/components/controls";
import { ExclamationMarkCircle, Globe, Key, Terminal } from "@/components/icons";
import { SettingsCard } from "@/components/shared";
import { cn } from "@/lib/utils";
import { usedAnywhere, usedIn, type IntegrationAdapter, type Scope } from "./adapter";
import { changeNote, InfoRow, IntegrationSection, listNames, OWN_FILES, ownFiles, SOURCE_NAMES, sourcePath, WhereUsedSection, WorkerRow } from "./parts";
import { needsSignIn, SIGN_IN_NOTE, SignInSection } from "./SignInSection";
import type { IntegrationAgent, IntegrationProject, IntegrationServer, IntegrationSource } from "./types";

/** The agent that reads each of a project's own configs. */
const OWN_AGENT: Partial<Record<IntegrationSource, IntegrationAgent>> = { ClaudeProjectFile: "claude", ClaudeLocal: "claude", CodexProject: "codex" };

export function serverCommand(server: IntegrationServer): string {
  return server.transport.kind === "stdio" ? [server.transport.command, ...server.transport.args].join(" ") : server.transport.url;
}
export function ServerMark({ server, large = false, dim = false }: { server: IntegrationServer; large?: boolean; dim?: boolean }) {
  // On the list, a server no worker can use until someone signs in shows a Key on orange.
  const key = !large && needsSignIn(server);
  const Icon = server.error ? ExclamationMarkCircle : key ? Key : server.transport.kind === "http" ? Globe : Terminal;
  return <span className={cn("flex shrink-0 items-center justify-center", large ? "size-11 rounded-xl" : "size-9 rounded-toolbar-button", server.error ? "bg-destructive/10 text-destructive" : key ? "bg-warning/15 text-warning" : large ? "bg-foreground/5 text-foreground/85" : "bg-foreground/5 text-foreground/65")}><Icon className={cn(large ? "size-5" : "size-4", dim && "text-foreground/30")} /></span>;
}
/** The label by a page's switch: the default, or for a project's own item kept there, its state in that project. */
export const switchLabel = (scope: Scope, on: boolean) => `${on ? "On" : "Off"} ${scope.stays && scope.origin ? `in ${scope.origin.name}` : "by default"}`;

/**
 * `scope` is where it's used; `project`, the project whose view the page was opened from.
 * The header switch is its default, Where it's used its exceptions.
 */
export function ServerPage({ server, scope, projects, project, busy, adapter, focusSignIn, onEnabled, onInProject, onEverywhere, onBack, onShowPath, run }: {
  server: IntegrationServer; scope: Scope; projects: IntegrationProject[]; project?: string | undefined; busy?: boolean; adapter: IntegrationAdapter | null; focusSignIn?: boolean | undefined;
  onEnabled: (enabled: boolean) => void; onInProject: (projectId: string, enabled: boolean | null) => void; onEverywhere: (on: boolean) => void; onBack: () => void; onShowPath: (path: string) => void; run: (action: () => Promise<unknown>) => void;
}) {
  const remote = server.transport.kind === "http";
  const signs = !server.error && Object.keys(server.signIn).length > 0;
  const { origin } = scope;
  const sources = listNames([...new Set(server.sources.map((source) => SOURCE_NAMES[source]))]);
  const files: NonNullable<IntegrationServer["sourceFiles"]> = server.sourceFiles ?? server.sources.map((source) => ({ source, path: sourcePath(source, server.project) }));
  const found = origin ? `from ${origin.name}'s own ${ownFiles(server)}${scope.stays ? "" : " · used in every project"}` : `found in ${sources}`;
  const description = server.error ? `Found in ${sources} · couldn't be read, so no worker gets it` : `${signs ? "Remote address with a sign-in" : remote ? "Remote address" : "Local command"} · ${found}`;
  // Whether workers get it: in the project the page is shown for, else in any project.
  const shownFor = project ?? (scope.stays ? origin?.id : undefined);
  const used = shownFor ? usedIn(scope, shownFor) : usedAnywhere(scope);
  // Where workers get it, when that's only some projects: its own, or the ones turning it on.
  const only = scope.stays ? origin?.name : !project && !scope.on && scope.exceptions.length ? listNames(scope.exceptions.map((item) => item.name)) : undefined;
  const cwd = server.transport.kind === "stdio" ? server.transport.cwd : undefined;
  return <SettingsPage title={server.name} description={description} mark={<ServerMark server={server} large />} back={{ label: "Plugins", onBack }} className="max-w-plugins pt-0" sectionsClassName="gap-9 pt-4" actions={server.error ? <Badge variant="secondary">Not used</Badge> : <label className="text-foreground/65 flex items-center gap-2.5 text-label">{switchLabel(scope, scope.on)}<SettingsSwitch label={server.name} checked={scope.on} disabled={busy || !server.canSwitchOff} onCheckedChange={onEnabled} /></label>}>
    {server.error && <IntegrationSection title="Couldn't read it"><SettingsCard><InfoRow title={server.error} value={`Brigadier reads local commands and remote addresses over http or sse. Change it in ${sources} and this page updates by itself.`} mono={false} error /></SettingsCard></IntegrationSection>}
    {!server.error && server.canSwitchOff && <WhereUsedSection name={server.name} scope={scope} file={ownFiles(server)} projects={projects} busy={!!busy} onInProject={onInProject} onEverywhere={onEverywhere} />}
    {signs && <SignInSection key={server.id} server={server} adapter={adapter} run={run} enabled={used} focus={focusSignIn} />}
    <IntegrationSection title={server.error ? "What it read" : "What it is"}><SettingsCard>
      <InfoRow title={server.error ? "Address" : remote ? "Remote address" : "Local command"} value={serverCommand(server)} />
      {cwd && <InfoRow title="Runs from" value={`${cwd}, wherever the session is`} mono={false} />}
      {server.env.length > 0 && <InfoRow title="Environment" value={server.env.join(", ")} />}
      {server.headers.length > 0 && <InfoRow title="Headers" value={server.headers.join(", ")} />}
    </SettingsCard></IntegrationSection>
    <IntegrationSection title="Found in" aside="Read-only · change it there"><SettingsCard>{files.map((file) => <InfoRow key={`${file.source}:${file.path}`} title={origin && OWN_FILES[file.source] ? `${origin.name} · ${OWN_FILES[file.source]}` : SOURCE_NAMES[file.source]} value={file.path + (file.line ? ` · line ${file.line}` : "")} show="file" onShow={() => onShowPath(file.path)} />)}</SettingsCard></IntegrationSection>
    {!signs && <IntegrationSection title="Workers"><SettingsCard>{(["claude", "codex"] as const).map((agent) => {
      const name = agent === "claude" ? "Claude Code" : "Codex";
      const native = server.nativeAgents.includes(agent);
      const own = server.sources.find((source) => OWN_AGENT[source] === agent);
      const line = server.error ? "Not given: Brigadier couldn't read it" : !used ? "Off: Brigadier doesn't give it to workers"
        : only ? native ? `In ${only}: have it, ${own ? `from the project's ${OWN_FILES[own]}` : `it's set up in ${name}`}` : `In ${only}: get it when a task there needs it`
        : native ? `Have it: it's set up in ${name}` : "The lead gives it to workers when their task needs it";
      return <WorkerRow key={agent} agent={agent} line={line} status={server.error ? "Not used" : !used ? "Off" : native ? "Has it" : only ? "When a task needs it" : "When needed"} tone={!server.error && used && native ? "success" : "secondary"} />;
    })}</SettingsCard></IntegrationSection>}
    {!server.error && <p className="text-foreground/50 text-xs leading-integration">{signs ? SIGN_IN_NOTE : changeNote("server")}</p>}
  </SettingsPage>;
}
