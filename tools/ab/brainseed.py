#!/usr/bin/env python3
"""The Brain seed for a token A/B (SESSION-FIXES-PLAN.md §3 Q6.7): a copy of a project's
brain.sqlite as it stood at a cut time, so both arms start from what the Brain knew then and
nothing it learned from the measured work itself.

The source is read through `?mode=ro` and copied with SQLite's backup API; nothing is written
to it. In the copy:
- every node with anything from the cut or later goes: created (by `created_ms` or its UUIDv7
  id's time, since a rewrite keeps the first `created_ms`), updated, superseded, marked stale, or
  a source recorded; with its edges, sources and files. That is conservative: a node rewritten
  after the cut can't be rolled back, so it isn't kept at all;
- the transcript rows at or after the cut go;
- then every kept node's sources are audited (origin, session, task, newest recording) and its
  text is searched for the measured task's own words (`--words a,b,…`).

usage: brainseed.py <source brain.sqlite> <cut_ms> <out.sqlite> [--words a,b,…] [--json report.json]"""
import json, os, sqlite3, sys


def id_ms(node_id):
    try:
        return int(node_id.replace("-", "")[:12], 16)
    except ValueError:
        return None


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    source, cut, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    if os.path.exists(out):
        sys.exit(f"brainseed: {out} exists")
    src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    dst = sqlite3.connect(out)
    src.backup(dst)
    src.close()
    dst.execute("PRAGMA foreign_keys = ON")
    before = dst.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
    # Conservative: a node goes when anything about it is from the cut or later (created, its
    # id's time, updated, superseded, marked stale, or a source recorded), since a rewrite keeps
    # the node's id and `created_ms` and an update can't be rolled back.
    late_sources = {row[0] for row in dst.execute("SELECT node_id FROM node_sources WHERE recorded_ms >= ?", (cut,))}
    late = [node_id for node_id, created, updated, superseded, stale in dst.execute(
                "SELECT id, created_ms, updated_ms, superseded_ms, stale_since_ms FROM nodes")
            if max(created, updated, superseded or 0, stale or 0, id_ms(node_id) or 0) >= cut
            or node_id in late_sources]
    dst.executemany("DELETE FROM nodes WHERE id = ?", [(node_id,) for node_id in late])
    sources = dst.execute("DELETE FROM node_sources WHERE recorded_ms >= ?", (cut,)).rowcount
    transcript = dst.execute("DELETE FROM transcript WHERE at_ms >= ?", (cut,)).rowcount
    # A kept node superseded by one that went: keep its state as it was (superseded) but drop
    # the pointer, so nothing points at a missing node.
    unpointed = dst.execute("UPDATE nodes SET superseded_by = NULL WHERE superseded_by IS NOT NULL AND "
                            "superseded_by NOT IN (SELECT id FROM nodes)").rowcount
    dangling = dst.execute("SELECT COUNT(*) FROM nodes WHERE superseded_by IS NOT NULL AND superseded_by NOT IN "
                           "(SELECT id FROM nodes)").fetchone()[0]
    dst.commit()
    dst.execute("VACUUM")
    kept = dst.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
    kinds = dict(dst.execute("SELECT kind, COUNT(*) FROM nodes GROUP BY kind"))
    states = dict(dst.execute("SELECT state, COUNT(*) FROM nodes GROUP BY state"))
    # The audit: where every kept fact came from, and the newest moment anything in it is from.
    origins = dict(dst.execute("SELECT origin, COUNT(*) FROM node_sources GROUP BY origin"))
    tasks = [{"task": task, "nodes": n, "newest_ms": newest} for task, n, newest in dst.execute(
        "SELECT task_id, COUNT(DISTINCT node_id), MAX(recorded_ms) FROM node_sources WHERE task_id IS NOT NULL "
        "GROUP BY task_id ORDER BY 3")]
    sessions = dict(dst.execute("SELECT coalesce(session_id, '-'), COUNT(DISTINCT node_id) FROM node_sources "
                                "GROUP BY 1"))
    newest = dst.execute("SELECT MAX(m) FROM (SELECT MAX(created_ms, updated_ms, coalesce(superseded_ms, 0), "
                         "coalesce(stale_since_ms, 0)) AS m FROM nodes UNION ALL SELECT MAX(recorded_ms) FROM "
                         "node_sources UNION ALL SELECT MAX(at_ms) FROM transcript)").fetchone()[0]
    words = sys.argv[sys.argv.index("--words") + 1].split(",") if "--words" in sys.argv else []
    mentions = {word: [{"id": i, "kind": k, "title": t} for i, k, t in dst.execute(
        "SELECT id, kind, title FROM nodes WHERE instr(lower(title || ' ' || body), lower(?)) > 0", (word,))]
        for word in words}
    sourceless = dst.execute("SELECT COUNT(*) FROM nodes WHERE id NOT IN (SELECT node_id FROM node_sources)").fetchone()[0]
    dst.close()
    report = {"source": source, "cut_ms": cut, "nodes_before": before, "nodes_kept": kept, "kinds": kinds,
              "nodes_removed": len(late), "sources_removed": sources, "transcript_rows_removed": transcript,
              "pointers_to_removed_nodes_cleared": unpointed, "dangling_superseded_by": dangling,
              "audit": {"newest_ms_anywhere": newest, "before_cut": newest is None or newest < cut, "states": states,
                        "source_origins": origins, "nodes_without_sources": sourceless,
                        "nodes_by_session": sessions, "tasks": tasks, "mentions": mentions}}
    print(json.dumps(report, indent=1))
    if "--json" in sys.argv:
        json.dump(report, open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1)


if __name__ == "__main__":
    main()
