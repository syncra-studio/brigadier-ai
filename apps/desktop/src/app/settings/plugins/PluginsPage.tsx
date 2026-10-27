import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";

import { SettingsPage } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { Button } from "@/components/controls";
import { Plus, Sparkle, Terminal } from "@/components/icons";
import { SettingsCard, SettingsSearch } from "@/components/shared";
import { cn } from "@/lib/utils";
import { openSettings, select } from "@/state/actions";
import { useApp } from "@/state/store";
import { AddDialog, ADD_HINT } from "./AddDialog";
import { usePlugins, type PluginAgentResult, type PluginsAdapter } from "./adapter";
import { Changes, type Overlay } from "./changes";
import { DetailsPage } from "./DetailsPage";
import { checked, RemoveDialog } from "./dialogs";
import { FileDialog } from "./FileDialog";
import { chipOf, lineOf, listed, TAB_LABEL } from "./model";
import { PluginCard } from "./parts";
import type { PluginItemView, PluginKind, PluginsView } from "./types";

const PLUGINS_DESCRIPTION = "Skills and MCP servers your agents can use.";

/** What Settings search finds on the page: its tabs, then each item. */
export function pluginSearchRows() {
  const { view } = usePlugins.getState();
  return [
    { label: "MCP servers", description: "Servers every agent can use, in every project or only some" },
    { label: "Skills", description: "Skills every agent can use, in every project or only some" },
    ...view.items.map((item) => ({ label: item.name, description: item.description || item.detail, target: `plugin:${item.key}`, section: TAB_LABEL[item.kind] })),
  ];
}

/** A dialog open over the page. */
type Open =
  | { type: "add" }
  | { type: "remove"; key: string }
  | { type: "file"; key: string };

export function PluginsPage() {
  const { view, adapter, error, refresh, loading } = usePlugins();
  useEffect(refresh, [refresh]);
  return <PluginsScreen view={view} adapter={adapter} error={error} refresh={refresh} loading={loading} />;
}

const TABS: PluginKind[] = ["server", "skill"];
const revealTab = (label: string | null): PluginKind => label === "Skills" ? "skill" : "server";

function PluginsScreen({ view: read, adapter, error, refresh, loading }: {
  view: PluginsView;
  adapter: PluginsAdapter | null;
  error: string | null;
  refresh: () => void;
  loading: boolean;
}) {
  const [tab, setTab] = useState<PluginKind>(() => revealTab(useReveal.getState().label));
  const [query, setQuery] = useState("");
  const selection = useApp((s) => s.selection);
  const selected = selection.type === "settings" && selection.page === "plugins" ? selection.detail?.plugin : undefined;
  const [open, setOpen] = useState<Open | null>(null);
  const liveDialog = useRef<Open | null>(null);
  const setDialog = useCallback((dialog: Open | null) => {
    liveDialog.current = dialog;
    setOpen(dialog);
  }, []);
  useEffect(() => {
    // Leaving a visit invalidates its dialog even if Back restores the same selection object.
    const unsubscribe = useApp.subscribe((state, previous) => {
      if (state.selection !== previous.selection) setDialog(null);
    });
    return () => {
      liveDialog.current = null;
      unsubscribe();
    };
  }, [setDialog]);
  // Each change shows at once; its agents take it meanwhile. It reads the page again through
  // the connection there is when it's done.
  const [changes] = useState(() => new Changes(() => usePlugins.getState().reload()));
  useSyncExternalStore(changes.subscribe, changes.snapshot);
  // Where a change is asked for: its failure shows there only, and goes once the user leaves.
  const place = selected ? `item:${selected}` : "list";
  useEffect(() => changes.leave(place), [changes, place]);
  const view = changes.apply(read);
  useEffect(() => useReveal.subscribe(({ label }) => {
    if (label === "Skills" || label === "MCP servers") {
      setTab(revealTab(label));
      setQuery("");
    } else if (label?.startsWith("plugin:")) {
      const item = usePlugins.getState().view.items.find((entry) => `plugin:${entry.key}` === label);
      if (item) {
        setTab(item.kind);
        setQuery("");
      }
    }
  }), []);
  const find = (key: string) => view.items.find((item) => item.key === key);
  const busy = !adapter || loading;

  /**
   * Runs a change to item `key` that the agents take in the background, the item showing
   * `overlay` meanwhile; the page is read again once it's done.
   */
  const change = (key: string | null, overlay: Overlay, run: () => Promise<PluginAgentResult[]>, after?: () => void) => {
    if (!adapter) return;
    void changes.run(key, place, overlay, async () => {
      await checked(run);
      after?.();
    });
  };
  /** The card's switch: the whole item on or off, in every agent, keeping where it's set to be. */
  const flip = (item: PluginItemView, on: boolean) => adapter && change(item.key, { on }, () => adapter.switchPlugin(item.key, on));

  const current = selected ? find(selected) : undefined;
  let page;
  if (current && adapter) {
    page = (
      <DetailsPage
        view={view}
        item={current}
        adapter={adapter}
        actions={{
          busy,
          onBack: () => openSettings("plugins"),
          onScope: (projectIds) => change(
            current.key,
            { scope: projectIds ? { type: "someProjects", projectIds, folders: current.scope.type === "someProjects" ? current.scope.folders : [] } : { type: "allProjects" } },
            () => adapter.setScope(current.key, projectIds),
          ),
          onRemove: () => setDialog({ type: "remove", key: current.key }),
          onRetry: refresh,
          onUpdate: () => change(current.key, {}, () => adapter.update(current.key)),
          onOpen: () => setDialog({ type: "file", key: current.key }),
          onTrust: (projectId) => void changes.run(null, place, {}, () => adapter.trustProject(projectId)),
        }}
      />
    );
  } else {
    page = (
      <SettingsPage title="Plugins" description={PLUGINS_DESCRIPTION}>
        <ListBody
          view={view}
          tab={tab}
          query={query}
          busy={busy}
          loading={loading}
          adapter={adapter}
          onTab={(next) => {
            setTab(next);
            setQuery("");
          }}
          onQuery={setQuery}
          onOpen={(item) => select({ type: "settings", page: "plugins", detail: { plugin: item.key } })}
          onFlip={flip}
          onAdd={() => setDialog({ type: "add" })}
        />
      </SettingsPage>
    );
  }

  const target = open && open.type !== "add" ? find(open.key) : undefined;
  // The same calls for as long as one item's file stays open, however often the page is read again.
  const fileKey = open?.type === "file" ? open.key : null;
  const fileCalls = useMemo(() => adapter && fileKey !== null ? {
    details: () => adapter.details(fileKey),
    save: (path: string, contents: string, expected: string) => adapter.saveSkill(fileKey, path, contents, expected),
  } : null, [adapter, fileKey]);
  const close = () => setDialog(null);
  const failure = changes.failure(place);
  return (
    <div className="@container relative h-full min-w-0 [&_[data-tauri-drag-region]]:h-11">
      {page}
      {adapter && open?.type === "add" && <AddDialog view={view} adapter={adapter} refresh={refresh} onClose={close} />}
      {adapter && open?.type === "remove" && target && (
        <RemoveDialog
          item={target}
          adapter={adapter}
          onClose={close}
          onRemoved={() => {
            // An item an agent has of its own stays listed: its details stay open.
            if (target.removal === "everything" && liveDialog.current === open && useApp.getState().selection === selection) {
              // The removed detail cannot be revisited; keep any existing Forward entries.
              select({ type: "settings", page: "plugins" }, "replace");
            }
            if (liveDialog.current === open) close();
            refresh();
          }}
        />
      )}
      {fileCalls && open?.type === "file" && target && (
        <FileDialog key={target.key} item={target} details={fileCalls.details} save={fileCalls.save} onClose={close} />
      )}
      {(failure || error) && <p role="alert" className="text-destructive bg-background absolute right-5 bottom-4 left-5 text-xs whitespace-pre-wrap">{failure ?? error}</p>}
    </div>
  );
}

function ListBody({ view, tab, query, busy, loading, adapter, onTab, onQuery, onOpen, onFlip, onAdd }: {
  view: PluginsView;
  tab: PluginKind;
  query: string;
  busy: boolean;
  loading: boolean;
  adapter: PluginsAdapter | null;
  onTab: (tab: PluginKind) => void;
  onQuery: (query: string) => void;
  onOpen: (item: PluginItemView) => void;
  onFlip: (item: PluginItemView, on: boolean) => void;
  onAdd: () => void;
}) {
  const count = (kind: PluginKind) => view.items.filter((item) => item.kind === kind).length;
  const items = listed(view, tab, query);
  return (
    <div className="flex flex-col gap-5">
      <Toolbar tab={tab} query={query} count={count} adding={!!adapter} onTab={onTab} onQuery={onQuery} onAdd={onAdd} />
      {loading ? (
        <p role="status" className="text-foreground/65 text-label py-6 text-center">Loading plugins…</p>
      ) : count(tab) === 0 ? (
        <EmptyTab tab={tab} onAdd={onAdd} />
      ) : items.length === 0 ? (
        <p data-slot="no-match" className="text-foreground/65 text-label py-6 text-center">Nothing matches “{query.trim()}”.</p>
      ) : (
        <div data-slot="plugins-grid" className="grid grid-cols-1 gap-2.5 @thread:grid-cols-2">
          {items.map((item) => (
            <PluginCard
              key={item.key}
              item={item}
              line={lineOf(item)}
              chip={chipOf(view, item)}
              busy={busy}
              adapter={adapter}
              onOpen={() => onOpen(item)}
              onSwitch={(on) => onFlip(item, on)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** The list's one row of controls: the two tabs, search and Add. */
function Toolbar({ tab, query, count, adding, onTab, onQuery, onAdd }: {
  tab: PluginKind;
  query: string;
  count: (kind: PluginKind) => number;
  adding: boolean;
  onTab: (tab: PluginKind) => void;
  onQuery: (query: string) => void;
  onAdd: () => void;
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
    <div data-slot="plugins-toolbar" className="flex flex-wrap items-center gap-3">
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
      {/* "Search" alone fits a narrow window; the field's name still says what it searches. */}
      <SettingsSearch className="max-w-60 min-w-28 flex-1" label={tab === "server" ? "Search MCP servers" : "Search skills"} placeholder="Search" value={query} onChange={onQuery} />
      <Button size="sm" className="shrink-0" disabled={!adding} onClick={onAdd}><Plus aria-hidden />Add</Button>
    </div>
  );
}

/** A tab with nothing in it yet. */
function EmptyTab({ tab, onAdd }: { tab: PluginKind; onAdd: () => void }) {
  const [help, setHelp] = useState(false);
  const Icon = tab === "server" ? Terminal : Sparkle;
  return (
    <SettingsCard data-slot="plugins-empty" split={false} className="items-center gap-2 px-6 py-12 text-center">
      <span className="bg-foreground/5 rounded-toolbar-button flex size-11 items-center justify-center">
        <Icon aria-hidden className="text-foreground/65 size-5" />
      </span>
      <span className="pt-2 text-sm font-medium">No {TAB_LABEL[tab] === "Skills" ? "skills" : "MCP servers"} yet</span>
      <span className="text-foreground/65 text-label">Add one once and every agent can use it, in all your projects or just some.</span>
      <span className="flex items-center gap-3 pt-3">
        <Button size="sm" onClick={onAdd}>Add</Button>
        <Button variant="ghost" size="sm" aria-expanded={help} onClick={() => setHelp(!help)}>What can I add?</Button>
      </span>
      {help && <span className="text-foreground/65 max-w-md pt-2 text-xs">{ADD_HINT}</span>}
    </SettingsCard>
  );
}
