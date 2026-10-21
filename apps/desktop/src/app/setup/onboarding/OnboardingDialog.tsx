import { useEffect } from "react";

import { Onboarding } from "@/app/setup/onboarding/Onboarding";
import { closeSetup } from "@/app/setup/onboarding/steps";
import { preloadWelcomeSound } from "@/app/setup/onboarding/welcomeSound";
import { Dialog, DialogContent } from "@/components/overlays";
import { introRelease } from "@/ipc/client";
import { useRevealed } from "@/lib/splash";
import { useOnboarding } from "@/state/onboarding";
import { useApp } from "@/state/store";

/**
 * The first launch: a welcome intro with its sound, the name to call the user by, the coding
 * agents Brigadier works through, then the projects to start with. It fills the window and
 * shows until finished; it can't be skipped. Settings can open it again, at the agents, and
 * that run can be closed (its X on the name and the agents, or Escape).
 */
export function OnboardingDialog() {
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const onboarded = useApp((s) => s.settings.onboarded);
  const introSeen = useApp((s) => s.settings.introSeen);
  const smoke = useApp((s) => s.info?.smoke ?? false);
  const reopened = useOnboarding((s) => s.reopened);
  const finishing = useOnboarding((s) => s.finishing);
  // Not under the startup screen: it would take focus there, unseen.
  const revealed = useRevealed();
  const open = revealed && catalogLoaded && !smoke && (!onboarded || reopened || finishing);

  // The sound loads under the startup screen, so the intro can start with it.
  const intro = catalogLoaded && !smoke && !onboarded && !introSeen;
  useEffect(() => {
    if (intro) preloadWelcomeSound().catch((error: unknown) => console.error(error));
  }, [intro]);

  // A window held for the intro (src-tauri/src/intro.rs) shows normally when there's none.
  useEffect(() => {
    if (revealed && catalogLoaded && !intro) introRelease().catch((error: unknown) => console.error(error));
  }, [revealed, catalogLoaded, intro]);

  // A first launch's setup fills the window, so the startup screen fades onto its backdrop
  // rather than the app it hasn't been seen over yet; the setup opens over this once revealed.
  const cover = catalogLoaded && !smoke && !onboarded;

  return (
    <>
      {cover && <div aria-hidden className="bg-background fixed inset-0 z-50" />}
      <Dialog open={open}>
        <DialogContent
          showCloseButton={false}
          aria-describedby={undefined}
          className="bg-background start-0 top-0 flex h-full w-full max-w-none translate-x-0 translate-y-0 flex-col gap-0 overflow-hidden rounded-none p-0 ring-0 rtl:translate-x-0"
          onInteractOutside={(event) => event.preventDefault()}
          // The setup holds the focus itself until a step focuses its own control (parts.tsx
          // useFocusOnMount): the first step dot would take it and flash its tooltip.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement | null)?.focus();
          }}
          onEscapeKeyDown={(event) => {
            event.preventDefault();
            if (reopened) closeSetup();
          }}
        >
          {open && <Onboarding />}
        </DialogContent>
      </Dialog>
    </>
  );
}
