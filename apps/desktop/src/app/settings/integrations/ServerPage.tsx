import { SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { Badge } from "@/components/controls";
import { ExclamationMarkCircle, Globe, Key, Terminal } from "@/components/icons";
import { CopyButton, SettingsCard } from "@/components/shared";
import { cn } from "@/lib/utils";
import { InfoRow, IntegrationSection, OFF_NOTE, SOURCE_NAMES, sourcePath, WorkerRow } from "./parts";
import type { IntegrationServer } from "./types";

export function serverCommand(server: IntegrationServer): string {
  return server.transport.kind === "stdio" ? [server.transport.command, ...server.transport.args].join(" ") : server.transport.url;
}
export function ServerMark({ server, large = false, dim = false }: { server: IntegrationServer; large?: boolean; dim?: boolean }) {
  const Icon = server.error ? ExclamationMarkCircle : server.transport.kind === "http" ? Globe : Terminal;
  return <span className={cn("flex shrink-0 items-center justify-center", large ? "size-11 rounded-xl" : "size-9 rounded-toolbar-button", server.error ? "bg-destructive/10 text-destructive" : large ? "bg-foreground/5 text-foreground/85" : "bg-foreground/5 text-foreground/65")}><Icon className={cn(large ? "size-5" : "size-4", dim && "text-foreground/30")} /></span>;
}
export function ServerPage({ server, enabled, busy, onEnabled, onBack, onShowPath }: {
  server: IntegrationServer; enabled: boolean; busy?: boolean; onEnabled: (enabled: boolean) => void; onBack: () => void; onShowPath: (path: string) => void;
}) {
  const remote = server.transport.kind === "http";
  const sources = new Intl.ListFormat("en", { style: "long", type: "conjunction" }).format([...new Set(server.sources.map((source) => SOURCE_NAMES[source]))]).replace(", and ", " and ");
  const missing = (["claude", "codex"] as const).filter((agent) => server.signIn[agent]?.kind === "required");
  const files: NonNullable<IntegrationServer["sourceFiles"]> = server.sourceFiles ?? server.sources.map((source) => ({ source, path: sourcePath(source, server.project) }));
  const description = server.error ? `Found in ${sources} · couldn't be read, so no worker gets it` : `${remote ? "Remote address" : "Local command"} · found in ${sources}${missing.length ? " · needs a sign-in" : ""}`;
  return <SettingsPage title={server.name} description={description} mark={<ServerMark server={server} large />} back={{ label: "Plugins", onBack }} className="max-w-plugins pt-0" sectionsClassName="gap-9 pt-4" actions={server.error ? <Badge variant="secondary">Not used</Badge> : <label className="text-foreground/65 flex items-center gap-2.5 text-label">{enabled ? "On" : "Off"}<SettingsSwitch label={server.name} checked={enabled} disabled={busy || !server.canSwitchOff} onCheckedChange={onEnabled} /></label>}>
    {server.error && <IntegrationSection title="Couldn't read it"><SettingsCard><InfoRow title={server.error} value={`Brigadier reads local commands and remote addresses over http or sse. Change it in ${sources} and this page updates by itself.`} mono={false} error /></SettingsCard></IntegrationSection>}
    <IntegrationSection title={server.error ? "What it read" : "What it is"}><SettingsCard>
      <InfoRow title={server.error ? "Address" : remote ? "Remote address" : "Local command"} value={serverCommand(server)} />
      {remote && !server.error && Object.keys(server.signIn).length > 0 && <InfoRow title="Sign-in" value={"Each agent keeps its own. " + (["claude", "codex"] as const).filter((agent) => server.signIn[agent]).map((agent, index) => {
        const state = server.signIn[agent];
        const who = agent === "claude" ? "Claude Code" : "Codex";
        return state?.kind === "native" ? `Signed in for ${who}` : state?.kind === "reusable" ? `${who} can use the sign-in from ${state.from === "claude" ? "Claude Code" : "Codex"}` : `${index === 0 ? "Not" : "not"} signed in for ${who}`;
      }).join("; ") + "."} mono={false} />}
      {server.env.length > 0 && <InfoRow title="Environment" value={server.env.join(", ")} />}
      {server.headers.length > 0 && <InfoRow title="Headers" value={server.headers.join(", ")} />}
    </SettingsCard></IntegrationSection>
    <IntegrationSection title="Found in" aside="Read-only · change it there"><SettingsCard>{files.map((file) => <InfoRow key={`${file.source}:${file.path}`} title={SOURCE_NAMES[file.source]} value={file.path + (file.line ? ` · line ${file.line}` : "")} show="file" onShow={() => onShowPath(file.path)} />)}</SettingsCard></IntegrationSection>
    <IntegrationSection title="Workers"><SettingsCard>{(["claude", "codex"] as const).map((agent) => {
      const name = agent === "claude" ? "Claude Code" : "Codex";
      const native = server.nativeAgents.includes(agent);
      const auth = server.signIn[agent];
      const unavailable = remote && auth?.kind === "required";
      const otherSignedIn = server.signIn[agent === "claude" ? "codex" : "claude"]?.kind;
      const line = server.error ? "Not given: Brigadier couldn't read it" : !enabled ? "Off: Brigadier doesn't give it to workers" : unavailable ? otherSignedIn && otherSignedIn !== "required" ? `Can't use it: it's signed in for ${agent === "codex" ? "Claude Code" : "Codex"} only` : `Can't use it: not signed in for ${name}` : native ? `Have it: it's set up in ${name}${remote && auth?.kind === "native" ? " and signed in there" : ""}` : "The lead gives it to workers when their task needs it";
      return <WorkerRow key={agent} agent={agent} line={line} status={server.error ? "Not used" : !enabled ? "Off" : unavailable ? "Not signed in" : native ? "Has it" : "When needed"} tone={!server.error && enabled ? unavailable ? "warning" : native ? "success" : "secondary" : "secondary"} />;
    })}</SettingsCard>
      {remote && enabled && !server.error && missing.map((agent) => {
        const other = agent === "codex" ? "Claude Code" : "Codex";
        const name = agent === "codex" ? "Codex" : "Claude";
        const loginName = /^[\w.-]+$/.test(server.name) ? server.name : `'${server.name.replaceAll("'", "'\\''")}'`;
        const command = `${agent} mcp login ${loginName}`;
        const otherSignedIn = server.signIn[agent === "codex" ? "claude" : "codex"]?.kind;
        const reason = otherSignedIn && otherSignedIn !== "required" ? `Signed in for ${other} only, so only ${agent === "codex" ? "Claude" : "Codex"} workers can use it.` : `Not signed in for ${name}, so ${name} workers can't use it.`;
        return <div key={agent} className="bg-foreground/5 text-foreground/65 rounded-control flex items-start gap-2 px-3 py-2.5 text-xs leading-integration"><Key className="size-3.5 shrink-0" /><div className="flex min-w-0 flex-col gap-2"><p>{reason} Brigadier can't sign in for you. To let {name} workers use it, sign in for {name}:</p><div className="flex flex-wrap items-center gap-2">In a terminal <span className="bg-code-surface border-divider flex items-center gap-1.5 rounded-md border py-0.5 pr-0.5 pl-2"><code className="text-foreground">{command}</code><CopyButton text={command} size="xs" /></span></div></div></div>;
      })}
    </IntegrationSection>
    {!server.error && <p className="text-foreground/50 text-xs leading-integration">{OFF_NOTE}</p>}
  </SettingsPage>;
}
