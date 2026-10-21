import { SettingsCard } from "@/components/shared";

/**
 * While the browsers are first listed: rows the size of a browser's, empty, so the list fills
 * in where they stand rather than pushing the page down.
 */
export function ListingRows() {
  return (
    <SettingsCard>
      <div role="status" aria-label="Looking for browsers" data-slot="import-listing">
        {[0, 1, 2].map((row) => (
          <div key={row} aria-hidden className="flex items-center gap-3 px-4 py-3">
            <span className="bg-foreground/5 size-8 shrink-0 rounded-lg" />
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              {/* Lines of a row's sizes, their words hidden. */}
              <span className="text-label w-fit rounded-sm bg-foreground/5 text-transparent select-none">
                A browser&apos;s name
              </span>
              <span className="w-fit rounded-sm bg-foreground/5 text-xs text-transparent select-none">
                Where it stands, and its profiles
              </span>
            </div>
          </div>
        ))}
      </div>
    </SettingsCard>
  );
}
