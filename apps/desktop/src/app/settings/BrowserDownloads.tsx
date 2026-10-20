import { useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { SettingsButton, SettingsCard, SettingsPage, SettingsSearch, SettingsSection } from "@/app/settings/parts";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { DotsHorizontal, Download, Folder, Trash } from "@/components/ui/icon";
import { useNow } from "@/hooks/use-now";
import { revealPath } from "@/ipc/client";
import type { BrowserDownload } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { browserDownloads, byDay, removeDownloads, siteOf } from "@/state/browserData";
import { useApp } from "@/state/store";

const STATES: Record<BrowserDownload["state"], string | null> = {
  running: "Downloading",
  done: null,
  failed: "Failed",
};

/** Settings › Browser › Download history: the files the browser downloaded, by day. */
export function BrowserDownloads({ onBack }: { onBack: () => void }) {
  const [query, setQuery] = useState("");
  const [downloads, setDownloads] = useState<BrowserDownload[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [changed, setChanged] = useState(0);
  const mac = useApp((s) => s.info?.platform === "macos");
  const now = useNow(60_000);

  useEffect(() => {
    let current = true;
    const timer = setTimeout(() => {
      browserDownloads(query)
        .then((found) => current && setDownloads(found))
        .catch((cause: unknown) => current && setError(errorText(cause)));
    }, query ? 150 : 0);
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // `changed` reads the list again after a removal.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [query, changed]);

  const remove = (ids: number[] | null) => {
    removeDownloads(ids)
      .then(() => setChanged((count) => count + 1))
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  const shown = downloads ?? [];
  const days = byDay(shown, now);
  return (
    <SettingsPage title="Download history" back={{ label: "Browser", onBack }}>
      <SettingsSearch label="Search download history" value={query} onChange={setQuery} />
      <SettingsSection
        title="All-time history"
        actions={
          <SettingsButton disabled={!shown.some((d) => d.state !== "running")} onClick={() => remove(null)}>
            Clear all
          </SettingsButton>
        }
      >
        <ErrorLine error={error} />
        {downloads && days.length === 0 && (
          <SettingsCard>
            <div className="flex flex-col items-center gap-1.5 py-8 text-center">
              <Download aria-hidden className="text-foreground/65 size-icon-lg" />
              <p className="text-sm font-medium">{query ? "No downloads match" : "No downloads yet"}</p>
              {!query && (
                <p className="text-foreground/65 text-label">Files you download in the built-in browser show here.</p>
              )}
            </div>
          </SettingsCard>
        )}
        {days.map((day) => (
          <SettingsSection key={day.key} title={day.label}>
            <SettingsCard>
              <ul className="divide-divider divide-y">
                {day.items.map((download) => {
                  const name = download.path.split(/[\\/]/).pop() || download.path;
                  const state = STATES[download.state];
                  return (
                    <li key={download.id} className="flex min-h-12 items-center gap-3 py-1.5">
                      <Download aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
                      <span className="flex min-w-0 flex-1 flex-col">
                        <span className="text-label truncate" title={download.path}>{name}</span>
                        <span className="text-foreground/65 truncate text-xs">
                          {state ? `${state} · ` : ""}
                          {siteOf(download.url)}
                        </span>
                      </span>
                      <span className="text-foreground/65 shrink-0 text-xs tabular-nums">
                        {formatTime(download.atMs)}
                      </span>
                      <DropdownMenu modal={false}>
                        <DropdownMenuTrigger asChild>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            aria-label={`More for ${name}`}
                            className="text-foreground/65 shrink-0"
                          >
                            <DotsHorizontal />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          {download.state === "done" && (
                            <DropdownMenuItem
                              onSelect={() =>
                                void revealPath(download.path).catch((cause: unknown) => setError(errorText(cause)))
                              }
                            >
                              <Folder />
                              {mac ? "Reveal in Finder" : "Show in folder"}
                            </DropdownMenuItem>
                          )}
                          <DropdownMenuItem variant="destructive" onSelect={() => remove([download.id])}>
                            <Trash />
                            Remove from history
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </li>
                  );
                })}
              </ul>
            </SettingsCard>
          </SettingsSection>
        ))}
      </SettingsSection>
    </SettingsPage>
  );
}
