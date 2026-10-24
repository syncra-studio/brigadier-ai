import { useState, type ReactNode } from "react";

import { Badge } from "@/components/controls";
import { ChevronDown, ChevronRight, Folder, Globe, Plugin, ProviderGlyph, Sparkle, Spinner, Terminal } from "@/components/icons";
import { SettingsCard } from "@/components/shared";
import type { AgentItemState, PluginAgentView, PluginItemView, PluginsView, ProviderKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { has, KIND_LABEL, type Line, type Tone } from "./model";

/** The agents with a mark of their own; any other shows its name's first letter. */
const GLYPHS: readonly string[] = ["claude", "codex"] satisfies ProviderKind[];
const glyphed = (agent: string): agent is ProviderKind => GLYPHS.includes(agent);

const TONE: Record<Tone, string> = { muted: "text-foreground/65", warning: "text-warning", error: "text-destructive" };

/** An agent's small mark, in a row's strip and the Add dialog. */
export function AgentMark({ agent, dim = false }: { agent: PluginAgentView; dim?: boolean }) {
  return (
    <span
      data-slot="agent-mark"
      data-agent={agent.agent}
      data-dim={dim || undefined}
      title={agent.label}
      className={cn("bg-foreground/8 text-foreground/85 flex size-4.5 shrink-0 items-center justify-center rounded-sm text-2xs font-medium", dim && "opacity-20")}
    >
      {glyphed(agent.agent) ? <ProviderGlyph provider={agent.agent} className="size-2.5" /> : agent.label.charAt(0).toUpperCase()}
    </span>
  );
}

/** An agent's tile, beside its name in the details and the dialogs. */
function AgentTile({ agent, dim = false }: { agent: PluginAgentView; dim?: boolean }) {
  if (glyphed(agent.agent)) return <ProviderGlyph provider={agent.agent} tile="md" dim={dim} />;
  return (
    <span
      aria-hidden
      data-slot="agent-mark"
      data-agent={agent.agent}
      className={cn("bg-foreground/8 text-foreground/85 rounded-nav flex size-8.5 shrink-0 items-center justify-center text-sm font-medium", dim && "opacity-50")}
    >
      {agent.label.charAt(0).toUpperCase()}
    </span>
  );
}

/** Every agent's mark, the ones `lit` says no to dimmed. */
function AgentStrip({ view, item, lit }: { view: PluginsView; item: PluginItemView; lit: (state: AgentItemState) => boolean }) {
  const shown = view.agents.filter((agent) => lit(stateOf(item, agent.agent)));
  return (
    <span data-slot="agent-strip" aria-label={shown.length ? `In ${shown.map((agent) => agent.label).join(", ")}` : "In no agent"} className="flex shrink-0 items-center gap-0.75">
      {view.agents.map((agent) => <AgentMark key={agent.agent} agent={agent} dim={!lit(stateOf(item, agent.agent))} />)}
    </span>
  );
}

export const stateOf = (item: PluginItemView, agent: string): AgentItemState =>
  item.agents.find((entry) => entry.agent === agent)?.state ?? { type: "missing" };

/** An item's tile: a skill's sparkle, a plugin's piece, a server's globe (remote) or terminal. */
export function ItemMark({ item, large = false }: { item: Pick<PluginItemView, "kind" | "remote" | "key" | "attention">; large?: boolean }) {
  const Icon = item.kind === "skill" ? (item.key.startsWith("plugin:") ? Plugin : Sparkle) : item.remote ? Globe : Terminal;
  const broken = item.attention?.type === "wontStart";
  return (
    <span className={cn("bg-foreground/5 flex shrink-0 items-center justify-center", large ? "size-11 rounded-xl" : "rounded-toolbar-button size-9")}>
      <Icon aria-hidden className={cn(large ? "size-5" : "size-4", broken ? "text-destructive" : item.kind === "skill" ? "text-file-pink" : "text-foreground/65")} />
    </span>
  );
}

/** A project tag: where it's on, or "Off in steady-app". */
function ScopeChip({ children }: { children: ReactNode }) {
  return (
    <span data-slot="scope-chip" className="bg-foreground/5 text-foreground/65 rounded-capsule flex h-pill shrink-0 items-center gap-1 px-2 text-xs">
      <Folder aria-hidden className="size-3" />
      {children}
    </span>
  );
}

/** One item: its tile, name and kind, a line, its agents, tags, and its switch or button. */
export function PluginRow({ view, item, line, dim = false, chips = [], lit, end, onOpen }: {
  view: PluginsView;
  item: PluginItemView;
  line: Line;
  /** Off where the page looks. */
  dim?: boolean;
  chips?: { text: string }[];
  lit?: (state: AgentItemState) => boolean;
  /** Its switch or button. */
  end: ReactNode;
  onOpen?: () => void;
}) {
  const words = (
    <>
      <ItemMark item={item} />
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="flex min-w-0 items-baseline gap-2">
          <span className="truncate text-label font-medium">{item.name}</span>
          <span data-slot="row-kind" className="text-foreground/50 shrink-0 text-xs">{kindLine(item)}</span>
        </span>
        <span data-slot="row-line" data-tone={line.tone} className={cn("truncate text-xs", TONE[line.tone])}>{line.text}</span>
      </span>
    </>
  );
  return (
    <div data-slot="plugin-row" data-key={item.key} data-off={dim || undefined} data-setting={`plugin:${item.key}`} className="flex min-w-0 items-center gap-3 px-4 py-3">
      {onOpen ? (
        <button type="button" onClick={onOpen} aria-label={`Open ${item.name}`} className={cn("flex min-w-0 flex-1 items-center gap-3 text-start", dim && "opacity-50")}>{words}</button>
      ) : (
        <span className={cn("flex min-w-0 flex-1 items-center gap-3", dim && "opacity-50")}>{words}</span>
      )}
      <span className={cn("flex shrink-0 items-center gap-3", dim && "opacity-50")}>
        <AgentStrip view={view} item={item} lit={dim ? () => false : (lit ?? has)} />
        {chips.map((chip) => <ScopeChip key={chip.text}>{chip.text}</ScopeChip>)}
      </span>
      {end}
    </div>
  );
}

/** "MCP server", "Plugin · 9 skills", "MCP server · in Claude Desktop". */
function kindLine(item: PluginItemView): string {
  const plugin = item.key.startsWith("plugin:") && item.kind === "skill";
  const kind = plugin ? "Plugin" : KIND_LABEL[item.kind];
  if (item.foundIn) return `${kind} · in ${item.foundIn}`;
  return plugin && item.skills > 0 ? `${kind} · ${item.skills} ${item.skills === 1 ? "skill" : "skills"}` : kind;
}

/** A card that folds open: its title and line, then what it holds. */
export function FoldCard({ title, line, initiallyOpen = false, setting, children }: {
  title: string;
  line: string;
  initiallyOpen?: boolean;
  setting?: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(initiallyOpen);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <SettingsCard data-slot="fold" data-setting={setting}>
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)} className="flex w-full items-center gap-4 px-4 py-3 text-start">
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-label font-medium">{title}</span>
          <span className="text-foreground/65 text-xs">{line}</span>
        </span>
        <Chevron aria-hidden className="text-foreground/65 size-icon-sm shrink-0" />
      </button>
      {open && children}
    </SettingsCard>
  );
}

/** How an agent stands with an item, as a pill. */
export function StateBadge({ state }: { state: AgentItemState }) {
  switch (state.type) {
    case "ready": return <Badge variant="success">Ready</Badge>;
    case "needsSignIn": return <Badge variant="warning">Needs sign-in</Badge>;
    case "wontStart": return <Badge variant="destructive">Won't start</Badge>;
    case "off": return <Badge variant="secondary">Off</Badge>;
    case "missing": return <Badge variant="secondary">Not added</Badge>;
    case "notSupported": return <Badge variant="outline">Not supported</Badge>;
    case "working": return <Spinner aria-label="Working" className="text-foreground/65 size-icon-sm animate-spin" />;
  }
}

/** An agent's row: its tile, name and line, and what sits at its end. */
export function AgentRow({ agent, line, tone = "muted", dim = false, end, children, className }: {
  agent: PluginAgentView;
  line: ReactNode;
  tone?: Tone;
  dim?: boolean;
  end?: ReactNode;
  /** Under the line: a note's fix. */
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div data-slot="agent-row" data-agent={agent.agent} className={cn("flex items-center gap-3", className)}>
      <span className={cn("flex min-w-0 flex-1 items-center gap-3", dim && "opacity-50")}>
        <AgentTile agent={agent} />
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-label font-medium">{agent.label}</span>
          <span data-slot="row-line" className={cn("text-xs", TONE[tone])}>{line}</span>
          {children}
        </span>
      </span>
      {end}
    </div>
  );
}

/** One of a choice's options, drawn as the design's radio row. */
export function RadioRow({ checked, title, line, onSelect, end }: { checked: boolean; title: string; line: string; onSelect: () => void; end?: ReactNode }) {
  return (
    <div className="flex items-center gap-3">
      <button type="button" role="radio" aria-checked={checked} onClick={onSelect} className="flex min-w-0 flex-1 items-start gap-3 text-start">
        <span className={cn("mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full", checked ? "bg-toggle-on" : "border-foreground/30 border")}>
          {checked && <span className="bg-foreground size-1.5 rounded-full" />}
        </span>
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-label font-medium">{title}</span>
          <span className="text-foreground/65 text-xs">{line}</span>
        </span>
      </button>
      {end}
    </div>
  );
}
