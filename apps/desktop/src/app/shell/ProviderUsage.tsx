import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import type { UsageEntry } from "@/app/shell/BarStatus";
import { Button } from "@/components/controls";
import { ProviderGlyph } from "@/components/icons";
import { Share, StateDot } from "@/components/shared";
import { formatCountdown } from "@/lib/format";
import { used, windowTitle } from "@/lib/quota";
import { cn } from "@/lib/utils";

/**
 * One provider: its mark and name, then its account in use's tightest limit ("Weekly · 1d 8h",
 * a bar and the share), saying so when its limit is reached; clicking it opens the provider's
 * page. Needing a sign-in, that and Sign in.
 */
export function ProviderUsage({
  entry,
  now,
  account,
  onOpen,
  onSignIn,
}: {
  entry: UsageEntry;
  now: number;
  /** Who its account in use is, masked, when it has several. */
  account: string | null;
  onOpen: () => void;
  onSignIn: () => void;
}) {
  const name = PROVIDER_LABELS[entry.provider];
  const mark = (
    <ProviderGlyph
      provider={entry.provider}
      className="size-icon-md shrink-0"
    />
  );
  const row = "h-control-sm rounded-capsule flex w-full items-center gap-2 px-2 text-sm";
  if (entry.needs) {
    return (
      <div data-slot="usage-row" className={row}>
        {mark}
        <span className="truncate">{name}</span>
        <span className="flex min-w-0 flex-1 items-center gap-1.5">
          <StateDot tone="needs" />
          <span className="text-warning truncate text-xs">Not signed in</span>
        </span>
        <Button type="button" variant="outline" size="xs" onClick={onSignIn} className="shrink-0">
          Sign in
        </Button>
      </div>
    );
  }
  const { top, limitReached } = entry;
  const resets = top.resetsAtMs === null ? null : formatCountdown(top.resetsAtMs, now);
  return (
    <button
      type="button"
      data-slot="usage-row"
      title={account ? `In use: ${account}` : undefined}
      onClick={onOpen}
      className={cn(row, "hover:bg-foreground/8 text-start")}
    >
      {mark}
      <span className="truncate">{name}</span>
      <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">
        {[limitReached ? "At limit" : windowTitle(top), resets].filter(Boolean).join(" · ")}
      </span>
      <Share percent={used(top)} limitReached={limitReached} />
    </button>
  );
}
