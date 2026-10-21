import { useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { DayGroup } from "@/app/settings/DayGroup";
import { FilterMenu } from "@/app/settings/FilterMenu";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { VisitRow } from "@/app/settings/VisitRow";
import { Button, ErrorLine } from "@/components/controls";
import { EmptyState, SettingsSearch } from "@/components/shared";
import { useNow } from "@/hooks/use-now";
import type { BrowserVisit, HistoryFilter } from "@/ipc/generated";
import { browserHistory, byDay, HISTORY_PAGE, removeHistory } from "@/state/browserData";

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
        actions={onClear && <Button variant="soft" size="sm" onClick={onClear}>Clear browsing data</Button>}
      >
        <ErrorLine error={error} />
        {visits && days.length === 0 && (
          <EmptyState title={query ? "No pages match." : EMPTY[filter]} />
        )}
        {days.map((day) => (
          <DayGroup key={day.key} label={day.label}>
            {day.items.map((visit) => (
              <VisitRow key={visit.id} visit={visit} onRemove={() => remove(visit.id)} />
            ))}
          </DayGroup>
        ))}
        {more && (
          <Button variant="soft" size="sm" className="self-center" onClick={loadMore}>
            Show more
          </Button>
        )}
      </SettingsSection>
    </SettingsPage>
  );
}
