import { ArrowRotateCw, DownloadSimple } from "@openai/apps-sdk-ui/components/Icon";

import { BarItem } from "@/app/BarItem";
import { FootButton } from "@/app/sidebar/nav";
import { FOOT_CHIP } from "@/app/UpdatesRow";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { Spinner } from "@/components/glyphs/spinner";
import { useSidebar } from "@/components/ui/sidebar";
import { type AppUpdate, clickAppUpdate, useAppUpdate } from "@/state/appUpdate";

const TIPS: Record<AppUpdate["state"], (version: string) => string> = {
  available: (version) => `Update Brigadier to ${version}`,
  updating: () => "Updating Brigadier",
  failed: () => "Couldn't update Brigadier. Click to try again",
  ready: () => "Restart to update",
};

/**
 * A nightly update at the sidebar's foot, as the agents' update pill is: a chip of an icon and
 * Brigadier's mark in the expanded row, the icon with a dot on the collapsed strip. Found, it
 * offers the update; a click downloads it while a spinner turns, and then restarts into it. Only
 * the icon changes between them, so the pill keeps its place and size throughout.
 */
export function AppUpdatePill() {
  const update = useAppUpdate((s) => s.update);
  const { open: expanded } = useSidebar();
  const state = update?.state ?? "available";
  const busy = state === "updating";
  return (
    <BarItem show={update !== null} axis={expanded ? "x" : "y"}>
      <FootButton
        tip={TIPS[state](update?.version ?? "")}
        aria-busy={busy || undefined}
        icon={
          busy ? (
            <Spinner aria-hidden className="animate-spin" />
          ) : state === "ready" ? (
            <ArrowRotateCw aria-hidden />
          ) : (
            <DownloadSimple aria-hidden />
          )
        }
        dot={expanded || busy ? null : "bg-foreground"}
        className={expanded ? FOOT_CHIP : undefined}
        onClick={clickAppUpdate}
        end={expanded && <BrigadierGlyph aria-hidden className="size-icon-sm mx-0.5 shrink-0" />}
      />
    </BarItem>
  );
}
