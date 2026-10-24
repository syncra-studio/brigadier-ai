"""Model-written handoff only: pinned upstream instructions, plain native CLI sessions."""
from pathlib import Path
from urllib.request import urlopen

from plain import PlainBattery
from zed import SkippedArm, ZedBattery

COMMIT = "5beb35b040e3de97e54fa9e386d7e42d15c295c2"
PROMPT_URL = (f"https://raw.githubusercontent.com/mattpocock/skills/{COMMIT}/"
              "skills/productivity/handoff/SKILL.md")


def fetch_prompt(root):
    """Keep the upstream MIT skill verbatim in the run's disposable scratch cache."""
    cached = Path(root) / "summary-skill.md"
    try:
        if not cached.exists():
            with urlopen(PROMPT_URL, timeout=20) as response:
                content = response.read(64_001)
            if not content.strip() or len(content) > 64_000:
                raise ValueError("empty or oversized skill")
            cached.write_text(content.decode("utf-8"))
        return cached.read_text()
    except (OSError, ValueError) as error:
        raise SkippedArm(f"skipped: could not fetch pinned handoff skill ({error})") from error


class SummaryBattery(PlainBattery):
    # Use the same latest input + output threshold and between-turn cut points as
    # the seam/explicit-compaction measurement, including cached context tokens.
    update_context = ZedBattery.update_context

    def environment(self):
        env = super().environment()
        if self.args.provider == "claude":
            env["DISABLE_AUTO_COMPACT"] = "1"
        return env

    def setup(self):
        self.summary_handoffs = []
        self.hot_tokens = None
        self.handoff_skill = fetch_prompt(self.root)
        super().setup()

    def generations(self):
        return len(getattr(self, "summary_handoffs", []))

    def send(self, name, text, fault=None):
        if self.hot_tokens is not None and self.hot_tokens >= self.args.tokens:
            number = self.generations() + 1
            file = self.root / f"summary-{number}.md"
            request = self.handoff_skill + f"\n\nSave the handoff document to {file}."
            written = super().send(f"summary-write-{number}", request)
            if not written["settled"] or not file.is_file() or not file.read_text().strip():
                raise RuntimeError("summary arm: handoff document failed or was empty")
            previous = self.native_id
            self.native_id = None
            # This is the entire first input. The pending scenario turn is sent
            # separately after the fresh CLI has read the model's document.
            bootstrap = f"Read {file} and continue."
            resumed = super().send(f"summary-read-{number}", bootstrap)
            if not resumed["settled"] or self.native_id == previous:
                raise RuntimeError("summary arm: fresh successor failed")
            self.summary_handoffs.append({"n": number, "document": file.read_text(),
                                          "bootstrap": bootstrap, "previous_session_id": previous,
                                          "successor_session_id": self.native_id,
                                          "active_tokens": self.hot_tokens})
        start = len(self.raw_events)
        step = super().send(name, text, fault)
        self.update_context(start)
        return step

    def capture(self):
        super().capture()
        self.result.update(summary_compactions=self.generations(),
                           summary_handoffs=getattr(self, "summary_handoffs", []), summary_commit=COMMIT)
