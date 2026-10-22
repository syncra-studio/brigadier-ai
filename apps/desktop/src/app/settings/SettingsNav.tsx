import { useIntegrations } from "@/app/settings/integrations/adapter";
import { useState } from "react";

import {
  groupTitle,
  searchSettings,
  SETTINGS_GROUPS,
  shownSettingsPages,
  type SettingsSearchResult,
} from "@/app/settings/pages";
import { revealSetting } from "@/app/settings/reveal";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";
import { NavRow } from "@/components/controls";
import { EmptyState, NavHeader, NavList, NavSection, SettingsSearch } from "@/components/shared";

/** Opens a result's page, then brings the setting it found into view. */
function openResult({ page, row }: SettingsSearchResult) {
  openSettings(page.id);
  if (row) revealSetting(row.target ?? row.label);
}

/**
 * The sidebar panel while Settings is open: its title, a search field, then its pages in their groups. Typing replaces the pages with the settings found, each with its page's name.
 */
export function SettingsNav() {
  // Catalog changes also refresh the integrations found by Settings search.
  useIntegrations((state) => state.snapshot);
  const current = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [query, setQuery] = useState("");
  // The result Enter opens; the arrow keys move it.
  const [active, setActive] = useState(0);
  const pages = shownSettingsPages();
  const results = query.trim() ? searchSettings(query) : null;

  const search = (value: string) => {
    setQuery(value);
    setActive(0);
  };

  return (
    <>
      <NavHeader title="Settings" />
      <div className="px-2 pb-2">
        <SettingsSearch
          label="Search settings"
          placeholder="Search"
          value={query}
          onChange={search}
          className="h-nav-search px-3"
          onKeyDown={(event) => {
            if (!results?.length) return;
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              const step = event.key === "ArrowDown" ? 1 : -1;
              setActive((index) => (index + step + results.length) % results.length);
            }
            const chosen = results[active];
            if (event.key === "Enter" && chosen) openResult(chosen);
          }}
        />
      </div>

      <div className="scroll-edge-fade flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-2 pt-1 pb-2">
        {results ? (
          results.length === 0 ? (
            <EmptyState title="No settings match." />
          ) : (
            <NavList>
              {results.map((result, index) => {
                const Icon = result.page.icon;
                return (
                  <li key={`${result.page.id}:${result.row?.target ?? ""}:${result.row?.label ?? ""}`}>
                    <NavRow
                      data-active={index === active}
                      className="h-auto items-start py-2.5"
                      onMouseEnter={() => setActive(index)}
                      onClick={() => openResult(result)}
                    >
                      <Icon aria-hidden className="text-foreground/65 mt-0.5" />
                      <span className="flex min-w-0 flex-col">
                        <span className="truncate">{result.row?.label ?? result.page.label}</span>
                        {result.row && (
                          <span className="text-foreground/50 truncate text-xs">
                            {result.page.label}{result.row.section ? ` › ${result.row.section}` : ""}{result.row.detail ? ` · ${result.row.detail}` : ""}
                          </span>
                        )}
                      </span>
                    </NavRow>
                  </li>
                );
              })}
            </NavList>
          )
        ) : (
          SETTINGS_GROUPS.map((group) => (
            <NavSection key={group} title={groupTitle(group)}>
              <NavList>
                {pages.filter((page) => page.group === group).map((page) => {
                  const Icon = page.icon;
                  return (
                    <li key={page.id}>
                      <NavRow
                        aria-current={page.id === current ? "page" : undefined}
                        onPointerEnter={page.prefetch}
                        onFocus={page.prefetch}
                        onClick={() => openSettings(page.id)}
                      >
                        <Icon aria-hidden className="text-foreground/65" />
                        <span className="truncate">{page.label}</span>
                      </NavRow>
                    </li>
                  );
                })}
              </NavList>
            </NavSection>
          ))
        )}
      </div>
    </>
  );
}
