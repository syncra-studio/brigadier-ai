import type { ProviderKind } from "@/ipc/generated";
import { cliFacts } from "@/lib/providerKinds";
import { cn } from "@/lib/utils";

/**
 * An agent's mark, from its CLI's facts. `tile` sets it in its rounded square ("lg" beside an
 * agent page's title), in a soft grey; `idle` outlines the square for an agent not set up, `dim`
 * fades one that is off. Without a tile the caller sizes and colours it.
 */
export function ProviderGlyph({
  provider,
  className,
  tile,
  idle = false,
  dim = false,
}: {
  provider: ProviderKind;
  className?: string;
  tile?: "md" | "lg";
  idle?: boolean;
  dim?: boolean;
}) {
  if (!tile) {
    const { glyph } = cliFacts(provider);
    return (
      <svg viewBox={glyph.viewBox} fill="currentColor" aria-hidden className={cn(glyph.className, className)}>
        <path d={glyph.path} />
      </svg>
    );
  }
  return (
    <span
      aria-hidden
      data-slot="agent-mark"
      data-provider={provider}
      className={cn(
        "rounded-nav flex shrink-0 items-center justify-center",
        tile === "lg" ? "size-11" : "size-8.5",
        idle ? "border-divider text-muted-foreground border" : "bg-foreground/8 text-foreground/85",
        dim && "opacity-50",
        className,
      )}
    >
      <ProviderGlyph provider={provider} className={tile === "lg" ? "size-5.5" : "size-4.25"} />
    </span>
  );
}
