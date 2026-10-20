import { useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsSearch,
  SettingsSection,
  selectTrigger,
} from "@/app/settings/parts";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Check,
  ChevronDown,
  DotsHorizontal,
  ExternalLink,
  Filter,
  Globe,
  Trash,
} from "@/components/ui/icon";
import { useNow } from "@/hooks/use-now";
import type { BrowserVisit, HistoryFilter } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { browserHistory, byDay, HISTORY_PAGE, removeHistory, siteOf } from "@/state/browserData";
import { openLink } from "@/state/links";

const FILTERS = [
  { value: "all", label: "All" },
  { value: "agent", label: "Agent" },
  { value: "other", label: "Other" },
] as const satisfies readonly { value: HistoryFilter; label: string }[];

const HEADINGS: Record<HistoryFilter, string> = {
  all: "All-time history",
  agent: "Pages agents gave you",
  other: "Pages you opened",
};

const EMPTY: Record<HistoryFilter, string> = {
  all: "Pages you visit in Brigadier's browser show here.",
  agent: "Pages opened from an agent's reply or a preview show here.",
  other: "Pages you open yourself, and history you import, show here.",
};

/**
 * Settings › Browser › Browsing history: a search and a filter (All, Agent: pages in tabs an
 * agent's link or preview opened; Other: the rest) over the pages, by day, newest first.
 */
export function BrowserHistory({
  onBack,
  onClear,
  cleared = 0,
}: {
  onBack: () => void;
  onClear?: () => void;
  /** Counts the clears since the page opened: each one reads the history again. */
  cleared?: number;
}) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<HistoryFilter>("all");
  const [visits, setVisits] = useState<BrowserVisit[] | null>(null);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const now = useNow(60_000);

  useEffect(() => {
    let current = true;
    const timer = setTimeout(() => {
      browserHistory(query, filter, null)
        .then((found) => {
          if (!current) return;
          setVisits(found);
          setMore(found.length === HISTORY_PAGE);
          setError(null);
        })
        .catch((cause: unknown) => current && setError(errorText(cause)));
    }, query ? 150 : 0);
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // `cleared` reads the history again after a clear.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [query, filter, cleared]);

  const loadMore = () => {
    const last = visits?.at(-1);
    if (!last) return;
    browserHistory(query, filter, last)
      .then((found) => {
        setVisits((shown) => [...(shown ?? []), ...found]);
        setMore(found.length === HISTORY_PAGE);
      })
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  const remove = (id: number) => {
    removeHistory([id])
      .then(() => setVisits((shown) => shown?.filter((visit) => visit.id !== id) ?? null))
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  const days = byDay(visits ?? [], now);
  return (
    <SettingsPage title="Browsing history" back={{ label: "Browser", onBack }}>
      <div className="flex items-center gap-2">
        <SettingsSearch label="Search browsing history" value={query} onChange={setQuery} />
        <FilterMenu value={filter} onChange={setFilter} />
      </div>
      <SettingsSection
        title={HEADINGS[filter]}
        actions={onClear && <SettingsButton onClick={onClear}>Clear browsing data</SettingsButton>}
      >
        <ErrorLine error={error} />
        {visits && days.length === 0 && (
          <p className="text-foreground/65 text-label px-1 py-2">
            {query ? "No pages match." : EMPTY[filter]}
          </p>
        )}
        {days.map((day) => (
          <SettingsCard key={day.key}>
            <Collapsible defaultOpen>
              <CollapsibleTrigger className="group flex min-h-11 w-full items-center justify-between text-start text-sm font-medium">
                {day.label}
                <ChevronDown
                  aria-hidden
                  className="text-foreground/65 size-icon-sm transition-transform group-data-[state=open]:rotate-180"
                />
              </CollapsibleTrigger>
              <CollapsibleContent>
                <ul className="divide-divider border-divider divide-y border-t">
                  {day.items.map((visit) => (
                    <VisitRow key={visit.id} visit={visit} onRemove={() => remove(visit.id)} />
                  ))}
                </ul>
              </CollapsibleContent>
            </Collapsible>
          </SettingsCard>
        ))}
        {more && (
          <SettingsButton className="self-center" onClick={loadMore}>
            Show more
          </SettingsButton>
        )}
      </SettingsSection>
    </SettingsPage>
  );
}

function VisitRow({ visit, onRemove }: { visit: BrowserVisit; onRemove: () => void }) {
  const site = siteOf(visit.url);
  return (
    <li className="flex min-h-11 items-center gap-3 py-1.5">
      <Globe aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      <button
        type="button"
        title={visit.url}
        onClick={() => void openLink(visit.url)}
        className="text-label flex min-w-0 flex-1 items-baseline gap-2 text-start"
      >
        <span className="truncate">{visit.title || site}</span>
        {visit.title && <span className="text-foreground/65 shrink-0 text-xs">{site}</span>}
      </button>
      <span className="text-foreground/65 shrink-0 text-xs tabular-nums">{formatTime(visit.atMs)}</span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`More for ${visit.title || site}`}
            className="text-foreground/65 shrink-0"
          >
            <DotsHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => void openLink(visit.url)}>
            <ExternalLink />
            Open
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onSelect={onRemove}>
            <Trash />
            Remove from history
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}

function FilterMenu({ value, onChange }: { value: HistoryFilter; onChange: (value: HistoryFilter) => void }) {
  const chosen = FILTERS.find((choice) => choice.value === value)?.label;
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Filter browsing history"
          className={cn(selectTrigger, "flex w-32 shrink-0 gap-2")}
        >
          <Filter aria-hidden className="size-icon-sm shrink-0" />
          <span className="min-w-0 flex-1 truncate text-start">{chosen}</span>
          <ChevronDown aria-hidden className="text-foreground/65 size-icon-sm shrink-0" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-(--radix-dropdown-menu-trigger-width)">
        <DropdownMenuRadioGroup value={value} onValueChange={(next) => onChange(next as HistoryFilter)}>
          {FILTERS.map((choice) => (
            <DropdownMenuRadioItem
              key={choice.value}
              value={choice.value}
              indicator={<Check className="size-icon-md" />}
            >
              {choice.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
