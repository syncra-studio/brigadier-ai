import { DotsHorizontal, Plus, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useEffect, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { SetupTerminal } from "@/app/onboarding/SetupTerminal";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SwitchSetting,
} from "@/app/settings/parts";
import { WindowBar } from "@/app/usage/WindowBar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { useNow } from "@/hooks/use-now";
import type { AccountView, ProviderKind } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { glance } from "@/lib/quota";
import { PROVIDERS } from "@/lib/routing";
import {
  accountLabel,
  accountsOf,
  addAccount,
  loadAccounts,
  makeDefaultAccount,
  refreshAccounts,
  removeAccount,
  renameAccount,
  signInAccount,
} from "@/state/accounts";
import { useApp } from "@/state/store";

/** The Accounts page's rows, for Settings search; the page renders this copy. */
export const ACCOUNTS_ROWS = {
  switch: {
    label: "Switch accounts when one runs out",
    description:
      "When an account hits its limit, the chat carries on with another account of the same kind. When all are used up, Brigadier uses the other provider.",
  },
  add: {
    label: "Add account",
    description: "Sign in to another Claude Code or Codex account.",
  },
  own: {
    label: "This computer's login",
    description: "Also used by your terminal.",
  },
} as const;

/** How long after its sign-in ends an account may take to read as signed in. */
const SIGN_IN_SLACK_MS = 2000;

/**
 * Settings → Accounts: the switch for moving chats between accounts, then each agent's accounts
 * (the computer's own login first) with who is signed in and how much is left, and Add account.
 */
export function AccountsPage() {
  const views = useApp((s) => s.accounts?.accounts);
  const now = useNow(30_000);
  const refresh = useAction();
  useEffect(() => {
    loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
  }, []);
  const checkedAtMs = Math.max(0, ...(views ?? []).map((view) => view.checkedAtMs ?? 0));

  return (
    <SettingsPage
      title="Accounts"
      description="Sign in to more than one Claude or Codex account. Brigadier can move a chat to another account when one runs out."
      actions={
        <>
          {checkedAtMs > 0 && (
            <span className="text-muted-foreground text-xs">Checked {formatAgo(checkedAtMs, now)}</span>
          )}
          <SettingsButton
            aria-label="Check the accounts again"
            disabled={refresh.busy}
            onClick={() => refresh.run(refreshAccounts)}
          >
            <Reload className={refresh.busy ? "animate-spin motion-reduce:animate-none" : undefined} />
            Check again
          </SettingsButton>
        </>
      }
    >
      <SettingsSection>
        <SettingsCard>
          <SwitchSetting setting="switchAccounts" row={ACCOUNTS_ROWS.switch} />
        </SettingsCard>
      </SettingsSection>
      {PROVIDERS.map((provider) => (
        <ProviderAccounts
          key={provider}
          provider={provider}
          views={views === undefined ? null : accountsOf(views, provider)}
          now={now}
        />
      ))}
    </SettingsPage>
  );
}

/** An account being signed in to: a new one (`id` null until it is added) or an extra one again. */
type SigningIn = { id: string | null; exitedAtMs: number | null };

/** One agent's accounts, with Add account and the terminal of the sign-in under way. */
function ProviderAccounts({
  provider,
  views,
  now,
}: {
  provider: ProviderKind;
  views: AccountView[] | null;
  now: number;
}) {
  const label = PROVIDER_LABELS[provider];
  const [signing, setSigning] = useState<SigningIn | null>(null);
  const signingId = useRef<string | null>(null);
  const start = (id: string | null) => {
    signingId.current = id;
    setSigning({ id, exitedAtMs: null });
  };
  const openTerminal = useCallback(
    async (cols: number, rows: number) => {
      // "Try again" opens it again: once added, the same account signs in again.
      const id = signingId.current;
      if (id !== null) return signInAccount(id, cols, rows);
      const { account, terminal } = await addAccount(provider, cols, rows);
      signingId.current = account.id;
      setSigning((current) => current && { ...current, id: account.id });
      return terminal;
    },
    [provider],
  );
  const onExited = useCallback(() => {
    setSigning((current) => current && { ...current, exitedAtMs: Date.now() });
  }, []);

  // Its terminal closes by itself once the account reads as signed in after the sign-in ended.
  const signed = signing?.id ? views?.find((view) => view.account.account === signing.id) : undefined;
  const finished =
    signing?.exitedAtMs != null &&
    signed?.status?.loggedIn === true &&
    (signed.checkedAtMs ?? 0) >= signing.exitedAtMs - SIGN_IN_SLACK_MS;
  const active = finished ? null : signing;

  return (
    <SettingsSection
      title={label}
      actions={
        <SettingsButton
          aria-label={`Add a ${label} account`}
          disabled={active !== null}
          onClick={() => start(null)}
        >
          <Plus />
          {ACCOUNTS_ROWS.add.label}
        </SettingsButton>
      }
    >
      <SettingsCard>
        {views === null ? (
          <SettingsRow label="Reading the accounts…" />
        ) : (
          views.map((view) => (
            <AccountRow
              key={view.account.account ?? "own"}
              view={view}
              now={now}
              busy={active !== null}
              onSignIn={() => start(view.account.account ?? null)}
            />
          ))
        )}
      </SettingsCard>
      {active && (
        <SetupTerminal
          provider={provider}
          install={false}
          openTerminal={openTerminal}
          check={null}
          onExited={onExited}
          onClose={() => {
            setSigning(null);
            void refreshAccounts().catch(() => {});
          }}
        />
      )}
    </SettingsSection>
  );
}

/**
 * One account: its name, who is signed in, its usage windows and when they were read; a
 * Default badge, Sign in when it needs it, and a menu for the rest.
 */
function AccountRow({
  view,
  now,
  busy,
  onSignIn,
}: {
  view: AccountView;
  now: number;
  busy: boolean;
  onSignIn: () => void;
}) {
  const id = view.account.account;
  const own = id === undefined;
  const provider = view.account.provider;
  const action = useAction();
  const [renaming, setRenaming] = useState(false);
  const status = view.status;
  const needsSignIn = status !== null && status.path !== null && !status.loggedIn;
  const windows = view.quota ? glance(view.quota) : [];

  const who = !status
    ? "Checking…"
    : !status.path
      ? "Not installed"
      : !status.loggedIn
        ? "Not signed in"
        : [status.email ?? "Signed in", status.plan && `${capitalize(status.plan)} plan`]
            .filter(Boolean)
            .join(" · ");
  const read =
    view.quotaAtMs !== null ? `Usage read ${formatAgo(view.quotaAtMs, now)}` : "Usage not read yet";

  return (
    <SettingsRow
      label={
        renaming && id ? (
          <RenameField
            name={view.name || accountLabel(view)}
            onDone={(name) => {
              setRenaming(false);
              if (name !== null && name !== view.name) action.run(() => renameAccount(id, name));
            }}
          />
        ) : own ? (
          ACCOUNTS_ROWS.own.label
        ) : (
          accountLabel(view)
        )
      }
      description={
        <span className="flex flex-col gap-1.5">
          <span>
            {who}
            {own && ` · ${ACCOUNTS_ROWS.own.description}`}
          </span>
          {view.error && <span className="text-destructive">{view.error}</span>}
          {windows.length > 0 && (
            <span className="grid max-w-md grid-cols-1 gap-2 pt-0.5 @md:grid-cols-2">
              {windows.map((window) => (
                <WindowBar key={window.id} window={window} now={now} />
              ))}
            </span>
          )}
          {status?.loggedIn && <span className="text-muted-foreground">{read}</span>}
        </span>
      }
      error={action.error}
    >
      {view.default && <Badge variant="secondary">Default</Badge>}
      {needsSignIn && !own && (
        <SettingsButton disabled={busy} onClick={onSignIn}>
          Sign in
        </SettingsButton>
      )}
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            size="icon-xs"
            variant="ghost"
            aria-label={`${own ? ACCOUNTS_ROWS.own.label : accountLabel(view)}: actions`}
            className="text-muted-foreground"
            disabled={action.busy}
          >
            <DotsHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuLabel>{own ? ACCOUNTS_ROWS.own.label : accountLabel(view)}</DropdownMenuLabel>
          <DropdownMenuItem
            disabled={view.default}
            onSelect={() => action.run(() => makeDefaultAccount(provider, id))}
          >
            Make default
          </DropdownMenuItem>
          {!own && (
            <>
              <DropdownMenuItem onSelect={() => setRenaming(true)}>Rename</DropdownMenuItem>
              <DropdownMenuItem disabled={busy} onSelect={onSignIn}>
                Sign in again
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={() => action.run(() => removeAccount(id))}>
                Remove
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </SettingsRow>
  );
}

/** The account's name, editable: Enter or leaving the field keeps it, Escape drops the edit. */
function RenameField({ name, onDone }: { name: string; onDone: (name: string | null) => void }) {
  const [value, setValue] = useState(name);
  const done = useRef(false);
  const finish = (next: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(next === null || next.trim() === "" ? null : next.trim());
  };
  return (
    <Input
      aria-label="Account name"
      autoFocus
      value={value}
      className="h-7 max-w-xs"
      onChange={(event) => setValue(event.target.value)}
      onBlur={() => finish(value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") finish(value);
        if (event.key === "Escape") finish(null);
      }}
    />
  );
}

function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}
