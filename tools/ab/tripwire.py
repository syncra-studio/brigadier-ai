#!/usr/bin/env python3
"""The token tripwire for one bench task (WORKSPACES-PLAN Phase 4, token doc item 3), read-only
over a Brigadier data dir's `routing.sqlite`. Run with `python3 -I`.

- Tokens: the task's tab and its tasks in the window, input + cached input + cache writes +
  output. Above 1.25 × the same task at the last ship (`--last`), the phase report needs one line
  saying what bought it. It never blocks.
- Cache-hit share: cached input ÷ (input + cached input + cache writes).
- Opening-call cache writes and misses by cause: each session's first call (`cause_write`,
  `cause_missed` on the row with its `cause`). All other writes and misses are one unattributed
  line, with the calls no miss could be estimated for.
- Invalid tool calls: Brigadier tool calls whose arguments missed the tool's schema
  (`bad_tool_calls`), by tool.
- The tier behind each `brain` and `upkeep` row in the window (any project), from the registry the
  daemon reads (the newer of the data dir's download and the bundled `registry/models.json`); a
  row above standard tier is flagged.

usage: tripwire.py <data-dir> <conversation> <from_ms> <to_ms> [--last TOKENS] [--registry FILE]"""
import argparse, atexit, json, os, shutil, sqlite3, sys, tempfile

TRIPWIRE = 1.25
ABOVE_STANDARD = ("strong", "frontier")


BUNDLED = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
                       "registry", "models.json")


def connect(path):
    """`path` read-only. A WAL database whose daemon is gone has no -shm to open read-only: a
    copy with its WAL is opened instead, so rows still in the WAL count."""
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        db.execute("PRAGMA schema_version").fetchone()
        return db
    except sqlite3.OperationalError:
        db.close()
    copy = os.path.join(tempfile.mkdtemp(prefix="tripwire-db-"), os.path.basename(path))
    atexit.register(shutil.rmtree, os.path.dirname(copy), True)
    for suffix in ("", "-wal"):
        if os.path.exists(path + suffix):
            shutil.copyfile(path + suffix, copy + suffix)
    return sqlite3.connect(copy)


def tiers(paths):
    """Each model id the registry knows, with its tier: the highest revision of `paths` (the
    data dir's download, the app's bundled copy), as the daemon picks."""
    best = None
    for path in paths:
        try:
            with open(path) as f:
                registry = json.load(f)
        except (OSError, ValueError):
            continue
        if best is None or registry.get("revision", 0) > best.get("revision", 0):
            best = registry
    out = {}
    for model in (best or {}).get("models") or []:
        for id_ in [model.get("key"), *((model.get("match") or {}).get("ids") or [])]:
            if id_:
                out[id_] = model.get("tier")
    return out


def n(value):
    return f"{value:,}"


def report(db, conversation, since, until, last, tier_of):
    lines = []
    columns = {row[1] for row in db.execute("PRAGMA table_info(turn_usage)")}
    traced = "cause" in columns
    where = "conversation_id = ? AND at_ms BETWEEN ? AND ?"
    args = (conversation, since, until)
    raw, inp, cached, written = db.execute(
        "SELECT COALESCE(SUM(input + cached_input + cache_write + output), 0), COALESCE(SUM(input), 0), "
        f"COALESCE(SUM(cached_input), 0), COALESCE(SUM(cache_write), 0) FROM turn_usage WHERE {where}",
        args).fetchone()
    line = f"Tokens: {n(raw)}"
    if last:
        ratio = raw / last
        line += f", {ratio:.2f}× the last ship's {n(last)}"
        if ratio > TRIPWIRE:
            line += f": over {TRIPWIRE}×, so the phase report needs one line saying what bought it"
    lines.append(line + ".")
    prompt = inp + cached + written
    share = f"{cached / prompt:.1%}" if prompt else "none"
    lines.append(f"Cache-hit share: {share} ({n(cached)} of {n(prompt)} prompt tokens read from cache).")
    if traced:
        lines.append("Opening-call cache writes and misses by cause:")
        causes = db.execute(
            "SELECT cause, COUNT(*), COALESCE(SUM(cause_write), 0), COALESCE(SUM(cause_missed), 0) "
            f"FROM turn_usage WHERE {where} AND cause IS NOT NULL GROUP BY cause "
            "ORDER BY SUM(COALESCE(cause_write, 0) + COALESCE(cause_missed, 0)) DESC, cause", args).fetchall()
        for cause, sessions, wrote, missed in causes:
            plural = "session" if sessions == 1 else "sessions"
            lines.append(f"  {cause}: {sessions} {plural}, wrote {n(wrote)}, missed {n(missed)}")
        if not causes:
            lines.append("  none")
        missed, opening_write, opening_missed, unestimated = db.execute(
            "SELECT COALESCE(SUM(missed), 0), COALESCE(SUM(cause_write), 0), COALESCE(SUM(cause_missed), 0), "
            f"COALESCE(SUM(unestimated_calls), 0) FROM turn_usage WHERE {where}", args).fetchone()
        lines.append(f"Unattributed: wrote {n(max(written - opening_write, 0))}, "
                     f"missed {n(max(missed - opening_missed, 0))}; unestimated calls {n(unestimated)}.")
    else:
        lines.append("Opening-call causes: not recorded by this data dir's Brigadier.")
    has_bad = db.execute("SELECT 1 FROM sqlite_master WHERE name = 'bad_tool_calls'").fetchone()
    if has_bad:
        bad = db.execute(
            "SELECT tool, COUNT(*) FROM bad_tool_calls WHERE conversation_id = ? AND at_ms BETWEEN ? AND ? "
            "GROUP BY tool ORDER BY tool", args).fetchall()
        total = sum(count for _, count in bad)
        detail = f" ({', '.join(f'{tool} {count}' for tool, count in bad)})" if bad else ""
        lines.append(f"Invalid tool calls: {total}{detail}.")
    else:
        lines.append("Invalid tool calls: not counted by this data dir's Brigadier.")
    lines.append("Brain and upkeep rows by tier:")
    rows = db.execute(
        "SELECT step, model, COUNT(*), SUM(input + cached_input + cache_write + output) FROM turn_usage "
        "WHERE step IN ('brain', 'upkeep') AND at_ms BETWEEN ? AND ? GROUP BY step, model ORDER BY step, model",
        (since, until)).fetchall()
    for step, model, count, tokens in rows:
        tier = tier_of.get(model) or "unknown"
        flag = ": above standard" if tier in ABOVE_STANDARD else ""
        plural = "row" if count == 1 else "rows"
        lines.append(f"  {step} on {model} ({tier}): {count} {plural}, {n(tokens)} tokens{flag}")
    if not rows:
        lines.append("  none")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("data_dir")
    ap.add_argument("conversation")
    ap.add_argument("since", type=int)
    ap.add_argument("until", type=int)
    ap.add_argument("--last", type=int, help="the same task's tokens at the last ship")
    ap.add_argument("--registry", help="default: the newer of <data-dir>/cache/registry/models.json and the repo's")
    args = ap.parse_args()
    db = connect(os.path.join(args.data_dir, "routing.sqlite"))
    registries = [args.registry] if args.registry else [
        os.path.join(args.data_dir, "cache", "registry", "models.json"), BUNDLED]
    print(report(db, args.conversation, args.since, args.until, args.last, tiers(registries)))


if __name__ == "__main__":
    sys.exit(main())
