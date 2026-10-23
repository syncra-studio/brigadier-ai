"""Read opted-in record/ lineages. Never copies conversation content into reports."""
import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path


def compactions(file, provider):
    """Count native boundaries, not their duplicate stream notifications."""
    count = 0
    with Path(file).open() as stream:
        for line in stream:
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if provider == "claude":
                count += row.get("type") == "system" and row.get("subtype") == "compact_boundary"
            else:
                count += row.get("type") == "compacted"
    return count


def sample(records, week=None, limit=20):
    week = week or dt.date.today().strftime("%G-W%V")
    candidates, errors = [], []
    for record in records:
        try:
            path = Path(record).expanduser().resolve()
            if path.name != "record":
                raise ValueError("opt in a record/ folder, not a home or transcript directory")
            lineage = json.loads((path / "lineage.json").read_text())
            # Runtime writes an array of provider/session_id/file/started_at entries.
            if not isinstance(lineage, list):
                raise ValueError("lineage.json must be an array")
            candidates.append((hashlib.sha256((week + str(path)).encode()).hexdigest(), lineage))
        except (OSError, ValueError) as error:
            errors.append(str(error))
    rows = []
    for _, lineage in sorted(candidates)[:limit]:
        for entry in lineage:
            provider = entry.get("provider")
            sid = entry.get("sessionId") or entry.get("session_id")
            if provider not in ("claude", "codex") or not sid:
                errors.append("lineage entry missing provider or session id")
                continue
            row = {"provider": provider, "session_id": sid}
            try:
                row["compactions"] = compactions(entry["file"], provider)
            except (OSError, ValueError, KeyError, TypeError) as error:
                row.update(compactions=None, error=type(error).__name__)
            rows.append(row)
    return {"week": week, "status": "complete" if candidates else "skipped",
            "reason": None if records else "needs the user: opt in record/ folders on a lineage runner",
            "sampled_lineages": min(len(candidates), limit), "sessions": rows, "errors": errors}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", action="append", default=[])
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    Path(args.output).write_text(json.dumps(sample(args.record), indent=2))
