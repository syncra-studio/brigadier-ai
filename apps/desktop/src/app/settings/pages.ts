import {
  Archive,
  Brain,
  Branch,
  Cursor,
  Dock,
  Download,
  Globe,
  KeyboardShortcut,
  Mic,
  Robot,
  SettingsCog,
  SettingsSlider,
  Shuffle,
  Storage,
} from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentType, SVGProps } from "react";

import { ARCHIVED_ROWS, ArchivedPage } from "@/app/settings/ArchivedPage";
import { BROWSER_IMPORT_ROWS, BrowserImportPage } from "@/app/settings/BrowserImportPage";
import { BROWSER_ROWS, BrowserPage } from "@/app/settings/BrowserPage";
import { COMPUTER_USE_ROWS, ComputerUsePage, computerUseAvailable } from "@/app/settings/ComputerUsePage";
import { CONFIGURATION_ROWS, ConfigurationPage } from "@/app/settings/ConfigurationPage";
import { GENERAL_ROWS, GeneralPage } from "@/app/settings/GeneralPage";
import { GIT_ROWS, GitPage } from "@/app/settings/GitPage";
import { MEMORY_ROWS, MemoryPage } from "@/app/settings/MemoryPage";
import { STORAGE_ROWS, StoragePage } from "@/app/settings/storage/StoragePage";
import { VOICE_ROWS, VoicePage } from "@/app/settings/VoicePage";
import { SHORTCUTS_ROWS, ShortcutsPage } from "@/app/settings/ShortcutsPage";
import { PROVIDERS_ROWS, ProvidersPage } from "@/app/providers/ProvidersPage";
import { ROUTING_PAGE_ROWS, RoutingPage } from "@/app/routing/RoutingPage";
import { preloadable } from "@/lib/preloadable";
import { prefetchImportSources } from "@/state/browserImport";
import { type SettingsPageId, useApp } from "@/state/store";
import { prefetchUsage } from "@/state/usage";

/**
 * The pages of Settings, in the order the navigation lists them, each in its group. A page's
 * rows (label and description) are what Settings search finds; each page module exports the
 * copy it renders, so the two never drift apart.
 */

/** A row search finds; `target` is the row it brings into view, when that is another one. */
export type SettingsRowCopy = {
  readonly label: string;
  readonly description?: string;
  readonly target?: string;
};

/** A group of the navigation; "end" is the last, set apart by its gap alone, without a title. */
export type SettingsGroup = "Personal" | "Agents" | "Browser & Data" | "System" | "end";

export type SettingsPageEntry = {
  id: SettingsPageId;
  label: string;
  /** Names the page went by, which search still finds it under. */
  formerly?: readonly string[];
  icon: ComponentType<SVGProps<SVGSVGElement>>;
  group: SettingsGroup;
  component: ComponentType;
  rows: readonly SettingsRowCopy[];
  /** Whether the page shows on this system; every page does unless it says otherwise. */
  shown?: () => boolean;
  /**
   * Gets what the page shows ready before it opens (its code, or a cheap read of its data), as
   * the pointer comes to its row: the page then shows it on its first frame.
   */
  prefetch?: () => void;
};

// The Inspector (a developer view) loads when first wanted, off the cold-start path: once
// Settings is open and idle, or as the pointer comes to its row.
const inspector = preloadable(() =>
  import("@/app/inspector/InspectorPage").then((module) => module.InspectorPage),
);

// What search finds on the Inspector's page: its tabs.
const INSPECTOR_ROWS: readonly SettingsRowCopy[] = [
  { label: "Events", description: "The live event stream" },
  { label: "Orchestrator", description: "The open session's orchestrator context" },
  { label: "Project Brains", description: "What each project's Brain holds" },
  { label: "Processes", description: "The processes Brigadier runs" },
  { label: "Performance", description: "Metrics against the performance budgets" },
  { label: "Providers", description: "Raw provider sessions" },
  { label: "Routing preview", description: "What routing would pick" },
];

export const SETTINGS_GROUPS: readonly SettingsGroup[] = ["Personal", "Agents", "Browser & Data", "System", "end"];

/** A group's title in the navigation; the last group has none. */
export function groupTitle(group: SettingsGroup): string | undefined {
  return group === "end" ? undefined : group;
}

export const SETTINGS_PAGES: readonly SettingsPageEntry[] = [
  {
    id: "general",
    label: "General",
    icon: SettingsCog,
    group: "Personal",
    component: GeneralPage,
    rows: Object.values(GENERAL_ROWS),
  },
  {
    id: "voice",
    label: "Voice",
    icon: Mic,
    group: "Personal",
    component: VoicePage,
    rows: Object.values(VOICE_ROWS),
  },
  {
    id: "configuration",
    label: "Configuration",
    formerly: ["Configurations", "Conversations", "Personalization"],
    icon: SettingsSlider,
    group: "Personal",
    component: ConfigurationPage,
    rows: Object.values(CONFIGURATION_ROWS),
  },
  {
    id: "memory",
    label: "Memory",
    formerly: ["Memories"],
    icon: Brain,
    group: "Personal",
    component: MemoryPage,
    rows: Object.values(MEMORY_ROWS),
  },
  {
    id: "shortcuts",
    label: "Keyboard shortcuts",
    icon: KeyboardShortcut,
    group: "Personal",
    component: ShortcutsPage,
    rows: SHORTCUTS_ROWS,
  },
  {
    id: "providers",
    label: "Providers",
    formerly: ["Accounts", "Usage"],
    icon: Robot,
    group: "Agents",
    component: ProvidersPage,
    rows: Object.values(PROVIDERS_ROWS),
    prefetch: prefetchUsage,
  },
  {
    id: "routing",
    label: "Routing",
    formerly: ["Who does what"],
    icon: Shuffle,
    group: "Agents",
    component: RoutingPage,
    rows: Object.values(ROUTING_PAGE_ROWS),
    prefetch: prefetchUsage,
  },
  {
    id: "git",
    label: "Git",
    icon: Branch,
    group: "Agents",
    component: GitPage,
    rows: Object.values(GIT_ROWS),
  },
  {
    id: "computerUse",
    label: "Computer use",
    icon: Cursor,
    group: "Agents",
    component: ComputerUsePage,
    rows: Object.values(COMPUTER_USE_ROWS),
    // Only on macOS.
    shown: computerUseAvailable,
  },
  {
    id: "browser",
    label: "Browser",
    icon: Globe,
    group: "Browser & Data",
    component: BrowserPage,
    rows: Object.values(BROWSER_ROWS),
  },
  {
    id: "browserImport",
    label: "Import Browser Data",
    icon: Download,
    group: "Browser & Data",
    component: BrowserImportPage,
    rows: Object.values(BROWSER_IMPORT_ROWS),
    prefetch: prefetchImportSources,
    // Importing reads other browsers' macOS profiles into WebKit's store: macOS only.
    shown: () => useApp.getState().info?.platform === "macos",
  },
  {
    id: "storage",
    label: "Storage",
    icon: Storage,
    group: "System",
    component: StoragePage,
    rows: Object.values(STORAGE_ROWS),
  },
  {
    id: "inspector",
    label: "Inspector",
    icon: Dock,
    group: "System",
    component: inspector.View,
    rows: INSPECTOR_ROWS,
    prefetch: () => void inspector.preload().catch((error: unknown) => console.error("loading the Inspector failed", error)),
  },
  {
    id: "archived",
    label: "Archived chats",
    icon: Archive,
    group: "end",
    component: ArchivedPage,
    rows: Object.values(ARCHIVED_ROWS),
  },
];

/**
 * Once Settings shows and the app is idle: the pages' code that loads apart (the Inspector) and
 * the usage read Usage and Providers show, if there is none yet.
 */
export function prefetchSettingsPages(): void {
  settingsPage("inspector").prefetch?.();
  prefetchUsage();
}

export function settingsPage(id: SettingsPageId): SettingsPageEntry {
  // Every id has an entry: the list is written out above.
  return SETTINGS_PAGES.find((page) => page.id === id) ?? SETTINGS_PAGES[0]!;
}

/** The pages that show on this system. */
export function shownSettingsPages(): SettingsPageEntry[] {
  return SETTINGS_PAGES.filter((page) => page.shown?.() ?? true);
}

export type SettingsSearchResult = { page: SettingsPageEntry; row: SettingsRowCopy | null };

/** Pages and rows whose words contain every word of the query, pages first. */
export function searchSettings(query: string): SettingsSearchResult[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const matches = (text: string) => {
    const haystack = text.toLowerCase();
    return words.every((word) => haystack.includes(word));
  };
  const results: SettingsSearchResult[] = [];
  const pages = shownSettingsPages();
  for (const page of pages) {
    if (matches([page.label, ...(page.formerly ?? [])].join(" "))) results.push({ page, row: null });
  }
  for (const page of pages) {
    for (const row of page.rows) {
      if (matches(`${row.label} ${row.description ?? ""}`)) results.push({ page, row });
    }
  }
  return results;
}
