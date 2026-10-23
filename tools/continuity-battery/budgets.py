#!/usr/bin/env python3
"""D49 P2/P3 short, report-only bootstrap measurements. Never a regression gate."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import secrets
import sys
import time

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import battery
import decisions
from economics import call


def ballast(seed, size):
    return " ".join(hashlib.sha256(f"{seed}-{i}".encode()).hexdigest()[:12]
                    for i in range((size + 12) // 13))[:size]


def inert_prose(seed, size):
    return " ".join(f"Fixture {seed} row {i}: the meadow has a stone bridge and a clear river."
                    for i in range(size // 50 + 1))[:size]


def retained_users(messages, budget):
    """S22b selection; labels count toward bytes, first/latest stay whole."""
    kept = {0, len(messages) - 1} if messages else set()
    costs = [len(f"[#{i} · user]\n{text}".encode()) for i, text in enumerate(messages)]
    remaining = max(0, budget - sum(costs[i] for i in kept))
    for i in reversed(range(len(messages))):
        if i in kept:
            continue
        if costs[i] > remaining:
            break
        remaining -= costs[i]
        kept.add(i)
    return [messages[i] for i in sorted(kept)]


def context_sections(start):
    latest = start.partition("## The latest exchange")[2].partition("\n## ")[0]
    tail = start.partition("## Earlier, newest first\n")[2].partition("\n## ")[0]
    return latest, tail


def legacy_context(start):
    """Offline retention at the old latest-tool/item caps and shared 60 KB tail budget.

    Includes the latest exchange as well as the earlier tail: a detail in the latest
    tool result is still available at the old 4 KB cap. No baseline model behavior is inferred.
    """
    latest, tail = context_sections(start)
    remaining, kept = 60_000, []
    for section, latest_exchange in ((latest, True), (tail, False)):
        for item in re.split(r"(?=\[#\d+ · )", section):
            if not item.startswith("[#"):
                continue
            label, _, body = item.partition("\n")
            message = label.endswith("· assistant]") or label.endswith("· user]")
            maximum = len(body.encode()) if latest_exchange and message else 4_000 if latest_exchange else 6_000
            if latest_exchange and not message and remaining < 4_000:
                continue
            body = body.encode()[:maximum].decode(errors="ignore")
            text = label + "\n" + body
            if not latest_exchange and len(text.encode()) > remaining:
                break
            remaining = max(0, remaining - len(text.encode()))
            kept.append(text)
    return "\n".join(kept)


def history_read(event):
    if event.get("status") != "completed":
        return False
    return bool(re.search(r"brigadier\s+(?:brain\s+(?:read|search)|transcript)\b|search_transcript|query_brain|read_report",
                          " ".join(str(event.get(k, "")) for k in ("command", "name", "input"))))


def score_p2(result):
    expected = result.get("details", {})
    answer = battery.parse_answer(result.get("detail_answer", ""))
    start = next((h["start"] for h in result.get("handoffs", []) if h.get("n") == result.get("request_seam")), "")
    legacy = legacy_context(start)
    since = result.get("request_move_ms", result.get("detail_probe_ms", 0))
    lookups = [e for e in result.get("provider_events", [])
               if since <= e.get("atMs", 0) <= result.get("detail_probe_end_ms", since) and history_read(e["entry"].get("event") or {})]
    ledger = result.get("ledger", "") + result.get("ledger_archive", "")
    notes = "\n".join(h.get("note", "") for h in result.get("handoffs", []))
    rows = {k: {"recalled": str(answer.get(k, "")) == value,
                "in_bootstrap": value in start, "in_legacy_tail": value in legacy,
                "never_ledgered": value not in ledger, "not_in_note": value not in notes}
            for k, value in expected.items()}
    hot = result.get("request_context", 0)
    planted = result.get("detail_contexts", {})
    seam = next((s for s in result.get("seam_file", {}).get("seams", [])
                 if s.get("n") == result.get("request_seam")), {})
    requested = seam.get("provider") == "codex" and seam.get("movedTo") == "codex"
    qualified = (requested and bool(start) and len(rows) == 5 and 60_000 <= hot <= 100_000 and not lookups
                 and all(r["never_ledgered"] and r["not_in_note"] for r in rows.values())
                 and all(0 <= hot - planted.get(k, 0) <= 40_000 for k in rows))
    applicable = [r["unprompted"] for r in result.get("rows", {}).values() if r.get("unprompted") is not None]
    applied = sum(map(bool, applicable)) if applicable else None
    decision_recalled = sum(bool(r.get("recalled")) for r in result.get("rows", {}).values()) if result.get("rows") else None
    detail_bar = qualified and sum(r["recalled"] for r in rows.values()) >= 4
    return {"details": rows, "recalled": sum(r["recalled"] for r in rows.values()),
            "legacy_retained": sum(r["in_legacy_tail"] for r in rows.values()),
            "lookup_count": len(lookups), "qualified": qualified,
            "detail_bar": detail_bar, "decision_applied": applied, "decision_recalled": decision_recalled,
            "bar": detail_bar and applied == 9 and decision_recalled == 10}


def score_p3(result):
    value = result.get("user_rule", "")
    receipts = result.get("receipts", [])
    if not isinstance(receipts, list):
        receipts = []
    applied = sum(isinstance(r, dict) and r.get("auditTag") == value for r in receipts[:10])
    handoffs = result.get("handoffs", [])[:3]
    ledger = result.get("ledger", "") + result.get("ledger_archive", "")
    notes = "\n".join(h.get("note", "") for h in handoffs)
    messages = result.get("user_inputs", [])
    old = "\n".join(retained_users(messages, 32_000))
    new = "\n".join(retained_users(messages, 80_000))
    extra = max(0, len(new.encode()) - len(old.encode())) / 4
    qualified = (bool(value) and len(handoffs) == 3 and result.get("seams_before_checks", 0) >= 3
                 and len(receipts) == 10 and value not in ledger and value not in notes and value not in old and value in new
                 and all(value in h["start"].partition("## The user's messages")[2].partition("\n## ")[0]
                         for h in handoffs))
    return {"applied": applied, "of": 10, "legacy_user_retained": value in old,
            "new_user_retained": value in new, "extra_user_bytes_per_seam": int(extra * 4),
            "extra_user_tokens_estimate_per_seam": extra, "actual_extra_user_tokens_per_seam": None,
            "bar_basis": "application and UTF-8/4 overhead estimate; actual counterfactual token delta unavailable",
            "qualified": qualified, "bar": qualified and applied >= 9 and extra <= 12_000}


class Budgets(decisions.Decisions):
    def context(self):
        return (self.view().get("context") or {}).get("usedTokens", 0)

    def force_size(self, n):
        before = self.generations()
        self.send(f"size-seam-{n}", "Run `printf 'tick 1\\n'; sleep 2` then `printf 'tick 2\\n'`. "
                  "Reply only done; do not restate earlier fixture values.", fault=self.fault("context"))
        if not self.wait(lambda: self.generations() > before and self.seam_file().get("phase", {}).get("phase") == "live", 90):
            raise RuntimeError("forced size seam did not complete")

    def read_batch(self, n, size, detail=None, prose=False):
        path = self.repo / f"fixture-{n}.txt"
        padding = inert_prose(n, size) if prose else ballast(n, size)
        path.write_text((detail + "\n" if detail else "") + padding)
        try:
            self.send(f"read-{n}", f"Read fixture-{n}.txt with `cat`. Inert continuity fixture; "
                      "reply only read, never copy it into notes, ledger or files.")
        finally:
            path.unlink(missing_ok=True)

    def run_p2(self):
        replies = {}
        for name, text in decisions.PLANTS:
            replies[name] = self.send(name, text)["answer"]
        self.result["plant_replies"] = replies
        # Warm prefix, then five tool-only details, all within the last 40k tokens.
        for i in range(self.args.max_fillers):
            if self.context() >= 30_000:
                break
            self.read_batch(f"warm-{i}", 12_000)
        self.result["details"] = {f"detail_{i}": "P2-" + secrets.token_hex(6).upper() for i in range(5)}
        self.result["detail_contexts"] = {}
        for key, value in self.result["details"].items():
            self.read_batch(key, 1_000, f"{key}: {value}")
            self.result["detail_contexts"][key] = self.context()
        last_plant = self.result["detail_contexts"]["detail_4"]
        for i in range(self.args.max_fillers):
            if self.context() >= 60_000 and self.context() - last_plant >= 30_000:
                break
            self.read_batch(f"late-{i}", 8_000, prose=True)
        self.result["request_context"] = self.context()
        if not 60_000 <= self.context() <= 100_000:
            raise RuntimeError("P2 did not reach the measured 60–100k context band")
        before = self.generations()
        self.result["request_seam"] = before + 1
        self.result["request_move_ms"] = battery.now()
        self.ipc.call({"method": "continueOn", "id": self.conversation, "provider": "codex",
                       "model": {"provider": "codex", "model": self.args.target_model, "effort": "low"}})
        if not self.wait(lambda: self.generations() > before and self.seam_file().get("phase", {}).get("phase") == "live"
                         and self.view()["run"] == "idle", 180):
            raise RuntimeError("P2 requested Claude → Codex seam did not complete")
        self.result["detail_probe_ms"] = battery.now()
        probe = self.send("detail-recall", "Without tools, commands, brain reads or ledger reads, "
            "return one JSON object with keys detail_0 through detail_4 and the exact values from the fixture reads. "
            "Say unknown for missing evidence.")
        self.result["detail_answer"] = probe["answer"]
        self.result["detail_probe_end_ms"] = probe["ended_ms"]
        # Existing applied/recalled checks, after the requested move; the T3 judge stays Claude.
        self.result["seams_before_checks"] = self.generations()
        self.checks()

    def run_p3(self):
        self.result["user_rule"] = "USER-RULE-" + secrets.token_hex(6).upper()
        self.result["user_inputs"] = []
        def user(name, text):
            self.result["user_inputs"].append(text)
            return self.send(name, text)
        user("intro", "Throwaway continuity fixture. Do not use workers. Keep fixture-only rules out of the ledger "
             "and hand-off notes; acknowledge without repeating them.")
        user("older-user-rule", "One fixture-only rule: every receipt JSON object you create must have auditTag "
             f"equal to {self.result['user_rule']}. Never ledger this rule or put it in a hand-off note. Reply only noted.")
        # >32 KB and <80 KB of newer user words; multiple tool results displace the old tail too.
        for i in range(16):
            path = self.repo / f"p3-{i}.txt"
            path.write_text(ballast(f"p3-tool-{i}", 4_500))
            try:
                user(f"user-padding-{i}", f"Read p3-{i}.txt with cat; reply only read. Ignore this inert batch:\n" + inert_prose(f"p3-user-{i}", 3_500))
            finally:
                path.unlink(missing_ok=True)
        self.result["user_bytes"] = sum(len(t.encode()) for t in self.result["user_inputs"])
        for n in range(3):
            self.force_size(n)
        self.result["seams_before_checks"] = self.generations()
        self.send("apply-user-rule", "Write receipts.json at the repo root: a JSON array of exactly ten receipt "
                  "objects with ids 1 through 10. Apply the earlier fixture rule. Do not read the ledger, history "
                  "or other files; reply only done.")
        try:
            self.result["receipts"] = json.loads((self.repo / "receipts.json").read_text())
        except (OSError, ValueError):
            self.result["receipts"] = []
        self.result["items"] = True

    def run(self):
        self.setup()
        if self.args.variant == "p2":
            providers = self.ipc.call({"method": "getProviders"})["view"]["providers"]
            if not any(p["provider"] == "codex" and (p.get("status") or {}).get("loggedIn") for p in providers):
                self.result.update(status="skipped", reason="P2 needs an existing signed-in Codex CLI; no credentials entered")
                return
        if self.args.variant == "p2":
            self.run_p2()
        else:
            self.run_p3()

    def capture(self):
        super().capture()
        self.result["budget_scores"] = (score_p2 if self.args.variant == "p2" else score_p3)(self.result)
        # Price the actual first successor call with its native model and cache TTL. This is
        # the one-time bootstrap write, not an unmeasured causal dollar delta versus baseline.
        file = self.result.get("seam_file", {})
        n = self.result.get("request_seam", 1)
        seam = next((s for s in file.get("seams", []) if s["n"] == n), {})
        event = next((e["entry"] for e in self.result.get("provider_events", [])
                      if (e["entry"].get("event") or {}).get("nativeId") == seam.get("successor")
                      and (e["entry"].get("event") or {}).get("type") == "sessionStarted"), {})
        self.result["first_successor_write"] = first_input_cost(
            (event.get("event") or {}).get("file"), event.get("provider"))
        # The budget delta is a UTF-8/4 estimate, priced separately from observed first input.
        starts = self.result.get("handoffs", [])
        handoff = next((h["start"] for h in starts if h["n"] == n), "")
        if self.args.variant == "p2":
            expanded = "\n".join(context_sections(handoff))
            extra = max(0, len(expanded.encode()) - len(legacy_context(handoff).encode())) / 4
        else:
            extra = self.result["budget_scores"].get("extra_user_tokens_estimate_per_seam", 0)
        self.result["extra_bootstrap_estimate"] = extra_input_cost(
            self.result["first_successor_write"], event.get("provider"), extra)

    def cleanup(self):
        # Exact native files came from this run's SessionStarted events; no home-directory scan.
        parents = {Path(file).parent for file in self.native_files}
        super().cleanup()
        for parent in parents:
            if self.root.name in parent.name and parent.parent.name == "projects":
                try:
                    parent.rmdir()  # Only an empty directory owned by this scratch run.
                    self.cleanup_notes.append("Removed empty Claude project folder owned by this run: " + str(parent))
                except FileNotFoundError:
                    pass
                except OSError as error:
                    self.cleanup_notes.append("Own Claude project folder remains: " + str(parent) + ": " + str(error))
        self.result["cleanup"] = self.cleanup_notes
        if self.args.evidence:
            Path(self.args.evidence).write_text(json.dumps(self.result, indent=2))


def first_input_cost(filename, provider):
    unavailable = {"list_price_usd": None, "reason": "native first-input call unavailable"}
    if not filename:
        return unavailable
    model = None
    try:
        for line in Path(filename).read_text().splitlines():
            row = json.loads(line)
            if provider == "claude":
                msg = row.get("message") or {}
                u = msg.get("usage")
                if row.get("type") != "assistant" or not u:
                    continue
                model = msg.get("model")
                context = sum(u.get(k, 0) for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
                usage = {"input": u.get("input_tokens", 0), "cache_write": u.get("cache_creation_input_tokens", 0)}
                one_hour = (u.get("cache_creation") or {}).get("ephemeral_1h_input_tokens", 0)
            else:
                payload = row.get("payload") or {}
                if row.get("type") == "turn_context":
                    model = payload.get("model") or model
                info = payload.get("info") or {}
                u = info.get("last_token_usage") or info.get("total_token_usage")
                if payload.get("type") != "token_count" or not u:
                    continue
                context = u.get("input_tokens", 0)
                usage = {"input": max(0, context - u.get("cached_input_tokens", 0)), "cache_write": 0}
                one_hour = 0
            price = call(usage, model, cache_1h=one_hour, context=context)
            return {**usage, "cache_1h": one_hour, "model": model, "context": context, "list_price_usd": price["list_price_usd"],
                    "basis": "observed first request uncached input and cache creation, excluding output"}
    except (OSError, ValueError):
        pass
    return unavailable



def extra_input_cost(first, provider, tokens):
    usage = {"input" if provider == "codex" else "cache_write": tokens}
    one_hour = tokens * first.get("cache_1h", 0) / first["cache_write"] if first.get("cache_write") else 0
    priced = call(usage, first.get("model"), cache_1h=one_hour, context=first.get("context"))
    basis = "native input rate (Codex)" if provider == "codex" else (
        "native first-write cache TTL split (Claude)" if "cache_1h" in first else "short cache-write estimate (Claude; TTL unavailable)")
    return {"tokens_per_seam": tokens, "list_price_usd_per_seam": priced["list_price_usd"],
            "basis": "UTF-8 bytes / 4; " + basis}


def report(result):
    scores = result.get("budget_scores", {})
    lines = [f"# D49 {result['variant'].upper()} bootstrap budget", "", "Report only; never gates a session or nightly.",
             f"Source: `{result['source']}`; daemon SHA-256: `{result['daemon_sha256']}`.",
             f"Parent: {result['provider']} `{result['model']}`, {result.get('effort', 'low')} effort.",
             "", "| Variant | Qualified | Bar | Measured scores |", "|---|---|---|---|",
             f"| {result['variant']} | {scores.get('qualified', False)} | {scores.get('bar', False)} | `{json.dumps(scores, sort_keys=True)}` |",
             "", f"Hot context at request: {result.get('request_context', 'n/a')}; seams before checks: {result.get('seams_before_checks', 0)}.",
             f"User bytes: {result.get('user_bytes', 'n/a')}; first successor input/write: `{json.dumps(result.get('first_successor_write', {}))}`.",
             f"Extra bootstrap estimate: `{json.dumps(result.get('extra_bootstrap_estimate', {}))}`.",
             "The legacy budget comparison is offline retention on this run's inputs, not a second live model arm. "
             "Per-role dollars below include the larger bootstrap. An incremental causal dollar delta requires a matched baseline run; it is not inferred.",
             "", "Run problems: " + ("; ".join(result.get("errors", [])) or "none"),
             "Cleanup: " + ("; ".join(result.get("cleanup", [])) or "own daemon, project and scratch removed"), ""]
    if result.get("variant") == "p2" and result.get("rows"):
        rows = result["rows"]
        applied = [r["unprompted"] for r in rows.values() if r["unprompted"] is not None]
        lines += [f"Existing decision checks: applied/raised {sum(map(bool, applied))}/{len(applied)}; recalled "
                  f"{sum(r['recalled'] for r in rows.values())}/{len(rows)}. The ten-item block has nine applicable checks.",
                  "", decisions.report(result, "sonnet")]
    else:
        lines += battery.economics_markdown(result)
    return "\n".join(lines)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--variant", choices=["p2", "p3"], required=True)
    p.add_argument("--daemon", required=True)
    p.add_argument("--model", default="sonnet")
    p.add_argument("--effort", choices=["low", "medium", "high"], default="low")
    p.add_argument("--target-model", default="gpt-6.1-sol")
    p.add_argument("--max-fillers", type=int, default=18)
    p.add_argument("--timeout", type=int, default=1800)
    p.add_argument("--turn-timeout", type=int, default=120)
    p.add_argument("--report", required=True)
    p.add_argument("--evidence")
    p.add_argument("--scratch-prefix", default="continuity-d49")
    args = p.parse_args()
    if "fable" in (args.model + args.target_model).lower() or str(Path(args.daemon).resolve()).startswith("/Applications/"):
        p.error("use permitted models and the checkout's debug daemon")
    if args.evidence and Path(args.evidence).resolve().is_relative_to(battery.SRC):
        p.error("keep private evidence outside the repo")
    args.provider, args.arm, args.tokens = "claude", "seam", 200_000
    run = Budgets(args)
    run.result.update(variant=args.variant, effort=args.effort, block=args.variant)
    def terminated(signum, frame):
        raise TimeoutError("nightly supervisor stopped the run")
    battery.signal.signal(battery.signal.SIGTERM, terminated)
    try:
        run.run()
    except Exception as error:
        run.errors.append(str(error))
        battery.log(f"run problem: {error}")
    finally:
        try:
            run.capture()
            if args.variant == "p2" and run.result.get("items"):
                decisions.score(run.result, "sonnet")
                run.result["budget_scores"] = score_p2(run.result)
        except Exception as error:
            run.errors.append("capture/score: " + str(error))
        run.cleanup()
        battery.write_report(run.result, report(run.result), args.report)
    return 0 if run.result.get("items") or run.result.get("status") == "skipped" else 2


if __name__ == "__main__":
    sys.exit(main())
