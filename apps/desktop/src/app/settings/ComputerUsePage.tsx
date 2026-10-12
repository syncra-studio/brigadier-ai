import { SettingsPage } from "@/app/settings/parts";
import { useApp } from "@/state/store";

/** The Computer use page's rows, for Settings search: none for now. */
export const COMPUTER_USE_ROWS = {} as const;

/** Whether this system lists the page: macOS only, from the app's own info. */
export function computerUseAvailable(): boolean {
  return useApp.getState().info?.platform === "macos";
}

/** Settings → Computer use, while the feature is away. */
export function ComputerUsePage() {
  return (
    <SettingsPage title="Computer use">
      <p className="text-foreground/65 text-sm">Computer use settings are coming back soon.</p>
    </SettingsPage>
  );
}
