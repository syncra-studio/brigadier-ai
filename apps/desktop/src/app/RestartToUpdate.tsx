import { ArrowRotateCw } from "@openai/apps-sdk-ui/components/Icon";

import { BarItem } from "@/app/BarItem";
import { FootButton } from "@/app/sidebar/nav";
import { useSidebar } from "@/components/ui/sidebar";
import { restartToUpdate } from "@/ipc/client";
import { useAppUpdate } from "@/state/appUpdate";

/**
 * "Restart to update" at the sidebar's foot while a nightly update waits: its words in the
 * expanded row, an icon with a dot on the collapsed strip. Quitting any other way installs it
 * too.
 */
export function RestartToUpdate() {
  const ready = useAppUpdate((s) => s.ready);
  const { open: expanded } = useSidebar();
  return (
    <BarItem show={ready !== null} axis={expanded ? "x" : "y"}>
      <FootButton
        tip="Restart to update"
        icon={<ArrowRotateCw aria-hidden />}
        dot={expanded ? null : "bg-foreground"}
        // A narrow sidebar cuts the words short rather than widening the row.
        className={expanded ? "min-w-0" : undefined}
        end={expanded && <span className="truncate text-sm">Restart to update</span>}
        onClick={() => void restartToUpdate()}
      />
    </BarItem>
  );
}
