import { useState } from "react";

import { openPermissionsHelp } from "@/app/shell/SetupPickers";
import { Button, TooltipIconButton } from "@/components/controls";
import { ShieldExclamation, X } from "@/components/icons";
import type { Conversation } from "@/ipc/generated";
import { pageShown } from "@/lib/pageHidden";
import { cn } from "@/lib/utils";
import { useQueueRows } from "@/state/queueRows";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** Conversations whose Full access notice was closed, until the app restarts. */
const closedNotices = new Set<string>();

/**
 * The "Full access is on" card above the composer, while a session is on Full access: the full
 * warning at nine tenths of a card's usual size (its padding, type, icon
 * and buttons; heights rounded to whole pixels, so the thread's footer stays on the pixel grid),
 * rounded like the composer. It steps aside while a question, proposal or approval
 * waits above the composer, so it never pushes that card or the thread's last lines out of
 * view, and while messages wait in the queue, so they stay right under the conversation (#448). × closes it for this conversation; "Don't show again" turns it off in Settings, where it
 * can be turned back on.
 */
export function FullAccessNotice({ conversation }: { conversation: Conversation }) {
  const show = useApp((s) => s.settings.showFullAccessNotice);
  const [, setClosed] = useState(0);
  // One that came while nobody could see the window is simply there when it shows again.
  const [opens] = useState(pageShown);
  const full = conversation.setup?.type === "session" && conversation.setup.permission === "full";
  const { rows } = useQueueRows(conversation.id);
  const queued = rows.length > 0;
  if (!show || !full || queued || closedNotices.has(conversation.id)) return null;
  const close = () => {
    closedNotices.add(conversation.id);
    setClosed((count) => count + 1);
  };
  return (
    <section
      aria-label="Full access is on"
      data-slot="full-access-notice"
      className={cn("@container/notice bg-rail border-foreground/10 rounded-composer mb-2 border px-notice-x py-notice-y text-notice", opens && "animate-rail-open motion-reduce:animate-none")}
    >
      {/* Narrow, the buttons go under the text, so the text keeps the width to read. */}
      <div className="flex flex-col gap-notice-gap @lg/notice:flex-row @lg/notice:items-center @lg/notice:gap-notice-gap-wide">
        <div className="flex min-w-0 flex-1 items-center gap-notice-gap-wide">
          <ShieldExclamation className="size-notice-icon shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="font-medium">Full access is on</p>
            <p className="text-muted-foreground">
              Brigadier’s workers can edit any file and run commands with internet access without
              your approval. This increases the risk of data loss, exposed information, and
              unexpected changes.{" "}
              <button type="button" className="underline underline-offset-2 hover:text-foreground" onClick={openPermissionsHelp}>
                Learn more
              </button>{" "}
              about elevated risks.
            </p>
          </div>
        </div>
        <div className="border-foreground/10 flex shrink-0 items-center justify-end gap-1 @lg/notice:self-stretch @lg/notice:border-s @lg/notice:ps-notice-gap-wide">
          <Button
            size="xs"
            className="rounded-capsule h-notice-button text-notice-button"
            onClick={() =>
              void setSetting("showFullAccessNotice", false).catch(
                (error: unknown) => console.error("couldn't save the setting", error),
              )
            }
          >
            Don’t show again
          </Button>
          <TooltipIconButton
            tooltip="Close"
            side="bottom"
            className="rounded-capsule size-notice-close"
            onClick={close}
          >
            <X className="size-notice-close-icon" />
          </TooltipIconButton>
        </div>
      </div>
    </section>
  );
}
