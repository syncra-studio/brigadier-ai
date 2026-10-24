"""Opt-in comparison arms. Candidate behaviour always comes from the real daemon."""
from pathlib import Path

from battery import Battery
from plain import PlainBattery
from zed import SkippedArm, ZedBattery
from summary import SummaryBattery

# Phase B integration point: change this one constant if the build names its switch differently.
LONG_SESSION_SWITCH = "BRIGADIER_LONG_SESSIONS"
CANDIDATES = ("native-bare", "native", "native-full", "restore-only", "instructions-only",
              "native+checkpoint", "native-full+checkpoint")
ARMS = ("seam", "plain", "plain@200k", "zed", "summary", *CANDIDATES)


def supports_switch(daemon):
    # Read-only capability preflight. An old build must never silently run a candidate
    # as the default seam. Phase B's binary must contain the environment switch name.
    with Path(daemon).open("rb") as file:
        tail = b""
        while chunk := file.read(1024 * 1024):
            text = tail + chunk
            if LONG_SESSION_SWITCH.encode() in text:
                return True
            tail = text[-len(LONG_SESSION_SWITCH):]
    return False


class CandidateBattery(Battery):
    def setup(self):
        if not supports_switch(self.args.daemon):
            raise SkippedArm(f"build lacks {LONG_SESSION_SWITCH}; candidate {self.args.arm} not run")
        super().setup()

    def configure_environment(self, env):
        env[LONG_SESSION_SWITCH] = self.args.arm
        return env

    def generations(self):
        # Endurance depths for a native candidate count completed native compactions.
        # Rare fallback seams remain separate evidence and cannot inflate that depth.
        from lineages import compactions
        from battery import native_path
        files = {(e["entry"].get("provider"), event["file"]) for e in self.entries()
                 if (event := e["entry"].get("event") or {}).get("type") == "sessionStarted" and event.get("file")}
        return sum(compactions(native_path(file, provider), provider)
                                          for provider, file in files if native_path(file, provider).is_file())


class SharedPlainBattery(PlainBattery):
    """Same native CLI, with the comparison's explicit threshold only when requested."""


def transport(arm):
    return {"seam": Battery, "plain": SharedPlainBattery, "plain@200k": SharedPlainBattery,
            "zed": ZedBattery, "summary": SummaryBattery}.get(arm, CandidateBattery)
