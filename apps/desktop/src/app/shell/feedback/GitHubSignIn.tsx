import { Button } from "@/components/controls";
import { ArrowUpRight, Spinner } from "@/components/icons";
import { CopyButton } from "@/components/shared";
import type { FeedbackSignIn } from "@/state/feedback";

/** What to run when the GitHub CLI is missing. */
export const INSTALL_GH = "brew install gh";

/**
 * Send feedback's Sign in to GitHub step, the same in the dialog and the thread's card: shown
 * after Send when `gh` isn't signed in. Its one-time code in a mono chip with Copy, and Open
 * GitHub, the page to enter it on; once that is open it waits for GitHub, and filing goes on by
 * itself when GitHub says yes. Without the GitHub CLI it shows the command to install it instead.
 */
export function GitHubSignIn({ signIn, onOpen }: { signIn: FeedbackSignIn; onOpen: () => void }) {
  const noGh = signIn.step === "noGh";
  const chip = noGh ? INSTALL_GH : signIn.step === "code" ? signIn.code : null;
  const waiting = signIn.step === "starting" || (signIn.step === "code" && signIn.opened);
  return (
    <div aria-label="Sign in to GitHub" className="flex flex-col gap-2.5">
      <p className="text-sm">
        {noGh ? "GitHub CLI isn’t installed. Install it, then try again." : "Sign in to GitHub so Brigadier can post your reports"}
      </p>
      {chip && (
        <div className="flex items-center gap-2">
          <span className="bg-foreground/5 border-foreground/10 rounded-control flex h-control-md items-center gap-1.5 border ps-3 pe-1">
            <span className={noGh ? "font-mono text-sm" : "font-mono text-base font-medium tracking-widest"}>{chip}</span>
            <CopyButton text={chip} tooltip={noGh ? "Copy command" : "Copy code"} size="xs" className="text-muted-foreground" />
          </span>
          {!noGh && (
            <Button variant="ghost" size="sm" onClick={onOpen}>
              Open GitHub
              <ArrowUpRight className="text-muted-foreground size-icon-sm" />
            </Button>
          )}
        </div>
      )}
      {waiting && (
        <p className="text-muted-foreground flex items-center gap-2 text-label">
          <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" />
          Waiting for GitHub…
        </p>
      )}
    </div>
  );
}
