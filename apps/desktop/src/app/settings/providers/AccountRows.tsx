import { Fragment, type MouseEvent, type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { SetupTerminal } from "@/app/setup/onboarding/SetupTerminal";
import { ROW_LINE, ROW_TITLE } from "@/app/settings/providers/parts";
import { Badge, Button, Input, ErrorLine } from "@/components/controls";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { ChevronDown, ChevronRight, DotsHorizontal, Eye, EyeOff, Plus } from "@/components/icons";
import { UsageBar } from "@/components/shared";
import type { AccountView, OrderReason, ProviderKind, QuotaWindow } from "@/ipc/generated";
import { hasEmail, maskEmailsIn } from "@/lib/email";
import { VENDOR_LABELS } from "@/lib/routing";
import { formatCountdown, formatTime } from "@/lib/format";
import { ordered, usageTone, used, windowTitle } from "@/lib/quota";
import { cn } from "@/lib/utils";
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
 * a sign-in that never finishes leaves no account behind) or an extra one again. `wasSignedIn`:
 * it read as signed in when the sign-in started.
 */
type SigningIn = {
  id: string | null;
  fresh: boolean;
  startedAtMs: number;
  wasSignedIn: boolean;
  exitedAtMs: number | null;
};

/**
 * Whether the sign-in is done: the account reads as signed in from a check made after it ended,
 * or, for one that wasn't signed in when it started, from any check made since (the daemon checks
 * it the moment its login is stored, seconds before the sign-in ends).
 */
export function signInFinished(
  signing: Pick<SigningIn, "startedAtMs" | "wasSignedIn" | "exitedAtMs">,
  view: Pick<AccountView, "status" | "checkedAtMs"> | undefined,
): boolean {
  if (view?.status?.loggedIn !== true) return false;
  const checkedAtMs = view.checkedAtMs ?? 0;
  if (signing.exitedAtMs != null && checkedAtMs >= signing.exitedAtMs - SIGN_IN_SLACK_MS) return true;
  return !signing.wasSignedIn && checkedAtMs >= signing.startedAtMs;
}

/**
 * One agent's accounts on its Settings › Providers page, inside a card the caller draws,
 * in routing order, ending in a link that adds another and shows its sign-in.
 */
export function ProviderAccounts({
  provider,
  views,
  now,
  revealed,
  opened,
}: {
  provider: ProviderKind;
  views: AccountView[] | null;
  now: number;
  revealed: Revealed;
  opened: OpenRows;
}) {
  const label = VENDOR_LABELS[provider];
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
    const wasSignedIn = id !== null && views?.find((view) => view.account.account === id)?.status?.loggedIn === true;
    setSigning({ id, fresh: id === null, startedAtMs: Date.now(), wasSignedIn, exitedAtMs: null });
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

  // Its row closes by itself once the account reads as signed in (see `signInFinished`).
  const signed = signing?.id ? views?.find((view) => view.account.account === signing.id) : undefined;
  const finished = signing !== null && signInFinished(signing, signed);
  const active = finished ? null : signing;
  // A new account shows in the list only once it is signed in.
  const shown = oneRowPerAccount(
    (views ?? []).filter(
      (view) =>
        !(active?.fresh && view.account.account === active.id && view.status?.loggedIn !== true),
    ),
  ).toSorted((a, b) => (a.view.place ?? Infinity) - (b.view.place ?? Infinity));
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
      <p className="text-foreground/65 px-4 py-3 text-sm">Reading the accounts…</p>
    ) : (
      shown.map(({ view, ownLogin }, index) => (
        <AccountRow
          key={revealKey(view)}
          view={view}
          ownLogin={ownLogin}
          now={now}
          busy={active !== null}
          open={opened.has(revealKey(view), index === 0)}
          onToggle={() => opened.toggle(revealKey(view), index === 0)}
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
      openTerminal={openTerminal}
      check={null}
      onExited={onExited}
      onClose={close}
    />
  );
  const addLabel = anySignedIn ? `Add another ${label} account` : `Add a ${label} account`;

  return (
    <>
      {rows}
      {terminal ? (
        <div className="px-4 py-3">{terminal}</div>
      ) : (
        <button
          type="button"
          aria-label={addLabel}
          disabled={views === null}
          onClick={() => start(null)}
          className="text-link text-label flex w-full items-center gap-2 px-4 py-3 font-medium hover:underline disabled:opacity-50"
        >
          <Plus aria-hidden className="size-icon-sm" />
          {addLabel}
        </button>
      )}
    </>
  );
}

/** What an account is called and who signed in, masked unless its row is revealed. */
export function accountNames(
  view: AccountView,
  revealed: boolean,
): { title: string; email: string | null; editable: string } {
  const email = view.status?.email ?? null;
  const renamed = view.name !== "" && view.name !== email;
  const show = (text: string) => (revealed ? text : maskEmailsIn(text));
  const title = renamed ? show(view.name) : email ? show(email) : accountLabel(view);
  return {
    title,
    // Under a name of its own, the email shows on the line below (D2).
    email: renamed && email ? show(email) : null,
    // The name to edit: never one taken from the email, and an email-like one only revealed.
    editable: !renamed || (hasEmail(view.name) && !revealed) ? "" : view.name,
  };
}

const OWN_LOGIN = "This computer's login";

/** Which account rows are open: the page's own state, the first row open until it is closed (D7). */
export type OpenRows = { has: (key: string, first: boolean) => boolean; toggle: (key: string, first: boolean) => void };

/** The account rows open, kept only while the page showing them is open; several may be. */
export function useOpenRows(): OpenRows {
  const [toggled, setToggled] = useState<ReadonlyMap<string, boolean>>(() => new Map());
  return useMemo(() => {
    const has = (key: string, first: boolean) => toggled.get(key) ?? first;
    return {
      has,
      toggle: (key: string, first: boolean) =>
        setToggled((current) => new Map(current).set(key, !(current.get(key) ?? first))),
    };
  }, [toggled]);
}

/**
 * One account (D2, D7, D11, D14): a chevron, its initial, who it is (its name, else its email,
 * masked until its eye is pressed) over its email, plan and whether it is this computer's login;
 * closed, its tightest limit; then In use where the others have Use (or Sign in), and its ⋯
 * menu. A press on the row opens it to every limit with when each resets; its controls don't.
 */
function AccountRow({
  view,
  ownLogin,
  now,
  busy,
  open,
  onToggle,
  revealed,
  onReveal,
  onSignIn,
}: {
  view: AccountView;
  /** It is this computer's login, itself or an account added here that is the same one. */
  ownLogin: boolean;
  now: number;
  busy: boolean;
  open: boolean;
  onToggle: () => void;
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
  const limitsId = useId();
  const status = view.status;
  const signedIn = status?.loggedIn === true;
  const signsIn = !signedIn && !own && Boolean(status?.path);
  const { title, email, editable } = accountNames(view, revealed);
  const hidden = hasEmail(view.name) || Boolean(status?.email);
  const windows = signedIn && view.quota ? ordered(view.quota) : [];
  // Only an account with limits read opens (signed out, or usage not read yet: nothing to show).
  const opens = windows.length > 0;
  const shown = opens && open;
  const order = signedIn ? orderLine(view.place, view.reason, now) : null;

  const eye = hidden && (
    <Button
      type="button"
      size="icon-xs"
      variant="ghost"
      aria-label={revealed ? "Hide email" : "Show email"}
      aria-pressed={revealed}
      className="text-muted-foreground size-5 shrink-0"
      onClick={onReveal}
    >
      {revealed ? <EyeOff /> : <Eye />}
    </Button>
  );
  const line: ReactNode[] = [];
  if (!status) line.push("Checking…");
  else if (!status.path) line.push(`${PROVIDER_LABELS[provider]} isn't installed`);
  else {
    if (email)
      line.push(
        <span key="email" className="flex min-w-0 items-center gap-1.5">
          <span data-slot="account-email" className="truncate">
            {email}
          </span>
          {eye}
        </span>,
      );
    if (!signedIn) line.push("Not signed in");
    else if (status.plan) line.push(`${capitalize(status.plan)} plan`);
  }
  // Called "This computer's login" already (no name, no email), the line doesn't say it again.
  if (ownLogin && title !== OWN_LOGIN) line.push(OWN_LOGIN);

  const tightest = windows.reduce<QuotaWindow | null>((top, window) => (top && used(top) >= used(window) ? top : window), null);
  const usage = !status || !status.path || !signedIn ? null : view.quotaAtMs === null || !tightest ? (
    <span className={ROW_LINE}>Usage not read yet</span>
  ) : (
    !shown && <TightestUsage window={tightest} />
  );

  const menu = (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          size="icon-xs"
          variant="ghost"
          aria-label={`${title}: more`}
          className="text-muted-foreground data-[state=open]:bg-toolbar-hover data-[state=open]:text-foreground shrink-0"
          disabled={action.busy}
        >
          <DotsHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={() => setRenaming(true)}>Rename</DropdownMenuItem>
        {/* Signed out, Sign in is on the row (D12); the computer's own login signs in in a terminal. */}
        {!own && signedIn && (
          <DropdownMenuItem disabled={busy} onSelect={() => onSignIn(id)}>
            Sign in again
          </DropdownMenuItem>
        )}
        {!own && (
          <DropdownMenuItem variant="destructive" onSelect={() => action.run(() => removeAccount(id))}>
            Remove
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );

  // A press on the row toggles it, but not one on its controls (the eye, Use, ⋯, the name being
  // edited) or in its menu, which React passes up from the menu's own layer.
  const onClick = (event: MouseEvent<HTMLDivElement>) => {
    if (!opens || !(event.target instanceof Element) || !event.currentTarget.contains(event.target)) return;
    const control = event.target.closest("button, input, a");
    if (control && control.getAttribute("data-slot") !== "account-toggle") return;
    onToggle();
  };

  return (
    <div data-slot="account-row" data-open={shown || undefined} className="flex flex-col">
      {/* oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions -- the button over it is the keyboard's and a screen reader's */}
      <div
        data-slot="account-header"
        onClick={onClick}
        className={cn(
          "group/account relative flex cursor-default items-center gap-4 px-4 py-3 transition-colors",
          opens && "hover:bg-toolbar-hover",
        )}
      >
        {/* What opens the row for the keyboard and a screen reader: a real button over the
            header, beside its controls rather than around them; the pointer goes through it to
            the header. */}
        {opens && (
          <button
            type="button"
            data-slot="account-toggle"
            aria-expanded={shown}
            aria-controls={limitsId}
            aria-label={`${title}: ${shown ? "hide" : "show"} its limits`}
            className="pointer-events-none absolute inset-0"
          />
        )}
        <span className="flex shrink-0 items-center gap-2">
          <span aria-hidden className="flex size-3.5 items-center justify-center">
            {opens &&
              (shown ? (
                <ChevronDown className="text-muted-foreground group-hover/account:text-foreground size-3.5" />
              ) : (
                <ChevronRight className="text-muted-foreground group-hover/account:text-foreground size-3.5" />
              ))}
          </span>
          <span
            aria-hidden
            className="bg-foreground/10 text-foreground text-label flex size-8 items-center justify-center rounded-full font-medium"
          >
            {initial(title)}
          </span>
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.75">
          {renaming ? (
            <RenameField
              name={editable}
              onDone={(name) => {
                setRenaming(false);
                if (name !== null && name !== view.name) action.run(() => renameAccount(provider, id, name));
              }}
            />
          ) : (
            <span className="flex min-w-0 items-center gap-1.5">
              <span data-slot="account-title" className={cn(ROW_TITLE, "truncate")}>
                {title}
              </span>
              {!email && eye}
            </span>
          )}
          <span data-slot="account-line" className={cn(ROW_LINE, "text-muted-foreground flex min-w-0 items-center gap-1.5")}>
            {line.map((part, index) => (
              <Fragment key={index}>
                {index > 0 && <span aria-hidden>·</span>}
                {typeof part === "string" ? <span className="shrink-0">{part}</span> : part}
              </Fragment>
            ))}
          </span>
          {order && <span data-slot="account-order" className={cn(ROW_LINE, "truncate")}>{order}</span>}
          {view.error && <span className="text-destructive text-xs">{view.error}</span>}
          <ErrorLine error={action.error} />
        </div>
        <span className="flex w-37.5 shrink-0 items-center justify-end">{usage}</span>
        <div className="flex w-24 shrink-0 items-center justify-end gap-2.5">
          {signsIn ? (
            <Button variant="soft" size="sm" disabled={busy} onClick={() => id && onSignIn(id)}>
              Sign in
            </Button>
          ) : view.inUse ? (
            <Badge data-slot="in-use" variant="success" title="New chats start on this account" className="shrink-0">
              In use
            </Badge>
          ) : (
            <Button
              variant="soft"
              size="sm"
              disabled={action.busy || !signedIn}
              title="Start new chats on this account"
              onClick={() => action.run(() => makeDefaultAccount(provider, id))}
            >
              Use
            </Button>
          )}
          {menu}
        </div>
      </div>
      {shown && (
        <div id={limitsId} data-slot="account-limits" className="flex flex-col gap-2.5 pr-16 pb-4 pl-21.5">
          {windows.map((window) => (
            <LimitLine key={window.id} window={window} now={now} />
          ))}
        </div>
      )}
    </div>
  );
}

const PLACES = ["First", "Second", "Third"];

/** Where an account stands in the engine's routing order, and why (D23). */
export function orderLine(place: number | null, reason: OrderReason | null, now: number): string | null {
  if (reason === null) return null;
  const nth = place === null ? null : (PLACES[place - 1] ?? `${place}th`);
  switch (reason.kind) {
    case "picked":
      return "Picked with Use: work goes here first";
    case "resetsSooner":
      return `${nth} · weekly limit resets soonest, in ${formatCountdown(reason.resetsAtMs, now)}`;
    case "nearerLimit":
      return `${nth} · nearest its ${reason.window.toLowerCase()} limit, ${Math.round(reason.usedPercent)}% used`;
    case "added":
      return `${nth} · added earlier`;
    case "last":
      return place === 1 ? "First" : `${nth} · after the others`;
    case "atLimit":
      return reason.readyAtMs === undefined
        ? "At its limit: waits for its reset"
        : `At its limit: back in ${formatCountdown(reason.readyAtMs, now)}`;
    case "signedOut":
      return null;
  }
}

/** A closed row's figure: its fullest limit's name, a short bar and the share used. */
function TightestUsage({ window }: { window: QuotaWindow }) {
  const percent = used(window);
  const tone = usageTone(percent);
  return (
    <span
      data-slot="account-usage"
      className="flex min-w-0 items-center gap-2 text-xs tabular-nums"
      title={`${windowTitle(window)}: ${percent}% used`}
    >
      <span className="text-muted-foreground truncate">{windowTitle(window)}</span>
      <UsageBar value={percent} fill={tone.fill} className="w-10 shrink-0" />
      <span data-slot="usage-value" className={cn("font-medium", tone.text)}>
        {percent}%
      </span>
    </span>
  );
}

/** An open row's limit: its name, a bar, the share used and when it resets. */
function LimitLine({ window, now }: { window: QuotaWindow; now: number }) {
  const percent = used(window);
  const tone = usageTone(percent);
  return (
    <span data-slot="account-limit" className="flex items-center gap-3 text-xs tabular-nums">
      <span className="text-foreground w-33 shrink-0 truncate">{windowTitle(window)}</span>
      {/* The design's 340 bar, narrower in a narrower window before the reset gives way. */}
      <UsageBar value={percent} fill={tone.fill} className="max-w-85 min-w-16 flex-1 shrink" />
      <span data-slot="usage-value" className={cn("w-18 shrink-0 text-right font-medium", tone.text)}>
        {percent}% used
      </span>
      <span className="text-muted-foreground w-36 shrink-0 truncate">
        {window.resetsAtMs !== null && resetsWords(window.resetsAtMs, now)}
      </span>
    </span>
  );
}

/** When a limit resets: "Resets in 2h 14m" within a day, else its day and time ("Resets Tue 9:00 AM"). */
export function resetsWords(resetsAtMs: number, now: number): string {
  if (resetsAtMs - now < DAY_MS) return `Resets in ${formatCountdown(resetsAtMs, now)}`;
  return `Resets ${WEEKDAY.format(resetsAtMs)} ${formatTime(resetsAtMs)}`;
}

const DAY_MS = 86_400_000;
const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: "short" });

/**
 * The account's name, editable: Enter or leaving the field keeps it (emptied, the account goes
 * by its email again), Escape drops the edit (`null`).
 */
function RenameField({ name, onDone }: { name: string; onDone: (name: string | null) => void }) {
  const [value, setValue] = useState(name);
  const done = useRef(false);
  const finish = (next: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(next === null ? null : next.trim());
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
        if (event.key === "Escape") {
          // Esc here drops the edit rather than leaving Settings.
          event.stopPropagation();
          finish(null);
        }
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
