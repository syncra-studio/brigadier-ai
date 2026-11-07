import { FlowHeader } from "@/app/settings/FlowHeader";
import { ProfileDisc } from "@/app/settings/ProfileMark";
import { Badge, Button } from "@/components/controls";
import { Check, X } from "@/components/icons";
import { DialogFooter } from "@/components/overlays";
import { cn } from "@/lib/utils";
import { counted, type ImportWhat, type ListedSource, openProfile, passwordsRetryable, type ProfileImportResult, signInOn } from "@/state/browserImport";

type Done = Extract<ProfileImportResult["outcome"], { type: "done" }>;
type Mark = "yes" | "no" | "part";

function Tick({ mark }: { mark: Mark }) {
  if (mark === "no") return <X aria-hidden className="text-destructive size-icon-sm shrink-0" />;
  return <Check aria-hidden className={cn("size-icon-sm shrink-0", mark === "part" ? "text-warning" : "text-success")} />;
}

function PartRow({ mark, name, line, onRetry }: { mark: Mark; name: string; line: string; onRetry?: (() => void) | undefined }) {
  return (
    <div data-slot="import-part" data-mark={mark} className="flex items-center gap-2 py-1">
      <Tick mark={mark} />
      <span className="text-label w-28 shrink-0">{name}</span>
      <span className={cn("min-w-0 flex-1 text-xs", mark === "no" ? "text-destructive" : "text-foreground/65")}>{line}</span>
      {onRetry && <Button variant="ghost" size="xs" className="-my-1" onClick={onRetry}>Try again</Button>}
    </div>
  );
}

function SiteRow({ site, why, onSignIn }: { site: string; why: string; onSignIn: () => void }) {
  return (
    <div data-slot="import-site" className="flex items-center gap-2 py-1 ps-6">
      <span className="text-label min-w-0 flex-1 truncate">{site} <span className="text-foreground/65 text-xs">· {why}</span></span>
      <Button variant="soft" size="xs" onClick={onSignIn}>Sign in</Button>
    </div>
  );
}

/** What didn't come across, for the heading: "Passwords for Side project", in the person's words. */
export function resultSummary(results: readonly ProfileImportResult[]): string {
  const ready = results.filter((result) => result.outcome.type === "done");
  const lines = [ready.length === 0 ? "Nothing came across." : `${counted(ready.length, "profile is", "profiles are")} ready.`];
  for (const { profile, outcome } of results) {
    if (outcome.type === "failed") lines.push(`${profile.name} couldn't be imported.`);
    else {
      const parts = [outcome.passwordsError && "Passwords", outcome.bookmarksError && "Bookmarks", outcome.historyError && "History"].filter(Boolean);
      if (parts.length > 0) lines.push(`${parts.join(" and ")} for ${profile.name} didn't come across.`);
    }
  }
  const sites = ready.reduce((sum, { outcome }) => sum + (outcome as Done).record.skippedDomainCount, 0);
  if (sites > 0) lines.push(`${counted(sites, "site needs", "sites need")} a fresh sign-in.`);
  return lines.join(" ");
}

function ProfileResult({
  result,
  source,
  onRetry,
}: {
  result: ProfileImportResult;
  source: ListedSource;
  onRetry: (part: keyof ImportWhat) => void;
}) {
  const { profile, replaced, what, outcome } = result;
  const header = (
    <div className="mb-2 flex items-center gap-2.5">
      <ProfileDisc profile={profile} />
      <span className="text-label font-medium">{profile.name}</span>
      <Badge variant="secondary" className="font-normal">{replaced ? "Replaced" : "New"}</Badge>
    </div>
  );
  if (outcome.type === "failed") {
    return (
      <div data-slot="import-result" className="not-last:border-divider px-4 py-3 not-last:border-b">
        {header}
        <PartRow mark="no" name="Everything" line={outcome.message} />
      </div>
    );
  }
  const { record } = outcome;
  const appBound = new Set(record.appBoundDomains ?? []);
  const more = record.skippedDomainCount - record.skippedDomains.length;
  return (
    <div data-slot="import-result" className="not-last:border-divider px-4 py-3 not-last:border-b">
      {header}
      {what.signIns && (
        <>
          <PartRow
            mark={record.skippedDomainCount > 0 ? "part" : "yes"}
            name="Sign-ins"
            line={`${counted(record.imported, "cookie", "cookies")}${record.skippedDomainCount > 0 ? ` · ${counted(record.skippedDomainCount, "site", "sites")} left out` : ""}`}
          />
          {record.skippedDomains.map((site) => (
            <SiteRow
              key={site}
              site={site}
              why={appBound.has(site) ? `tied to ${source.name} on this device` : "its sign-in couldn't be copied"}
              onSignIn={() => signInOn(profile.id, site)}
            />
          ))}
          {more > 0 && <p className="text-foreground/65 ps-6 text-xs">and {counted(more, "more site", "more sites")}</p>}
          {(record.googleLeftOut ?? 0) > 0 && (
            <p data-slot="import-google" className="text-foreground/65 ps-6 text-xs">Google sign-ins aren't imported: sign in once here</p>
          )}
          {outcome.siteStorage === "nextStart" && (
            <p data-slot="import-next-start" className="text-foreground/65 ps-6 text-xs">Sites that keep their sign-in in page storage catch up when Brigadier next starts.</p>
          )}
        </>
      )}
      {what.history && (
        <PartRow
          mark={outcome.historyError ? "no" : "yes"}
          name="History"
          line={outcome.historyError ?? counted(record.history, "page", "pages")}
          onRetry={outcome.historyError ? () => onRetry("history") : undefined}
        />
      )}
      {what.bookmarks && (
        <PartRow
          mark={outcome.bookmarksError ? "no" : "yes"}
          name="Bookmarks"
          line={outcome.bookmarksError ?? counted(record.bookmarks ?? 0, "bookmark", "bookmarks")}
          onRetry={outcome.bookmarksError ? () => onRetry("bookmarks") : undefined}
        />
      )}
      {what.passwords && (
        <PartRow
          mark={outcome.passwordsError ? "no" : (record.passwordsSkipped ?? 0) > 0 ? "part" : "yes"}
          name="Passwords"
          line={outcome.passwordsError
            ?? `${counted(record.passwords ?? 0, "password", "passwords")}${(record.passwordsSkipped ?? 0) > 0 ? ` · ${counted(record.passwordsSkipped ?? 0, "left out", "left out")}` : ""}`}
          onRetry={outcome.passwordsError && passwordsRetryable(source.family) ? () => onRetry("passwords") : undefined}
        />
      )}
    </div>
  );
}

/** What each profile brought, part by part: what failed with Try again, each site to sign in to again. */
export function ProfileImportResults({
  source,
  results,
  onRetry,
  onDone,
}: {
  source: ListedSource;
  results: readonly ProfileImportResult[];
  onRetry: (index: number, part: keyof ImportWhat) => void;
  onDone: () => void;
}) {
  const first = results.find((result) => result.outcome.type === "done");
  return (
    <>
      <FlowHeader source={source} title={`Imported from ${source.name}`} description={resultSummary(results)} />
      <div data-slot="import-results" className="border-divider bg-card rounded-settings flex max-h-import-results flex-col overflow-y-auto border">
        {results.map((result, index) => (
          <ProfileResult key={result.choice.id} result={result} source={source} onRetry={(part) => onRetry(index, part)} />
        ))}
      </div>
      <DialogFooter>
        {first && (
          <Button
            variant="ghost"
            onClick={() => {
              onDone();
              openProfile(first.profile.id);
            }}
          >
            Open {first.profile.name}
          </Button>
        )}
        <Button onClick={onDone}>Done</Button>
      </DialogFooter>
    </>
  );
}
