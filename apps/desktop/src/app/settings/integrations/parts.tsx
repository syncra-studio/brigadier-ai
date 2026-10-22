import { useState, type ComponentType, type ReactNode, type SVGProps } from "react";

import { SettingsSection } from "@/app/settings/parts";
import { Badge, Button, TooltipIconButton } from "@/components/controls";
import { Check, ClaudeGlyph, Folder, IdGlyph, Info, Lock, MinusCircle, Plus, ProviderGlyph, Search, X } from "@/components/icons";
import { Popover, PopoverContent, PopoverTrigger, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/overlays";
import { CopyButton, SettingsCard } from "@/components/shared";
import { cn } from "@/lib/utils";
import type { Scope } from "@/app/settings/integrations/adapter";
import type { IntegrationAgent, IntegrationKind, IntegrationProject, IntegrationServer, IntegrationSkill, IntegrationSource } from "@/app/settings/integrations/types";

export const SESSION_NOTE = "A change takes effect the next time a session starts or resumes, not in one already mid-turn.";
/** The footnote of a server's or skill's page (settings.pen › Plugins · Where it's used). */
export const changeNote = (kind: IntegrationKind) => `A change applies the next time a session in that project starts or resumes, not to one already mid-turn. Brigadier keeps reading the ${kind} where it lives, so edits there show up by themselves.`;
export const SOURCE_NAMES: Record<IntegrationSource, string> = {
  ClaudeUser: "Claude Code", ClaudeLocal: "Claude Code", ClaudeProjectFile: "Claude Code",
  CodexUser: "Codex", CodexProject: "Codex", Desktop: "Claude Desktop", CursorUser: "Cursor", CursorProject: "Cursor",
  ClaudePlugin: "Claude Code plugin", CodexPlugin: "Codex plugin",
};
export function sourcePath(source: IntegrationSource, project?: string): string {
  switch (source) {
    case "ClaudeUser": return "~/.claude.json";
    case "ClaudeLocal": return `${project ?? ""}/.claude/settings.local.json`;
    case "ClaudeProjectFile": return `${project ?? ""}/.mcp.json`;
    case "CodexUser": return "~/.codex/config.toml";
    case "CodexProject": return `${project ?? ""}/.codex/config.toml`;
    case "Desktop": return "~/Library/Application Support/Claude/claude_desktop_config.json";
    case "CursorUser": return "~/.cursor/mcp.json";
    case "CursorProject": return `${project ?? ""}/.cursor/mcp.json`;
    case "ClaudePlugin":
    case "CodexPlugin": return ".mcp.json";
  }
}
export function AgentMarks({ agents }: { agents: IntegrationAgent[] }) {
  return <span className="flex shrink-0 items-center gap-0.75" aria-label={agents.map((a) => a === "claude" ? "Claude" : "Codex").join(", ")}>
    {agents.map((agent) => <ProviderGlyph key={agent} provider={agent} className="text-foreground/75 size-3" />)}
  </span>;
}
export function ClaudeAlways({ compact = false }: { compact?: boolean }) {
  return <span title="Claude: always on" className={cn("bg-foreground/5 text-foreground/65 rounded-capsule inline-flex h-5 shrink-0 items-center text-tiny", compact ? "gap-1 px-1.75" : "gap-1.25 px-2")}>
    <Lock className="size-2.75" />{compact ? <ClaudeGlyph className="size-2.75" /> : "Claude: always on"}
  </span>;
}
export function SkillMark({ id, large = false, dim = false }: { id: string; large?: boolean; dim?: boolean }) {
  return <span className={cn("bg-foreground/5 flex shrink-0 items-center justify-center", large ? "size-11 rounded-xl" : "size-9 rounded-toolbar-button")}>
    <IdGlyph id={id} className={cn(large ? "size-5.5" : "size-4.5", dim && "opacity-50")} />
  </span>;
}
export function IntegrationSection({ title, description, aside, children }: { title: string; description?: string | undefined; aside?: string | undefined; children: ReactNode }) {
  return <SettingsSection title={title} description={description} aside={aside && <span className="text-muted-foreground">{aside}</span>} setting={title} className="[&>div:first-child]:min-h-0">{children}</SettingsSection>;
}
export function InfoRow({ title, value, mono = true, copy = mono, titleMono = false, error = false, show, onShow }: {
  title: string; value: string; mono?: boolean; copy?: boolean; titleMono?: boolean; error?: boolean; show?: "file" | "folder"; onShow?: () => void;
}) {
  return <div data-setting={title} className="flex items-center gap-6 px-4 py-3">
    <div className="flex min-w-0 flex-1 flex-col gap-1">
      <span className={cn("text-label font-medium", titleMono && "font-mono text-xs", error && "text-destructive")}>{title}</span>
      <span className={cn("text-foreground/65 break-words text-xs leading-integration-value", mono && "font-mono")}>{value}</span>
    </div>
    {show ? <Button variant="soft" size="sm" onClick={onShow}>Show {show}</Button> : copy && <CopyButton text={value} size="xs" />}
  </div>;
}
export function WorkerRow({ agent, title, line, status, tone = "secondary", always = false, dim = false, trailing }: {
  agent: IntegrationAgent; title?: string; line: string; status?: string; tone?: "secondary" | "success" | "warning"; always?: boolean; dim?: boolean; trailing?: ReactNode;
}) {
  return <div data-slot="worker-row" className="flex items-center gap-3 px-4 py-3">
    <span className={cn("bg-foreground/5 rounded-control flex size-7 shrink-0 items-center justify-center", dim && "opacity-50")}><ProviderGlyph provider={agent} className="text-foreground/85 size-3.5" /></span>
    <div className="flex min-w-0 flex-1 flex-col gap-0.75"><span className="text-label font-medium">{title ?? `${agent === "claude" ? "Claude" : "Codex"} workers`}</span><span className="text-foreground/65 text-xs leading-integration">{line}</span></div>
    {always ? <ClaudeAlways /> : trailing ?? (status && <Badge variant={tone}>{status}</Badge>)}
  </div>;
}
/** A command to copy, in a mono chip (`codex mcp login linear`). */
export function CommandChip({ command }: { command: string }) {
  return <span className="bg-code-surface border-divider flex min-w-0 shrink-0 items-center gap-1.5 rounded-md border py-0.5 pr-0.5 pl-2 text-xs"><code className="text-foreground truncate">{command}</code><CopyButton text={command} size="xs" /></span>;
}
export function IntegrationNote({ icon: Icon = Info, children }: { icon?: ComponentType<SVGProps<SVGSVGElement>>; children: ReactNode }) {
  return <div className="bg-foreground/5 text-foreground/65 rounded-control flex items-start gap-2 px-3 py-2.5 text-xs leading-integration"><Icon className="size-3.5 shrink-0" /><div>{children}</div></div>;
}

/** "a", "a and b", "a, b and c". */
export function listNames(names: string[]): string {
  return new Intl.ListFormat("en", { style: "long", type: "conjunction" }).format(names).replace(", and ", " and ");
}
const COUNTS = ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];
const count = (n: number) => COUNTS[n] ?? String(n);

export const OWN_FILES: Partial<Record<IntegrationSource, string>> = {
  ClaudeProjectFile: ".mcp.json", ClaudeLocal: "local Claude Code setup", CodexProject: ".codex/config.toml", CursorProject: ".cursor/mcp.json",
};
/** Where a project's own item is set up in its project: ".mcp.json", ".agents/skills". */
export function ownFiles(item: IntegrationServer | IntegrationSkill): string {
  const files = "description" in item
    ? item.locations.filter((path) => item.projectPath && path.startsWith(`${item.projectPath}/`)).map((path) => path.slice(item.projectPath!.length + 1, path.lastIndexOf("/")))
    : item.sources.flatMap((source) => OWN_FILES[source] ?? []);
  return listNames([...new Set(files)]);
}

/** Where a server or skill differs from its default (#1102): "Off in steady-app", "From steady-app", "On here". */
export function ScopeChip({ label, tip }: { label: string; tip?: string | undefined }) {
  const chip = <span data-slot="scope-chip" className="bg-foreground/5 text-foreground/65 rounded-capsule inline-flex h-4.5 shrink-0 items-center gap-1 px-1.75 text-tiny whitespace-nowrap"><Folder className="size-2.5 shrink-0" />{label}</span>;
  if (!tip) return chip;
  return <TooltipProvider delayDuration={0}><Tooltip><TooltipTrigger asChild>{chip}</TooltipTrigger><TooltipContent>{tip}</TooltipContent></Tooltip></TooltipProvider>;
}
/** A tile's chips on the list: where it comes from, and where it differs from its default. */
export function ScopeChips({ scope, project }: { scope: Scope; project?: string | undefined }) {
  const origin = !scope.stays && scope.origin && scope.origin.id !== project ? <ScopeChip label={`From ${scope.origin.name}`} /> : null;
  const turned = scope.on ? "Off" : "On";
  let differs = null;
  if (project) {
    if (scope.exceptions.some((item) => item.id === project)) differs = <ScopeChip label={`${turned} here`} />;
  } else if (scope.exceptions.length) {
    const names = scope.exceptions.map((item) => item.name);
    differs = names.length === 1 ? <ScopeChip label={`${turned} in ${names[0]}`} /> : <ScopeChip label={`${turned} in ${names.length} projects`} tip={`${turned} in ${listNames(names)}`} />;
  }
  return <>{origin}{differs}</>;
}

/** One project's exception under the summary (#1103); its X puts the project back on the default. */
export function ExceptionRow({ project, on, busy, onRemove }: { project: IntegrationProject; on: boolean; busy: boolean; onRemove: () => void }) {
  return <div data-slot="exception-row" className="flex items-center gap-3 px-4 py-2.5">
    <span className="bg-foreground/5 rounded-control flex size-7 shrink-0 items-center justify-center"><Folder className="text-foreground/65 size-3.5" /></span>
    <div className="flex min-w-0 flex-1 flex-col gap-0.75">
      <span className={cn("truncate text-label font-medium", !project.trusted && "text-foreground/50")}>{project.name}</span>
      <span className={cn("truncate text-xs", project.trusted ? "text-foreground/65 font-mono" : "text-foreground/50")}>{project.trusted ? project.path : "No longer trusted · kept, but not read"}</span>
    </div>
    <Badge variant={on ? "secondary" : "success"}>{on ? "Off here" : "On here"}</Badge>
    <TooltipIconButton tooltip="Remove exception" aria-label={`Remove ${project.name}'s exception`} disabled={busy} onClick={onRemove}><X className="text-muted-foreground size-3.5" /></TooltipIconButton>
  </div>;
}

/** One of a project's own item's two places (#1104). */
function RadioRow({ checked, title, line, disabled, onSelect }: { checked: boolean; title: string; line: string; disabled: boolean; onSelect: () => void }) {
  return <button type="button" role="radio" aria-checked={checked} disabled={disabled} onClick={() => { if (!checked) onSelect(); }} className="flex w-full items-start gap-3 px-4 py-3 text-left">
    <span className={cn("mt-0.25 flex size-4 shrink-0 items-center justify-center rounded-full", checked ? "bg-toggle-on" : "border-foreground/20 border")}>{checked && <span className="bg-background size-1.5 rounded-full" />}</span>
    <span className="flex min-w-0 flex-1 flex-col gap-0.75"><span className="text-label font-medium">{title}</span><span className="text-foreground/65 text-xs leading-integration">{line}</span></span>
  </button>;
}

/** The projects to make an exception in (#1105): one with an exception already, or not trusted, can't be picked. */
function ProjectPicker({ name, on, projects, exceptions, busy, onPick }: {
  name: string; on: boolean; projects: IntegrationProject[]; exceptions: IntegrationProject[]; busy: boolean; onPick: (projectId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [find, setFind] = useState("");
  const turn = on ? "off" : "on";
  const shown = projects.filter((project) => `${project.name} ${project.path}`.toLowerCase().includes(find.toLowerCase().trim()));
  return <Popover open={open} onOpenChange={(value) => { setOpen(value); setFind(""); }}>
    <PopoverTrigger asChild><Button variant="soft" size="sm" disabled={busy}>{on ? <MinusCircle className="size-3.5" /> : <Plus className="size-3.5" />}Turn {turn} in a project…</Button></PopoverTrigger>
    <PopoverContent align="end" aria-label={`Turn ${name} ${turn} in…`} className="flex max-h-(--radix-popover-content-available-height) w-85 flex-col gap-0.5 rounded-2xl p-1.5">
      <label className="border-divider flex h-8.5 shrink-0 items-center gap-2 border-b px-2.5">
        <Search className="text-foreground/50 size-3.5 shrink-0" />
        <input aria-label="Find a project" placeholder="Find a project" value={find} onChange={(event) => setFind(event.target.value)} className="placeholder:text-foreground/50 min-w-0 flex-1 bg-transparent text-label outline-none" />
      </label>
      <p className="text-foreground/50 shrink-0 px-2.5 pt-1.5 pb-1 text-tiny font-medium">Turn {name} {turn} in…</p>
      {/* Only the projects scroll: the find field and the footer stay put. */}
      <div className="flex min-h-0 flex-col gap-0.5 overflow-y-auto">{shown.map((project) => {
        const done = exceptions.some((item) => item.id === project.id);
        const locked = !project.trusted;
        const Icon = locked ? Lock : Folder;
        return <button key={project.id} type="button" disabled={done || locked} onClick={() => { setOpen(false); onPick(project.id); }} className={cn("rounded-control flex h-10 w-full shrink-0 items-center gap-2.5 px-2.5 text-left enabled:hover:bg-foreground/5", locked && !done && "opacity-45")}>
          <Icon className="text-foreground/65 size-3.5 shrink-0" />
          <span className="flex min-w-0 flex-1 flex-col gap-px"><span className="truncate text-label">{project.name}</span><span className="text-foreground/50 truncate font-mono text-tiny">{project.path}</span></span>
          {done ? <span className="text-foreground/50 flex shrink-0 items-center gap-1 text-xs"><Check className="size-3" />Already {turn} here</span> : locked && <span className="text-foreground/50 shrink-0 text-xs">Not trusted</span>}
        </button>;
      })}</div>
      <p className="border-divider text-foreground/50 shrink-0 border-t px-2.5 pt-2 pb-1.5 text-tiny leading-integration">Brigadier only reads trusted projects. Trust one from its menu in the sidebar.</p>
    </PopoverContent>
  </Popover>;
}

/** The first row of Where it's used: where it's used, in plain words. */
function summary(scope: Scope): { title: string; line: string } {
  const names = scope.exceptions.map((project) => project.name);
  const [only] = names;
  if (scope.on) {
    if (!names.length) return { title: "Every project", line: "On by default, so sessions in every trusted project get it." };
    return { title: `Every project except ${listNames(names)}`, line: names.length === 1 ? `On by default. ${only} turns it off for itself.` : `On by default. These ${count(names.length)} projects turn it off for themselves.` };
  }
  if (!names.length) return { title: "No project", line: "Off by default, and no project turns it on." };
  return { title: `Only in ${listNames(names)}`, line: names.length === 1 ? `Off by default. ${only} turns it on for itself.` : `Off by default. These ${count(names.length)} projects turn it on for themselves.` };
}

/** Where it's used (#1106), the first section of a server's or skill's page. */
export function WhereUsedSection({ name, scope, file, projects, busy, onInProject, onEverywhere }: {
  name: string; scope: Scope; file: string; projects: IntegrationProject[]; busy: boolean;
  onInProject: (projectId: string, enabled: boolean | null) => void; onEverywhere: (on: boolean) => void;
}) {
  const { origin } = scope;
  const told = summary(scope);
  const description = !origin ? "The switch above is the default. A trusted project can make an exception for itself." : scope.stays ? "It comes from a project's own config." : `It comes from ${origin.name}'s own config.`;
  return <IntegrationSection title="Where it's used" description={description}>
    {origin && <SettingsCard role="radiogroup" aria-label="Where it's used">
      <RadioRow checked={scope.stays} disabled={busy} title={`Only in ${origin.name}`} line={scope.stays ? `Its config lives in this project's ${file}, so only sessions in ${origin.name} get it.` : `Its config lives in that project's ${file}. Picking this again drops the exceptions below.`} onSelect={() => onEverywhere(false)} />
      <RadioRow checked={!scope.stays} disabled={busy} title="Every project" line={scope.stays ? `Use it like one of your own: on everywhere, with exceptions per project. Brigadier keeps reading it from ${origin.name}'s ${file}.` : `Used like one of your own: on by default everywhere, with exceptions. Brigadier keeps reading it from ${origin.name}'s ${file}.`} onSelect={() => onEverywhere(true)} />
    </SettingsCard>}
    {!scope.stays && <SettingsCard>
      <div data-slot="where-used" className="flex items-center gap-6 px-4 py-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1"><span className="text-label font-medium">{told.title}</span><span className="text-foreground/65 break-words text-xs leading-integration-value">{told.line}</span></div>
        <ProjectPicker name={name} on={scope.on} projects={projects} exceptions={scope.exceptions} busy={busy} onPick={(projectId) => onInProject(projectId, !scope.on)} />
      </div>
      {scope.exceptions.map((project) => <ExceptionRow key={project.id} project={project} on={scope.on} busy={busy} onRemove={() => onInProject(project.id, null)} />)}
    </SettingsCard>}
  </IntegrationSection>;
}
