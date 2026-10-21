import { ActionError } from "@/app/inspector/providers/ActionError";
import { FixtureRow } from "@/app/inspector/providers/FixtureRow";
import { Section, useAction } from "@/app/inspector/providers/shared";
import { replayFixture } from "@/state/actions";
import { useApp } from "@/state/store";

export function FixtureList() {
  const fixtures = useApp((s) => s.providers.view?.fixtures ?? []);
  const replay = useAction();
  return (
    <Section title="Replay fixtures">
      <ActionError error={replay.error} />
      {fixtures.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          No fixtures. Sessions started with Record are saved here.
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {fixtures.map((fixture) => (
            <FixtureRow
              key={fixture.id}
              fixture={fixture}
              busy={replay.busy}
              onReplay={() => void replay.run(() => replayFixture(fixture.id))}
            />
          ))}
        </ul>
      )}
    </Section>
  );
}
