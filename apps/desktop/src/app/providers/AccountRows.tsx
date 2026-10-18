import { CheckMd, DotsHorizontal, Eye, EyeOff, Plus } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { SetupTerminal } from "@/app/onboarding/SetupTerminal";
import { ProvidersButton, UsageMeter } from "@/app/providers/parts";
import { SettingsButton, SettingsCard, SettingsRow } from "@/app/settings/parts";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import type { AccountView, ProviderKind, QuotaWindow } from "@/ipc/generated";
import { hasEmail, maskEmailsIn } from "@/lib/email";
import { VENDOR_LABELS } from "@/lib/routing";
import { formatCountdown } from "@/lib/format";
import { glance, ordered, usageTone, used } from "@/lib/quota";
import {
  accountLabel,
  addAccount,
  makeDefaultAccount,
  oneRowPerAccount,
  refreshAccounts,
  removeAccount,
  renameAccount,
  signInAccount,
} from "@/state/accounts";
import { closeSetupTerminal } from "@/state/onboarding";

/** How long after its sign-in ends an account may take to read as signed in. */
const SIGN_IN_SLACK_MS = 2000;

/** Which account rows show their email: the page's own state, so leaving it hides them again. */
export type Revealed = { has: (key: string) => boolean; toggle: (key: string) => void };

/** The account rows showing their email, kept only while the page showing them is open. */
export function useRevealedEmails(): Revealed {
  const [shown, setShown] = useState<ReadonlySet<string>>(() => new Set());
  return useMemo(
    () => ({
      has: (key: string) => shown.has(key),
      toggle: (key: string) =>
        setShown((current) => {
          const next = new Set(current);
          if (!next.delete(key)) next.add(key);
          return next;
        }),
    }),
    [shown],
  );
}

/** The key a row's reveal goes by: the account's id, or the provider's own login. */
export function revealKey(view: AccountView): string {
  return view.account.account ?? `own:${view.account.provider}`;
}

/**
 * An account being signed in to: a new one (`id` null until it is added; `fresh` stays true so
 * a sign-in that never finishes leaves no account behind) or an extra one again.
 */
type SigningIn = { id: string | null; fresh: boolean; exitedAtMs: number | null };

/**
 * One agent's accounts as a card (the computer's own login first), ending in a row that adds
 * another and shows its sign-in. `page` lays them out as Settings › Providers' agent page does,
 * drawn to the user's design for a card the caller draws: every window's usage on the row
 * itself, and a link that adds another.
 */
export function ProviderAccounts({
  provider,
  views,
  now,
  revealed,
  page = false,
}: {
  provider: ProviderKind;
  views: AccountView[] | null;
  now: number;
  revealed: Revealed;
  page?: boolean;
}) {
  const label = page ? VENDOR_LABELS[provider] : PROVIDER_LABELS[provider];
  const [signing, setSigning] = useState<SigningIn | null>(null);
  const signingId = useRef<string | null>(null);
  // Off the page: a new account added only after that goes again with its sign-in.
  const left = useRef(false);
  useEffect(() => {
    left.current = false;
    return () => {
      left.current = true;
    };
  }, []);
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
      if (left.current) {
        closeSetupTerminal(terminal.id);
        void removeAccount(account.id).catch(() => {});
        throw new Error("The page closed before the sign-in started.");
      }
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
  const shown = oneRowPerAccount(
    (views ?? []).filter(
      (view) =>
        !(active?.fresh && view.account.account === active.id && view.status?.loggedIn !== true),
    ),
  );
  const anySignedIn = shown.some(({ view }) => view.status?.loggedIn === true);

  const close = () => {
    const current = signing;
    setSigning(null);
    // Cancelled before it finished: the new account goes again.
    if (current?.fresh && current.id && signed?.status?.loggedIn !== true) {
      void removeAccount(current.id).catch(() => {});
    }
    void refreshAccounts().catch(() => {});
  };
  // Leaving with a sign-in under way (another page, the setup closing) cancels it as closing
  // its row does, so a new account that never signed in doesn't stay behind.
  const cancel = useRef<(() => void) | null>(null);
  useEffect(() => {
    cancel.current = active ? close : null;
  });
  useEffect(() => () => cancel.current?.(), []);

  const rows =
    views === null ? (
      page ? (
        <p className="text-settings-text-secondary py-4.5 text-settings-body">Reading the accounts…</p>
      ) : (
        <SettingsRow label="Reading the accounts…" />
      )
    ) : (
      shown.map(({ view, ownLogin }) => (
        <AccountRow
          key={revealKey(view)}
          view={view}
          ownLogin={ownLogin}
          now={now}
          busy={active !== null}
          page={page}
          revealed={revealed.has(revealKey(view))}
          onReveal={() => revealed.toggle(revealKey(view))}
          onSignIn={start}
        />
      ))
    );
  const terminal = active && (
    <SetupTerminal
      bare
      provider={provider}
      install={false}
      openTerminal={openTerminal}
      check={null}
      onExited={onExited}
      onClose={close}
    />
  );
  const addLabel = anySignedIn ? `Add another ${label} account` : `Add a ${label} account`;

  if (page) {
    return (
      <>
        {rows}
        {terminal ? (
          <div className="py-3.5">{terminal}</div>
        ) : (
          <button
            type="button"
            aria-label={addLabel}
            disabled={views === null}
            onClick={() => start(null)}
            className="text-settings-link flex w-full items-center gap-2 py-3.5 text-settings-body font-medium hover:underline disabled:opacity-50"
          >
            <Plus aria-hidden className="size-3.5" />
            {addLabel}
          </button>
        )}
      </>
    );
  }

  return (
    <SettingsCard>
      {rows}
      {terminal || (
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
          {addLabel}
        </button>
      )}
    </SettingsCard>
  );
}

/** Short names for usage windows on an account's row. */
const SHORT_WINDOW: Record<string, string> = { "5-hour": "5h", Weekly: "Week" };

/** What an account is called and who signed in, masked unless its row is revealed. */
export function accountNames(
  view: AccountView,
  revealed: boolean,
): { title: string; email: string | null; editable: string } {
  const own = view.account.account === undefined;
  const email = view.status?.email ?? null;
  const renamed = !own && view.name !== "" && view.name !== email;
  const show = (text: string) => (revealed ? text : maskEmailsIn(text));
  const title = renamed ? show(view.name) : email ? show(email) : accountLabel(view);
  return {
    title,
    // Under a name of its own, the email shows on the line below.
    email: renamed && email ? show(email) : null,
    // The name to edit: never one taken from the email, and an email-like one only revealed.
    editable: !renamed || (hasEmail(view.name) && !revealed) ? "" : view.name,
  };
}

/**
 * One account: who it is (its email masked until its eye is pressed), its plan and a bar for
 * how much of each window is used, then Use (or In use for the one new chats start on).
 * Rename, Sign in again and Remove sit in its menu.
 */
function AccountRow({
  view,
  ownLogin,
  now,
  busy,
  page,
  revealed,
  onReveal,
  onSignIn,
}: {
  view: AccountView;
  /** It is this computer's login, itself or an account added here that is the same one. */
  ownLogin: boolean;
  now: number;
  busy: boolean;
  /** An agent's page's row, drawn to the user's design. */
  page: boolean;
  revealed: boolean;
  onReveal: () => void;
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
  const { title, email, editable } = accountNames(view, revealed);
  const hidden = hasEmail(view.name) || Boolean(status?.email);
  const windows = signedIn && view.quota ? glance(view.quota) : [];

  const details: ReactNode[] = [];
  if (!status) details.push("Checking…");
  else if (!status.path) details.push(`${PROVIDER_LABELS[provider]} isn't installed`);
  else if (!signedIn) details.push("Not signed in");
  else {
    if (email) details.push(email);
    if (status.plan) details.push(capitalize(status.plan));
    if (view.quotaAtMs === null) details.push("usage not read yet");
  }
  if (ownLogin) details.push("your terminal's login");

  const eye = hidden && (
    <Button
      type="button"
      size="icon-xs"
      variant="ghost"
      aria-label={revealed ? "Hide email" : "Show email"}
      aria-pressed={revealed}
      className="text-muted-foreground shrink-0"
      onClick={onReveal}
    >
      {revealed ? <EyeOff /> : <Eye />}
    </Button>
  );
  const nameField = renaming && id && (
    <RenameField
      name={editable}
      onDone={(name) => {
        setRenaming(false);
        if (name !== null && name !== view.name) action.run(() => renameAccount(id, name));
      }}
    />
  );
  const use =
    !signedIn && !own && status?.path ? (
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
    );
  const menu = own ? (
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
  );

  if (page) {
    const shownEmail = status?.email ? (revealed ? status.email : maskEmailsIn(status.email)) : title;
    const lines: string[] = [];
    if (!status) lines.push("Checking…");
    else if (!status.path) lines.push(`${PROVIDER_LABELS[provider]} isn't installed`);
    else if (!signedIn) lines.push("Not signed in");
    else {
      if (status.plan) lines.push(`${capitalize(status.plan)} plan`);
      if (view.quotaAtMs === null) lines.push("usage not read yet");
    }
    if (ownLogin) lines.push("This computer's login");
    else if (shownEmail !== title) lines.push(title);
    const pageWindows = signedIn && view.quota ? ordered(view.quota) : [];
    return (
      <div data-slot="account-row" className="flex items-center gap-6 py-4.5">
        <div className="flex min-w-48 flex-1 flex-col gap-1.25">
          {nameField || (
            <span className="flex min-w-0 items-center gap-2">
              <span
                data-slot="account-title"
                className="text-settings-text truncate font-mono text-settings-small"
              >
                {shownEmail}
              </span>
              {hidden && (
                <button
                  type="button"
                  aria-label={revealed ? "Hide email" : "Show email"}
                  aria-pressed={revealed}
                  className="text-settings-text-tertiary hover:text-settings-text-secondary shrink-0"
                  onClick={onReveal}
                >
                  {revealed ? <EyeOff className="size-3.25" /> : <Eye className="size-3.25" />}
                </button>
              )}
              {view.default && (
                <span
                  data-slot="in-use"
                  title="New chats start on this account"
                  className="bg-settings-success-soft text-settings-success rounded-capsule flex shrink-0 items-center gap-1 px-2 py-0.5 text-settings-badge font-medium"
                >
                  <CheckMd aria-hidden className="size-2.75" />
                  In use
                </span>
              )}
            </span>
          )}
          <span className="text-settings-text-secondary truncate text-settings-small">
            {lines.join(" · ")}
          </span>
          {view.error && <span className="text-destructive text-xs">{view.error}</span>}
          <ErrorLine error={action.error} />
        </div>
        {/* Many windows wrap under each other before the account's name gives way. */}
        <div className="flex min-w-0 flex-wrap justify-end gap-x-5 gap-y-3">
          {pageWindows.map((window) => (
            <UsageMeter key={window.id} window={window} now={now} />
          ))}
        </div>
        <div className="flex w-24 shrink-0 items-center justify-end gap-1.5">
          {!signedIn && !own && status?.path ? (
            <ProvidersButton disabled={busy} onClick={() => id && onSignIn(id)}>
              Sign in
            </ProvidersButton>
          ) : (
            !view.default && (
              <ProvidersButton
                disabled={action.busy || !signedIn}
                title="Start new chats on this account"
                onClick={() => action.run(() => makeDefaultAccount(provider, id))}
              >
                Use
              </ProvidersButton>
            )
          )}
          {own ? (
            <span aria-hidden className="size-7 shrink-0" />
          ) : (
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  aria-label={`${title}: more`}
                  disabled={action.busy}
                  className="text-settings-text-secondary hover:bg-settings-raised rounded-providers-menu flex size-7 shrink-0 items-center justify-center"
                >
                  <DotsHorizontal className="size-3.75" />
                </button>
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
      </div>
    );
  }

  return (
    <div data-slot="account-row" className="flex items-center gap-3 px-4 py-3">
      <span
        aria-hidden
        className="bg-foreground/10 text-foreground/80 flex size-8 shrink-0 items-center justify-center rounded-full text-sm font-medium"
      >
        {initial(title)}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {nameField || (
          <span className="flex min-w-0 items-center gap-1">
            <span data-slot="account-title" className="text-label truncate font-medium">
              {title}
            </span>
            {eye}
          </span>
        )}
        <span className="text-foreground/65 flex flex-wrap gap-x-1.5 text-xs tabular-nums">
          {details.map((detail, index) => (
            <span key={index} className="flex gap-x-1.5">
              {index > 0 && <span aria-hidden>·</span>}
              {detail}
            </span>
          ))}
        </span>
        {windows.length > 0 && (
          <span className="flex flex-wrap gap-x-4 gap-y-1 pt-1">
            {windows.map((window) => (
              <UsageBar key={window.id} window={window} now={now} />
            ))}
          </span>
        )}
        {view.error && <span className="text-destructive text-xs">{view.error}</span>}
        <ErrorLine error={action.error} />
      </div>
      {use}
      {menu}
    </div>
  );
}

/** A usage window: its short name, a bar of how much is used, and the share. */
function UsageBar({ window, now }: { window: QuotaWindow; now: number }) {
  const percent = used(window);
  const usage = usageTone(percent, { normal: { text: "text-foreground/65" } });
  const resets = window.resetsAtMs !== null ? `, resets in ${formatCountdown(window.resetsAtMs, now)}` : "";
  return (
    <span
      data-slot="account-usage"
      className="flex items-center gap-1.5 text-xs tabular-nums"
      title={`${window.label}: ${percent}% used${resets}`}
    >
      <span className="text-foreground/65">{SHORT_WINDOW[window.label] ?? window.label}</span>
      <span aria-hidden className="bg-foreground/10 h-1 w-16 overflow-hidden rounded-full">
        <span className={`block h-full rounded-full ${usage.fill}`} style={{ width: `${percent}%` }} />
      </span>
      <span className={usage.text}>{percent}%</span>
    </span>
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
      placeholder="Name this account"
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
