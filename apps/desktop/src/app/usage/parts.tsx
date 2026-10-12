import { Shuffle } from "@openai/apps-sdk-ui/components/Icon";
import type { ReactNode } from "react";

import { accountNames } from "@/app/providers/AccountRows";
import { SettingsSection } from "@/app/settings/parts";
import { Badge } from "@/components/ui/badge";
import type { AccountUsage, AccountView, ProviderUsage } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { PROVIDERS, VENDOR_LABELS } from "@/lib/routing";
import { useApp } from "@/state/store";

/* What the Usage page's Limits and Activity tabs share: a section per provider and a card head per account. */

/** The account `usage` is, as Settings → Providers knows it. */
export function useAccountView(usage: AccountUsage): AccountView | undefined {
  return useApp((s) =>
    s.accounts?.accounts.find(
      (view) =>
        view.account.provider === usage.account.provider &&
        view.account.account === usage.account.account,
    ),
  );
}

/**
 * An account card's head: who it is (its email masked, as on Providers), its plan, In use while
 * work runs on it, and when its usage was last read.
 */
export function AccountHeader({ usage, now }: { usage: AccountUsage; now: number }) {
  const view = useAccountView(usage);
  const title = view
    ? accountNames(view, false).title
    : usage.account.account === undefined
      ? "This computer's login"
      : "Account";
  const plan = view?.status?.plan;
  return (
    <div data-slot="usage-account" className="flex items-center gap-2 px-4 pt-3.5 pb-1">
      <span className="flex min-w-0 flex-1 items-center gap-2">
        <span data-slot="usage-account-name" className="text-label truncate font-medium">
          {title}
        </span>
        {plan && (
          <span className="text-muted-foreground text-xs">
            {plan.charAt(0).toUpperCase()}
            {plan.slice(1)}
          </span>
        )}
        {usage.working && (
          <Badge variant="secondary" title="Work runs on this account now">
            In use
          </Badge>
        )}
      </span>
      {usage.readAtMs !== null && (
        <span data-slot="usage-checked" className="text-muted-foreground shrink-0 text-xs">
          Checked {formatAgo(usage.readAtMs, now)}
        </span>
      )}
    </div>
  );
}

/** Why an account shows no windows, in plain words; `null` when it has some. */
export function missingReason(view: AccountView | undefined, usage: AccountUsage): string | null {
  const status = view?.status;
  if (status && !status.path) return "Not installed";
  if (status && !status.loggedIn) return "Not signed in";
  if (!usage.quota) return "No usage read yet";
  if (usage.quota.windows.length === 0) return "It reports no usage windows";
  return null;
}

/** The provider's balancing line, shown once over its accounts while switching is on. */
function Balancing({ usage }: { usage: ProviderUsage }) {
  const switching = useApp((s) => s.settings.switchAccounts);
  if (!switching || !usage.balancing) return null;
  return (
    <p data-slot="usage-balancing" className="text-muted-foreground flex items-start gap-1.5 pb-1 text-xs">
      <Shuffle aria-hidden className="size-icon-xs mt-0.5 shrink-0" />
      <span>{usage.balancing}</span>
    </p>
  );
}

/** Each provider's accounts, a section each, in Brigadier's order. */
export function ProviderSections({
  providers,
  children,
}: {
  providers: readonly ProviderUsage[];
  children: (usage: ProviderUsage) => ReactNode;
}) {
  return PROVIDERS.map((provider) => providers.find((usage) => usage.provider === provider))
    .filter((usage) => usage !== undefined)
    .map((usage) => (
      <SettingsSection key={usage.provider} title={VENDOR_LABELS[usage.provider]}>
        <Balancing usage={usage} />
        <div className="flex flex-col gap-3">{children(usage)}</div>
      </SettingsSection>
    ));
}

