import { useEffect, useRef, useState } from "react";

import { recordKeyPress } from "@/app/keyCapture";
import { SettingsCard, SettingsPage } from "@/app/settings/parts";
import { SHORTCUTS, formatKeys, searchShortcuts, shortcutsFor, type Shortcut } from "@/app/shortcutCatalog";
import { KeyboardShortcut, MagnifyingGlassSearch, X } from "@/components/icons";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

/** What Settings search finds on the page: every shortcut by its name and line. */
export const SHORTCUTS_ROWS = SHORTCUTS.map((shortcut) => ({ label: shortcut.name, description: shortcut.line }));

/** The search: words typed, a key press recorded, or recording one now. */
type Search = { type: "words"; words: string } | { type: "press"; press: string } | { type: "recording" };

/**
 * Settings › Keyboard shortcuts: every shortcut Brigadier has, in one card, each with what it
 * does and its keys. A search finds them by words, or by keystrokes: the keyboard button records
 * the next key press (Esc cancels) and shows what it does.
 */
export function ShortcutsPage() {
  const mac = useApp((s) => s.info?.platform === "macos");
  const [search, setSearch] = useState<Search>({ type: "words", words: "" });
  const browser = useApp((s) => s.settings.browserEnabled);
  const followUp = useApp((s) => s.settings.followUp);
  const all = shortcutsFor(mac, browser, { followUp });
  const shown =
    search.type === "press"
      ? searchShortcuts(all, { press: search.press }, mac)
      : search.type === "words"
        ? searchShortcuts(all, { words: search.words }, mac)
        : all;

  // Recording owns the keyboard until a key is pressed, Esc, or the page goes.
  const recording = search.type === "recording";
  useEffect(() => {
    if (!recording) return;
    return recordKeyPress((press) => setSearch(press ? { type: "press", press } : { type: "words", words: "" }));
  }, [recording]);

  return (
    <SettingsPage title="Keyboard shortcuts">
      <div className="flex flex-col gap-6">
        <SearchField search={search} mac={mac} onSearch={setSearch} />
        {shown.length > 0 ? (
          <SettingsCard>
            {shown.map((shortcut) => (
              <ShortcutRow key={shortcut.id} shortcut={shortcut} mac={mac} />
            ))}
          </SettingsCard>
        ) : (
          <p role="status" className="text-foreground/65 py-6 text-center text-sm">
            No shortcuts match
          </p>
        )}
      </div>
    </SettingsPage>
  );
}

function SearchField({
  search,
  mac,
  onSearch,
}: {
  search: Search;
  mac: boolean;
  onSearch: (search: Search) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const words = search.type === "words" ? search.words : "";
  return (
    <div
      data-slot="shortcuts-search"
      className={cn(
        "rounded-capsule bg-foreground/8 flex h-8 min-w-0 items-center gap-2 border px-2.5",
        search.type === "recording" ? "border-ring" : "border-foreground/16",
      )}
    >
      <MagnifyingGlassSearch aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      {search.type === "words" ? (
        <input
          ref={input}
          type="search"
          value={words}
          placeholder="Search shortcuts"
          aria-label="Search shortcuts"
          spellCheck={false}
          autoComplete="off"
          className="placeholder:text-foreground/50 min-w-0 flex-1 bg-transparent text-sm outline-none [&::-webkit-search-cancel-button]:hidden"
          onChange={(event) => onSearch({ type: "words", words: event.target.value })}
          onKeyDown={(event) => {
            if (event.key === "Escape" && words) {
              // Esc clears the search before it leaves Settings.
              event.stopPropagation();
              onSearch({ type: "words", words: "" });
            }
          }}
        />
      ) : search.type === "recording" ? (
        <span role="status" className="text-foreground/65 min-w-0 flex-1 truncate text-sm">
          Press a shortcut… Esc cancels.
        </span>
      ) : (
        <span className="flex min-w-0 flex-1 items-center gap-2 text-sm">
          <KeyChip keys={formatKeys(search.press, mac)} />
        </span>
      )}
      {(words || search.type === "press") && (
        <button
          type="button"
          aria-label="Clear search"
          className="text-foreground/50 hover:text-foreground flex shrink-0 items-center"
          onClick={() => {
            onSearch({ type: "words", words: "" });
            requestAnimationFrame(() => input.current?.focus());
          }}
        >
          <X className="size-icon-sm" />
        </button>
      )}
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label="Search by keystrokes"
            aria-pressed={search.type === "recording"}
            className={cn(
              "hover:text-foreground flex shrink-0 items-center rounded-xs",
              search.type === "recording" ? "text-foreground" : "text-foreground/65",
            )}
            onClick={() =>
              onSearch(search.type === "recording" ? { type: "words", words: "" } : { type: "recording" })
            }
          >
            <KeyboardShortcut className="size-icon-md" />
          </button>
        </TooltipTrigger>
        <TooltipContent>Search by keystrokes</TooltipContent>
      </Tooltip>
    </div>
  );
}

function ShortcutRow({ shortcut, mac }: { shortcut: Shortcut; mac: boolean }) {
  return (
    <div
      data-slot="shortcut-row"
      data-setting={shortcut.name}
      className="grid grid-cols-2 items-center gap-6 px-4 py-3"
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <div className="text-label font-medium break-words">{shortcut.name}</div>
        <div className="text-foreground/65 text-xs break-words">{shortcut.line}</div>
      </div>
      <div className="flex flex-col items-start gap-1.5">
        {shortcut.keys.map((keys) => (
          <KeyChip key={keys} keys={formatKeys(keys, mac)} />
        ))}
      </div>
    </div>
  );
}

function KeyChip({ keys }: { keys: string }) {
  return (
    <Kbd className="text-foreground/65 bg-foreground/6 h-auto rounded-full border-transparent px-2 py-0.5 font-sans text-xs">
      {keys}
    </Kbd>
  );
}
