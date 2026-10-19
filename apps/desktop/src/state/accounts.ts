import type { AccountEntry, AccountView, ProviderKind, TerminalInfo } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { editSettings } from "@/state/settings";
import { useApp } from "@/state/store";

/*
 * The user's accounts of each agent: their own login (the one their terminal uses too) and
 * the extra ones they signed in to here. Each extra account's login lives in a folder of its
 * own that the agent's CLI keeps; Brigadier only asks the CLI who is signed in.
 */

/** Reads every account; `accountsChecked` events keep it current. */
export async function loadAccounts(): Promise<void> {
  const { accounts } = await request({ method: "getAccounts" });
  useApp.setState({ accounts });
}

/**
 * Checks every account's login and quota again; results arrive as events. Answers the check
 * generation each account reaches once checked afresh (`AccountView.checkedGeneration`).
 */
export async function refreshAccounts(): Promise<number> {
  const { generation } = await request({ method: "refreshAccounts" });
  return generation;
}

/** Sign-ins being opened, by what they open. */
const opening = new Map<string, Promise<unknown>>();

/**
 * Runs `open` unless the same sign-in is already being opened, in which case its result is
 * shared: a terminal view mounted twice (React does so in development) or a double click
 * must not add two accounts or open two browser sign-ins.
 */
function once<T>(key: string, open: () => Promise<T>): Promise<T> {
  const pending = opening.get(key);
  if (pending) return pending as Promise<T>;
  const started = open().finally(() => opening.delete(key));
  opening.set(key, started);
  return started;
}

/** Adds an account of `provider` and opens a terminal signing in to it. */
export function addAccount(
  provider: ProviderKind,
  cols: number,
  rows: number,
): Promise<{ account: AccountEntry; terminal: TerminalInfo }> {
  return once(`add:${provider}`, async () => {
    const { account, terminal } = await request({ method: "addAccount", provider, cols, rows });
    return { account, terminal };
  });
}

/** Opens a terminal signing in to the extra account `id` again. */
export function signInAccount(id: string, cols: number, rows: number): Promise<TerminalInfo> {
  return once(`signIn:${id}`, async () => {
    const { terminal } = await request({ method: "signInAccount", id, cols, rows });
    return terminal;
  });
}

/** Signs the extra account `id` out and forgets it. */
export async function removeAccount(id: string): Promise<void> {
  await request({ method: "removeAccount", id });
}

/** Renames the extra account `id`. */
export function renameAccount(id: string, name: string): Promise<unknown> {
  return editSettings((settings) => ({
    ...settings,
    accounts: settings.accounts.map((entry) => (entry.id === id ? { ...entry, name } : entry)),
  }));
}

/** Makes new work of `provider` start on the account `id` (absent: the user's own login). */
export function makeDefaultAccount(provider: ProviderKind, id: string | undefined): Promise<unknown> {
  return editSettings((settings) => ({
    ...settings,
    accounts: settings.accounts.map((entry) =>
      entry.provider === provider ? { ...entry, default: entry.id === id } : entry,
    ),
  }));
}

/** What an account is called: its name, else who is signed in; "This computer's login" for the own one. */
export function accountLabel(view: AccountView): string {
  if (view.account.account === undefined) return "This computer's login";
  return view.name || view.status?.email || "New account";
}

/** Who a signed-in account is: its email (any case) in its organization; `null` until known. */
function loginOf(view: AccountView): string | null {
  const email = view.status?.loggedIn ? view.status.email : undefined;
  return email ? `${email.toLowerCase()}\u0000${view.status?.organization ?? ""}` : null;
}

/** An account's row: the account, and whether it is this computer's login too. */
export type AccountRowView = { view: AccountView; ownLogin: boolean };

/**
 * One row per account. When this computer's login is signed in to an account also added here,
 * the two are one row in the login's place: the added account's, saying it is this computer's
 * login, in use (and the default) when either is.
 */
export function oneRowPerAccount(views: readonly AccountView[]): AccountRowView[] {
  const twins = new Map<AccountView, AccountView>();
  for (const own of views.filter((view) => view.account.account === undefined)) {
    const login = loginOf(own);
    const twin = login
      ? views.find(
          (view) =>
            view.account.account !== undefined &&
            view.account.provider === own.account.provider &&
            loginOf(view) === login,
        )
      : undefined;
    if (twin) twins.set(own, twin);
  }
  const merged = new Set(twins.values());
  return views.flatMap((view) => {
    if (merged.has(view)) return [];
    const twin = twins.get(view);
    if (!twin) return [{ view, ownLogin: view.account.account === undefined }];
    return [{ view: { ...twin, default: twin.default || view.default, inUse: twin.inUse || view.inUse }, ownLogin: true }];
  });
}

/** The accounts of `provider`, the user's own login first. */
export function accountsOf(
  views: readonly AccountView[] | undefined,
  provider: ProviderKind,
): AccountView[] {
  return (views ?? []).filter((view) => view.account.provider === provider);
}
