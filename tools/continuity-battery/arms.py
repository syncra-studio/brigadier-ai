"""Opt-in comparison arms. Candidate behaviour always comes from the real daemon."""
from pathlib import Path

from battery import Battery
from plain import PlainBattery
from zed import SkippedArm, ZedBattery
from summary import SummaryBattery

# Phase B integration point: change this one constant if the build names its switch differently.
LONG_SESSION_SWITCH = "BRIGADIER_LONG_SESSIONS"
CODEX_DEFAULT_ARMS = {f"{arm}@codex-default": arm for arm in ("native-bare", "native", "plain")}
# Since D13 nothing is put back after a compaction and checkpoints are pulled only: the daemon
# runs `native`, `native-full`, `restore-only` and the `+checkpoint` arms as its default
# (`instructions-only`), and `seam-repaired` as the size seams.
CANDIDATES = ("native-bare", "native", "native-full", "restore-only", "instructions-only",
              "native+checkpoint", "native-full+checkpoint", "seam-repaired",
              *(arm for arm, base in CODEX_DEFAULT_ARMS.items() if base != "plain"))
ARMS = ("seam", "plain", "plain@200k", "plain@codex-default", "zed", "summary", *CANDIDATES)


def supports_switch(daemon, capability=LONG_SESSION_SWITCH):
    # Read-only capability preflight. An old build must never silently run a candidate
    # as the default seam. Phase B's binary must contain the environment switch name.
    with Path(daemon).open("rb") as file:
        tail = b""
        while chunk := file.read(1024 * 1024):
            text = tail + chunk
            if capability.encode() in text:
                return True
            tail = text[-len(capability):]
    return False


class CandidateBattery(Battery):
    def setup(self):
        if not supports_switch(self.args.daemon):
            raise SkippedArm(f"build lacks {LONG_SESSION_SWITCH}; candidate {self.args.arm} not run")
        if self.args.arm == "seam-repaired" and not supports_switch(self.args.daemon, "seam-repaired"):
            raise SkippedArm("build lacks test-only seam-repaired mode")
        super().setup()

    def configure_environment(self, env):
        env[LONG_SESSION_SWITCH] = CODEX_DEFAULT_ARMS.get(self.args.arm, self.args.arm)
        if self.args.arm in CODEX_DEFAULT_ARMS:
            # Battery.start installs the seam point first. Native Codex must receive
            # no explicit point so the real daemon leaves its CLI default alone.
            env.pop("BRIGADIER_SEAM_TOKENS", None)
        return env

    def generations(self):
        if self.args.arm == "seam-repaired":
            return super().generations()
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
    arm = CODEX_DEFAULT_ARMS.get(arm, arm)
    return {"seam": Battery, "plain": SharedPlainBattery, "plain@200k": SharedPlainBattery,
            "zed": ZedBattery, "summary": SummaryBattery}.get(arm, CandidateBattery)


def configure(args, parser, legacy_tokens):
    """Explicit fair options never alter an existing nightly invocation."""
    if args.arm in CODEX_DEFAULT_ARMS and args.provider != "codex":
        parser.error("@codex-default arms require --provider codex")
    args.fair = args.fair or args.arm not in ("seam", "plain", "zed", "summary")
    args.shared_transition = args.fair
    args.tokens = args.tokens if args.tokens is not None else (200000 if args.fair else legacy_tokens)
    if args.fair and not 100000 <= args.tokens <= 1000000:
        parser.error("fair shared threshold must be 100000..1000000")


def make(scenario, args):
    """Bind the existing scenario/scorer to its transport; optional shared record access."""
    vehicle = transport(args.arm)
    base = vehicle if scenario is Battery else scenario if vehicle is Battery else type("ScenarioRun", (scenario, vehicle), {})
    if not args.fair:
        return base(args)
    from profiles import Work
    class RecordAccess:
        setup_archive = Work.setup_archive
        lookup_instruction = Work.lookup_instruction
        source_files = Work.source_files
        refresh_archive = Work.refresh_archive

        def setup(self):
            self.candidate_arms = CANDIDATES
            super().setup()
            self.setup_archive()

        def send(self, name, text, fault=None, **kwargs):
            if name in ("probe", "t6-recall"):
                text = text.replace("do not read fixture scripts, run commands, or edit files", "do not read fixture scripts or edit files")
                text = text.replace("do not run commands, read workspace fixtures, or edit files", "do not read workspace fixtures or edit files")
                text = self.lookup_instruction() + text
            step = super().send(name, text, fault, **kwargs)
            self.refresh_archive()
            return step
    return type("FairScenarioRun", (RecordAccess, base), {})(args)
