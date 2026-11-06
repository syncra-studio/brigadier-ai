import { type BrowserProfile, HUES, tabProfileId, useProfilePlaces, useProfiles } from "@/state/browserProfilePlaces";
import { cn } from "@/lib/utils";
import { useSessionTabs } from "@/state/sessionTabs";

const BACKGROUND: Record<number, string> = {
  1: "bg-glyph-1",
  2: "bg-glyph-2",
  3: "bg-glyph-3",
  4: "bg-glyph-4",
  5: "bg-glyph-5",
  6: "bg-glyph-6",
  7: "bg-glyph-7",
  8: "bg-glyph-8",
};

const HUE_NAMES: Record<number, string> = {
  1: "Violet",
  2: "Teal",
  3: "Pink",
  4: "Green",
  5: "Amber",
  6: "Blue",
  7: "Coral",
  8: "Lime",
};

const SIZES = {
  sm: "size-4 text-2xs",
  md: "size-5 text-2xs",
  lg: "size-6 text-xs",
  xl: "size-9 text-base",
} as const;

/** A profile's mark: its colour and first letter, so colour is never the only cue. */
export function ProfileDisc({ profile, size = "md" }: { profile: Pick<BrowserProfile, "name" | "hue">; size?: keyof typeof SIZES }) {
  return (
    <span
      aria-hidden
      data-slot="profile-disc"
      className={cn(
        "text-background flex shrink-0 items-center justify-center rounded-full leading-none font-semibold",
        BACKGROUND[profile.hue] ?? BACKGROUND[6],
        SIZES[size],
      )}
    >
      {[...profile.name.trim()][0]?.toUpperCase() ?? "?"}
    </span>
  );
}

/** The eight colours to choose from. */
export function ProfileSwatches({ value, onChange }: { value: number; onChange: (hue: number) => void }) {
  return (
    <div role="radiogroup" aria-label="Colour" className="flex items-center gap-1.5">
      {HUES.map((hue) => (
        <button
          key={hue}
          type="button"
          role="radio"
          aria-checked={hue === value}
          aria-label={HUE_NAMES[hue]}
          title={HUE_NAMES[hue]}
          onClick={() => onChange(hue)}
          className={cn(
            "flex size-5 items-center justify-center rounded-full",
            BACKGROUND[hue],
            hue === value && "ring-foreground ring-offset-card ring-2 ring-offset-2",
          )}
        />
      ))}
    </div>
  );
}

/** A Browser tab's dot, once its session's tabs sit on more than one profile. */
export function TabProfileDot({ conversationId, tabId }: { conversationId: string; tabId: string }) {
  const profiles = useProfiles();
  useProfilePlaces();
  const mixed = useSessionTabs((s) => {
    const on = new Set((s.sessions[conversationId]?.tabs ?? []).filter((tab) => tab.kind === "browser").map((tab) => tabProfileId(tab.id)));
    return on.size > 1;
  });
  if (!mixed) return null;
  const profile = profiles.find((each) => each.id === tabProfileId(tabId));
  if (!profile) return null;
  return (
    <span
      data-slot="tab-profile-dot"
      title={profile.name}
      className={cn("ring-panel-tab absolute -end-0.5 -bottom-0.5 block size-1.75 rounded-full ring-2", BACKGROUND[profile.hue] ?? BACKGROUND[6])}
    />
  );
}
