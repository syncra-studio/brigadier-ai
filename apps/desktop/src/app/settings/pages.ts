import {
  Archive,
  AvatarProfile,
  Branch,
  Chats,
  MemoryOnRemember,
  Robot,
  Shuffle,
  SettingsCog,
  Storage,
  Terminal,
  Usage,
} from "@openai/apps-sdk-ui/components/Icon";
import { lazy, type ComponentType, type SVGProps } from "react";

import { ACCOUNTS_ROWS, AccountsPage } from "@/app/settings/AccountsPage";
import { ARCHIVED_ROWS, ArchivedPage } from "@/app/settings/ArchivedPage";
import { CONVERSATIONS_ROWS, ConversationsPage } from "@/app/settings/ConversationsPage";
import { GENERAL_ROWS, GeneralPage } from "@/app/settings/GeneralPage";
import { GIT_ROWS, GitPage } from "@/app/settings/GitPage";
import { PERSONALIZATION_ROWS, PersonalizationPage } from "@/app/settings/PersonalizationPage";
import { STORAGE_ROWS, StoragePage } from "@/app/settings/StoragePage";
import { PROVIDERS_ROWS, ProvidersPage } from "@/app/providers/ProvidersPage";
import { ROUTING_PAGE_ROWS, RoutingPage } from "@/app/routing/RoutingPage";
import type { SettingsPageId } from "@/state/store";

/**
 * The pages of Settings, in the order the navigation lists them, each in its group. A page's
 * rows (label and description) are what Settings search finds; each page module exports the
 * copy it renders, so the two never drift apart.
 */

export type SettingsRowCopy = { readonly label: string; readonly description?: string };

export type SettingsGroup = "Personal" | "Agents" | "System" | "Archived";

export type SettingsPageEntry = {
  id: SettingsPageId;
  label: string;
  icon: ComponentType<SVGProps<SVGSVGElement>>;
  group: SettingsGroup;
  component: ComponentType;
  rows: readonly SettingsRowCopy[];
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
  { label: "Recent activity", description: "Hand-offs, and work waiting for quota." },
];

export const SETTINGS_GROUPS: readonly SettingsGroup[] = ["Personal", "Agents", "System", "Archived"];

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
    id: "conversations",
    label: "Conversations",
    icon: Chats,
    group: "Personal",
    component: ConversationsPage,
    rows: Object.values(CONVERSATIONS_ROWS),
  },
  {
    id: "personalization",
    label: "Personalization",
    icon: MemoryOnRemember,
    group: "Personal",
    component: PersonalizationPage,
    rows: Object.values(PERSONALIZATION_ROWS),
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
    icon: Terminal,
    group: "System",
    component: InspectorPage,
    rows: INSPECTOR_ROWS,
  },
  {
    id: "archived",
    label: "Archived chats",
    icon: Archive,
    group: "Archived",
    component: ArchivedPage,
    rows: Object.values(ARCHIVED_ROWS),
  },
];

export function settingsPage(id: SettingsPageId): SettingsPageEntry {
  // Every id has an entry: the list is written out above.
  return SETTINGS_PAGES.find((page) => page.id === id) ?? SETTINGS_PAGES[0]!;
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
  for (const page of SETTINGS_PAGES) {
    if (matches(page.label)) results.push({ page, row: null });
  }
  for (const page of SETTINGS_PAGES) {
    for (const row of page.rows) {
      if (matches(`${row.label} ${row.description ?? ""}`)) results.push({ page, row });
    }
  }
  return results;
}
