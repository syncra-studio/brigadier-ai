import { DotsHorizontal, Plus, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { useNow } from "@/hooks/use-now";
import type { AccountView, ProviderKind } from "@/ipc/generated";
import { formatAgo, formatCountdown } from "@/lib/format";
import { glance, tone, used } from "@/lib/quota";
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
    label: "Switch to another account when one runs out",
    description:
      "A chat that hits its account's limit carries on with your next account. When every account is used up, Brigadier uses the other provider.",
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
 * Settings → Accounts: each agent's accounts (the computer's own login first) with who is signed
 * in and how much is left, a row adding another, then the switch for moving chats between them.
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
      description="Sign in once to each of your accounts, then pick the one new chats use."
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
      {PROVIDERS.map((provider) => (
        <ProviderAccounts
          key={provider}
          provider={provider}
          views={views === undefined ? null : accountsOf(views, provider)}
          now={now}
        />
      ))}
      <SettingsSection>
        <SettingsCard>
          <SwitchSetting setting="switchAccounts" row={ACCOUNTS_ROWS.switch} />
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}

/**
 * An account being signed in to: a new one (`id` null until it is added; `fresh` stays true so
 * a sign-in that never finishes leaves no account behind) or an extra one again.
 */
type SigningIn = { id: string | null; fresh: boolean; exitedAtMs: number | null };

/** One agent's accounts as a list, ending in a row that adds another and shows its sign-in. */
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
    setSigning({ id, fresh: id === null, exitedAtMs: null });
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

  // Its row closes by itself once the account reads as signed in after the sign-in ended.
  const signed = signing?.id ? views?.find((view) => view.account.account === signing.id) : undefined;
  const finished =
    signing?.exitedAtMs != null &&
    signed?.status?.loggedIn === true &&
    (signed.checkedAtMs ?? 0) >= signing.exitedAtMs - SIGN_IN_SLACK_MS;
  const active = finished ? null : signing;
  // A new account shows in the list only once it is signed in.
  const shown = (views ?? []).filter(
    (view) =>
      !(active?.fresh && view.account.account === active.id && view.status?.loggedIn !== true),
  );
  const anySignedIn = shown.some((view) => view.status?.loggedIn === true);

  const close = () => {
    const current = signing;
    setSigning(null);
    // Cancelled before it finished: the new account goes again.
    if (current?.fresh && current.id && signed?.status?.loggedIn !== true) {
      void removeAccount(current.id).catch(() => {});
    }
    void refreshAccounts().catch(() => {});
  };

  return (
    <SettingsSection title={label}>
      <SettingsCard>
        {views === null ? (
          <SettingsRow label="Reading the accounts…" />
        ) : (
          shown.map((view) => (
            <AccountRow
              key={view.account.account ?? "own"}
              view={view}
              now={now}
              busy={active !== null}
              onSignIn={start}
            />
          ))
        )}
        {active ? (
          <SetupTerminal
            bare
            provider={provider}
            install={false}
            openTerminal={openTerminal}
            check={null}
            onExited={onExited}
            onClose={close}
          />
        ) : (
          <button
            type="button"
            aria-label={`Add a ${label} account`}
            disabled={views === null}
            onClick={() => start(null)}
            className="text-muted-foreground hover:text-foreground hover:bg-foreground/5 flex items-center gap-3 px-4 py-3 text-left text-sm transition-colors disabled:opacity-50"
          >
            <span className="border-divider flex size-8 shrink-0 items-center justify-center rounded-full border border-dashed">
              <Plus className="size-icon-sm" />
            </span>
            {anySignedIn ? `Add another ${label} account` : `Add a ${label} account`}
          </button>
        )}
      </SettingsCard>
    </SettingsSection>
  );
}

/** Short names for usage windows in an account's summary line. */
const SHORT_WINDOW: Record<string, string> = { "5-hour": "5h", Weekly: "week" };

/**
 * One account: who it is, its plan and how much of each window is used, then Use (or In use for
 * the one new chats start on). Rename, Sign in again and Remove sit in its menu.
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
  /** Signs in to the account `id` again. */
  onSignIn: (id: string) => void;
}) {
  const id = view.account.account;
  const own = id === undefined;
  const provider = view.account.provider;
  const action = useAction();
  const [renaming, setRenaming] = useState(false);
  const status = view.status;
  const signedIn = status?.loggedIn === true;
  const email = status?.email ?? null;
  const renamed = !own && view.name !== "" && view.name !== email;
  const title = renamed ? view.name : (email ?? accountLabel(view));
  const windows = signedIn && view.quota ? glance(view.quota) : [];

  const details: ReactNode[] = [];
  if (!status) details.push("Checking…");
  else if (!status.path) details.push(`${PROVIDER_LABELS[provider]} isn't installed`);
  else if (!signedIn) details.push("Not signed in");
  else {
    if (renamed && email) details.push(email);
    if (status.plan) details.push(capitalize(status.plan));
    for (const window of windows) {
      const percent = used(window);
      const resets =
        window.resetsAtMs !== null ? `, resets in ${formatCountdown(window.resetsAtMs, now)}` : "";
      details.push(
        <span key={window.id} className={tone(percent).text} title={`${window.label}: ${percent}% used${resets}`}>
          {SHORT_WINDOW[window.label] ?? window.label.toLowerCase()} {percent}%
        </span>,
      );
    }
    if (signedIn && view.quotaAtMs === null) details.push("usage not read yet");
  }
  if (own) details.push("your terminal's login");

  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <span
        aria-hidden
        className="bg-foreground/10 text-foreground/80 flex size-8 shrink-0 items-center justify-center rounded-full text-sm font-medium"
      >
        {initial(title)}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {renaming && id ? (
          <RenameField
            name={view.name || accountLabel(view)}
            onDone={(name) => {
              setRenaming(false);
              if (name !== null && name !== view.name) action.run(() => renameAccount(id, name));
            }}
          />
        ) : (
          <span className="text-label truncate font-medium">{title}</span>
        )}
        <span className="text-foreground/65 flex flex-wrap gap-x-1.5 text-xs tabular-nums">
          {details.map((detail, index) => (
            <span key={index} className="flex gap-x-1.5">
              {index > 0 && <span aria-hidden>·</span>}
              {detail}
            </span>
          ))}
        </span>
        {view.error && <span className="text-destructive text-xs">{view.error}</span>}
        <ErrorLine error={action.error} />
      </div>
      {!signedIn && !own && status?.path ? (
        <SettingsButton disabled={busy} onClick={() => id && onSignIn(id)}>
          Sign in
        </SettingsButton>
      ) : view.default ? (
        <Badge variant="secondary" title="New chats start on this account">
          In use
        </Badge>
      ) : (
        <SettingsButton
          disabled={action.busy || !signedIn}
          title="Start new chats on this account"
          onClick={() => action.run(() => makeDefaultAccount(provider, id))}
        >
          Use
        </SettingsButton>
      )}
      {own ? (
        <span aria-hidden className="size-6 shrink-0" />
      ) : (
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              size="icon-xs"
              variant="ghost"
              aria-label={`${title}: more`}
              className="text-muted-foreground shrink-0"
              disabled={action.busy}
            >
              <DotsHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => setRenaming(true)}>Rename</DropdownMenuItem>
            <DropdownMenuItem disabled={busy} onSelect={() => onSignIn(id)}>
              Sign in again
            </DropdownMenuItem>
            <DropdownMenuItem variant="destructive" onSelect={() => action.run(() => removeAccount(id))}>
              Remove
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </div>
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

function initial(text: string): string {
  return (text.trim().charAt(0) || "?").toUpperCase();
}
