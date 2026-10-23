"""Shared list-price estimates; provider-reported charges and regression units stay separate."""
from collections import Counter
import json
from pathlib import Path
import re

PRICES = json.loads(Path(__file__).with_name("prices.json").read_text())
FIELDS = ("input", "cache_write", "cache_read", "output")
NOTE = ("USD estimates use standard list prices from tools/bench/prices.json, not subscription charges. "
        "Native cache TTL and per-call context tiers are used when available; absent TTL uses short writes. "
        "Missing models/prices are n/a; partial totals and means are never priced. "
        "Service-tier, regional and tool fees are excluded. Cost units and regression rules are unchanged.")


def rates(model):
    # Only dated snapshots of a known model are aliases; never resolve CLI aliases like opus.
    return PRICES["models"].get(model) or PRICES["models"].get(re.sub(r"-\d{8}$", "", model or ""))


def call(usage, model, cache_1h=0, context=None, notes=()):
    usage = {k: usage.get(k, 0) or 0 for k in FIELDS}
    price = rates(model)
    result = {"usage": usage, "models": [model] if model else [], "notes": list(notes)}
    if not price:
        result.update(list_price_usd=None, notes=list(notes) + [f"No public price for native model {model or 'unknown'}"])
        return result
    if price.get("long_context") and context is None:
        result.update(list_price_usd=None, notes=list(notes) + ["Per-call context unavailable for tiered price"])
        return result
    if price.get("long_context") and context > price["long_context"]["above_tokens"]:
        price = {**price, **price["long_context"]}
    cache_1h = min(cache_1h, usage["cache_write"])
    result["list_price_usd"] = (usage["input"] * price["input"]
                               + (usage["cache_write"] - cache_1h) * price["cache_write"]
                               + cache_1h * price.get("cache_write_1h", price["cache_write"])
                               + usage["cache_read"] * price["cache_read"]
                               + usage["output"] * price["output"]) / 1_000_000
    return result


def combine(parts):
    parts = list(parts)
    usage = Counter({k: 0 for k in FIELDS})
    for part in parts:
        usage.update(part["usage"])
    uncached = usage["input"] + usage["cache_write"]
    total = uncached + usage["cache_read"]
    available = bool(parts) and all(p.get("usage_available", True) for p in parts)
    priced = available and all(p["list_price_usd"] is not None for p in parts)
    return {"usage": dict(usage), "usage_available": available, "uncached_input": uncached, "cache_read": usage["cache_read"],
            "cache_hit_ratio": usage["cache_read"] / total if total else None, "output": usage["output"],
            "models": sorted({m for p in parts for m in p["models"]}),
            "list_price_usd": sum(p["list_price_usd"] for p in parts) if priced else None,
            "price_status": "priced" if priced else "n/a",
            "notes": sorted({n for p in parts for n in p.get("notes", [])})}


def historical(usage, models):
    """Old artifacts retain model sets, but have no per-call model, context or cache TTL."""
    return combine([call(usage, models[0] if len(models) == 1 else None,
                         notes=["Legacy aggregate: cache TTL and per-call context unavailable; short-write estimate"])])


def usd(value):
    return "n/a" if value is None else f"${value:.6f}"


def ratio(value):
    return "n/a" if value is None else f"{value:.3f}×"


def mean_usd(rows):
    values = [r.get("list_price_usd") for r in rows]
    return sum(values) / len(values) if values and all(v is not None for v in values) else None


def role_table(roles):
    lines = ["| Role | Native model(s) | Uncached input | Cache read | Cache hit ratio | Output | List-price USD |",
             "|---|---|---:|---:|---:|---:|---:|"]
    for name, row in roles.items():
        available = row.get("usage_available", True)
        value = lambda key: row[key] if available else "n/a"
        hit = row["cache_hit_ratio"] if available else None
        lines.append(f"| {name} | {', '.join(row['models']) or 'unknown'} | {value('uncached_input')} | "
                     f"{value('cache_read')} | {f'{hit:.2%}' if hit is not None else 'n/a'} | "
                     f"{value('output')} | {usd(row['list_price_usd'])} |")
    return lines
