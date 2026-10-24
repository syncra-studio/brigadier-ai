"""Verify exact-marker eviction from native compaction payloads, never token estimates."""
import json


def snapshots(rows, provider):
    result = []
    if provider == "codex":
        for index, row in enumerate(rows):
            if row.get("type") != "compacted":
                continue
            payload = row.get("payload") or {}
            history = payload.get("replacement_history")
            result.append({"row": index, "kind": "native", "complete": isinstance(history, list) and not opaque(history),
                           "kept": json.dumps(history if isinstance(history, list) else [], ensure_ascii=False) +
                                   str(payload.get("message", "")), "payload": payload})
        for number, snapshot in enumerate(result):
            end = result[number + 1]["row"] if number + 1 < len(result) else len(rows)
            snapshot["active_kept"] = snapshot["kept"] + json.dumps(rows[snapshot["row"] + 1:end], ensure_ascii=False)
        return result
    by_id = {row["uuid"]: row for row in rows if row.get("uuid")}
    boundaries = [index for index, row in enumerate(rows) if row.get("type") == "system" and
                  row.get("subtype") == "compact_boundary"]
    for number, index in enumerate(boundaries):
        end = boundaries[number + 1] if number + 1 < len(boundaries) else len(rows)
        metadata = rows[index].get("compactMetadata") or {}
        preserved = metadata.get("preservedMessages") or {}
        ids = preserved.get("allUuids", preserved.get("uuids"))
        summaries = [row for row in rows[index + 1:end] if row.get("isCompactSummary")]
        kept = [by_id[key] for key in ids or [] if key in by_id] + summaries
        # Missing preservation metadata or a sealed/missing summary cannot prove absence.
        complete = isinstance(ids, list) and all(key in by_id for key in ids) and bool(summaries)
        result.append({"row": index, "kind": "native", "complete": complete,
                       "kept": json.dumps(kept, ensure_ascii=False), "payload": rows[index],
                       "active_kept": json.dumps(kept + rows[index + 1:end], ensure_ascii=False)})
    return result


def classify(marker, snapshot):
    if not snapshot:
        return "unverified"
    if marker in snapshot.get("kept", ""):
        return "retained"
    return "evicted" if snapshot.get("complete") else "unverified"


def classify_facts(facts, snapshot):
    return {key: classify(marker, snapshot) for key, marker in facts.items()}


def dropped_ids(expected, snapshot):
    """Count only proven losses, including after-compaction put-back items in context."""
    import re
    if not expected:
        return []
    if not snapshot or not snapshot.get("complete"):
        return None
    present = set(re.findall(r"\bL\d+\b", snapshot.get("active_kept", snapshot.get("kept", ""))))
    return sorted(set(expected) - present)


def opaque(value):
    """A sealed/encrypted summary cannot establish a marker's absence."""
    if isinstance(value, list):
        return any(opaque(item) for item in value)
    if isinstance(value, dict):
        return any(key in ("encrypted_content", "encryptedContent", "sealed") and bool(item) or opaque(item)
                   for key, item in value.items())
    return False
