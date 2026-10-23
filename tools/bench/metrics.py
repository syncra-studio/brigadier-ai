"""Native transcript accounting. Cache reads are separate from uncached input."""
from collections import Counter
import json
from pathlib import Path
import re

from economics import call, combine

FIELDS = ("input", "cache_write", "cache_read", "output")


def records(path):
    if not Path(path).is_file():
        raise RuntimeError(f"Missing native transcript: {path}")
    rows = []
    for line in Path(path).read_text().splitlines():
        try:
            rows.append(json.loads(line))
        except ValueError:
            # A provider may still be appending the last line during a poll.
            continue
    return rows


def cost(usage, provider):
    return (usage["input"] + (1.25 if provider == "claude" else 1) * usage["cache_write"]
            + .1 * usage["cache_read"] + (5 if provider == "claude" else 8) * usage["output"]) / 1000


def quoted(key, source):
    """The strings given to `key` in JavaScript source, each up to its own closing quote."""
    found = re.findall(rf"{key}\s*:\s*(?:\"((?:\\.|[^\"\\])*)\"|'((?:\\.|[^'\\])*)'|`((?:\\.|[^`\\])*)`)", source, re.S)
    return ["".join(groups) for groups in found]


def parse(path, provider, prompt):
    rows = records(path)
    usage = Counter({k: 0 for k in FIELDS})
    tools, injections, models = [], [], set()
    calls = 0
    priced = []
    if provider == "claude":
        best = {}
        for row in rows:
            msg = row.get("message") or {}
            if row.get("type") == "assistant":
                u = msg.get("usage")
                if u and msg.get("id"):
                    old = best.get(msg["id"], {})
                    if u.get("output_tokens", 0) >= old.get("output_tokens", 0):
                        best[msg["id"]] = {**u, "native_model": msg.get("model") or old.get("native_model")}
                    if msg.get("model"):
                        models.add(msg["model"])
                for item in msg.get("content", []):
                    if isinstance(item, dict) and item.get("type") == "tool_use":
                        tools.append((item.get("id"), item.get("input", {})))
            elif row.get("type") == "user":
                content = msg.get("content", [])
                texts = [content] if isinstance(content, str) else [i.get("text", "") for i in content if i.get("type") == "text"]
                injections.extend(t.replace(prompt, "", 1) for t in texts if t.replace(prompt, "", 1).strip())
        calls = len(best)
        for u in best.values():
            usage.update(input=u.get("input_tokens", 0), cache_write=u.get("cache_creation_input_tokens", 0),
                         cache_read=u.get("cache_read_input_tokens", 0), output=u.get("output_tokens", 0))
            normalized = {"input": u.get("input_tokens", 0), "cache_write": u.get("cache_creation_input_tokens", 0),
                          "cache_read": u.get("cache_read_input_tokens", 0), "output": u.get("output_tokens", 0)}
            creation = u.get("cache_creation") or {}
            priced.append(call(normalized, u.get("native_model"), creation.get("ephemeral_1h_input_tokens", 0),
                               context=sum(normalized[k] for k in FIELDS[:3]),
                               notes=["Cache TTL absent; short-write estimate"] if normalized["cache_write"] and not creation else []))
    else:
        last, seen = None, set()
        model = None
        for row in rows:
            p = row.get("payload") or {}
            if row.get("type") == "turn_context" and p.get("model"):
                models.add(p["model"])
                model = p["model"]
            if p.get("type") == "token_count" and p.get("info"):
                current = p["info"].get("total_token_usage")
                if current and current != last:
                    calls += 1
                    delta = {k: current.get(k, 0) - (last or {}).get(k, 0)
                             for k in ("input_tokens", "cached_input_tokens", "output_tokens")}
                    normalized = {"input": delta["input_tokens"] - delta["cached_input_tokens"],
                                  "cache_read": delta["cached_input_tokens"], "cache_write": 0,
                                  "output": delta["output_tokens"]}
                    context = (p["info"].get("last_token_usage") or {}).get("input_tokens")
                    if context is None:
                        context = delta["input_tokens"]
                    priced.append(call(normalized, model, context=context))
                    last = current
            if row.get("type") == "response_item":
                if p.get("type") in ("function_call", "custom_tool_call"):
                    tools.append((p.get("call_id"), p.get("arguments", p.get("input", ""))))
                if p.get("type") == "message" and p.get("role") in ("user", "developer"):
                    for item in p.get("content", []):
                        t = item.get("text", "").replace(prompt, "", 1)
                        if t.strip() and t not in seen:
                            injections.append(t)
                            seen.add(t)
        if last:
            usage.update(input=last.get("input_tokens", 0) - last.get("cached_input_tokens", 0),
                         cache_read=last.get("cached_input_tokens", 0), output=last.get("output_tokens", 0))
    if not calls:
        raise RuntimeError(f"No native usage records in {path}")
    unique = {key or str(i): value for i, (key, value) in enumerate(tools)}
    # Inspect only executed shell command fields, never prose describing a command.
    commands = []
    for value in unique.values():
        if isinstance(value, str):
            try:
                value = json.loads(value)
            except ValueError:
                # Codex code-mode tools carry shell calls inside JavaScript source.
                commands.extend(quoted(r"(?:cmd|command)", value))
                continue
        if isinstance(value, dict):
            if isinstance(value.get("code"), str):
                commands.extend(quoted(r"[\"']?(?:cmd|command)[\"']?", value["code"]))
            commands += [value[k] for k in ("cmd", "command") if isinstance(value.get(k), str)]
    text = "\n".join(commands)
    return {"usage": dict(usage), "raw_tokens": sum(usage.values()), "cost_units": cost(usage, provider),
            "economics": combine(priced), "api_calls": calls, "tools": len(unique), "models": sorted(models),
            "brigadier_commands": len(re.findall(r'\bbrigadier\s+(?:--\S+\s+)*[a-z]+', text)),
            "ledger_adds": len(re.findall(r'\bbrigadier\s+ledger\s+add\b', text)),
            "injected_messages": len(injections), "injected_bytes": sum(len(t.encode()) for t in injections)}
