/** The Plugins page's view and its parts, as the daemon sends them. */
export type {
  OnlyInReason,
  PluginElsewhereView,
  PluginItemDetails,
  PluginKind,
  PluginProblem,
  PluginProjectView,
  PluginRemoval,
  PluginScope,
  PluginServerSettings,
  PluginSkillFile,
} from "@/ipc/generated";

import type { CatalogSignIn, PluginItemView as LegacyItem, PluginsView as LegacyView } from "@/ipc/generated";

export type PluginItemView = LegacyItem & {
  differentPerAgent?: boolean;
  signIn?: CatalogSignIn;
};
export type PluginsView = Omit<LegacyView, "items"> & { items: PluginItemView[] };
