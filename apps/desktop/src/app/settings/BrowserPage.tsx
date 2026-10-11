import { Globe } from "@openai/apps-sdk-ui/components/Icon";

import { useAction } from "@/app/conversation/useAction";
import {
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  SettingsSwitch,
  SwitchSetting,
} from "@/app/settings/parts";
import type { LinkTarget } from "@/ipc/generated";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The Browser page's rows, for the page and for Settings search. */
export const BROWSER_ROWS = {
  browser: {
    label: "Browser",
    description: "Use Brigadier's built-in browser for web pages, in a tab beside your session.",
  },
  webLinks: {
    label: "Open web links in",
    description: "Where links in answers, search results and pull requests open.",
  },
  localLinks: {
    label: "Open local dev URLs in",
    description: "Where localhost and other local development sites open, such as a running preview.",
  },
  fullUrl: {
    label: "Show full URL",
    description: "Include the path, query, and fragment in the address bar.",
  },
} as const;

const PAGE_DESCRIPTION = "The built-in browser, where links open, and what it keeps.";
const OFF_NOTE = "The built-in browser is off: links open in your default browser, and agents are told it isn't there.";

const TARGETS = [
  { value: "brigadier", label: "Brigadier's browser" },
  { value: "system", label: "Your default browser" },
] as const satisfies readonly { value: LinkTarget; label: string }[];

/** Settings › Browser. Off, only the switch shows: the browser doesn't exist anywhere else. */
export function BrowserPage() {
  const on = useApp((s) => s.settings.browserEnabled);
  const embedded = useApp((s) => s.info?.platform !== "linux");
  return (
    <SettingsPage title="Browser" description={PAGE_DESCRIPTION}>
      <SettingsSection>
        <SettingsCard>
          <BrowserSwitch on={on} />
        </SettingsCard>
        {!on && <p className="text-foreground/65 px-1 text-xs">{OFF_NOTE}</p>}
      </SettingsSection>
      {on && embedded && (
        <SettingsSection title="General">
          <SettingsCard>
            <LinkRow setting="webLinksIn" row={BROWSER_ROWS.webLinks} />
            <LinkRow setting="localLinksIn" row={BROWSER_ROWS.localLinks} />
            <SwitchSetting setting="browserFullUrl" row={BROWSER_ROWS.fullUrl} />
          </SettingsCard>
        </SettingsSection>
      )}
    </SettingsPage>
  );
}

function BrowserSwitch({ on }: { on: boolean }) {
  const save = useAction();
  return (
    <SettingsRow
      label={
        <span className="flex items-center gap-3">
          <Globe aria-hidden className="size-icon-lg text-foreground/80 shrink-0" />
          {BROWSER_ROWS.browser.label}
        </span>
      }
      description={<span className="ps-9 block">{BROWSER_ROWS.browser.description}</span>}
      error={save.error}
      className="py-4"
    >
      <SettingsSwitch
        label={BROWSER_ROWS.browser.label}
        checked={on}
        onCheckedChange={(next) => save.run(() => setSetting("browserEnabled", next))}
      />
    </SettingsRow>
  );
}

function LinkRow({
  setting,
  row,
}: {
  setting: "webLinksIn" | "localLinksIn";
  row: { label: string; description: string };
}) {
  const value = useApp((s) => s.settings[setting]);
  const save = useAction();
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error}>
      <SettingsSelect<LinkTarget>
        label={row.label}
        value={value}
        options={TARGETS}
        onChange={(next) => save.run(() => setSetting(setting, next))}
      />
    </SettingsRow>
  );
}
