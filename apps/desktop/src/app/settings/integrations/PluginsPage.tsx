import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { BookOpen, Folder, Plugin, ShieldCheck } from "@/components/icons";
import { SettingsCard, SettingsSearch, StateDot } from "@/components/shared";
import { cn } from "@/lib/utils";
import { useIntegrations, type IntegrationAdapter, type IntegrationSnapshot } from "./adapter";
import { AgentMarks, ClaudeAlways, SkillMark } from "./parts";
import { ServerMark, ServerPage, serverCommand } from "./ServerPage";
import { needsSignIn } from "./SignInSection";
import { SkillPage } from "./SkillPage";
import type { IntegrationServer, IntegrationSkill } from "./types";

export const PLUGINS_DESCRIPTION = "MCP servers and skills Brigadier found on this Mac. It only reads them: add, remove or change one in that agent's own setup.";
export const PLUGINS_ROWS = [
  { label: "MCP servers", description: "Claude Code, Codex, Claude Desktop and Cursor, for all your work" },
  { label: "Skills", description: "Every worker gets every skill, whichever agent it came from" },
  { label: "What it is", target: "MCP servers" },
  { label: "Found in", description: "Read-only · change it there", target: "MCP servers" },
  { label: "Sign-in", target: "MCP servers" },
  { label: "Workers", target: "MCP servers" },
  { label: "Folders", target: "Skills" },
] as const;

export function integrationSearchRows() {
  const { catalog } = useIntegrations.getState().snapshot;
  return [
    ...PLUGINS_ROWS,
    ...catalog.servers.map((item) => ({ label: item.name, description: serverCommand(item), target: `server:${item.id}`, section: "MCP servers" })),
    ...catalog.skills.map((item) => ({ label: item.name, description: item.description, target: `skill:${item.id}`, section: "Skills" })),
  ];
}

export function PluginsPage() {
  const { snapshot, adapter, error } = useIntegrations();
  return <PluginsView snapshot={snapshot} adapter={adapter} error={error} />;
}

type Tab = "servers" | "skills";
type Selection = { tab: Tab; id: string; signIn?: boolean } | null;
export function PluginsView({ snapshot, adapter, error, initialTab = "servers", initialId }: {
  snapshot: IntegrationSnapshot; adapter: IntegrationAdapter | null; error?: string | null; initialTab?: Tab; initialId?: string;
}) {
  const [tab, setTab] = useState<Tab>(() => {
    const label = useReveal.getState().label;
    return label === "Skills" || label?.startsWith("skill:") ? "skills" : initialTab;
  });
  const [selected, setSelected] = useState<Selection>(initialId ? { tab: initialTab, id: initialId } : null);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState<string | null>(() => useReveal.getState().label);
  const action = useAction();
  const { catalog, switches } = snapshot;
  useEffect(() => useReveal.subscribe(({ label: reveal }) => {
    if (!reveal) return;
    if (reveal === "Skills" || reveal.startsWith("skill:")) {
      setTab("skills"); setSelected(null); setQuery(""); setHighlight(reveal);
    } else if (reveal === "MCP servers" || reveal.startsWith("server:")) {
      setTab("servers"); setSelected(null); setQuery(""); setHighlight(reveal);
    }
  }), []);
  const toggleServer = (id: string, value: boolean) => { if (adapter) action.run(() => adapter.setServerEnabled(id, value)); };
  const toggleSkill = (id: string, value: boolean) => { if (adapter) action.run(() => adapter.setSkillEnabled(id, value)); };
  const show = (path: string) => { if (adapter) action.run(() => adapter.showPath(path)); };
  const server = selected?.tab === "servers" ? catalog.servers.find((item) => item.id === selected.id) : undefined;
  const skill = selected?.tab === "skills" ? catalog.skills.find((item) => item.id === selected.id) : undefined;
  const back = () => setSelected(null);
  let page;
  if (server) page = <ServerPage server={server} enabled={!switches.mcpServersOff.includes(server.id)} busy={action.busy || !adapter} adapter={adapter} focusSignIn={selected?.signIn} run={action.run} onEnabled={(value) => toggleServer(server.id, value)} onBack={back} onShowPath={show} />;
  else if (skill) page = <SkillPage skill={skill} skills={catalog.skills} enabled={!switches.skillsOff.includes(skill.id)} busy={action.busy || !adapter} onEnabled={(value) => toggleSkill(skill.id, value)} onBack={back} onShowPath={show} />;
  else {
    const items = tab === "servers" ? catalog.servers : catalog.skills;
    const filtered = items.filter((item) => `${item.name} ${"description" in item ? item.description : serverCommand(item)} ${item.project ?? ""}`.toLowerCase().includes(query.toLowerCase().trim()));
    const projects = [...new Set(filtered.map((item) => item.project).filter((project): project is string => !!project))];
    const groups = [undefined, ...projects];
    const restricted = tab === "skills" && catalog.skills.some((item) => !item.canSwitchOff && item.nativeAgents.includes("claude"));
    page = <SettingsPage title="Plugins" description={<><span className="hidden @lg:block">{PLUGINS_DESCRIPTION}</span><span className="@lg:hidden">MCP servers and skills Brigadier found on this Mac. It only reads them: change them in each agent's own setup.</span></>} className="max-w-plugins pt-0" sectionsClassName="gap-8 pt-2" actions={adapter && !error ? <span className="text-muted-foreground flex items-center gap-1.5 text-label"><StateDot tone="ready" />Up to date</span> : undefined}>
      <div className="flex items-center justify-between gap-3 pt-1 @lg:gap-4">
        <div role="tablist" aria-label="Plugins" className="flex shrink-0 items-center gap-1">
          {(["servers", "skills"] as const).map((value) => <button key={value} type="button" role="tab" aria-selected={tab === value} data-setting={value === "servers" ? "MCP servers" : "Skills"} aria-controls="integrations-list" className={cn("rounded-capsule flex h-7 items-center gap-1.5 px-3 text-label", tab === value ? "bg-foreground/10" : "text-foreground/65")} onClick={() => { setTab(value); setQuery(""); setHighlight(null); }} onKeyDown={(event) => {
            if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
            event.preventDefault();
            const next = event.key === "Home" ? "servers" : event.key === "End" ? "skills" : value === "servers" ? "skills" : "servers";
            setTab(next); setQuery("");
            const tabs = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("[role=tab]");
            tabs?.[next === "servers" ? 0 : 1]?.focus();
          }} tabIndex={tab === value ? 0 : -1}>{value === "servers" ? "MCP servers" : "Skills"}<span className="text-foreground/50">{value === "servers" ? catalog.servers.length : catalog.skills.length}</span></button>)}
        </div>
        <div className="min-w-0 flex-1 @lg:max-w-60"><SettingsSearch className="@lg:hidden" label={tab === "servers" ? "Search MCP servers" : "Search skills"} placeholder="Search" value={query} onChange={setQuery} /><SettingsSearch className="hidden @lg:flex" label={tab === "servers" ? "Search MCP servers" : "Search skills"} value={query} onChange={setQuery} /></div>
      </div>
      <div id="integrations-list" role="tabpanel" aria-label={tab === "servers" ? "MCP servers" : "Skills"} className="flex flex-col gap-8">
        {groups.map((project) => {
          const members = filtered.filter((item) => item.project === project);
          if (project && members.length === 0) return null;
          return <section key={project ?? "yours"} aria-label={project ?? "Yours"} className="flex flex-col gap-2.5">
            <header className="flex items-center justify-between gap-4">
              <div className="flex min-w-0 flex-1 flex-col gap-0.5"><h2 className="flex items-center gap-2 text-sm font-medium">{project && <Folder className="text-muted-foreground size-4 shrink-0" />}<span className="truncate">{project?.split("/").pop() ?? "Yours"}</span></h2><p className="text-foreground/65 break-words text-label">{project ? `${project} · only in tasks for this project` : tab === "servers" ? PLUGINS_ROWS[0].description : restricted ? "Claude can't hide skills in ~/.claude/skills for one session: there, the switch only decides Codex workers" : PLUGINS_ROWS[1].description}</p></div>
              <span className={cn("text-muted-foreground hidden shrink-0 text-label @lg:block", !items.length && "@lg:hidden")}>{project ? `${members.length} ${tab === "servers" ? members.length === 1 ? "server" : "servers" : members.length === 1 ? "skill" : "skills"}` : "Switches apply to new and resumed sessions"}</span>
            </header>
            {members.length ? <div className="grid grid-cols-1 gap-x-2 gap-y-0.5 @lg:grid-cols-2">{members.map((item) => {
              const isSkill = "description" in item;
              const off = (isSkill ? switches.skillsOff : switches.mcpServersOff).includes(item.id);
              return <IntegrationTile key={item.id} item={item} off={off} highlighted={highlight === `${isSkill ? "skill" : "server"}:${item.id}`} busy={action.busy || !adapter} onOpen={() => setSelected({ tab, id: item.id, ...(!isSkill && needsSignIn(item) ? { signIn: true } : {}) })} onToggle={(value) => isSkill ? toggleSkill(item.id, value) : toggleServer(item.id, value)} />;
            })}</div> : <CatalogEmpty tab={tab} projects={catalog.trustedProjectCount ?? 0} />}
          </section>;
        })}
        {catalog.untrustedProjects?.length ? <div className="text-foreground/50 flex items-start gap-2 px-1 text-xs leading-integration"><ShieldCheck className="mt-0.25 size-3.5 shrink-0" /><p>Brigadier reads only projects you've trusted. {catalog.untrustedProjects.join(" and ")} aren't trusted, so their {tab === "servers" ? "servers" : "skills"} aren't read.</p></div> : null}
      </div>
    </SettingsPage>;
  }
  return <div className="@container relative h-full min-w-0 [&_[data-tauri-drag-region]]:h-11">{page}{(action.error || error) && <p role="alert" className="text-destructive bg-background absolute right-5 bottom-4 left-5 text-xs">{action.error ?? error}</p>}</div>;
}

function IntegrationTile({ item, off, busy, highlighted, onOpen, onToggle }: {
  item: IntegrationServer | IntegrationSkill; off: boolean; busy: boolean; highlighted: boolean; onOpen: () => void; onToggle: (enabled: boolean) => void;
}) {
  const skill = "description" in item;
  const always = skill && !item.canSwitchOff && item.nativeAgents.includes("claude");
  const error = !skill && item.error;
  const missing = !skill && needsSignIn(item);
  const line = skill ? always && off ? "Off for Codex workers · Claude workers still have it" : item.description : error ? `Couldn't read it: ${error.charAt(0).toLowerCase()}${error.slice(1)}` : missing ? "Needs a sign-in · no worker can use it yet" : serverCommand(item);
  return <div data-slot="integration-tile" data-setting={`${skill ? "skill" : "server"}:${item.id}`} className={cn("hover:bg-foreground/5 flex min-w-0 items-center gap-3 rounded-xl px-3 py-2.5", highlighted && "bg-foreground/8")}>
    <button type="button" onClick={onOpen} className="flex min-w-0 flex-1 items-center gap-3 text-left" aria-label={`Open ${item.name}${item.project ? ` in ${item.project}` : ""}`}>
      <span className="shrink-0">{skill ? <SkillMark id={item.id} dim={off && !always} /> : <ServerMark server={item} dim={off} />}</span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.75"><span className="flex min-w-0 items-center gap-2"><span className={cn("truncate text-label font-medium leading-tight", off && !always && "text-foreground/50")}>{item.name}</span>{!error && <span className={cn(off && !always && "opacity-50")}><AgentMarks agents={item.nativeAgents} /></span>}</span><span className={cn("block h-4 overflow-hidden whitespace-nowrap text-xs", off ? always ? "text-foreground/50" : "text-foreground/40" : error ? "text-destructive" : missing ? "text-warning" : "text-foreground/65", !skill && !error && !missing && "font-mono")}>{line}</span></span>
    </button>
    {always && <ClaudeAlways compact />}
    {error ? <span className="text-foreground/50 shrink-0 text-xs">Not used</span> : <SettingsSwitch label={always ? `${item.name}: Codex workers` : item.name} checked={!off} disabled={busy || (!item.canSwitchOff && !always)} onCheckedChange={onToggle} />}
  </div>;
}
function CatalogEmpty({ tab, projects }: { tab: Tab; projects: number }) {
  const skills = tab === "skills";
  const places = skills ? [["Shared", "~/.agents/skills"], ["Claude", "~/.claude/skills"], ["Codex", "~/.codex/skills"], [`${projects} trusted projects`, ".claude/skills · .agents/skills"]] : [["Claude Code", "~/.claude.json"], ["Codex", "~/.codex/config.toml"], ["Claude Desktop", "~/Library/Application Support/Claude/claude_desktop_config.json"], ["Cursor", "~/.cursor/mcp.json"], [`${projects} trusted projects`, ".mcp.json · .claude/settings.json · .codex/config.toml · .cursor/mcp.json"]];
  const Icon = skills ? BookOpen : Plugin;
  return <SettingsCard split={false}><div className="flex items-start gap-3 px-4 py-3.5"><Icon className="text-foreground/65 mt-0.25 size-3.5 shrink-0" /><div className="flex min-w-0 flex-col gap-1"><span className="text-label font-medium">No {skills ? "skills" : "MCP servers"} found</span><p className="text-foreground/65 text-xs leading-integration">{skills ? "Add a skill folder to one of these and it shows up here by itself. Brigadier looked in:" : "Set one up in Claude Code, Codex, Claude Desktop or Cursor and it shows up here by itself. Brigadier looked in:"}</p></div></div><div className="pr-4 pb-2 pl-11">{places.map(([name, path]) => <div key={name} className="border-divider flex gap-3 border-t py-1.75 text-xs"><span className="text-foreground/85 w-32 shrink-0">{name}</span><span className="text-foreground/65 min-w-0 break-words font-mono leading-integration">{path}</span></div>)}</div></SettingsCard>;
}
