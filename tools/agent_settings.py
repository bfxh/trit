#!/usr/bin/env python3
"""User/project settings for the agent workbench.

Precedence is intentionally boring and auditable:

    built-in defaults  <  user settings  <  project .agent/settings.json
                       <  explicit --settings file  <  explicit CLI flags

The default profile is ``deepseek-strict``: full audit evidence stays on disk,
the model reads a compact view, and the security gate never silently drops a
high/critical finding.  Different users can still adjust receipt shape, scanner
severity, LSP install policy, and timeouts in their own settings file; every
effective setting is echoed into the receipt so an auditor can see what was
actually used.

What this module is not: it does not choose a model, spend tokens, or weaken a
security decision.  It only resolves configuration and refuses malformed input.
"""

from __future__ import annotations

import json
import os
import sys
from copy import deepcopy

SCHEMA = "agent-settings/1"
DEFAULT_NAME = "deepseek-strict"

# Keep the default deliberately small.  A setting that is not here is either a
# caller concern (paths/flags) or belongs to a scanner-specific config file.
DEFAULTS = {
    "version": 1,
    "name": DEFAULT_NAME,
    "policy_profile": "workspace-write",
    "receipt": {
        "audit": "strict",
        "compact": False,
        "brief": False,
        "max_output_lines": 6,
        "model_view": "compact",
    },
    "audit": {
        "once": True,
        "require_full": True,
        "require_scan": False,
        "detect_tamper": True,
        "fail_on": ["critical", "high"],
    },
    "security": {
        "enabled": False,
        "mode": "quick",
        "fail_on": ["critical", "high"],
        "timeout_s": 900,
        "allow_gaps": False,
        "scanners": {
            "secrets": True,
            "privacy": True,
            "deps": True,
            "lint": True,
            "sast": True,
            "dynamic": False,
            "runtime": False,
        },
        "commands": [],
    },
    "lsp": {
        "enabled": True,
        "mode": "on-demand",
        "auto_install": False,
        "max_probe_s": 20,
    },
    # Model context envelope.  These are planning budgets, not a vendor
    # tokenizer's exact limit; the planner refuses rather than silently cutting.
    "context": {
        "window_tokens": 1_000_000,
        "auto_compact_at": 850_000,
        "target_tokens": 700_000,
        "recent_keep": 6,
        "preserve_tool_evidence": True,
    },
    # Search selection: relevance/safety/authority/freshness/diversity under a
    # token budget.  Thresholds are deliberately conservative and test-pinned.
    "search": {
        "enabled": True,
        "max_results": 10,
        "budget_tokens": 12_000,
        "min_relevance": 0.15,
        "max_result_tokens": 2_000,
        "dedupe": True,
        "freshness_half_life_days": 30,
        "trusted_domains": [],
        "safety_block": ["critical", "high"],
    },
    # Reasoning effort is selected deterministically from query/result signals
    # unless the user pins it with mode=fixed.
    "reasoning": {
        "mode": "auto",
        "effort": "medium",
        "min_effort": "low",
        "max_effort": "high",
    },
    # Goal/requirement/artifact/evidence alignment.  The gate runs when a
    # project manifest exists (or --alignment names one).  enabled=False opts
    # out of auto-detection; enabled=True does not synthesize a manifest, so a
    # project without one is unchanged.  CI should pass --alignment to require it.
    "alignment": {
        "enabled": True,
        "manifest": ".agent/alignment.json",
        "strict": True,
    },
    # Repeated-command stability probe.  Same opt-in shape as alignment: the
    # gate runs only when a manifest exists (or --stability names one); a
    # project without one pays nothing.  timeout_s is the outer watchdog for
    # the whole probe, not a per-command timeout (the manifest owns those).
    "stability": {
        "enabled": True,
        "manifest": ".agent/stability.json",
        "timeout_s": 1800,
    },
}

TOP_LEVEL = set(DEFAULTS)
USER_DIR = os.path.join("trit", "agent")


def _config_home(env=None):
    env = os.environ if env is None else env
    if os.name == "nt":
        base = env.get("APPDATA") or env.get("LOCALAPPDATA") or os.path.expanduser("~")
    else:
        base = env.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
    return base


def user_config_path(env=None):
    return os.path.join(_config_home(env), USER_DIR, "settings.json")


def project_config_path(root):
    return os.path.join(os.path.abspath(root), ".agent", "settings.json")


def deep_merge(base, override):
    """Dicts merge recursively; every other JSON type replaces the old value.

    ``null`` removes the key so a project can explicitly clear a user default
    without having to know the built-in value.  Arrays replace (not append):
    scanner lists and fail_on sets must be readable as "this is the effective
    list", not "the union of every layer".
    """
    out = deepcopy(base)
    for key, value in (override or {}).items():
        if value is None:
            out.pop(key, None)
        elif isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = deep_merge(out[key], value)
        else:
            out[key] = deepcopy(value)
    return out


def _read_json(path):
    with open(path, "r", encoding="utf-8-sig") as fh:
        doc = json.load(fh)
    if not isinstance(doc, dict):
        raise ValueError(f"{path}: settings must be a JSON object")
    return doc


def validate(settings, source="settings"):
    """Reject shapes that would make the receipt or a gate ambiguous.

    Unknown top-level keys are rejected: a typo like ``receipts`` silently
    doing nothing is worse than a loud failure.  Scanner-specific settings are
    intentionally free-form under ``security.commands`` because users bring
    their own tools.
    """
    if not isinstance(settings, dict):
        raise ValueError(f"{source}: not a JSON object")
    version = settings.get("version")
    if version != 1:
        raise ValueError(f"{source}: version must be 1 (got {version!r})")
    unknown = sorted(set(settings) - TOP_LEVEL)
    if unknown:
        raise ValueError(f"{source}: unknown top-level keys: {unknown}")
    for key in ("receipt", "audit", "security", "lsp", "context", "search", "reasoning",
                "alignment", "stability"):
        if key in settings and not isinstance(settings[key], dict):
            raise ValueError(f"{source}: {key} must be an object")
    for key in ("name", "policy_profile"):
        if key in settings and not isinstance(settings[key], str):
            raise ValueError(f"{source}: {key} must be a string")
    # Every nested field is optional: a user may set only ``receipt.brief`` and
    # rely on the built-in defaults for everything else.  Validate what is
    # present; ``load_settings`` validates the merged result at the end.
    receipt = settings.get("receipt", {})
    if "compact" in receipt and not isinstance(receipt["compact"], bool):
        raise ValueError(f"{source}: receipt.compact must be boolean")
    if "brief" in receipt and not isinstance(receipt["brief"], bool):
        raise ValueError(f"{source}: receipt.brief must be boolean")
    if "model_view" in receipt and receipt["model_view"] not in ("compact", "full"):
        raise ValueError(f"{source}: receipt.model_view must be compact or full")
    audit = settings.get("audit", {})
    for key in ("once", "require_full", "require_scan", "detect_tamper"):
        if key in audit and not isinstance(audit[key], bool):
            raise ValueError(f"{source}: audit.{key} must be boolean")
    if "fail_on" in audit:
        fail_on = audit["fail_on"]
        if not isinstance(fail_on, list) or any(x not in ("critical", "high", "medium", "low")
                                               for x in fail_on):
            raise ValueError(f"{source}: audit.fail_on must be a list of critical/high/medium/low")
    sec = settings.get("security", {})
    if "enabled" in sec and not isinstance(sec["enabled"], bool):
        raise ValueError(f"{source}: security.enabled must be boolean")
    if "mode" in sec and sec["mode"] not in ("quick", "full", "static", "dynamic", "runtime", "all"):
        raise ValueError(f"{source}: security.mode is not a known mode")
    if "timeout_s" in sec and (not isinstance(sec["timeout_s"], (int, float)) or sec["timeout_s"] <= 0):
        raise ValueError(f"{source}: security.timeout_s must be > 0")
    if "allow_gaps" in sec and not isinstance(sec["allow_gaps"], bool):
        raise ValueError(f"{source}: security.allow_gaps must be boolean")
    if "scanners" in sec and not isinstance(sec["scanners"], dict):
        raise ValueError(f"{source}: security.scanners must be an object")
    if "commands" in sec and not isinstance(sec["commands"], list):
        raise ValueError(f"{source}: security.commands must be a list")
    for index, cmd in enumerate(sec.get("commands", [])):
        if not isinstance(cmd, dict) or not isinstance(cmd.get("command"), list) \
                or not cmd["command"] or not all(isinstance(x, str) for x in cmd["command"]):
            raise ValueError(f"{source}: security.commands[{index}].command must be a non-empty string list")
        if cmd.get("category") not in ("static", "dynamic", "runtime", "deps", "lint"):
            raise ValueError(f"{source}: security.commands[{index}].category is invalid")
    lsp = settings.get("lsp", {})
    if "enabled" in lsp and not isinstance(lsp["enabled"], bool):
        raise ValueError(f"{source}: lsp.enabled must be boolean")
    if "mode" in lsp and lsp["mode"] not in ("on-demand", "detect-only", "disabled"):
        raise ValueError(f"{source}: lsp.mode is invalid")
    if "auto_install" in lsp and not isinstance(lsp["auto_install"], bool):
        raise ValueError(f"{source}: lsp.auto_install must be boolean")
    ctx = settings.get("context", {})
    for key in ("window_tokens", "auto_compact_at", "target_tokens", "recent_keep"):
        if key in ctx and (isinstance(ctx[key], bool) or not isinstance(ctx[key], int)
                           or ctx[key] < 0):
            raise ValueError(f"{source}: context.{key} must be a non-negative integer")
    for key in ("preserve_tool_evidence",):
        if key in ctx and not isinstance(ctx[key], bool):
            raise ValueError(f"{source}: context.{key} must be boolean")
    if "window_tokens" in ctx and ctx["window_tokens"] <= 0:
        raise ValueError(f"{source}: context.window_tokens must be > 0")
    if "auto_compact_at" in ctx and ctx["auto_compact_at"] <= 0:
        raise ValueError(f"{source}: context.auto_compact_at must be > 0")
    if "target_tokens" in ctx and ctx["target_tokens"] <= 0:
        raise ValueError(f"{source}: context.target_tokens must be > 0")
    if "auto_compact_at" in ctx and "window_tokens" in ctx \
            and ctx["auto_compact_at"] > ctx["window_tokens"]:
        raise ValueError(f"{source}: context.auto_compact_at must be <= window_tokens")
    if "target_tokens" in ctx and "auto_compact_at" in ctx \
            and ctx["target_tokens"] > ctx["auto_compact_at"]:
        raise ValueError(f"{source}: context.target_tokens must be <= auto_compact_at")
    srch = settings.get("search", {})
    for key in ("max_results", "budget_tokens", "max_result_tokens", "freshness_half_life_days"):
        if key in srch and (isinstance(srch[key], bool) or not isinstance(srch[key], int)
                            or srch[key] <= 0):
            raise ValueError(f"{source}: search.{key} must be a positive integer")
    if "min_relevance" in srch:
        value = srch["min_relevance"]
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not 0 <= value <= 1:
            raise ValueError(f"{source}: search.min_relevance must be in [0, 1]")
    if "dedupe" in srch and not isinstance(srch["dedupe"], bool):
        raise ValueError(f"{source}: search.dedupe must be boolean")
    if "trusted_domains" in srch and (not isinstance(srch["trusted_domains"], list)
                                      or not all(isinstance(x, str) for x in srch["trusted_domains"])):
        raise ValueError(f"{source}: search.trusted_domains must be a string list")
    if "safety_block" in srch and (not isinstance(srch["safety_block"], list)
                                   or any(x not in ("critical", "high", "medium", "low")
                                          for x in srch["safety_block"])):
        raise ValueError(f"{source}: search.safety_block is invalid")
    reason = settings.get("reasoning", {})
    if "mode" in reason and reason["mode"] not in ("auto", "fixed"):
        raise ValueError(f"{source}: reasoning.mode must be auto or fixed")
    for key in ("effort", "min_effort", "max_effort"):
        if key in reason and reason[key] not in ("low", "medium", "high"):
            raise ValueError(f"{source}: reasoning.{key} must be low/medium/high")
    if "min_effort" in reason and "max_effort" in reason:
        order = {"low": 0, "medium": 1, "high": 2}
        if order[reason["min_effort"]] > order[reason["max_effort"]]:
            raise ValueError(f"{source}: reasoning.min_effort > max_effort")
    align = settings.get("alignment", {})
    if "enabled" in align and not isinstance(align["enabled"], bool):
        raise ValueError(f"{source}: alignment.enabled must be boolean")
    if "strict" in align and not isinstance(align["strict"], bool):
        raise ValueError(f"{source}: alignment.strict must be boolean")
    if "manifest" in align:
        manifest = align["manifest"]
        if not isinstance(manifest, str) or not manifest.strip():
            raise ValueError(f"{source}: alignment.manifest must be a non-empty string")
        raw = manifest.replace("\\", "/")
        norm = os.path.normpath(raw).replace("\\", "/")
        if (os.path.isabs(manifest) or raw.startswith("../") or raw == ".."
                or ".." in norm.split("/") or (len(manifest) > 1 and manifest[1] == ":")):
            raise ValueError(f"{source}: alignment.manifest must stay inside the project root")
    stab = settings.get("stability", {})
    if "enabled" in stab and not isinstance(stab["enabled"], bool):
        raise ValueError(f"{source}: stability.enabled must be boolean")
    if "timeout_s" in stab:
        timeout_s = stab["timeout_s"]
        if (isinstance(timeout_s, bool) or not isinstance(timeout_s, (int, float))
                or timeout_s <= 0):
            raise ValueError(f"{source}: stability.timeout_s must be > 0")
    if "manifest" in stab:
        manifest = stab["manifest"]
        if not isinstance(manifest, str) or not manifest.strip():
            raise ValueError(f"{source}: stability.manifest must be a non-empty string")
        raw = manifest.replace("\\", "/")
        norm = os.path.normpath(raw).replace("\\", "/")
        if (os.path.isabs(manifest) or raw.startswith("../") or raw == ".."
                or ".." in norm.split("/") or (len(manifest) > 1 and manifest[1] == ":")):
            raise ValueError(f"{source}: stability.manifest must stay inside the project root")
    return True


def load_settings(root, explicit="", env=None):
    """Resolve effective settings and where each layer came from.

    Returns ``(settings, meta)``.  ``meta`` never contains a machine absolute
    path: receipts travel with the repo, and a user config path is private.
    """
    effective = deepcopy(DEFAULTS)
    sources = ["builtin"]
    if root:
        p = project_config_path(root)
        if os.path.isfile(p):
            doc = _read_json(p)
            validate(doc, ".agent/settings.json")
            effective = deep_merge(effective, doc)
            sources.append("project")
    # User settings come before project so project wins; load them first for
    # source order, then re-apply project only when it actually exists.
    user_path = user_config_path(env)
    if os.path.isfile(user_path):
        doc = _read_json(user_path)
        validate(doc, "user settings")
        effective = deep_merge(deepcopy(DEFAULTS), doc)
        # Project overrides user.
        if root and os.path.isfile(project_config_path(root)):
            effective = deep_merge(effective, _read_json(project_config_path(root)))
        sources = ["builtin", "user"] + (["project"] if "project" in sources else [])
    if explicit:
        if not os.path.isfile(explicit):
            raise ValueError(f"--settings file does not exist: {explicit}")
        doc = _read_json(explicit)
        validate(doc, "--settings")
        effective = deep_merge(effective, doc)
        sources.append("explicit")
    validate(effective, "effective")
    meta = {"name": effective.get("name", DEFAULT_NAME),
            "source": sources[-1],
            "sources": sources,
            "policy_profile": effective.get("policy_profile", "workspace-write")}
    return effective, meta


def receipt_echo(settings, meta):
    """The small, path-free settings block that travels in a receipt."""
    return {
        "name": meta.get("name", DEFAULT_NAME),
        "source": meta.get("source", "builtin"),
        "sources": list(meta.get("sources", ["builtin"])),
        "policy_profile": meta.get("policy_profile", "workspace-write"),
        "audit": settings.get("audit", {}).get("once", True),
        "require_full": settings.get("audit", {}).get("require_full", True),
        "security": {
            "enabled": settings.get("security", {}).get("enabled", False),
            "mode": settings.get("security", {}).get("mode", "quick"),
            "fail_on": list(settings.get("security", {}).get("fail_on", [])),
        },
        "lsp": {
            "mode": settings.get("lsp", {}).get("mode", "on-demand"),
            "auto_install": settings.get("lsp", {}).get("auto_install", False),
        },
        "context": {
            "window_tokens": settings.get("context", {}).get("window_tokens", 1_000_000),
            "auto_compact_at": settings.get("context", {}).get("auto_compact_at", 850_000),
            "target_tokens": settings.get("context", {}).get("target_tokens", 700_000),
        },
        "search": {
            "max_results": settings.get("search", {}).get("max_results", 10),
            "budget_tokens": settings.get("search", {}).get("budget_tokens", 12_000),
            "min_relevance": settings.get("search", {}).get("min_relevance", 0.15),
        },
        "reasoning": {
            "mode": settings.get("reasoning", {}).get("mode", "auto"),
            "effort": settings.get("reasoning", {}).get("effort", "medium"),
        },
        "alignment": {
            "enabled": settings.get("alignment", {}).get("enabled", True),
            "strict": settings.get("alignment", {}).get("strict", True),
        },
        "stability": {
            "enabled": settings.get("stability", {}).get("enabled", True),
        },
    }


def _self_test():
    import tempfile

    checks = 0

    def ok(cond, msg):
        nonlocal checks
        checks += 1
        if not cond:
            raise AssertionError(msg)

    with tempfile.TemporaryDirectory() as td:
        user_home = os.path.join(td, "user")
        os.makedirs(os.path.join(user_home, USER_DIR))
        user_path = os.path.join(user_home, USER_DIR, "settings.json")
        with open(user_path, "w", encoding="utf-8") as fh:
            json.dump({"version": 1, "receipt": {"brief": True}, "security": {"mode": "full"}}, fh)
        root = os.path.join(td, "repo")
        os.makedirs(os.path.join(root, ".agent"))
        project_path = os.path.join(root, ".agent", "settings.json")
        with open(project_path, "w", encoding="utf-8") as fh:
            json.dump({"version": 1, "receipt": {"compact": True}, "security": {"mode": "quick"}}, fh)

        env = {"APPDATA": user_home} if os.name == "nt" else {"XDG_CONFIG_HOME": user_home}
        settings, meta = load_settings(root, env=env)
        ok(settings["name"] == DEFAULT_NAME, "default name")
        ok(meta["source"] == "project", "project is the last source")
        ok(settings["receipt"]["brief"] is True, "user brief survives project merge")
        ok(settings["receipt"]["compact"] is True, "project compact wins")
        ok(settings["security"]["mode"] == "quick", "project security wins")
        ok(meta["policy_profile"] == "workspace-write", "policy profile echo")
        echo = receipt_echo(settings, meta)
        ok("C:" not in json.dumps(echo) and "/tmp" not in json.dumps(echo), "echo has no absolute path")

        # Explicit file wins over project.
        explicit = os.path.join(td, "explicit.json")
        with open(explicit, "w", encoding="utf-8") as fh:
            json.dump({"version": 1, "receipt": {"compact": False}, "security": {"mode": "all"}}, fh)
        settings2, meta2 = load_settings(root, explicit=explicit, env=env)
        ok(settings2["receipt"]["compact"] is False, "explicit beats project")
        ok(settings2["security"]["mode"] == "all", "explicit security mode")
        ok(meta2["source"] == "explicit", "explicit source")

        # Null deletes a key; arrays replace.
        settings3 = deep_merge({"a": {"b": 1, "c": 2}, "x": [1, 2]}, {"a": {"b": None}, "x": [3]})
        ok(settings3 == {"a": {"c": 2}, "x": [3]}, "null deletes and arrays replace")

        # Typo must be loud, not ignored.
        bad = os.path.join(td, "bad.json")
        with open(bad, "w", encoding="utf-8") as fh:
            json.dump({"version": 1, "receipts": {}}, fh)
        try:
            load_settings(root, explicit=bad, env=env)
            ok(False, "unknown key must raise")
        except ValueError as e:
            ok("unknown top-level" in str(e), "unknown key message")

        # Bad version.
        with open(bad, "w", encoding="utf-8") as fh:
            json.dump({"version": 2}, fh)
        try:
            load_settings(root, explicit=bad, env=env)
            ok(False, "version 2 must raise")
        except ValueError as e:
            ok("version" in str(e), "version message")

        # Malformed scanner command.
        with open(bad, "w", encoding="utf-8") as fh:
            json.dump({"version": 1, "security": {"commands": [{"category": "static", "command": "echo hi"}]}}, fh)
        try:
            load_settings(root, explicit=bad, env=env)
            ok(False, "string command must raise")
        except ValueError as e:
            ok("commands[0].command" in str(e), "scanner command message")

        # No configs at all: defaults must be valid on their own.
        empty = os.path.join(td, "empty")
        os.makedirs(empty)
        settings4, meta4 = load_settings(empty, env={"APPDATA": os.path.join(td, "nouser")} if os.name == "nt"
                                         else {"XDG_CONFIG_HOME": os.path.join(td, "nouser")})
        validate(settings4)
        ok(meta4["source"] == "builtin", "no config -> builtin")
        ok(settings4["audit"]["require_full"] is True, "default keeps full audit")
        ok(settings4["context"]["window_tokens"] == 1_000_000
           and settings4["context"]["auto_compact_at"] == 850_000
           and settings4["context"]["target_tokens"] == 700_000,
           "default context envelope is 1M/850K/700K")
        echo4 = receipt_echo(settings4, meta4)
        ok(echo4["context"]["auto_compact_at"] == 850_000
           and echo4["search"]["budget_tokens"] == 12_000
           and echo4["reasoning"]["mode"] == "auto",
           "receipt echoes context/search/reasoning")
        ok(settings4["alignment"]["enabled"] is True
           and settings4["alignment"]["strict"] is True
           and settings4["alignment"]["manifest"] == ".agent/alignment.json",
           "default alignment gate is on when a manifest exists")
        ok(echo4["alignment"] == {"enabled": True, "strict": True},
           "receipt echoes alignment toggle")
        ok(settings4["stability"]["enabled"] is True
           and settings4["stability"]["manifest"] == ".agent/stability.json"
           and settings4["stability"]["timeout_s"] == 1800,
           "default stability gate is on when a manifest exists")
        ok(echo4["stability"] == {"enabled": True},
           "receipt echoes stability toggle")
        ok("C:" not in json.dumps(echo4) and "/tmp" not in json.dumps(echo4),
           "new settings echo has no absolute path")

        # Bad context/search/reasoning/alignment shapes must be loud.
        for label, doc in (
                ("target over trigger",
                 {"version": 1, "context": {"auto_compact_at": 100, "target_tokens": 101}}),
                ("zero context window",
                 {"version": 1, "context": {"window_tokens": 0}}),
                ("bad search budget",
                 {"version": 1, "search": {"budget_tokens": 0}}),
                ("bad reasoning effort",
                 {"version": 1, "reasoning": {"effort": "max"}}),
                ("bad alignment enabled",
                 {"version": 1, "alignment": {"enabled": "yes"}}),
                ("bad alignment strict",
                 {"version": 1, "alignment": {"strict": 1}}),
                ("alignment manifest escapes",
                 {"version": 1, "alignment": {"manifest": "../outside.json"}}),
                ("alignment manifest middle escape",
                 {"version": 1, "alignment": {"manifest": "cfg/../../outside.json"}}),
                ("alignment manifest drive path",
                 {"version": 1, "alignment": {"manifest": "C:/outside.json"}}),
                ("bad stability enabled",
                 {"version": 1, "stability": {"enabled": "yes"}}),
                ("bad stability timeout",
                 {"version": 1, "stability": {"timeout_s": 0}}),
                ("stability manifest escapes",
                 {"version": 1, "stability": {"manifest": "../outside.json"}}),
                ("stability manifest middle escape",
                 {"version": 1, "stability": {"manifest": "cfg/../../outside.json"}}),
                ("stability manifest drive path",
                 {"version": 1, "stability": {"manifest": "C:/outside.json"}})):
            with open(bad, "w", encoding="utf-8") as fh:
                json.dump(doc, fh)
            try:
                load_settings(root, explicit=bad, env=env)
                ok(False, label)
            except ValueError:
                ok(True, label)

    print(f"agent-settings self-test: {checks}/{checks}")
    return 0


def main(argv):
    if "--self-test" in argv:
        return _self_test()
    if "--help" in argv or "-h" in argv:
        print("agent-settings: show|validate|init [--root DIR] [--settings FILE] [--json]")
        return 0
    if not argv:
        print("agent-settings: need a mode (show|validate|init)")
        return 2
    mode = argv[0]
    root = ""
    explicit = ""
    json_out = "--json" in argv
    i = 1
    while i < len(argv):
        if argv[i] in ("--root", "-Root"):
            root = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif argv[i] in ("--settings", "-Settings"):
            explicit = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        else:
            i += 1
    try:
        if mode == "init":
            if not root:
                print("agent-settings: init needs --root")
                return 2
            path = project_config_path(root)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            if os.path.exists(path):
                print(f"agent-settings: refusing to overwrite {path}")
                return 1
            with open(path, "w", encoding="utf-8", newline="\n") as fh:
                json.dump(DEFAULTS, fh, ensure_ascii=False, indent=2)
                fh.write("\n")
            print(f"agent-settings: wrote {path}")
            return 0
        settings, meta = load_settings(root, explicit=explicit)
        if mode == "validate":
            validate(settings, "effective")
            print(f"agent-settings: ok ({meta['source']}, {meta['name']})")
            return 0
        if mode == "show":
            if json_out:
                print(json.dumps({"settings": settings, "meta": meta}, ensure_ascii=False,
                                 sort_keys=True))
            else:
                print(f"name: {meta['name']}\nsource: {meta['source']}\n"
                      f"policy_profile: {meta['policy_profile']}\n"
                      f"user: {user_config_path()}\nproject: {project_config_path(root) if root else '(no root)'}")
            return 0
        print(f"agent-settings: unknown mode {mode}")
        return 2
    except (OSError, ValueError) as e:
        print(f"agent-settings: {e}")
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
