import { SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { Badge } from "@/components/controls";
import { ExclamationMarkCircle, Globe, Key, Terminal } from "@/components/icons";
import { SettingsCard } from "@/components/shared";
import { cn } from "@/lib/utils";
import type { IntegrationAdapter } from "./adapter";
import { InfoRow, IntegrationSection, OFF_NOTE, SOURCE_NAMES, sourcePath, WorkerRow } from "./parts";
import { needsSignIn, SIGN_IN_NOTE, SignInSection } from "./SignInSection";
import type { IntegrationServer } from "./types";

export function serverCommand(server: IntegrationServer): string {
  return server.transport.kind === "stdio" ? [server.transport.command, ...server.transport.args].join(" ") : server.transport.url;
}
export function ServerMark({ server, large = false, dim = false }: { server: IntegrationServer; large?: boolean; dim?: boolean }) {
  // On the list, a server no worker can use until someone signs in shows a Key on orange.
  const key = !large && needsSignIn(server);
  const Icon = server.error ? ExclamationMarkCircle : key ? Key : server.transport.kind === "http" ? Globe : Terminal;
  return <span className={cn("flex shrink-0 items-center justify-center", large ? "size-11 rounded-xl" : "size-9 rounded-toolbar-button", server.error ? "bg-destructive/10 text-destructive" : key ? "bg-warning/15 text-warning" : large ? "bg-foreground/5 text-foreground/85" : "bg-foreground/5 text-foreground/65")}><Icon className={cn(large ? "size-5" : "size-4", dim && "text-foreground/30")} /></span>;
}
export function ServerPage({ server, enabled, busy, adapter, focusSignIn, onEnabled, onBack, onShowPath, run }: {
  server: IntegrationServer; enabled: boolean; busy?: boolean; adapter: IntegrationAdapter | null; focusSignIn?: boolean | undefined; onEnabled: (enabled: boolean) => void; onBack: () => void; onShowPath: (path: string) => void; run: (action: () => Promise<unknown>) => void;
}) {
  const remote = server.transport.kind === "http";
  const signs = !server.error && Object.keys(server.signIn).length > 0;
  const sources = new Intl.ListFormat("en", { style: "long", type: "conjunction" }).format([...new Set(server.sources.map((source) => SOURCE_NAMES[source]))]).replace(", and ", " and ");
  const files: NonNullable<IntegrationServer["sourceFiles"]> = server.sourceFiles ?? server.sources.map((source) => ({ source, path: sourcePath(source, server.project) }));
  const description = server.error ? `Found in ${sources} · couldn't be read, so no worker gets it` : `${signs ? "Remote address with a sign-in" : remote ? "Remote address" : "Local command"} · found in ${sources}`;
  return <SettingsPage title={server.name} description={description} mark={<ServerMark server={server} large />} back={{ label: "Plugins", onBack }} className="max-w-plugins pt-0" sectionsClassName="gap-9 pt-4" actions={server.error ? <Badge variant="secondary">Not used</Badge> : <label className="text-foreground/65 flex items-center gap-2.5 text-label">{enabled ? "On" : "Off"}<SettingsSwitch label={server.name} checked={enabled} disabled={busy || !server.canSwitchOff} onCheckedChange={onEnabled} /></label>}>
    {server.error && <IntegrationSection title="Couldn't read it"><SettingsCard><InfoRow title={server.error} value={`Brigadier reads local commands and remote addresses over http or sse. Change it in ${sources} and this page updates by itself.`} mono={false} error /></SettingsCard></IntegrationSection>}
    {signs && <SignInSection key={server.id} server={server} adapter={adapter} run={run} focus={focusSignIn} />}
    <IntegrationSection title={server.error ? "What it read" : "What it is"}><SettingsCard>
      <InfoRow title={server.error ? "Address" : remote ? "Remote address" : "Local command"} value={serverCommand(server)} />
      {server.env.length > 0 && <InfoRow title="Environment" value={server.env.join(", ")} />}
      {server.headers.length > 0 && <InfoRow title="Headers" value={server.headers.join(", ")} />}
    </SettingsCard></IntegrationSection>
    <IntegrationSection title="Found in" aside="Read-only · change it there"><SettingsCard>{files.map((file) => <InfoRow key={`${file.source}:${file.path}`} title={SOURCE_NAMES[file.source]} value={file.path + (file.line ? ` · line ${file.line}` : "")} show="file" onShow={() => onShowPath(file.path)} />)}</SettingsCard></IntegrationSection>
    {!signs && <IntegrationSection title="Workers"><SettingsCard>{(["claude", "codex"] as const).map((agent) => {
      const name = agent === "claude" ? "Claude Code" : "Codex";
      const native = server.nativeAgents.includes(agent);
      const line = server.error ? "Not given: Brigadier couldn't read it" : !enabled ? "Off: Brigadier doesn't give it to workers" : native ? `Have it: it's set up in ${name}` : "The lead gives it to workers when their task needs it";
      return <WorkerRow key={agent} agent={agent} line={line} status={server.error ? "Not used" : !enabled ? "Off" : native ? "Has it" : "When needed"} tone={!server.error && enabled && native ? "success" : "secondary"} />;
    })}</SettingsCard></IntegrationSection>}
    {!server.error && <p className="text-foreground/50 text-xs leading-integration">{signs ? SIGN_IN_NOTE : OFF_NOTE}</p>}
  </SettingsPage>;
}
