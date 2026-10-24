"""Pinned Zed text-compaction algorithm over the plain CLI, for measurement only.

Independent implementation; no Zed code or prompt text is distributed here.
"""
import json
from pathlib import Path
from urllib.request import urlopen

from plain import PlainBattery

COMMIT = "9f305aaf88370e8ca39c702242186e3d8f97ecca"
PROMPT_URL = (f"https://raw.githubusercontent.com/zed-industries/zed/{COMMIT}/"
              "crates/agent_settings/src/prompts/compaction_prompt.txt")
# Pinned crates/agent/src/thread.rs:126-127 (decimal bytes, not 80 * 1024).
USER_BYTES = 80_000
SUMMARY_PREFIX = "The previous conversation was compacted. Use this summary as context:\n\n"


class SkippedArm(RuntimeError):
    pass


def fetch_prompt(root):
    """GPL prompt stays in this run's scratch cache and is removed with it."""
    cached = Path(root) / "zed-compaction-prompt.txt"
    try:
        if not cached.exists():
            with urlopen(PROMPT_URL, timeout=20) as response:
                text = response.read(64_001)
            if not text.strip() or len(text) > 64_000:
                raise ValueError("empty or oversized prompt")
            cached.write_text(text.decode("utf-8"))
        return cached.read_text()
    except (OSError, ValueError) as error:
        raise SkippedArm(f"skipped: could not fetch pinned Zed compaction prompt ({error})") from error


def retained_users(messages, budget=USER_BYTES):
    # thread.rs:5186-5220 selects newest first, then reverses for chronological replay.
    # thread.rs:5018-5079 keeps the PREFIX of the oldest partial text, on a UTF-8 boundary.
    retained = []
    for text in reversed(messages):
        if not text:
            continue
        encoded = text.encode("utf-8")
        if len(encoded) <= budget:
            retained.append(text)
            budget -= len(encoded)
        else:
            partial = encoded[:budget].decode("utf-8", errors="ignore")
            if partial:
                retained.append(partial)
            break
    return list(reversed(retained))


def bootstrap(messages, summary):
    # thread.rs:5152-5177 inserts retained users BEFORE compaction; :219-234 wraps summary.
    # CLI text input folds these user messages into one turn, preserving content and order.
    return "\n\n".join(retained_users(messages) + [SUMMARY_PREFIX + summary])


def active_tokens(events, provider):
    """Latest model request's hot context, not cumulative per-turn billing."""
    latest = None
    for event in events:
        if provider == "claude" and event.get("type") == "assistant":
            usage = event.get("message", {}).get("usage")
            if usage:
                latest = sum(usage.get(k, 0) or 0 for k in (
                    "input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"))
        if provider == "codex":
            payload = event.get("payload") or {}
            usage = (payload.get("info") or {}).get("last_token_usage")
            if payload.get("type") == "token_count" and usage:
                latest = (usage.get("input_tokens", 0) or 0) + (usage.get("output_tokens", 0) or 0)
    return latest


class ZedBattery(PlainBattery):
    def setup(self):
        self.user_messages = []
        self.zed_compactions = []
        self.hot_tokens = None
        self.compaction_prompt = fetch_prompt(self.root)
        super().setup()

    def environment(self):
        env = super().environment()
        if self.args.provider == "claude":
            # The explicit summary is the measured compaction; never invoke /compact.
            env["DISABLE_AUTO_COMPACT"] = "1"
        return env

    def generations(self):
        return len(getattr(self, "zed_compactions", []))

    def update_context(self, start):
        count = active_tokens(self.raw_events[start:], self.args.provider)
        if count is None:
            path = self.native_path()
            if path:
                events = []
                for line in path.read_text().splitlines():
                    try:
                        events.append(json.loads(line))
                    except ValueError:
                        continue
                count = active_tokens(events, self.args.provider)
        if count is None:
            raise RuntimeError("explicit handoff arm: latest native context usage unavailable; cannot apply threshold")
        self.hot_tokens = count

    def send(self, name, text, fault=None):
        # thread.rs:4702-4752 checks latest input + output before the next request.
        # At CLI turn boundaries only: this harness cannot intercept mid-tool model calls.
        if self.hot_tokens is not None and self.hot_tokens >= self.args.tokens:
            # thread.rs:4779-4808 uses the same model, existing prefix, and no tools.
            summary = super().send(f"zed-summary-{self.generations() + 1}", self.compaction_prompt,
                                   tool_less=self.args.provider == "claude")
            if not summary["settled"] or not summary["answer"].strip():
                raise RuntimeError("Zed arm: compaction summary failed or was empty")
            initial = bootstrap(self.user_messages, summary["answer"])
            self.zed_compactions.append({"n": self.generations() + 1, "summary": summary["answer"],
                                         "bootstrap": initial, "previous_session_id": self.native_id,
                                         "active_tokens": self.hot_tokens})
            self.native_id = None
            text_for_cli = initial + "\n\n" + text
        else:
            text_for_cli = text
        start = len(self.raw_events)
        step = super().send(name, text_for_cli, fault)
        # Retain only actual scenario user turns, never synthetic summaries or bootstraps.
        self.user_messages.append(text)
        self.update_context(start)
        return step

    def capture(self):
        super().capture()
        self.result.update(zed_compactions=self.generations(),
                           zed_handoffs=getattr(self, "zed_compactions", []), zed_commit=COMMIT)
