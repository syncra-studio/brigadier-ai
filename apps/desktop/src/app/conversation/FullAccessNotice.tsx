import { useState } from "react";

import { Button } from "@/components/controls";
import { ShieldExclamation } from "@/components/icons";
import type { Conversation } from "@/ipc/generated";
import { pageShown } from "@/lib/pageHidden";
import { cn } from "@/lib/utils";
import { useQueueRows } from "@/state/queueRows";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** Set once it stepped aside after showing: gone in every session, across restarts (D191, D216). */
const CLOSED = "brigadier.fullAccessNoticeClosed";
/** "shown": it showed in this run, so stepping aside counts as closing it. */
const seen = new Set<"shown">();

/**
 * The "Full access is on" card above the composer, while a session is on Full access: the full
 * warning at nine tenths of a card's usual size (its padding, type, icon
 * and buttons; heights rounded to whole pixels, so the thread's footer stays on the pixel grid),
 * rounded like the composer. It steps aside while a question, proposal or approval
 * waits above the composer, so it never pushes that card or the thread's last lines out of
 * view, and while messages wait in the queue, so they stay right under the conversation (#448).
 * Once it has shown and stepped aside it stays gone (demo12), in every session, new ones too, across
 * restarts (D191, D216); "Don't show again" turns it off in Settings, where turning it back on
 * shows it again.
 */
export function FullAccessNotice({ conversation, aside = false }: { conversation: Conversation; aside?: boolean }) {
  const show = useApp((s) => s.settings.showFullAccessNotice);
  // One that came while nobody could see the window is simply there when it shows again.
  const [opens] = useState(pageShown);
  const full = conversation.setup?.type === "session" && conversation.setup.permission === "full";
  const { rows } = useQueueRows(conversation.id);
  const queued = rows.length > 0;
  // Off in Settings: turning it back on shows it again.
  if (!show) localStorage.removeItem(CLOSED);
  if (show && full && (aside || queued) && seen.has("shown")) localStorage.setItem(CLOSED, "1");
  if (!show || !full || aside || queued || localStorage.getItem(CLOSED)) return null;
  seen.add("shown");
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
              unexpected changes.
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
        </div>
      </div>
    </section>
  );
}
