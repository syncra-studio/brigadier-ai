import type { AccountEntry, AccountView, ProviderKind, TerminalInfo } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { editSettings } from "@/state/settings";
import { useApp } from "@/state/store";

/*
 * The user's accounts of each agent: their own login (the one their terminal uses too) and
 * the extra ones they signed in to here. Each extra account's login lives in a folder of its
 * own that the agent's CLI keeps; Brigadier only asks the CLI who is signed in.
 */

/** The id a model choice names for the user's own login. */
export const OWN_ACCOUNT = "own";

/** Reads every account; `accountsChecked` events keep it current. */
export async function loadAccounts(): Promise<void> {
  const { accounts } = await request({ method: "getAccounts" });
  useApp.setState({ accounts });
}

/** Checks every account's login and quota again; results arrive as events. */
export async function refreshAccounts(): Promise<void> {
  await request({ method: "refreshAccounts" });
}

/** Adds an account of `provider` and opens a terminal signing in to it. */
export async function addAccount(
  provider: ProviderKind,
  cols: number,
  rows: number,
): Promise<{ account: AccountEntry; terminal: TerminalInfo }> {
  const { account, terminal } = await request({ method: "addAccount", provider, cols, rows });
  return { account, terminal };
}

/** Opens a terminal signing in to the extra account `id` again. */
export async function signInAccount(id: string, cols: number, rows: number): Promise<TerminalInfo> {
  const { terminal } = await request({ method: "signInAccount", id, cols, rows });
  return terminal;
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

/** The accounts of `provider`, the user's own login first. */
export function accountsOf(
  views: readonly AccountView[] | undefined,
  provider: ProviderKind,
): AccountView[] {
  return (views ?? []).filter((view) => view.account.provider === provider);
}

/** The id a model choice names for an account (`own` for the user's own login). */
export function choiceAccount(view: AccountView): string {
  return view.account.account ?? OWN_ACCOUNT;
}
