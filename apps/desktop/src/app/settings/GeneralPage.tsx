import { AboutSection } from "@/app/settings/AboutSection";
import { AppearanceSection } from "@/app/settings/AppearanceSection";
import { ComposerSection } from "@/app/settings/ComposerSection";
import { NotificationsSection } from "@/app/settings/NotificationsSection";
import { SettingsPage } from "@/app/settings/parts";
import { PowerSection } from "@/app/settings/PowerSection";

export function GeneralPage() {
  return (
    <SettingsPage title="General">
      <AppearanceSection />
      <ComposerSection />
      <NotificationsSection />
      <PowerSection />
      <AboutSection />
    </SettingsPage>
  );
}
