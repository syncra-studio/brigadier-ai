import { SettingsCard, SettingsPage, SettingsSection, SwitchSetting } from "@/app/settings/parts";

/** The Git page's rows, for the page and for Settings search. */
export const GIT_ROWS = {
  omitAiCoauthors: {
    label: "Leave out AI co-author lines",
    description:
      "Commits from the Source panel and the agents' commits leave out Co-authored-by lines that name an AI. People's stay.",
  },
} as const;

export function GitPage() {
  return (
    <SettingsPage title="Git">
      <SettingsSection title="Commits">
        <SettingsCard>
          <SwitchSetting setting="omitAiCoauthors" row={GIT_ROWS.omitAiCoauthors} />
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}
