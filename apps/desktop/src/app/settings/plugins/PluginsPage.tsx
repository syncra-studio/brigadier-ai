import { useEffect, useState, type KeyboardEvent } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SettingsPage, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { Button, SettingsSelectTrigger } from "@/components/controls";
import { Check, Folder, FolderPlus, Folders, Sparkle, Terminal } from "@/components/icons";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays";
import { SettingsCard, SettingsSearch } from "@/components/shared";
import type { PluginItemView, PluginKind, PluginProjectView, PluginsView } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { AddDialog, ADD_HINT } from "./AddDialog";
import { usePlugins, type PluginsAdapter } from "./adapter";
import { DetailsPage } from "./DetailsPage";
import { checked, RemoveDialog, SignInDialog, UseEverywhereDialog } from "./dialogs";
import { elsewhereLine, has, placesOf, projectSummary, sections, TAB_LABEL, type Row } from "./model";
import { FoldCard, PluginRow } from "./parts";

const PLUGINS_DESCRIPTION = "Skills and MCP servers your agents can use. Pick a project to choose what's on there.";

/** What Settings search finds on the page: its tabs, then each item and project. */
export function pluginSearchRows() {
  const { view } = usePlugins.getState();
  return [
    { label: "MCP servers", description: "Servers every agent can use, in every project or only some" },
    { label: "Skills", description: "Skills every agent can use, in every project or only some" },
    { label: "Found in other apps", description: "Servers only another app has", target: "Found in other apps" },
    ...view.items.map((item) => ({ label: item.name, description: item.description || item.detail, target: `plugin:${item.key}`, section: TAB_LABEL[item.kind] })),
  ];
}

/** A dialog open over the page. */
type Open =
  | { type: "add"; projectId: string | null }
  | { type: "signIn"; key: string }
  | { type: "useEverywhere"; key: string; after?: { projectId: string | null; on: boolean } }
  | { type: "remove"; key: string };

export function PluginsPage() {
  const { view, project, adapter, error, refresh, setProject } = usePlugins();
  return <PluginsScreen view={view} projectId={project} adapter={adapter} error={error} refresh={refresh} onProject={setProject} />;
}

const TABS: PluginKind[] = ["server", "skill"];
const revealTab = (label: string | null): PluginKind => label === "Skills" ? "skill" : "server";

function PluginsScreen({ view, projectId, adapter, error, refresh, onProject }: {
  view: PluginsView;
  projectId: string | null;
  adapter: PluginsAdapter | null;
  error: string | null;
  refresh: () => void;
  onProject: (projectId: string | null) => void;
}) {
  const [tab, setTab] = useState<PluginKind>(() => revealTab(useReveal.getState().label));
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [open, setOpen] = useState<Open | null>(null);
  const action = useAction();
  useEffect(() => useReveal.subscribe(({ label }) => {
    if (label === "Skills" || label === "MCP servers") {
      setTab(revealTab(label));
      setSelected(null);
      setQuery("");
    } else if (label?.startsWith("plugin:")) {
      const item = usePlugins.getState().view.items.find((entry) => `plugin:${entry.key}` === label);
      if (item) {
        setTab(item.kind);
        setSelected(null);
        setQuery("");
      }
    }
  }), []);
  const focus = view.projects.find((project) => project.id === projectId) ?? null;
  const find = (key: string) => view.items.find((item) => item.key === key) ?? view.elsewhere.find((item) => item.key === key);
  const busy = action.busy || !adapter;

  /** The row switch: kept to a project or off there, else everywhere; an item that can't be kept to a project goes into the Library first. */
  const flip = (item: PluginItemView, on: boolean, where: string | null = focus?.id ?? null) => {
    if (!adapter) return;
    if (!item.switchable) {
      setOpen({ type: "useEverywhere", key: item.key, after: { projectId: where, on } });
      return;
    }
    action.run(async () => {
      await checked(view, () => adapter.switchPlugin(item.key, where, on));
      refresh();
    });
  };

  /** A Library item some agents are missing: each of them gets it where the others have it. */
  const addMissing = (item: PluginItemView) => {
    if (!adapter) return;
    action.run(async () => {
      for (const { agent } of item.agents.filter((entry) => entry.state.type === "missing")) {
        for (const where of placesOf(view, item)) await checked(view, () => adapter.switchPlugin(item.key, where, true, agent));
      }
      refresh();
    });
  };

  const current = selected ? find(selected) : undefined;
  let page;
  if (current && adapter) {
    page = (
      <DetailsPage
        view={view}
        item={current}
        focus={focus}
        actions={{
          busy,
          onBack: () => setSelected(null),
          onSwitch: (where, on) => flip(current, on, where),
          onSwitchAgent: (agent, on) => action.run(async () => {
            await checked(view, () => adapter.switchPlugin(current.key, focus?.id ?? null, on, agent));
            refresh();
          }),
          onSignIn: adapter.signIn ? () => setOpen({ type: "signIn", key: current.key }) : undefined,
          onUseEverywhere: () => setOpen({ type: "useEverywhere", key: current.key }),
          onAddMissing: () => addMissing(current),
          onRemove: () => setOpen({ type: "remove", key: current.key }),
          onRetry: refresh,
          onUpdate: () => action.run(async () => {
            await checked(view, () => adapter.update(current.key));
            refresh();
          }),
          onTrust: (id) => action.run(async () => {
            await adapter.trustProject(id);
            refresh();
          }),
        }}
      />
    );
  } else {
    page = (
      <SettingsPage
        title="Plugins"
        description={PLUGINS_DESCRIPTION}
        actions={<Button size="sm" disabled={!adapter} onClick={() => setOpen({ type: "add", projectId: focus?.id ?? null })}>Add</Button>}
      >
        <ListBody
          view={view}
          focus={focus}
          tab={tab}
          query={query}
          busy={busy}
          adapter={adapter}
          onTab={(next) => {
            setTab(next);
            setQuery("");
          }}
          onQuery={setQuery}
          onProject={onProject}
          onOpen={(item) => setSelected(item.key)}
          onFlip={(item, on) => flip(item, on)}
          onDialog={setOpen}
        />
      </SettingsPage>
    );
  }

  const target = open && open.type !== "add" ? find(open.key) : undefined;
  const close = () => setOpen(null);
  return (
    <div className="@container relative h-full min-w-0 [&_[data-tauri-drag-region]]:h-11">
      {page}
      {adapter && open?.type === "add" && <AddDialog view={view} adapter={adapter} projectId={open.projectId} refresh={refresh} onClose={close} />}
      {adapter && open?.type === "signIn" && target && <SignInDialog view={view} item={target} projectId={focus?.id ?? null} adapter={adapter} refresh={refresh} onClose={close} />}
      {adapter && open?.type === "useEverywhere" && target && (
        <UseEverywhereDialog
          view={view}
          item={target}
          adapter={adapter}
          onClose={close}
          onDone={() => {
            const after = open.after;
            if (!after) return refresh();
            // The switch the user flipped, now on the Library's copy.
            action.run(async () => {
              const fresh = await adapter.read(focus?.id ?? null);
              const copy = fresh.items.find((item) => item.library && item.name === target.name && item.kind === target.kind);
              if (!copy) throw new Error(`${target.name} isn't in the library yet. Try the switch again in a moment.`);
              await checked(fresh, () => adapter.switchPlugin(copy.key, after.projectId, after.on));
              refresh();
            });
          }}
        />
      )}
      {adapter && open?.type === "remove" && target && (
        <RemoveDialog
          view={view}
          item={target}
          adapter={adapter}
          onClose={close}
          onRemoved={() => {
            setSelected(null);
            refresh();
          }}
        />
      )}
      {(action.error || error) && <p role="alert" className="text-destructive bg-background absolute right-5 bottom-4 left-5 text-xs whitespace-pre-wrap">{action.error ?? error}</p>}
    </div>
  );
}

function ListBody({ view, focus, tab, query, busy, adapter, onTab, onQuery, onProject, onOpen, onFlip, onDialog }: {
  view: PluginsView;
  focus: PluginProjectView | null;
  tab: PluginKind;
  query: string;
  busy: boolean;
  adapter: PluginsAdapter | null;
  onTab: (tab: PluginKind) => void;
  onQuery: (query: string) => void;
  onProject: (projectId: string | null) => void;
  onOpen: (item: PluginItemView) => void;
  onFlip: (item: PluginItemView, on: boolean) => void;
  onDialog: (open: Open) => void;
}) {
  const count = (kind: PluginKind) => view.items.filter((item) => item.kind === kind).length;
  const elsewhere = view.elsewhere.filter((item) => item.kind === tab);
  const lists = sections(view, focus, tab, query);
  const empty = count(tab) === 0;
  const row = (entry: Row, dim: boolean) => (
    <PluginRow
      key={entry.item.key}
      view={view}
      item={entry.item}
      line={entry.line}
      dim={dim}
      chips={entry.chips}
      onOpen={() => onOpen(entry.item)}
      end={
        <>
          {entry.item.attention?.type === "signIn" && adapter?.signIn && <Button variant="soft" size="sm" onClick={() => onDialog({ type: "signIn", key: entry.item.key })}>Sign in</Button>}
          <SettingsSwitch label={entry.item.name} checked={entry.on} disabled={busy} onCheckedChange={(on) => onFlip(entry.item, on)} />
        </>
      }
    />
  );
  return (
    <div className="flex flex-col gap-10">
      <Toolbar view={view} focus={focus} tab={tab} query={query} count={count} onTab={onTab} onQuery={onQuery} onProject={onProject} onAddProject={() => adapter?.addProject()} />
      {empty && !focus ? (
        <EmptyTab tab={tab} onAdd={() => onDialog({ type: "add", projectId: null })} />
      ) : (
        lists.map((section) => {
          if (!section.rows.length && section.key !== "own") return null;
          return (
            <SettingsSection key={section.key} title={section.title} description={section.description} aside={section.rows.length || undefined} setting={section.title}>
              <SettingsCard>
                {section.rows.length ? (
                  section.rows.map((entry) => row(entry, !entry.on))
                ) : (
                  <div data-slot="empty-row" className="flex items-center gap-4 px-4 py-4">
                    <span className="text-foreground/65 min-w-0 flex-1 text-label">Nothing only for {focus?.name} yet. Add one here, or turn one on below.</span>
                    <Button variant="soft" size="sm" disabled={!adapter} onClick={() => onDialog({ type: "add", projectId: focus?.id ?? null })}>Add to {focus?.name}</Button>
                  </div>
                )}
              </SettingsCard>
            </SettingsSection>
          );
        })
      )}
      {!focus && elsewhere.length > 0 && (
        <FoldCard title="Found in other apps" line={elsewhereLine(elsewhere)} setting="Found in other apps">
          {elsewhere.map((item) => (
            <PluginRow
              key={item.key}
              view={view}
              item={item}
              line={{ text: item.description || item.detail, tone: "muted" }}
              lit={has}
              end={<Button variant="soft" size="sm" disabled={!adapter} onClick={() => onDialog({ type: "useEverywhere", key: item.key })}>Add to every agent</Button>}
            />
          ))}
        </FoldCard>
      )}
    </div>
  );
}

function Toolbar({ view, focus, tab, query, count, onTab, onQuery, onProject, onAddProject }: {
  view: PluginsView;
  focus: PluginProjectView | null;
  tab: PluginKind;
  query: string;
  count: (kind: PluginKind) => number;
  onTab: (tab: PluginKind) => void;
  onQuery: (query: string) => void;
  onProject: (projectId: string | null) => void;
  onAddProject: () => void;
}) {
  const keys = (event: KeyboardEvent<HTMLButtonElement>, kind: PluginKind) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const at = TABS.indexOf(kind);
    const next = event.key === "Home" ? 0 : event.key === "End" ? TABS.length - 1 : (at + (event.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length;
    onTab(TABS[next]!);
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("[role=tab]")[next]?.focus();
  };
  return (
    <div data-slot="plugins-toolbar" className="flex items-center gap-3">
      <ProjectFilter view={view} focus={focus} onProject={onProject} onAddProject={onAddProject} />
      <span aria-hidden className="bg-divider h-5 w-px shrink-0" />
      <div role="tablist" aria-label="Plugins" className="flex shrink-0 items-center gap-1">
        {TABS.map((kind) => (
          <button
            key={kind}
            type="button"
            role="tab"
            aria-selected={tab === kind}
            tabIndex={tab === kind ? 0 : -1}
            data-setting={TAB_LABEL[kind]}
            className={cn("rounded-capsule flex h-7 items-center gap-1.5 px-3 text-label", tab === kind ? "bg-foreground/10" : "text-foreground/65")}
            onClick={() => onTab(kind)}
            onKeyDown={(event) => keys(event, kind)}
          >
            {TAB_LABEL[kind]}
            <span className="text-foreground/50">{count(kind)}</span>
          </button>
        ))}
      </div>
      <span className="flex-1" />
      <SettingsSearch className="max-w-60" label={tab === "server" ? "Search MCP servers" : "Search skills"} value={query} onChange={onQuery} />
    </div>
  );
}

/** The project filter: All projects, or one project, each with what it has. */
function ProjectFilter({ view, focus, onProject, onAddProject }: {
  view: PluginsView;
  focus: PluginProjectView | null;
  onProject: (projectId: string | null) => void;
  onAddProject: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [find, setFind] = useState("");
  const shown = view.projects.filter((project) => `${project.name} ${project.path}`.toLowerCase().includes(find.trim().toLowerCase()));
  const pick = (projectId: string | null) => {
    setOpen(false);
    onProject(projectId);
  };
  return (
    <Popover
      open={open}
      onOpenChange={(value) => {
        setOpen(value);
        setFind("");
      }}
    >
      <PopoverTrigger asChild>
        <SettingsSelectTrigger label="Project" icon={<Folder aria-hidden className="text-foreground/65 size-icon-sm" />} className="h-8 w-44">
          {focus?.name ?? "All projects"}
        </SettingsSelectTrigger>
      </PopoverTrigger>
      <PopoverContent align="start" aria-label="Projects" data-slot="project-filter" className="flex max-h-(--radix-popover-content-available-height) w-75 flex-col gap-0.5 p-1.5">
        <SettingsSearch variant="bordered" label="Find a project" value={find} onChange={setFind} clearOnEscape={false} className="border-divider rounded-none border-0 border-b" />
        <FilterRow icon={Folders} title="All projects" line="What every project gets" selected={!focus} onPick={() => pick(null)} />
        <p className="text-foreground/50 px-2.5 pt-1.5 pb-1 text-xs">Projects</p>
        <div className="flex min-h-0 flex-col gap-0.5 overflow-y-auto">
          {shown.map((project) => (
            <FilterRow key={project.id} icon={Folder} title={project.name} line={projectSummary(view, project)} selected={focus?.id === project.id} onPick={() => pick(project.id)} />
          ))}
        </div>
        <div className="border-divider border-t pt-0.5">
          <button
            type="button"
            onClick={() => {
              setOpen(false);
              onAddProject();
            }}
            className="rounded-control hover:bg-foreground/5 flex h-9 w-full items-center gap-2.5 px-2.5 text-start text-label"
          >
            <FolderPlus aria-hidden className="text-foreground/65 size-icon-sm" />
            Add project…
          </button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function FilterRow({ icon: Icon, title, line, selected, onPick }: { icon: typeof Folder; title: string; line: string; selected: boolean; onPick: () => void }) {
  return (
    <button type="button" aria-current={selected || undefined} onClick={onPick} className={cn("rounded-control hover:bg-foreground/5 flex w-full shrink-0 items-center gap-2.5 px-2.5 py-1.5 text-start", selected && "bg-foreground/5")}>
      <Icon aria-hidden className="text-foreground/65 size-icon-sm shrink-0" />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-label">{title}</span>
        <span className="text-foreground/50 truncate text-xs">{line}</span>
      </span>
      {selected && <Check aria-hidden className="size-icon-sm shrink-0" />}
    </button>
  );
}

/** L7: a tab with nothing in it yet. */
function EmptyTab({ tab, onAdd }: { tab: PluginKind; onAdd: () => void }) {
  const [help, setHelp] = useState(false);
  const Icon = tab === "server" ? Terminal : Sparkle;
  return (
    <SettingsCard data-slot="plugins-empty" split={false} className="items-center gap-2 px-6 py-12 text-center">
      <span className="bg-foreground/5 rounded-toolbar-button flex size-11 items-center justify-center">
        <Icon aria-hidden className="text-foreground/65 size-5" />
      </span>
      <span className="pt-2 text-sm font-medium">No {TAB_LABEL[tab] === "Skills" ? "skills" : "MCP servers"} yet</span>
      <span className="text-foreground/65 text-label">Add one once and every agent can use it, in all your projects or just one.</span>
      <span className="flex items-center gap-3 pt-3">
        <Button size="sm" onClick={onAdd}>Add</Button>
        <Button variant="ghost" size="sm" aria-expanded={help} onClick={() => setHelp(!help)}>What can I add?</Button>
      </span>
      {help && <span className="text-foreground/65 max-w-md pt-2 text-xs">{ADD_HINT}</span>}
    </SettingsCard>
  );
}
