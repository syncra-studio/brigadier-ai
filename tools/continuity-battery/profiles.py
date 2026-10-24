"""Opt-in work/endurance scenarios over the existing runner and arms."""
import hashlib
import json
from pathlib import Path
import random
import shutil

import battery
from evidence import records
from retention import classify_facts, dropped_ids, snapshots

DEPTHS = (3, 6, 10)
ITEM_KEYS = ("user_fact", "tool_fact", "release", "path", "units", "decision", "rule",
             "scoped", "closed", "replacement", "artifact", "open")


def token(rng, prefix):
    return prefix + "-" + rng.randbytes(8).hex().upper()


def scenario(seed, group):
    rng = random.Random(f"{seed}:{group}")
    facts = {key: token(rng, key.upper()) for key in ITEM_KEYS}
    facts["units"] = str(3000 + (ord(group) - ord("A")) * 2000 + rng.randrange(1000)) + " ms"
    facts["decision"] = str(6 + ord(group) - ord("A")) + " digits for accounting"
    facts["path"] = f"src/invoice-{group}.ts:{rng.randrange(30, 90)}"
    old = {"scoped": token(rng, "OLD-EXPORTER"), "closed": facts["closed"]}
    # 'closed' asks the current state of the old promise, not its obsolete identifier.
    facts["closed"] = "closed"
    return {"group": group, "facts": facts, "old": old}


def packet(seed, index):
    """Small, useful task receipts; no 28k inert filler batches."""
    rng = random.Random(f"{seed}:work:{index}")
    values = [rng.randrange(1, 500) for _ in range(24)]
    receipt = json.dumps({"batch": index, "amounts": values, "unit": "cents", "currency": "EUR",
                          "rule": "sum amounts without rounding; keep input order", "notes": [
                              f"Account {j:02}: amount {amount} cents; checked input, pending export."
                              for j, amount in enumerate(values)]}, indent=2)
    # ≤2KB per tool receipt; the agent reads and writes small files in each turn.
    return receipt, sum(values)


class Work(battery.Battery):
    def setup_archive(self):
        self.archive = self.root / "lookup-archive.json"
        self.lookup_script = self.root / "continuity_lookup.py"
        shutil.copyfile(Path(__file__).with_name("lookup.py"), self.lookup_script)
        self.archive.write_text("[]")
        self.snapshot_history = []
        self.refresh_archive()

    def lookup_instruction(self):
        import shlex
        return (f"Read-only record lookup is allowed: python3 {shlex.quote(str(self.lookup_script))} "
                f"{shlex.quote(str(self.archive))} search 'text' or read 'item-id'. "
                "Use this same record lookup before saying unknown; do not read fixture scripts, "
                "other harness files, or edit files during a recall probe. ")

    def source_files(self):
        if getattr(self, "native_sessions", None):
            return [(self.args.provider, Path(file)) for file in self.native_sessions.values() if file]
        if self.conversation:
            return list(dict.fromkeys((e["entry"].get("provider", self.args.provider), battery.native_path(event["file"],
                            e["entry"].get("provider", self.args.provider))) for e in self.entries()
                           if (event := e["entry"].get("event") or {}).get("type") == "sessionStarted" and event.get("file")))
        return []

    def refresh_archive(self):
        items, payloads = [], []
        for provider, path in self.source_files():
            if not path.is_file():
                continue
            rows = records(path)
            for index, raw in enumerate(path.read_text().splitlines()):
                try:
                    row = json.loads(raw)
                except ValueError:
                    continue
                # Native item id where present; otherwise a stable exact source/row id.
                key = row.get("uuid") or (row.get("payload") or {}).get("id") or hashlib.sha256(
                    (path.name + ":" + str(index)).encode()).hexdigest()[:20]
                items.append({"id": key, "provider": provider, "source": path.name, "raw": raw})
            for snapshot in snapshots(rows, provider):
                from datetime import datetime
                timestamp = rows[snapshot["row"]].get("timestamp")
                try:
                    at_ms = datetime.fromisoformat(timestamp.replace("Z", "+00:00")).timestamp() * 1000
                except (AttributeError, ValueError):
                    at_ms = 0
                payloads.append({**snapshot, "source": path.name, "at_ms": at_ms})
        self.archive.write_text(json.dumps(items, ensure_ascii=False))
        if self.args.arm in ("seam", *getattr(self, "candidate_arms", ())):
            file = self.seam_file() if self.conversation else {}
            for seam in file.get("seams", []):
                bootstrap = battery.read_text(self.record() / "handoff" / f"{seam['n']}-start.md")
                if bootstrap:
                    payloads.append({"kind": "seam", "complete": True, "kept": bootstrap, "n": seam["n"], "at_ms": seam.get("startedAtMs", 0)})
        payloads.sort(key=lambda value: value.get("at_ms", 0))
        self.snapshot_history = payloads
        self.result["compaction_payloads"] = payloads

    def observed(self, name, text):
        step = self.send(name, text)
        if not step["settled"]:
            raise RuntimeError(name + ": turn failed; lineage incomplete")
        self.refresh_archive()
        return step

    def plant(self, group):
        case = scenario(self.args.seed, group)
        facts, old = case["facts"], case["old"]
        directory = self.repo / "src/artifacts"
        directory.mkdir(parents=True, exist_ok=True)
        (directory / f"{group}.json").write_text('{"stage":"draft"}\n')
        user_facts = {key: value for key, value in facts.items() if key not in ("tool_fact", "closed", "scoped")}
        user_facts["scoped"] = old["scoped"]
        self.observed(f"plant-{group}", f"Release {group} instructions, given once. "
                      "Remember these for later work. Acknowledge without repeating values.\n" +
                      json.dumps(user_facts) + "\n" +
                      f"General artifact label: {facts['rule']}. Exporter label: {old['scoped']}. "
                      f"Promise {old['closed']} is open; replacement {facts['replacement']} is not active yet. "
                      f"Open question {facts['open']} waits on me. "
                      f"Later finish src/artifacts/{group}.json with fields stage='ready', "
                      "release (the release value above), timeout_ms (the units value as integer), "
                      "invoice_width (the digit decision as integer), general (the general label), "
                      "exporter (the exporter label), artifact (the artifact value above).")
        receipt = self.repo / "receipt.txt"
        text = f"One-time receipt for release {group}: {facts['tool_fact']}\n"
        receipt.write_text(text + "checked event details\n" * ((2048 - len(text)) // 22))
        self.observed(f"tool-{group}", "Read receipt.txt. Do not save or repeat its value. Reply only 'read'.")
        receipt.unlink()
        self.result.setdefault("expected", {}).update({f"{group}.{key}": value for key, value in facts.items() if key != "closed"})
        self.result["expected"][f"{group}.closed"] = old["closed"]
        self.result["expected"][f"{group}.scoped.old"] = old["scoped"]
        case["planted_step"] = len(self.steps)
        case["fact_snapshot_counts"] = {key: len(self.snapshot_history) for key in facts}
        self.result.setdefault("plants", []).append(case)
        return case

    def revise(self, cases):
        for case in cases:
            f, old, group = case["facts"], case["old"], case["group"]
            self.observed(f"revision-{group}", f"For release {group}, change the exporter label ONLY to {f['scoped']}; "
                          "the general label is unchanged. "
                          f"Close promise {old['closed']} and replace it with {f['replacement']}, now open. "
                          "All other instructions stand. Acknowledge without repeating values.")
            for key in ("scoped", "closed", "replacement"):
                case["fact_snapshot_counts"][key] = len(self.snapshot_history)

    def work_turn(self, index):
        receipt, expected = packet(self.args.seed, index)
        file = self.repo / "work-receipt.json"
        file.write_text(receipt)
        # ~500 user + ~500 tool + ~600 output tokens per task. Measure the native
        # distribution afterwards; this target is not a claim about actual tokenization.
        context = " ".join(f"Account {j} has a checked amount. Preserve EUR cents and source order."
                           for j in range(48))
        self.observed(f"work-{index:04}", f"Import task {index}. Read work-receipt.json, calculate its total in cents, "
                      f"and write src/work-{index:04}.json with batch, currency and total_cents. "
                      "Use small read/write commands. Do not touch release artifacts. "
                      "Explain your calculation and checks in about 400 words (roughly 600 output tokens). " + context)
        file.unlink(missing_ok=True)
        actual = battery.parse_answer(battery.read_text(self.repo / f"src/work-{index:04}.json"))
        self.result.setdefault("work_checks", []).append(actual.get("total_cents") == expected and actual.get("currency") == "EUR")

    def checkpoint_ids(self):
        import re
        return sorted(set(re.findall(r"\bL\d+\b", battery.read_text(self.record() / "ledger.md"))))

    def probe(self, case, depth):
        group, facts = case["group"], case["facts"]
        # Snapshot BEFORE application/recall can refresh anything. Keep the full native
        # payload in private evidence; public scores expose only classification/counts.
        snapshot = self.snapshot_history[-1] if self.snapshot_history else None
        markers = {**facts, "closed": case["old"]["closed"]}
        retention = classify_facts(markers, snapshot)
        for key in facts:
            if len(self.snapshot_history) <= case.get("fact_snapshot_counts", {}).get(key, 0):
                retention[key] = "unverified"
        self.result.setdefault("probe_retention", {})[group] = retention
        ledger_ids = self.checkpoint_ids()
        self.observed(f"apply-{group}", f"Finish src/artifacts/{group}.json to its earlier release specification. "
                      "Make it ready to ship; do not ask me to repeat settled choices. Do not commit or push.")
        artifact = battery.parse_answer(battery.read_text(self.repo / f"src/artifacts/{group}.json"))
        apply = {"release": artifact.get("release") == facts["release"],
                 "units": artifact.get("timeout_ms") == int(facts["units"].split()[0]),
                 "decision": artifact.get("invoice_width") == int(facts["decision"].split()[0]),
                 "rule": artifact.get("general") == facts["rule"],
                 "scoped": artifact.get("exporter") == facts["scoped"],
                 "artifact": artifact.get("artifact") == facts["artifact"] and artifact.get("stage") == "ready"}
        status = self.observed(f"status-{group}", f"Where are we on release {group}? Mention what is still owed "
                               "or unresolved by its exact identifier.")["answer"]
        apply["replacement"] = facts["replacement"] in status and (
            facts["closed"] == "open" or case["old"]["closed"] not in status)
        apply["open"] = facts["open"] in status
        answer = self.observed(f"probe-{group}", self.lookup_instruction() + f"For release {group}, return one JSON object "
                               "with keys " + ", ".join(ITEM_KEYS) + ". Give the current values only. For closed, say "
                               "'closed' if the original promise was closed, otherwise 'open'. "
                               "Use only conversation context and the allowed record lookup. Say unknown without evidence.")["answer"]
        parsed = battery.parse_answer(answer)
        rows = {}
        for key in ITEM_KEYS:
            value = str(parsed.get(key, "unknown"))
            # Random exact identifiers and full typed values, not loose keyword matches.
            recalled = value.strip() == facts[key]
            passed = recalled and apply.get(key, True)
            rows[key] = {"verdict": "pass" if passed else "fail", "recalled": recalled,
                         "action": apply.get(key), "retention": retention[key], "answer": value,
                         "stale": key == "scoped" and (case["old"]["scoped"] in value or artifact.get("exporter") == case["old"]["scoped"])}
        eligible = [row for row in rows.values() if row["retention"] == "evicted"]
        decisions = ("rule", "scoped", "closed", "replacement", "open")
        self.result.setdefault("depth_scores", []).append({"depth": depth, "group": group, "items": rows,
             "memory_items": len(eligible), "recall_apply": sum(row["verdict"] == "pass" for row in eligible) / len(eligible) if eligible else None,
             "retained": sum(row["retention"] == "retained" for row in rows.values()),
             "unverified": sum(row["retention"] == "unverified" for row in rows.values()),
             "stale_answers": sum(row["stale"] for row in rows.values()),
             "decision_violations": {key: int(rows[key]["verdict"] != "pass") for key in decisions},
             "ledger_ids": ledger_ids, "dropped_ledger_ids": dropped_ids(ledger_ids, snapshot)})

    def run(self):
        from arms import CANDIDATES
        self.candidate_arms = CANDIDATES
        self.setup()
        self.setup_archive()
        self.result["expected"] = {}
        self.result.update(profile=self.args.profile, seed=self.args.seed, lineage=self.args.lineage,
                           phase="pilot", shared_transition=self.args.shared_transition, block=self.args.profile)
        self.observed("setup", "This is a throwaway continuity work session. Do not start workers or commit or push. " + self.lookup_instruction())
        cases = []
        groups = ("A", "B", "C") if self.args.profile == "endurance" else ("A",)
        for index, group in enumerate(groups):
            self.work_turn(index)
            cases.append(self.plant(group))
        if self.args.profile == "endurance" and self.generations() > 0:
            raise RuntimeError("endurance: compaction occurred before every plant; increase threshold")
        self.result["initial_ledger_ids"] = self.checkpoint_ids()
        revised, asked = False, set()
        for index in range(len(groups), self.args.turns):
            self.work_turn(index)
            depth = self.generations()
            if not revised and (depth >= 5 if self.args.profile == "endurance" else index >= self.args.turns // 2):
                self.revise(cases)
                revised = True
            if self.args.profile == "endurance":
                for target, case in zip(DEPTHS, cases):
                    if depth >= target and target not in asked:
                        # Set A is pre-reversal; B/C use the revised exporter/promise state.
                        if target == 3:
                            initial = {**case, "facts": {**case["facts"], "scoped": case["old"]["scoped"], "closed": "open", "replacement": case["old"]["closed"]}}
                            self.probe(initial, target)
                        else:
                            self.probe(case, target)
                        asked.add(target)
                if len(asked) == len(DEPTHS):
                    break
        if self.args.profile != "endurance":
            self.probe(cases[0], self.generations())
        elif asked != set(DEPTHS):
            self.errors.append("endurance: fewer than ten observed transitions; lineage incomplete")
        self.result["items"] = {f"{score['group']}.{key}": row for score in self.result.get("depth_scores", [])
                                for key, row in score["items"].items()}
        self.result["seams_before_checks"] = self.generations()
