/**
 * Settings › Browser › Saved passwords and Bookmarks, the save prompt, the toolbar's star and the delete dialog's line,
 * over a faked shell and made-up data. It presses through them with real clicks and writes what it saw to
 * #passwords-result (BrowserPasswordsRender.test.ts).
 */
import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserPage } from "@/app/settings/BrowserPage";
import { BrowserPasswordsPrompts } from "@/app/settings/BrowserPasswordsPrompts";
import { ProfileDeleteDialog } from "@/app/settings/ProfileDeleteDialog";
import { BookmarkStar } from "@/app/side-panel/BookmarkStar";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick } from "@/fixtures/input";
import { mockIPC } from "@/ipc/nativeMocks";
import type { Bookmark } from "@/state/browserBookmarks";
import { type Reveal, showSaveOffer, useSaveOffers } from "@/state/browserPasswords";
import { showBrowserSettingsView } from "@/state/browserSettings";
import { useProfilePlaces } from "@/state/browserProfilePlaces";
import { useApp } from "@/state/store";

const still = document.createElement("style");
still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
document.head.append(still);

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true },
  selection: { type: "settings", page: "browser" },
}));
useProfilePlaces.setState({
  profiles: [{ id: "personal", name: "Personal", hue: 4 }, { id: "work", name: "Work", hue: 6 }],
  defaultId: "personal",
  sessions: {},
  tabs: {},
});

const LOGINS: Record<string, { id: number; site: string; username: string }[]> = {
  personal: [
    { id: 1, site: "https://www.forum.example", username: "fake-sam" },
    { id: 2, site: "https://mail.example", username: "fake-kim" },
  ],
  work: [{ id: 3, site: "https://tickets.example", username: "fake-work" }],
};
const NEVER: Record<string, string[]> = { personal: ["https://bank.example"], work: [] };
const MARKS: Record<string, Bookmark[]> = {
  personal: [
    { id: 1, url: "https://docs.example/", title: "Docs", folders: ["Bookmarks bar"], addedMs: 1 },
    { id: 2, url: "https://board.example/", title: "Board", folders: ["Bookmarks bar", "Work"], addedMs: 2 },
  ],
  work: [],
};
/** What the system's check answers next. */
const reveals: Reveal[] = [
  { kind: "declined" },
  { kind: "unavailable" },
  { kind: "shown", password: "fake-Tulip-41" },
];
const asked: unknown[] = [];
let nextId = 100;

mockIPC((command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  const profile = args.profile as string;
  switch (command) {
    case "ipc_request":
      return { method: (args.request as { method: string }).method };
    case "browser_site_count":
      return 0;
    case "browser_passwords":
      return LOGINS[profile] ?? [];
    case "browser_password_never_sites":
      return NEVER[profile] ?? [];
    case "browser_password_reveal":
      return reveals.shift();
    case "browser_password_remove":
      asked.push(["remove password", args.id]);
      LOGINS[profile] = (LOGINS[profile] ?? []).filter(({ id }) => id !== args.id);
      return null;
    case "browser_password_allow_site":
      asked.push(["allow", args.site]);
      NEVER[profile] = (NEVER[profile] ?? []).filter((site) => site !== args.site);
      return null;
    case "browser_password_offer_answer":
      asked.push(["answer", args.offer, args.answer]);
      return null;
    case "browser_bookmarks":
      return MARKS[profile] ?? [];
    case "browser_bookmark_add": {
      const added = { ...(args.bookmark as Bookmark), id: nextId++, addedMs: 3 };
      MARKS[profile]?.push(added);
      asked.push(["add bookmark", added.url, added.title, added.folders]);
      return added;
    }
    case "browser_bookmark_remove":
      asked.push(["remove bookmark", args.id]);
      MARKS[profile] = (MARKS[profile] ?? []).filter(({ id }) => id !== args.id);
      return null;
    default:
      return null;
  }
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const text = () => document.body.textContent ?? "";
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const named = (name: string): HTMLElement | undefined =>
  all("button, [role=menuitem]").find((element) => element.getAttribute("aria-label") === name || element.textContent?.trim() === name);

async function until<T>(what: string, read: () => T | null | undefined | false): Promise<T> {
  for (let tries = 0; tries < 100; tries += 1) {
    const value = read();
    if (value) return value;
    await wait(50);
  }
  throw new Error(`Never came: ${what}`);
}

/** A real click on the named thing, once it shows. */
async function press(name: string): Promise<void> {
  await fixtureClick(await until(name, () => named(name)));
  await wait(60);
}

const rows = () => all('[data-slot="password-row"]');
const value = () => rows()[0]?.querySelector('[data-slot="password-value"]')?.textContent ?? null;
const prompt = () => document.querySelector<HTMLElement>('body > [data-above-page] [data-slot="save-password-prompt"]');
const star = () => document.querySelector<HTMLElement>('[data-slot="bookmark-star"]');
const starState = () => {
  const button = star();
  return button && { pressed: button.getAttribute("aria-pressed"), name: button.textContent, icon: button.querySelector("svg")?.outerHTML ?? null };
};

async function passwords(seen: Record<string, unknown>): Promise<void> {
  seen.pageSections = await until("the page", () => all("section h2").length > 0 && all("section h2").map((one) => one.textContent));
  showBrowserSettingsView("passwords");
  await until("rows", () => rows().length > 0);
  seen.sites = rows().map((row) => row.querySelector("[title]")?.textContent);
  seen.masked = value();
  seen.neverSites = text().includes("bank.example");

  await press("Show password");
  await until("declined", () => text().includes("stays hidden"));
  seen.declined = { message: text().includes("didn't confirm it's you"), value: value() };
  await press("Show password");
  await until("unavailable", () => text().includes("can't ask for Touch ID"));
  seen.unavailable = value();
  await press("Show password");
  await until("shown", () => value() !== "••••••••");
  seen.shown = value();
  await press("Hide password");
  seen.hidden = value();

  await press("More for fake-kim on mail.example");
  await press("Remove password");
  seen.removeAsks = await until("confirm", () => text().includes("won't fill any more"));
  await press("Remove");
  await until("row gone", () => rows().length === 1);

  await press("Offer again");
  await until("never gone", () => !text().includes("bank.example"));
  seen.passwordAsks = asked.splice(0);
}

async function bookmarks(seen: Record<string, unknown>): Promise<void> {
  showBrowserSettingsView("bookmarks");
  await until("bookmarks", () => all('[data-slot="bookmark-row"]').length > 0);
  seen.bookmarkRows = all('[data-slot="bookmark-row"]').length;
  seen.folders = ["Bookmarks bar", "Bookmarks bar › Work"].filter((label) => text().includes(label));
  await press("More for Board");
  await press("Remove bookmark");
  await until("bookmark gone", () => all('[data-slot="bookmark-row"]').length === 1);
  seen.bookmarkAsks = asked.splice(0);
}

async function offers(seen: Record<string, unknown>): Promise<void> {
  showSaveOffer({ id: "o1", tab: "tab", profile: "personal", site: "https://www.site.example", username: "fake-sam", update: false });
  const first = await until("the prompt", prompt);
  seen.offer = {
    says: first.textContent,
    armedAtOnce: !(named("Save") as HTMLButtonElement).disabled,
  };
  await until("armed", () => !(named("Save") as HTMLButtonElement).disabled);
  await press("Never for this site");
  seen.offerGone = prompt() === null && useSaveOffers.getState().offers.length === 0;

  showSaveOffer({ id: "o2", tab: "tab", profile: "personal", site: "https://www.site.example", username: "fake-sam", update: true });
  await until("armed update", () => named("Update") && !(named("Update") as HTMLButtonElement).disabled);
  seen.update = { says: prompt()?.textContent?.includes("Update this password?"), never: named("Never for this site") !== undefined };
  await press("Update");

  // An offer of another tab doesn't show over this one.
  showSaveOffer({ id: "o3", tab: "another", profile: "personal", site: "https://www.site.example", username: "fake-sam", update: false });
  await wait(100);
  seen.otherTab = prompt() === null;
  seen.offerAsks = asked.splice(0);
}

async function starStates(seen: Record<string, unknown>): Promise<void> {
  seen.starOff = await until("the star", () => starState());
  seen.starOnOtherPage = document.querySelector('[data-slot="not-web"] [data-slot="bookmark-star"]') === null;
  await fixtureClick(star()!);
  seen.starOn = await until("star on", () => starState()?.pressed === "true" && starState());
  await fixtureClick(star()!);
  await until("star off", () => starState()?.pressed === "false");
  seen.starOffAgain = starState();
  seen.starAsks = asked.splice(0);
}

/** Holds the things the page doesn't, in the fixture's own corner of the window. */
function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const page = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      await passwords(seen);
      await bookmarks(seen);
      await offers(seen);
      await starStates(seen);
      setDeleting(true);
      seen.deleteLine = await until("the dialog", () =>
        all('[data-slot="profile-delete"] p').map((line) => line.textContent).find((line) => line?.includes("saved passwords")));
    })().catch((cause: unknown) => { seen.failed = `${String(cause)} on: ${text().slice(0, 600)}`; }).then(() => setResult(JSON.stringify(seen)));
  }, []);

  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen flex-col">
        <div ref={page} data-slot="tab-area" className="flex h-44 shrink-0 items-start gap-2 p-2">
          <div data-slot="web"><BookmarkStar tab="tab" url="https://star.example/" title="Star page" /></div>
          <div data-slot="not-web"><BookmarkStar tab="tab" url="about:blank" title="" /></div>
        </div>
        <BrowserPasswordsPrompts tab="tab" anchor={page} />
        <main className="flex min-h-0 min-w-0 flex-1"><div className="min-w-0 flex-1"><BrowserPage /></div></main>
      </div>
      <ProfileDeleteDialog profile={{ id: "work", name: "Work", hue: 6 }} open={deleting} onOpenChange={setDeleting} />
      {result && <pre id="passwords-result">{result}</pre>}
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
