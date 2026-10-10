import {
  ArrowLeft,
  MagnifyingGlassSearch,
  XCircleFilled,
} from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import {
  searchSettings,
  SETTINGS_GROUPS,
  shownSettingsPages,
  type SettingsSearchResult,
} from "@/app/settings/pages";
import { navRow, NavHeader, NavList, NavSection } from "@/app/sidebar/nav";
import { cn } from "@/lib/utils";
import { closeSettings, openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/** How many frames a result waits for its page (which may load lazily) to show the setting. */
const FIND_FRAMES = 60;

/** A setting's row, else a section named like it (the Usage page's sections). */
function findSetting(label: string): HTMLElement | undefined {
  const rows = document.querySelectorAll<HTMLElement>("[data-setting]");
  const row = [...rows].find((element) => element.dataset.setting === label);
  if (row) return row;
  return [...document.querySelectorAll<HTMLElement>("section")].find((section) => {
    const labelledBy = section.getAttribute("aria-labelledby");
    const name =
      section.getAttribute("aria-label") ??
      (labelledBy ? document.getElementById(labelledBy)?.textContent : null);
    return name?.trim() === label;
  });
}

/** Opens a result's page, then brings the setting it found into view. */
function openResult({ page, row }: SettingsSearchResult) {
  openSettings(page.id);
  if (!row) return;
  let frames = 0;
  const reveal = () => {
    const target = findSetting(row.label);
    if (target) target.scrollIntoView({ block: "center" });
    else if (++frames < FIND_FRAMES) requestAnimationFrame(reveal);
  };
  requestAnimationFrame(reveal);
}

/**
 * The sidebar panel while Settings is open: the way back to the app, its title, a search
 * field, then its pages in their groups. Typing replaces the pages with the settings found, each with its page's name.
 */
export function SettingsNav() {
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
      <div className="px-2 pt-2">
        <button type="button" className={navRow} onClick={closeSettings}>
          <ArrowLeft aria-hidden className="text-foreground/65" />
          Back to app
        </button>
      </div>
      <NavHeader title="Settings" />
      <div className="px-2 pb-2">
        <label className="h-nav-search rounded-capsule bg-foreground/8 flex items-center gap-2 px-3">
          <MagnifyingGlassSearch aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
          <input
            type="search"
            value={query}
            placeholder="Search"
            aria-label="Search settings"
            spellCheck={false}
            autoComplete="off"
            className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none [&::-webkit-search-cancel-button]:hidden"
            onChange={(event) => search(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && query) {
                // Esc clears the search first, before it leaves Settings.
                event.stopPropagation();
                search("");
              }
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
          {query && (
            <button
              type="button"
              aria-label="Clear search"
              className="text-muted-foreground hover:text-foreground flex shrink-0 items-center"
              onClick={() => search("")}
            >
              <XCircleFilled className="size-icon-md" />
            </button>
          )}
        </label>
      </div>

      <div className="scroll-edge-fade flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-2 pt-1 pb-2">
        {results ? (
          results.length === 0 ? (
            <p className="text-muted-foreground px-2 py-1 text-sm">No settings match.</p>
          ) : (
            <NavList>
              {results.map((result, index) => {
                const Icon = result.page.icon;
                return (
                  <li key={`${result.page.id}:${result.row?.label ?? ""}`}>
                    <button
                      type="button"
                      data-active={index === active}
                      className={cn(navRow, "h-auto items-start py-2.5")}
                      onMouseEnter={() => setActive(index)}
                      onClick={() => openResult(result)}
                    >
                      <Icon aria-hidden className="text-foreground/65 mt-0.5" />
                      <span className="flex min-w-0 flex-col">
                        <span className="truncate">{result.row?.label ?? result.page.label}</span>
                        {result.row && (
                          <span className="text-foreground/50 truncate text-xs">
                            {result.page.label}
                          </span>
                        )}
                      </span>
                    </button>
                  </li>
                );
              })}
            </NavList>
          )
        ) : (
          SETTINGS_GROUPS.map((group) => (
            <NavSection key={group} title={group}>
              <NavList>
                {pages.filter((page) => page.group === group).map((page) => {
                  const Icon = page.icon;
                  return (
                    <li key={page.id}>
                      <button
                        type="button"
                        aria-current={page.id === current ? "page" : undefined}
                        className={navRow}
                        onClick={() => openSettings(page.id)}
                      >
                        <Icon aria-hidden className="text-foreground/65" />
                        <span className="truncate">{page.label}</span>
                      </button>
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
