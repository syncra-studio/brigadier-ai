"""Private, expiring evidence. Never put this directory in a public artifact."""
import argparse
import json
import os
from pathlib import Path
import secrets
import shutil
import time

DAYS = 7
STORE = Path(os.environ.get("BRIGADIER_BATTERY_EVIDENCE_DIR", Path.home() / ".cache/brigadier-continuity/private"))
SCHEMA = "brigadier-continuity-private-v1"


def private_json(path, value):
    path = Path(path)
    absolute = path.resolve()
    if any((parent / ".git").exists() for parent in absolute.parents):
        raise ValueError("private evidence must stay outside every git checkout")
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    # Refuse links, rather than following a public or user-owned export target.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(fd, "w") as output:
        os.fchmod(output.fileno(), 0o600)
        json.dump(value, output, indent=2)
        output.write("\n")


def purge(store=STORE, clock=None):
    """Only delete our marked bundles; invoked at every harness/nightly startup."""
    clock = time.time() if clock is None else clock
    removed = []
    if not Path(store).exists():
        return removed
    for directory in Path(store).glob("evidence-*"):
        if directory.is_symlink() or not directory.is_dir():
            continue
        try:
            manifest = json.loads((directory / "manifest.json").read_text())
            if manifest.get("schema") == SCHEMA and manifest["expires_at"] <= clock:
                shutil.rmtree(directory)
                removed.append(directory.name)
        except (OSError, ValueError, KeyError, TypeError):
            continue
    return removed


def retain(run, store=STORE, days=DAYS):
    """Called before IPC Delete or transcript cleanup, including for partial runs."""
    store = Path(store).resolve()
    from battery import SRC
    if store.is_relative_to(SRC) or store.is_relative_to(run.repo.resolve()) or store.is_relative_to(run.root.resolve()):
        raise ValueError("private evidence must outlive scratch and stay outside the fixture")
    purge(store)
    store.mkdir(parents=True, exist_ok=True, mode=0o700)
    store.chmod(0o700)
    directory = store / ("evidence-" + time.strftime("%Y%m%dT%H%M%S") + "-" + secrets.token_hex(6))
    directory.mkdir(mode=0o700)
    created = time.time()
    private_json(directory / "manifest.json", {"schema": SCHEMA, "created_at": created,
                 "expires_at": created + days * 86400, "retention_days": days})
    run.result["private_evidence"] = {"directory": str(directory), "retention_days": days}
    private_json(directory / "evidence.json", run.result)
    sources, audit = [], []
    # Read before Delete: a daemon may delete/move the native session and its sidecars.
    from battery import native_path
    for index, (provider, filename) in enumerate(getattr(run, "native_sources", [(run.args.provider, file) for file in run.native_files])):
        file = native_path(filename, provider)
        if not file.is_file():
            continue
        target = directory / f"native-{index}.jsonl"
        target.write_bytes(file.read_bytes())
        target.chmod(0o600)
        sources.append({"source": str(file), "copy": target.name, "provider": provider})
        rows = records(target)
        audit.append({"file": target.name, "calls": calls(rows, provider),
                      "lookups": lookups(rows, provider, run.result.get("expected", {}))})
        sidecar = file.with_suffix("")
        if sidecar.is_dir():
            copy_tree(sidecar, directory / f"native-{index}-sidecar")
    record = run.record()
    if record.is_dir():
        # The plain arm's record() is its throwaway repo; retain artifacts too.
        copy_tree(record, directory / "record")
    log = run.root / "daemon.log"
    if log.is_file():
        (directory / "daemon.log").write_bytes(log.read_bytes())
        (directory / "daemon.log").chmod(0o600)
    private_json(directory / "sources.json", sources)
    private_json(directory / "call-audit.json", audit)
    lookup_rows = [item for source in audit for item in source["lookups"]]
    run.result["lookup_stats"] = {"calls": len(lookup_rows), "calls_with_hits": sum(bool(item["hits"]) for item in lookup_rows),
                                  "distinct_items_hit": len({key for item in lookup_rows for key in item["hits"] or []})}
    return directory


def copy_tree(source, destination):
    """Copy regular evidence only, never links out of the owned record."""
    destination.mkdir(mode=0o700)
    for file in source.iterdir():
        if file.is_symlink() or file.name == ".git":
            continue
        target = destination / file.name
        if file.is_dir():
            copy_tree(file, target)
        elif file.is_file():
            target.write_bytes(file.read_bytes())
            target.chmod(0o600)


def records(file):
    rows = []
    for line in Path(file).read_text().splitlines():
        try:
            rows.append(json.loads(line))
        except ValueError:
            continue
    return rows


def calls(rows, provider):
    """Raw usage includes TTL details; no bytes/4 substitutes for billed tokens."""
    if provider == "claude":
        best = {}
        for row in rows:
            message = row.get("message") or {}
            if row.get("type") != "assistant" or not message.get("id") or not message.get("usage"):
                continue
            previous = best.get(message["id"])
            if previous and previous["usage"].get("output_tokens", 0) > message["usage"].get("output_tokens", 0):
                continue
            best[message["id"]] = {"id": message["id"], "model": message.get("model"),
                                  "usage": message["usage"], "timestamp": row.get("timestamp")}
        return list(best.values())
    result, last, model = [], {}, None
    for row in rows:
        payload = row.get("payload") or {}
        if row.get("type") == "turn_context":
            model = payload.get("model", model)
        info = payload.get("info") or {}
        total = info.get("total_token_usage")
        if payload.get("type") == "token_count" and total and total != last:
            result.append({"model": model, "timestamp": row.get("timestamp"), "ttl": "not reported",
                           "usage": {key: value - last.get(key, 0) for key, value in total.items()},
                           "last_token_usage": info.get("last_token_usage")})
            last = total
    return result


def lookups(rows, provider, markers):
    """Pair actual lookup commands with their output, separately from search counts."""
    from battery import LOOKUP_COMMAND
    pending, found = {}, []
    for row in rows:
        payload = row.get("payload") or {} if provider == "codex" else row.get("message") or {}
        contents = ([payload] if provider == "codex" else payload.get("content", []))
        if not isinstance(contents, list):
            continue
        for item in contents:
            if not isinstance(item, dict):
                continue
            kind = item.get("type")
            if kind in ("tool_use", "function_call", "custom_tool_call"):
                arguments = item.get("input", item.get("arguments", ""))
                text = arguments if isinstance(arguments, str) else json.dumps(arguments)
                if LOOKUP_COMMAND.search(text) or "continuity_lookup.py" in text:
                    pending[item.get("id", item.get("call_id"))] = text
            if kind in ("tool_result", "function_call_output", "custom_tool_call_output"):
                key = item.get("tool_use_id", item.get("call_id"))
                if key in pending:
                    output = item.get("content", item.get("output", ""))
                    output = output if isinstance(output, str) else json.dumps(output)
                    found.append({"id": key, "command": pending.pop(key), "output": output,
                                  "hits": [key for key, marker in markers.items() if marker in output]})
    found += [{"id": key, "command": command, "output": None, "hits": None}
              for key, command in pending.items()]
    return found


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["purge"])
    parser.add_argument("--store", type=Path, default=STORE)
    args = parser.parse_args()
    print(json.dumps({"removed": purge(args.store)}))
