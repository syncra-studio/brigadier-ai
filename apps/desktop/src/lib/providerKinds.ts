import type { ProviderKind } from "@/ipc/generated";
import { CLIS, type CliFacts } from "@/ipc/generated/clis";

/** Agents in the order used by the app's navigation and settings. */
export const PROVIDERS: readonly ProviderKind[] = CLIS.map((cli) => cli.id);

/** A neutral mark (a terminal prompt) for a CLI the app has no facts for. */
const GENERIC_GLYPH: CliFacts["glyph"] = {
  viewBox: "0 0 24 24",
  path: "M4 4h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2Zm0 2v12h16V6H4Zm2.3 3.3 1.4-1.4 4.1 4.1-4.1 4.1-1.4-1.4L9 12 6.3 9.3ZM12 15h6v2h-6v-2Z",
};

/** A CLI's facts; one the app doesn't know (an old record's) goes by its id. */
export function cliFacts(id: ProviderKind): CliFacts {
  return CLIS.find((cli) => cli.id === id) ?? {
    id, label: id, vendor: id, maker: id, cliName: id, installGuide: "", usualWindows: [], glyph: GENERIC_GLYPH,
    oneSkillPerMessage: false, reportsCacheWrites: true, subagentTools: [], extraAccounts: false,
  };
}
