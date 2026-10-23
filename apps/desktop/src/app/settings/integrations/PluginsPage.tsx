import { useEffect, useState, type ComponentType, type ReactNode, type SVGProps } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { SettingsSelect } from "@/components/controls";
import { BookOpen, Folder, Globe, MinusCircle, Plugin, SettingsSlider, ShieldCheck } from "@/components/icons";
import { SettingsCard, SettingsSearch, StateDot } from "@/components/shared";
import { cn } from "@/lib/utils";
import { scopeOf, useIntegrations, usedIn, type IntegrationAdapter, type IntegrationSnapshot, type Scope } from "./adapter";
import { AgentMarks, ClaudeAlways, IntegrationNote, ownFiles, ScopeChips, SkillMark, SOURCE_NAMES, sourcePath } from "./parts";
import { ServerMark, ServerPage, serverCommand } from "./ServerPage";
import { needsSignIn } from "./SignInSection";
import { SkillPage } from "./SkillPage";
import type { IntegrationServer, IntegrationSkill } from "./types";

export const PLUGINS_DESCRIPTION = "MCP servers and skills Brigadier found on this Mac. It only reads them: add, remove or change one in that agent's own setup.";
export const PLUGINS_ROWS = [
  { label: "MCP servers", description: "Claude Code, Codex, Claude Desktop and Cursor — on in every project unless a project says otherwise" },
  { label: "Skills", description: "Every worker gets every skill, whichever agent it came from" },
  { label: "What it is", target: "MCP servers" },
  { label: "Found in", description: "Read-only · change it there", target: "MCP servers" },
  { label: "Sign-in", target: "MCP servers" },
  { label: "Workers", target: "MCP servers" },
  { label: "Folders", target: "Skills" },
] as const;

/** Where a found item lives, after its search result's page: "from steady-app" when used everywhere, "steady-app" when it stays there. */
function livesIn(item: IntegrationServer | IntegrationSkill, snapshot: IntegrationSnapshot): { detail?: string } {
  const { origin, stays } = scopeOf(item, snapshot);
  return origin ? { detail: stays ? origin.name : `from ${origin.name}` } : {};
}

export function integrationSearchRows() {
  const { snapshot } = useIntegrations.getState();
  const { catalog } = snapshot;
  return [
    ...PLUGINS_ROWS,
    ...catalog.servers.map((item) => ({ label: item.name, description: serverCommand(item), target: `server:${item.id}`, section: "MCP servers", ...livesIn(item, snapshot) })),
    ...catalog.skills.map((item) => ({ label: item.name, description: item.description, target: `skill:${item.id}`, section: "Skills", ...livesIn(item, snapshot) })),
    ...(catalog.projects ?? []).filter((project) => project.trusted).map((project) => ({ label: project.name, description: project.path, target: `project:${project.id}`, detail: "project view" })),
  ];
}

export function PluginsPage() {
  const { snapshot, adapter, error } = useIntegrations();
  return <PluginsView snapshot={snapshot} adapter={adapter} error={error} />;
}

type Tab = "servers" | "skills";
type Selection = { tab: Tab; id: string; signIn?: boolean } | null;
type Item = IntegrationServer | IntegrationSkill;
/** A tile: its item, where it's used, whether it's off in the view, and the view's own line or end text in place of its switch. */
type Row = { item: Item; scope: Scope; off: boolean; line?: string; trailing?: string };
type Group = { key: string; title: string; icon?: ComponentType<SVGProps<SVGSVGElement>>; description: ReactNode; aside: string; rows: Row[] };

const ALL = "all";
const counted = (n: number, tab: Tab) => `${n} ${tab === "servers" ? n === 1 ? "server" : "servers" : n === 1 ? "skill" : "skills"}`;
const revealedProject = (label: string | null) => label?.startsWith("project:") ? label.slice("project:".length) : null;
const OWN_CONFIG: Record<Tab, string> = { servers: "Its .mcp.json, .codex/config.toml and .cursor/mcp.json", skills: "Its .claude/skills and .agents/skills" };

export function PluginsView({ snapshot, adapter, error, initialTab = "servers", initialId, initialProject }: {
  snapshot: IntegrationSnapshot; adapter: IntegrationAdapter | null; error?: string | null; initialTab?: Tab; initialId?: string; initialProject?: string;
}) {
  const [tab, setTab] = useState<Tab>(() => {
    const label = useReveal.getState().label;
    return label === "Skills" || label?.startsWith("skill:") ? "skills" : initialTab;
  });
  const [project, setProject] = useState<string | null>(() => revealedProject(useReveal.getState().label) ?? initialProject ?? null);
  const [selected, setSelected] = useState<Selection>(initialId ? { tab: initialTab, id: initialId } : null);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState<string | null>(() => useReveal.getState().label);
  const action = useAction();
  const { catalog } = snapshot;
  useEffect(() => useReveal.subscribe(({ label: reveal }) => {
    if (!reveal) return;
    if (reveal === "Skills" || reveal.startsWith("skill:")) {
      setTab("skills"); setSelected(null); setQuery(""); setHighlight(reveal);
    } else if (reveal === "MCP servers" || reveal.startsWith("server:")) {
      setTab("servers"); setSelected(null); setQuery(""); setHighlight(reveal);
    } else if (reveal.startsWith("project:")) {
      setProject(revealedProject(reveal)); setSelected(null); setQuery(""); setHighlight(null);
    }
  }), []);
  const busy = action.busy || !adapter;
  const toggleServer = (id: string, value: boolean) => { if (adapter) action.run(() => adapter.setServerEnabled(id, value)); };
  const toggleSkill = (id: string, value: boolean) => { if (adapter) action.run(() => adapter.setSkillEnabled(id, value)); };
  const show = (path: string) => { if (adapter) action.run(() => adapter.showPath(path)); };
  const back = () => setSelected(null);
  const scoped = (item: Item) => scopeOf(item, snapshot);
  // The project view: a trusted project whose own catalog was read.
  const trusted = (catalog.projects ?? []).filter((item) => item.trusted && catalog.projectCatalogs?.[item.id]);
  const view = trusted.find((item) => item.id === project);
  const all = (of: Tab): Item[] => of === "servers" ? catalog.servers : catalog.skills;
  /** What sessions in the view's project find, then the other projects' own items that stay there. */
  const mine = (of: Tab): Item[] => view ? of === "servers" ? catalog.projectCatalogs![view.id]!.servers : catalog.projectCatalogs![view.id]!.skills : all(of);
  const others = (of: Tab): Item[] => view ? all(of).filter((item) => {
    const scope = scoped(item);
    return scope.stays && scope.origin && scope.origin.id !== view.id && !mine(of).some((own) => own.id === item.id);
  }) : [];
  const listed = (of: Tab) => [...mine(of), ...others(of)];
  const usedHere = (item: Item) => !("error" in item && item.error) && (!view || usedIn(scoped(item), view.id));
  const pageProps = (item: Item) => {
    const scope = scoped(item);
    return {
      scope, projects: catalog.projects ?? [], project: view?.id, busy,
      onInProject: (projectId: string, enabled: boolean | null) => { if (adapter) action.run(() => adapter.setInProject(scope.kind, scope.ids, projectId, enabled)); },
      onEverywhere: (on: boolean) => { if (adapter && scope.origin) { const origin = scope.origin.id; action.run(() => adapter.setEverywhere(scope.kind, scope.ids, origin, on)); } },
      onBack: back, onShowPath: show,
    };
  };
  const server = selected?.tab === "servers" ? listed("servers").find((item): item is IntegrationServer => item.id === selected.id) : undefined;
  const skill = selected?.tab === "skills" ? listed("skills").find((item): item is IntegrationSkill => item.id === selected.id) : undefined;
  let page;
  if (server) page = <ServerPage server={server} {...pageProps(server)} adapter={adapter} focusSignIn={selected?.signIn} run={action.run} onEnabled={(value) => toggleServer(server.id, value)} />;
  else if (skill) page = <SkillPage skill={skill} skills={listed("skills").filter((item): item is IntegrationSkill => "description" in item)} {...pageProps(skill)} onEnabled={(value) => toggleSkill(skill.id, value)} />;
  else {
    const matches = (item: Item) => `${item.name} ${"description" in item ? item.description : serverCommand(item)} ${item.project ?? ""}`.toLowerCase().includes(query.toLowerCase().trim());
    const restricted = tab === "skills" && catalog.skills.some((item) => !item.canSwitchOff && item.nativeAgents.includes("claude"));
    const groups: Group[] = [];
    if (view) {
      const own: Row[] = [], changed: Row[] = [], defaults: Row[] = [], unused: Row[] = [];
      for (const item of mine(tab).filter(matches)) {
        const scope = scoped(item);
        const off = !usedHere(item);
        if (scope.origin?.id === view.id) own.push({ item, scope, off });
        else if ("error" in item && item.error) unused.push({ item, scope, off });
        else if (scope.exceptions.some((other) => other.id === view.id)) changed.push({ item, scope, off });
        else if (!off) defaults.push({ item, scope, off });
        else unused.push({ item, scope, off, line: "Off by default · switch on to use it here" });
      }
      for (const item of others(tab).filter(matches)) {
        const scope = scoped(item);
        const name = scope.origin!.name;
        unused.push({ item, scope, off: true, line: `Only in ${name} · from its own ${ownFiles(item)}`, trailing: `${name} only` });
      }
      groups.push(
        { key: "own", title: `From ${view.name}'s own config`, icon: Folder, description: OWN_CONFIG[tab], aside: counted(own.length, tab), rows: own },
        { key: "changed", title: `Changed for ${view.name}`, icon: SettingsSlider, description: "Its exceptions to your defaults", aside: counted(changed.length, tab), rows: changed },
        { key: "defaults", title: "Your defaults it uses", icon: Globe, description: "On in every project, and not changed here", aside: counted(defaults.length, tab), rows: defaults },
        { key: "unused", title: `Not used in ${view.name}`, icon: MinusCircle, description: "Off by default, couldn't be read, or another project's own", aside: counted(unused.length, tab), rows: unused },
      );
    } else {
      const rows = all(tab).filter(matches).map((item) => {
        const scope = scoped(item);
        return { item, scope, off: !scope.on };
      });
      // Yours: the user's own items, and projects' items used in every project.
      groups.push({ key: "yours", title: "Yours", description: tab === "servers" ? PLUGINS_ROWS[0].description : restricted ? "Claude can't hide skills in ~/.claude/skills for one session: there, the switch only decides Codex workers" : PLUGINS_ROWS[1].description, aside: "Switches apply to new and resumed sessions", rows: rows.filter((row) => !row.scope.stays) });
      for (const row of rows) {
        const origin = row.scope.stays ? row.scope.origin : undefined;
        if (!origin) continue;
        const key = `project:${origin.id || origin.name}`;
        const group = groups.find((item) => item.key === key);
        if (group) group.rows.push(row);
        else groups.push({ key, title: origin.name.split("/").pop()!, icon: Folder, description: `${origin.path} · its own config, only in this project`, aside: "", rows: [row] });
      }
      for (const group of groups.slice(1)) group.aside = counted(group.rows.length, tab);
    }
    // Servers of one name set up apart (pencil in Claude Code, Codex and Claude Desktop, each
    // with its own arguments): each row says where it comes from instead of its command.
    for (const group of groups) {
      const named = (name: string) => group.rows.filter((row) => !("description" in row.item) && row.item.name === name).length;
      for (const row of group.rows) {
        if (!("description" in row.item) && !row.line && !row.item.error && named(row.item.name) > 1) row.line = foundIn(row.item);
      }
    }
    const shown = groups.filter((group) => group.key === "yours" || group.rows.length);
    const toggle = ({ item, scope }: Row, value: boolean) => {
      if (view && !(scope.stays && scope.origin?.id === view.id)) {
        if (adapter) action.run(() => adapter.setInProject(scope.kind, scope.ids, view.id, value));
      } else if (scope.kind === "skill") toggleSkill(item.id, value);
      else toggleServer(item.id, value);
    };
    page = <SettingsPage title="Plugins" description={<><span className="hidden @lg:block">{PLUGINS_DESCRIPTION}</span><span className="@lg:hidden">MCP servers and skills Brigadier found on this Mac. It only reads them: change them in each agent's own setup.</span></>} actions={adapter && !error ? <span className="text-muted-foreground flex items-center gap-1.5 text-label"><StateDot tone="ready" />Up to date</span> : undefined}>
      <div className="flex items-center justify-between gap-3 pt-1 @lg:gap-4">
        <div role="tablist" aria-label="Plugins" className="flex shrink-0 items-center gap-1">
          {(["servers", "skills"] as const).map((value) => <button key={value} type="button" role="tab" aria-selected={tab === value} data-setting={value === "servers" ? "MCP servers" : "Skills"} aria-controls="integrations-list" className={cn("rounded-capsule flex h-7 items-center gap-1.5 px-3 text-label", tab === value ? "bg-foreground/10" : "text-foreground/65")} onClick={() => { setTab(value); setQuery(""); setHighlight(null); }} onKeyDown={(event) => {
            if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
            event.preventDefault();
            const next = event.key === "Home" ? "servers" : event.key === "End" ? "skills" : value === "servers" ? "skills" : "servers";
            setTab(next); setQuery("");
            const tabs = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("[role=tab]");
            tabs?.[next === "servers" ? 0 : 1]?.focus();
          }} tabIndex={tab === value ? 0 : -1}>{value === "servers" ? "MCP servers" : "Skills"}<span className="text-foreground/50">{view ? mine(value).filter(usedHere).length : all(value).length}</span></button>)}
        </div>
        <div className="flex min-w-0 flex-1 items-center justify-end gap-2">
          {trusted.length > 0 && <div data-setting={`project:${view?.id ?? ALL}`} className="min-w-0 shrink"><SettingsSelect label="Project" value={view?.id ?? ALL} options={[{ value: ALL, label: "All projects" }, ...trusted.map((item) => ({ value: item.id, label: item.name }))]} onChange={(value) => { setProject(value === ALL ? null : value); setHighlight(null); }} className={cn("max-w-40", view && "bg-foreground/10")} /></div>}
          <div className="min-w-0 flex-1 @lg:max-w-60"><SettingsSearch className="@lg:hidden" label={tab === "servers" ? "Search MCP servers" : "Search skills"} placeholder="Search" value={query} onChange={setQuery} /><SettingsSearch className="hidden @lg:flex" label={tab === "servers" ? "Search MCP servers" : "Search skills"} value={query} onChange={setQuery} /></div>
        </div>
      </div>
      <div id="integrations-list" role="tabpanel" aria-label={tab === "servers" ? "MCP servers" : "Skills"} className="flex flex-col gap-8">
        {view && <IntegrationNote icon={Folder}>Showing what sessions in {view.name} get. A switch here changes only {view.name}: it adds or removes this project's exception. The default stays as it is.</IntegrationNote>}
        {shown.map((group) => {
          const Icon = group.icon;
          return <section key={group.key} aria-label={group.title} className="flex flex-col gap-2.5">
            <header className="flex items-center justify-between gap-4">
              <div className="flex min-w-0 flex-1 flex-col gap-0.5"><h2 className="flex items-center gap-2 text-sm font-medium">{Icon && <Icon className="text-muted-foreground size-4 shrink-0" />}<span className="truncate">{group.title}</span></h2><p className="text-foreground/65 break-words text-label">{group.description}</p></div>
              <span className={cn("text-muted-foreground hidden shrink-0 text-label @lg:block", !all(tab).length && "@lg:hidden")}>{group.aside}</span>
            </header>
            {group.rows.length ? <div className="grid grid-cols-1 gap-x-2 gap-y-0.5 @lg:grid-cols-2">{group.rows.map((row) => {
              const { item } = row;
              const isSkill = "description" in item;
              return <IntegrationTile key={item.id} row={row} project={view?.id} highlighted={highlight === `${isSkill ? "skill" : "server"}:${item.id}`} busy={busy} onOpen={() => setSelected({ tab, id: item.id, ...(!isSkill && needsSignIn(item) ? { signIn: true } : {}) })} onToggle={(value) => toggle(row, value)} />;
            })}</div> : <CatalogEmpty tab={tab} projects={catalog.trustedProjectCount ?? 0} />}
          </section>;
        })}
        {catalog.untrustedProjects?.length ? <div className="text-foreground/50 flex items-start gap-2 px-1 text-xs leading-integration"><ShieldCheck className="mt-0.25 size-3.5 shrink-0" /><p>Brigadier reads only projects you've trusted. {catalog.untrustedProjects.join(" and ")} aren't trusted, so their {tab === "servers" ? "servers" : "skills"} aren't read.</p></div> : null}
      </div>
    </SettingsPage>;
  }
  return <div className="@container relative h-full min-w-0 [&_[data-tauri-drag-region]]:h-11">{page}{(action.error || error) && <p role="alert" className="text-destructive bg-background absolute right-5 bottom-4 left-5 text-xs">{action.error ?? error}</p>}</div>;
}

/** Where a server is set up, in a few words: "Codex · ~/.codex/config.toml". */
function foundIn(server: IntegrationServer): string {
  const files = server.sourceFiles?.length
    ? server.sourceFiles.map((file) => `${SOURCE_NAMES[file.source]} · ${file.path}`)
    : server.sources.map((source) => `${SOURCE_NAMES[source]} · ${sourcePath(source, server.projectPath)}`);
  return [...new Set(files)].join(", ");
}

function IntegrationTile({ row, project, busy, highlighted, onOpen, onToggle }: {
  row: Row; project?: string | undefined; busy: boolean; highlighted: boolean; onOpen: () => void; onToggle: (enabled: boolean) => void;
}) {
  const { item, off } = row;
  const skill = "description" in item;
  const always = skill && !item.canSwitchOff && item.nativeAgents.includes("claude");
  const error = !skill && item.error;
  const missing = !skill && needsSignIn(item);
  const line = row.line ?? (skill ? always && off ? "Off for Codex workers · Claude workers still have it" : item.description : error ? `Couldn't read it: ${error.charAt(0).toLowerCase()}${error.slice(1)}` : missing ? "Needs a sign-in · no worker can use it yet" : serverCommand(item));
  return <div data-slot="integration-tile" data-setting={`${skill ? "skill" : "server"}:${item.id}`} className={cn("hover:bg-foreground/5 flex min-w-0 items-center gap-3 rounded-xl px-3 py-2.5", highlighted && "bg-foreground/8")}>
    <button type="button" onClick={onOpen} className="flex min-w-0 flex-1 items-center gap-3 text-left" aria-label={`Open ${item.name}${item.project ? ` in ${item.project}` : ""}`}>
      <span className="shrink-0">{skill ? <SkillMark id={item.id} dim={off && !always} /> : <ServerMark server={item} dim={off} />}</span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.75"><span className="flex min-w-0 items-center gap-2"><span className={cn("truncate text-label font-medium leading-tight", off && !always && "text-foreground/50")}>{item.name}</span>{!error && <span className={cn(off && !always && "opacity-50")}><AgentMarks agents={item.nativeAgents} /></span>}{!error && <ScopeChips scope={row.scope} project={project} />}</span><span className={cn("block h-4 overflow-hidden whitespace-nowrap text-xs", off ? always ? "text-foreground/50" : "text-foreground/40" : error ? "text-destructive" : missing ? "text-warning" : "text-foreground/65", !skill && !error && !missing && !row.line && "font-mono")}>{line}</span></span>
    </button>
    {always && <ClaudeAlways compact />}
    {error || row.trailing ? <span className="text-foreground/50 shrink-0 text-xs">{error ? "Not used" : row.trailing}</span> : <SettingsSwitch label={always ? `${item.name}: Codex workers` : item.name} checked={!off} disabled={busy || (!item.canSwitchOff && !always)} onCheckedChange={onToggle} />}
  </div>;
}
function CatalogEmpty({ tab, projects }: { tab: Tab; projects: number }) {
  const skills = tab === "skills";
  const places = skills ? [["Shared", "~/.agents/skills"], ["Claude", "~/.claude/skills"], ["Codex", "~/.codex/skills"], ["Plugins", "Claude Code's and Codex's, where they're on"], [`${projects} trusted projects`, ".claude/skills · .agents/skills"]] : [["Claude Code", "~/.claude.json"], ["Codex", "~/.codex/config.toml"], ["Claude Desktop", "~/Library/Application Support/Claude/claude_desktop_config.json"], ["Cursor", "~/.cursor/mcp.json"], [`${projects} trusted projects`, ".mcp.json · .claude/settings.json · .codex/config.toml · .cursor/mcp.json"]];
  const Icon = skills ? BookOpen : Plugin;
  return <SettingsCard split={false}><div className="flex items-start gap-3 px-4 py-3.5"><Icon className="text-foreground/65 mt-0.25 size-3.5 shrink-0" /><div className="flex min-w-0 flex-col gap-1"><span className="text-label font-medium">No {skills ? "skills" : "MCP servers"} found</span><p className="text-foreground/65 text-xs leading-integration">{skills ? "Add a skill folder to one of these and it shows up here by itself. Brigadier looked in:" : "Set one up in Claude Code, Codex, Claude Desktop or Cursor and it shows up here by itself. Brigadier looked in:"}</p></div></div><div className="pr-4 pb-2 pl-11">{places.map(([name, path]) => <div key={name} className="border-divider flex gap-3 border-t py-1.75 text-xs"><span className="text-foreground/85 w-32 shrink-0">{name}</span><span className="text-foreground/65 min-w-0 break-words font-mono leading-integration">{path}</span></div>)}</div></SettingsCard>;
}
