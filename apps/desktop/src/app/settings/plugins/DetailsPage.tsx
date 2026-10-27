import { SettingsPage, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { Folder } from "@/components/icons";
import { Banner, CopyButton, SettingsCard } from "@/components/shared";
import { shortPath } from "@/lib/format";
import { folderName, KIND_LABEL, shownDetail, signInLine } from "./model";
import { FoldCard, PluginMark, RadioRow } from "./parts";
import type { PluginItemView, PluginsView } from "./types";

/** What the details page can ask of the list page that holds it. */
type DetailsActions = {
  busy: boolean;
  onBack: () => void;
  /** Where it's on: every project (null), or only these projects (by id), never none. */
  onScope: (projectIds: string[] | null) => void;
  /** Absent while it can't be signed in from here. */
  onSignIn?: (() => void) | undefined;
  onRemove: () => void;
  onRetry: () => void;
  onUpdate: () => void;
  onOpen: () => void;
  onTrust: (projectId: string) => void;
};

/** One item: what needs the user, where it's on, Advanced (how it runs, where it's saved, Open), and Remove. */
export function DetailsPage({ view, item, actions }: { view: PluginsView; item: PluginItemView; actions: DetailsActions }) {
  const some = item.scope.type === "someProjects" ? item.scope : null;
  const on = some?.projectIds ?? [];
  const scope = (projectId: string, next: boolean) => actions.onScope(next ? [...on, projectId] : on.filter((id) => id !== projectId));
  const first = item.kind === "skill" ? "Folder" : item.url ? "Address" : "Command";
  return (
    <SettingsPage
      title={item.name}
      description={`${KIND_LABEL[item.kind]} · ${item.description || shownDetail(item.detail)}`}
      mark={<PluginMark item={item} large />}
      back={{ label: "Plugins", onBack: actions.onBack }}
      sectionsClassName="gap-9 pt-4"
    >
      {item.differentPerAgent && <p data-slot="different-per-agent" className="text-foreground/65 text-xs">Different per agent</p>}
      <Banners item={item} actions={actions} />
      <SettingsSection
        title="Projects"
        description="Where it's on. New sessions pick up changes."
        className="[&>div:first-child]:min-h-0"
        setting="Projects"
      >
        <SettingsCard role="radiogroup" aria-label="Where it's on">
          <div className="px-4 py-3">
            <RadioRow checked={!some} disabled={actions.busy} title="All projects" line="On in every project, and in new ones you add." onSelect={() => some && actions.onScope(null)} />
          </div>
          <div className="px-4 py-3">
            {/* From All projects, it starts on in each project: nothing changes until one is turned off. */}
            <RadioRow
              checked={!!some}
              disabled={actions.busy || view.projects.length === 0}
              title="Only some projects"
              line="On only in the projects you pick."
              onSelect={() => !some && view.projects.length > 0 && actions.onScope(view.projects.map((project) => project.id))}
            />
          </div>
        </SettingsCard>
        {some && (view.projects.length > 0 || some.folders.length > 0) && (
          <SettingsCard>
            {view.projects.map((project) => {
              const note = project.notes[0];
              const checked = on.includes(project.id);
              return (
                <div key={project.id} data-slot="project-switch" className="flex items-center gap-3 px-4 py-3">
                  <Folder aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
                  <span className="flex min-w-0 flex-1 flex-col gap-1">
                    <span className="text-label font-medium">{project.name}</span>
                    <span className={note ? "text-warning text-xs" : "text-foreground/65 truncate text-xs"}>{note ? note.message : shortPath(project.path)}</span>
                    {note && (
                      <Button variant="link-blue" size="inline" className="w-fit" disabled={actions.busy} onClick={() => actions.onTrust(project.id)}>
                        Trust {project.name}
                      </Button>
                    )}
                  </span>
                  {/* It stays on somewhere: the last project on can't be turned off. */}
                  <SettingsSwitch label={project.name} checked={checked} disabled={actions.busy || (checked && on.length === 1)} onCheckedChange={(next) => scope(project.id, next)} />
                </div>
              );
            })}
            {some.folders.map((folder) => (
              <div key={folder} data-slot="folder-switch" className="flex items-center gap-3 px-4 py-3">
                <Folder aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
                <span className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="text-label font-medium">{folderName(folder)}</span>
                  <span className="text-foreground/65 truncate text-xs">{shortPath(folder)} · not one of your projects</span>
                </span>
                <SettingsSwitch label={folderName(folder)} checked disabled onCheckedChange={() => {}} />
              </div>
            ))}
          </SettingsCard>
        )}
      </SettingsSection>
      <FoldCard title="Advanced" line={`${first}, where it's saved, and its ${item.kind === "skill" ? "SKILL.md" : "settings"}.`} initiallyOpen setting="Advanced">
        <>
          <InfoRow title={first} value={item.url ?? item.detail} />
          {item.files.map((file) => <InfoRow key={file} title="Saved in" value={shortPath(file)} />)}
          <div data-slot="open-row" className="flex items-center gap-6 px-4 py-3">
            <span className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-label font-medium">{item.kind === "skill" ? "SKILL.md" : "Settings"}</span>
              <span className="text-foreground/65 text-xs">
                {item.kind === "skill" ? "Read the skill's instructions." : item.url ? "Its address and header names. Secret values stay hidden." : "Its command, arguments and env variable names. Secret values stay hidden."}
              </span>
            </span>
            <Button variant="soft" size="sm" onClick={actions.onOpen}>Open</Button>
          </div>
        </>
      </FoldCard>
      {item.removal !== "none" && (
        <div data-slot="remove-row" className="flex items-center gap-4">
          <Button variant="soft-destructive" size="sm" disabled={actions.busy} onClick={actions.onRemove}>Remove {item.name}</Button>
          <span className="text-foreground/50 text-xs">
            Removes it from Brigadier's library. New sessions pick up changes.
          </span>
        </div>
      )}
    </SettingsPage>
  );
}

/** What needs the user, over everything else: it won't start, a sign-in, an update, only some agents have it. */
function Banners({ item, actions }: { item: PluginItemView; actions: DetailsActions }) {
  if (!item.signIn || item.signIn.type === "notRequired") return null;
  return (
    <div data-slot="banners" className="flex flex-col gap-2">
      <Banner tone="info" text={signInLine(item.signIn)} className="rounded-control border-b-0">
        {actions.onSignIn && item.signIn.type !== "signedIn" && <Button variant="soft" size="sm" disabled={actions.busy} onClick={actions.onSignIn}>{item.signIn.type === "signingIn" ? "Waiting for browser…" : "Sign in"}</Button>}
      </Banner>
    </div>
  );
}

function InfoRow({ title, value }: { title: string; value: string }) {
  return (
    <div className="flex items-center gap-6 px-4 py-3">
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="text-label font-medium">{title}</span>
        <span className="text-foreground/65 font-mono text-xs break-all">{value}</span>
      </span>
      <CopyButton text={value} size="xs" />
    </div>
  );
}
