import { useState, type ReactNode } from "react";

import { SettingsSwitch } from "@/app/settings/parts";
import { ChevronDown, ChevronRight, Folder, Folders, ProviderGlyph, ServerArt, ServerBrand, SkillArt } from "@/components/icons";
import { SettingsCard } from "@/components/shared";
import type { ProviderKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { brandOf } from "./brand";
import type { Line, Tone } from "./model";
import type { PluginItemView, PluginKind } from "./types";
import type { PluginsAdapter } from "./adapter";
import { SignInActions } from "./SignIn";

/** The agents with a mark of their own; any other shows its name's first letter. */
const GLYPHS: readonly string[] = ["claude", "codex"] satisfies ProviderKind[];
const glyphed = (agent: string): agent is ProviderKind => GLYPHS.includes(agent);

const TONE: Record<Tone, string> = { muted: "text-foreground/65", warning: "text-warning", error: "text-destructive" };

/** An agent, as the dialogs name it. */
export type PluginAgent = { agent: string; label: string };

/** An agent's tile, beside its name in the dialogs. */
function AgentTile({ agent, dim = false }: { agent: PluginAgent; dim?: boolean }) {
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

/**
 * An item's mark: its maker's mark for a well-known server, else its site's icon (the daemon
 * fetches it), else the illustration for its kind.
 */
export function PluginMark({ item, large = false }: {
  item: { kind: PluginKind; name: string; detail: string; icon?: string | null };
  large?: boolean;
}) {
  const [failed, setFailed] = useState<string | null>(null);
  const brand = brandOf(item);
  const site = !brand && item.icon && item.icon !== failed ? item.icon : null;
  const size = large ? "size-11" : "size-9";
  if (brand || site) {
    return (
      <span data-slot="plugin-mark" data-mark={brand ?? "icon"} className={cn("bg-foreground/5 flex shrink-0 items-center justify-center", size, large ? "rounded-xl" : "rounded-toolbar-button")}>
        {brand
          ? <ServerBrand brand={brand} className={cn("text-foreground/85", large ? "size-5.5" : "size-4.5")} />
          : site && <img src={site} alt="" onError={() => setFailed(site)} className={cn("rounded-xs", large ? "size-6" : "size-4.5")} />}
      </span>
    );
  }
  const Art = item.kind === "skill" ? SkillArt : ServerArt;
  return <Art data-slot="plugin-mark" data-mark={item.kind} className={size} />;
}

/** Where an item is on, as a tag: "Global", a project's name, "3 projects". */
function ScopeChip({ text }: { text: string }) {
  const Icon = text === "Global" ? Folders : Folder;
  return (
    <span data-slot="scope-chip" className="bg-foreground/5 text-foreground/65 rounded-capsule flex h-pill w-fit shrink-0 items-center gap-1 px-2 text-xs">
      <Icon aria-hidden className="size-3" />
      {text}
    </span>
  );
}

/** One item in the list's grid: its mark, name, line and where it's on; its switch at the corner. */
export function PluginCard({ item, line, chip, busy, adapter, onOpen, onSwitch }: {
  item: PluginItemView;
  line: Line;
  chip: string | null;
  busy: boolean;
  adapter: PluginsAdapter | null;
  onOpen: () => void;
  onSwitch: (on: boolean) => void;
}) {
  const on = item.on;
  // How it runs (a command, an address or a folder) is in code's letters.
  const code = line.text === item.detail && !item.problem;
  return (
    <SettingsCard data-slot="plugin-row" data-key={item.key} data-off={!on || undefined} data-setting={`plugin:${item.key}`} split={false} className="flex-row items-start gap-3 px-4 py-3.5">
      <div className="min-w-0 flex-1">
        <button type="button" onClick={onOpen} aria-label={`Open ${item.name}`} className={cn("flex w-full min-w-0 items-start gap-3 text-start", !on && "opacity-50")}>
          <PluginMark item={item} />
          <span className="flex min-w-0 flex-1 flex-col gap-1">
            <span data-slot="row-name" className="truncate text-label font-medium">{item.name}</span>
            {line.text && <span data-slot="row-line" data-tone={line.tone} className={cn("truncate text-xs", TONE[line.tone], code && "font-mono")}>{line.text}</span>}
            {chip && <span className="pt-1.5"><ScopeChip text={chip} /></span>}
          </span>
        </button>
        <div className="pl-12"><SignInActions item={item} adapter={adapter} disabled={busy} /></div>
      </div>
      <SettingsSwitch label={item.name} checked={on} disabled={busy} onCheckedChange={onSwitch} />
    </SettingsCard>
  );
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

/** An agent's row: its tile, name and line, and what sits at its end. */
export function AgentRow({ agent, line, tone = "muted", dim = false, end, children, className }: {
  agent: PluginAgent;
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
export function RadioRow({ checked, title, line, onSelect, end, disabled = false }: { checked: boolean; title: string; line: string; onSelect: () => void; end?: ReactNode; disabled?: boolean }) {
  return (
    <div className="flex items-center gap-3">
      <button type="button" role="radio" aria-checked={checked} disabled={disabled} onClick={onSelect} className="flex min-w-0 flex-1 items-start gap-3 text-start disabled:opacity-50">
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
