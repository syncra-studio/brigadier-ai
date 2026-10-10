import {
  Archive,
  AvatarProfile,
  Branch,
  Cursor,
  Robot,
  Shuffle,
  SettingsCog,
  SettingsSlider,
  Storage,
  Terminal,
  Usage,
} from "@openai/apps-sdk-ui/components/Icon";
import { lazy, type ComponentType, type SVGProps } from "react";

import { ACCOUNTS_ROWS, AccountsPage } from "@/app/settings/AccountsPage";
import { ARCHIVED_ROWS, ArchivedPage } from "@/app/settings/ArchivedPage";
import { COMPUTER_USE_ROWS, ComputerUsePage } from "@/app/settings/ComputerUsePage";
import { CONFIGURATION_ROWS, ConfigurationPage } from "@/app/settings/ConfigurationPage";
import { GENERAL_ROWS, GeneralPage } from "@/app/settings/GeneralPage";
import { GIT_ROWS, GitPage } from "@/app/settings/GitPage";
import { STORAGE_ROWS, StoragePage } from "@/app/settings/StoragePage";
import { PROVIDERS_ROWS, ProvidersPage } from "@/app/providers/ProvidersPage";
import { ROUTING_PAGE_ROWS, RoutingPage } from "@/app/routing/RoutingPage";
import { computerUseAvailable } from "@/state/computerAccess";
import type { SettingsPageId } from "@/state/store";

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

export type SettingsGroup = "Personal" | "Agents" | "System";

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
};

// The Usage page (charts and all) and the Inspector (a developer view) load when first
// opened, off the cold-start path.
const UsagePage = lazy(() =>
  import("@/app/usage/UsagePage").then((module) => ({ default: module.UsagePage })),
);

const InspectorPage = lazy(() =>
  import("@/app/inspector/InspectorPage").then((module) => ({ default: module.InspectorPage })),
);

// What search finds on the Inspector's page: its tabs.
const INSPECTOR_ROWS: readonly SettingsRowCopy[] = [
  { label: "Events", description: "The live event stream." },
  { label: "Orchestrator", description: "The open session's orchestrator context." },
  { label: "Project Brains", description: "What each project's Brain holds." },
  { label: "Processes", description: "The processes Brigadier runs." },
  { label: "Performance", description: "Metrics against the performance budgets." },
  { label: "Providers", description: "Raw provider sessions." },
  { label: "Routing preview", description: "What routing would pick." },
];

const USAGE_ROWS: readonly SettingsRowCopy[] = [
  { label: "Usage windows", description: "How much of each agent's usage is left." },
];

export const SETTINGS_GROUPS: readonly SettingsGroup[] = ["Personal", "Agents", "System"];

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
    id: "archived",
    label: "Archived chats",
    icon: Archive,
    group: "Personal",
    component: ArchivedPage,
    rows: Object.values(ARCHIVED_ROWS),
  },
  {
    id: "storage",
    label: "Storage",
    icon: Storage,
    group: "Personal",
    component: StoragePage,
    rows: Object.values(STORAGE_ROWS),
  },
  {
    id: "configuration",
    label: "Configurations",
    formerly: ["Conversations", "Personalization"],
    icon: SettingsSlider,
    group: "Personal",
    component: ConfigurationPage,
    rows: Object.values(CONFIGURATION_ROWS),
  },
  {
    id: "usage",
    label: "Usage",
    icon: Usage,
    group: "Personal",
    component: UsagePage,
    rows: USAGE_ROWS,
  },
  {
    id: "providers",
    label: "Providers",
    icon: Robot,
    group: "Agents",
    component: ProvidersPage,
    rows: Object.values(PROVIDERS_ROWS),
  },
  {
    id: "accounts",
    label: "Accounts",
    icon: AvatarProfile,
    group: "Agents",
    component: AccountsPage,
    rows: Object.values(ACCOUNTS_ROWS),
  },
  {
    id: "routing",
    label: "Routing",
    icon: Shuffle,
    group: "Agents",
    component: RoutingPage,
    rows: Object.values(ROUTING_PAGE_ROWS),
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
    // Only where the system has computer use (macOS).
    shown: computerUseAvailable,
  },
  {
    id: "inspector",
    label: "Inspector",
    icon: Terminal,
    group: "System",
    component: InspectorPage,
    rows: INSPECTOR_ROWS,
  },
];

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
