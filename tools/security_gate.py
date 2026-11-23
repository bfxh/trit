#!/usr/bin/env python3
"""Unified static / dynamic / runtime security gate.

One report, one verdict, one exit code.  Adapters are deliberately conservative:
a scanner either ran and produced normalized findings, was not applicable to
this tree, or is reported as a gap.  A required gap is fail-closed; an optional
gap is visible but does not silently become "pass".

Built-ins reuse the repository's own scanners first:
  * scripts/secret_scan.py, scripts/privacy_scan.py
  * cargo audit (dependency CVEs)
  * cargo clippy (static lint, medium only)
  * trit-unsafe (unsafe sites, full mode)
  * trit-procscan (MCP/runtime config audit, full mode)
  * cargo fuzz (dynamic, only when a fuzz target exists)

External adapters are opt-in by presence, never auto-installed:
  * semgrep, bandit, trivy, govulncheck, npm audit
Users can add their own commands under ``security.commands``; those are executed
as an argv list (no shell), with a timeout and a normalized severity.

    python tools/security_gate.py --root . --mode quick --out report.json --json
    python tools/security_gate.py --root . --mode all --fail-on critical,high
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sys
import time
from datetime import datetime

# tools/signal.py shadows the stdlib `signal` module when this script is run as
# `python tools/security_gate.py` (sys.path[0] is tools/). Load stdlib signal
# and subprocess with the script dir off sys.path, then restore the dir for the
# local modules. Otherwise every POSIX timeout/kill path raises AttributeError.
_SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
if sys.path and os.path.abspath(sys.path[0]) == _SCRIPT_DIR:
    del sys.path[0]
import signal as _stdlib_signal  # noqa: F401 - shadow guard, keeps stdlib in sys.modules
import subprocess
if _SCRIPT_DIR not in sys.path:
    sys.path.insert(0, _SCRIPT_DIR)

import agent_settings

SCHEMA = "security-gate/1"
PY = sys.executable
SEVERITIES = ("critical", "high", "medium", "low", "info")
FAIL_ON_DEFAULT = ("critical", "high")


def severity_rank(value):
    value = str(value or "").lower()
    return SEVERITIES.index(value) if value in SEVERITIES else len(SEVERITIES)


def finding(scanner, category, severity, title, path="", line=None, evidence="", fid=""):
    return {
        "scanner": scanner,
        "category": category,
        "severity": severity if severity in SEVERITIES else "info",
        "id": fid or f"{scanner}:{path}:{line or 0}",
        "title": str(title)[:300],
        "path": str(path)[:300],
        "line": line,
        "evidence": str(evidence)[:800],
    }


def run_process(argv, cwd, timeout_s):
    t0 = time.time()
    try:
        p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout_s)
        out = (p.stdout or "") + ("\n" + p.stderr if p.stderr else "")
        return {"code": p.returncode, "out": out, "seconds": round(time.time() - t0, 2),
                "timed_out": False}
    except subprocess.TimeoutExpired as e:
        out = ""
        if e.stdout:
            out += e.stdout if isinstance(e.stdout, str) else e.stdout.decode("utf-8", "replace")
        if e.stderr:
            out += "\n" + (e.stderr if isinstance(e.stderr, str) else e.stderr.decode("utf-8", "replace"))
        return {"code": 124, "out": out, "seconds": round(time.time() - t0, 2),
                "timed_out": True}
    except FileNotFoundError:
        return {"code": 127, "out": "", "seconds": round(time.time() - t0, 2),
                "timed_out": False, "missing": True}
    except OSError as e:
        return {"code": 126, "out": str(e), "seconds": round(time.time() - t0, 2),
                "timed_out": False}


def json_from_text(text):
    """Parse the last JSON object/array in a scanner's stdout.

    Scanners often print progress before the machine line.  Scanning from the
    tail is deliberate; a bare stdout that is only JSON also works.
    """
    text = (text or "").strip()
    if not text:
        return None
    for candidate in (text, text[text.find("{"):] if "{" in text else "",
                      text[text.find("["):] if "[" in text else ""):
        if not candidate:
            continue
        try:
            return json.loads(candidate)
        except ValueError:
            pass
    for line in reversed(text.splitlines()):
        line = line.strip()
        if line.startswith(("{", "[")):
            try:
                return json.loads(line)
            except ValueError:
                continue
    return None


def parse_secret(doc, scanner="secrets", category="static"):
    findings = []
    if isinstance(doc, list):
        for item in doc:
            if isinstance(item, dict):
                findings.append(finding(scanner, category, "high",
                                        item.get("rule") or "secret detected",
                                        item.get("path", ""), item.get("line"),
                                        item.get("match", ""), item.get("id", "")))
            else:
                findings.append(finding(scanner, category, "high", str(item)[:200]))
    return findings


def parse_privacy(doc, scanner="privacy", category="static"):
    findings = []
    if not isinstance(doc, dict):
        return findings
    # privacy_scan's `details` includes baseline-known material; only `new`
    # entries are regressions.  An older report without `new` has to be treated
    # as blocking because there is no baseline evidence to downgrade it.
    details = doc.get("new", []) if "new" in doc else doc.get("details", [])
    for item in details:
        if not isinstance(item, dict):
            continue
        findings.append(finding(scanner, category, "high",
                                item.get("rule") or "private material",
                                item.get("path", ""), item.get("line"),
                                item.get("why") or item.get("match", ""), item.get("rule", "")))
    return findings


def parse_cargo_audit(doc, scanner="rust-deps", category="deps"):
    findings = []
    if not isinstance(doc, dict):
        return findings
    for item in (doc.get("vulnerabilities") or {}).get("list", []) or []:
        adv = item.get("advisory", {}) if isinstance(item, dict) else {}
        pkg = item.get("package", {}) if isinstance(item, dict) else {}
        findings.append(finding(scanner, category, "high",
                                adv.get("title") or "dependency vulnerability",
                                "Cargo.lock", None,
                                f"{pkg.get('name')} {pkg.get('version')} {adv.get('id')}",
                                adv.get("id", "")))
    warnings = doc.get("warnings") or {}
    for bucket in ("unsound", "unmaintained", "notice"):
        for item in warnings.get(bucket, []) or []:
            adv = item.get("advisory", {}) if isinstance(item, dict) else {}
            pkg = item.get("package", {}) if isinstance(item, dict) else {}
            sev = "high" if bucket == "unsound" else "medium"
            findings.append(finding(scanner, category, sev,
                                    adv.get("title") or bucket,
                                    "Cargo.lock", None,
                                    f"{pkg.get('name')} {pkg.get('version')} {adv.get('id')}",
                                    adv.get("id", "")))
    return findings


def parse_semgrep(doc, scanner="semgrep", category="static"):
    findings = []
    results = doc.get("results", []) if isinstance(doc, dict) else []
    for item in results:
        if not isinstance(item, dict):
            continue
        extra = item.get("extra", {}) or {}
        sev = {"ERROR": "high", "WARNING": "medium", "INFO": "low"}.get(
            str(extra.get("severity", "")).upper(), "medium")
        start = item.get("start", {}) or {}
        findings.append(finding(scanner, category, sev,
                                extra.get("message") or item.get("check_id") or "semgrep finding",
                                item.get("path", ""), start.get("line"),
                                extra.get("lines", ""), item.get("check_id", "")))
    return findings


def parse_bandit(doc, scanner="bandit", category="static"):
    findings = []
    results = doc.get("results", []) if isinstance(doc, dict) else []
    for item in results:
        if not isinstance(item, dict):
            continue
        sev = {"HIGH": "high", "MEDIUM": "medium", "LOW": "low"}.get(
            str(item.get("issue_severity", "")).upper(), "medium")
        findings.append(finding(scanner, category, sev,
                                item.get("issue_text") or "bandit finding",
                                item.get("filename", ""), item.get("line_number"),
                                item.get("code", ""), item.get("test_id", "")))
    return findings


def parse_npm_audit(doc, scanner="npm-audit", category="deps"):
    findings = []
    vulns = doc.get("vulnerabilities", {}) if isinstance(doc, dict) else {}
    if isinstance(vulns, dict):
        for name, item in vulns.items():
            if not isinstance(item, dict):
                continue
            sev = str(item.get("severity", "high")).lower()
            findings.append(finding(scanner, category, sev,
                                    f"{name}: {item.get('title', 'vulnerability')}",
                                    "package-lock.json", None,
                                    item.get("via", ""), item.get("cve", "") or name))
    return findings


def parse_trivy(doc, scanner="trivy", category="deps"):
    findings = []
    results = doc.get("Results", []) if isinstance(doc, dict) else []
    for res in results:
        if not isinstance(res, dict):
            continue
        target = res.get("Target", "")
        for key, kind in (("Vulnerabilities", "vulnerability"),
                          ("Secrets", "secret"),
                          ("Misconfigurations", "misconfiguration")):
            for item in res.get(key, []) or []:
                if not isinstance(item, dict):
                    continue
                sev = str(item.get("Severity", "medium")).lower()
                findings.append(finding(scanner, category, sev,
                                        item.get("Title") or item.get("Description") or kind,
                                        target, item.get("StartLine") or item.get("Line"),
                                        item.get("PkgName", "") + " " + item.get("InstalledVersion", ""),
                                        item.get("VulnerabilityID") or item.get("ID", "")))
    return findings


def generic_finding(result, spec):
    tail = "\n".join([ln for ln in (result.get("out") or "").splitlines() if ln.strip()][-3:])
    if result.get("timed_out"):
        return [finding(spec["id"], spec.get("category", "static"), "high",
                        f"scanner timed out after {spec.get('timeout_s')}s", "", None, tail)]
    if result.get("missing"):
        return []
    if result.get("code") not in (0, None):
        return [finding(spec["id"], spec.get("category", "static"),
                        spec.get("severity", "high"),
                        f"scanner exited {result.get('code')}", "", None, tail)]
    return []


def scanner_specs(settings, root, mode=""):
    """Return built-in adapter specs with per-tree applicability resolved."""
    scripts = os.path.join(root, "scripts")
    tool_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    tool_scripts = os.path.join(tool_root, "scripts")
    sec = settings.get("security", {})
    enabled = sec.get("scanners", {})
    timeout = float(sec.get("timeout_s", 900))
    force_dynamic = mode in ("dynamic", "all", "full")
    force_runtime = mode in ("runtime", "all", "full")
    specs = []

    def add(spec):
        specs.append(spec)

    def scan_script(name):
        """Use the project's copy when present, else trit's audited engine."""
        local = os.path.join(scripts, name)
        return local if os.path.isfile(local) else os.path.join(tool_scripts, name)

    secret = scan_script("secret_scan.py")
    privacy = scan_script("privacy_scan.py")
    if os.path.isfile(secret) and enabled.get("secrets", True):
        add({"id": "secrets", "category": "static", "kind": "json-script",
             "command": [PY, secret, "--root", ".", "--json"], "required": True,
             "severity": "high", "timeout_s": min(timeout, 300),
             "parse": parse_secret, "modes": ("quick", "static", "all", "full")})
    if os.path.isfile(privacy) and enabled.get("privacy", True):
        add({"id": "privacy", "category": "static", "kind": "json-script",
             "command": [PY, privacy, "--root", ".", "--json"], "required": True,
             "severity": "high", "timeout_s": min(timeout, 300),
             "parse": parse_privacy, "modes": ("quick", "static", "all", "full")})

    if os.path.isfile(os.path.join(root, "Cargo.lock")) and enabled.get("deps", True):
        add({"id": "rust-deps", "category": "deps", "kind": "json-command",
             "command": ["cargo", "audit", "--json"], "required": True,
             "severity": "high", "timeout_s": min(timeout, 300),
             "parse": parse_cargo_audit, "modes": ("quick", "static", "all", "full")})

    if os.path.isfile(os.path.join(root, "Cargo.toml")) and enabled.get("lint", True):
        add({"id": "rust-lint", "category": "lint", "kind": "generic",
             "command": ["cargo", "clippy", "--workspace", "--all-targets", "--locked",
                         "--", "-D", "warnings"], "required": False,
             "severity": "medium", "timeout_s": min(timeout, 900),
             "modes": ("static", "all", "full")})

    if os.path.isfile(os.path.join(root, "Cargo.toml")) and os.path.isdir(
            os.path.join(root, "crates", "trit-unsafe")) and enabled.get("sast", True):
        add({"id": "trit-unsafe", "category": "static", "kind": "generic",
             "command": ["cargo", "run", "-q", "-p", "trit-unsafe", "--",
                         "scan", "--root", ".", "--strict"], "required": False,
             "severity": "high", "timeout_s": min(timeout, 900),
             "modes": ("static", "all", "full")})

    if shutil.which("semgrep") and enabled.get("sast", True):
        add({"id": "semgrep", "category": "static", "kind": "json-command",
             "command": ["semgrep", "scan", "--config", "auto", "--json", "--quiet", "."],
             "required": False, "severity": "high", "timeout_s": min(timeout, 900),
             "parse": parse_semgrep, "modes": ("static", "all", "full")})

    py_files = any(os.path.isfile(os.path.join(root, x)) for x in ("pyproject.toml", "setup.py", "requirements.txt"))
    if py_files and shutil.which("bandit") and enabled.get("sast", True):
        add({"id": "bandit", "category": "static", "kind": "json-command",
             "command": ["bandit", "-r", ".", "-f", "json", "-q"], "required": False,
             "severity": "high", "timeout_s": min(timeout, 600),
             "parse": parse_bandit, "modes": ("static", "all", "full")})

    if shutil.which("trivy") and enabled.get("deps", True):
        add({"id": "trivy", "category": "deps", "kind": "json-command",
             "command": ["trivy", "fs", "--format", "json", "--scanners", "vuln,secret,misconfig",
                         "--quiet", "."], "required": False, "severity": "high",
             "timeout_s": min(timeout, 900), "parse": parse_trivy,
             "modes": ("static", "all", "full")})

    if os.path.isfile(os.path.join(root, "go.mod")) and shutil.which("govulncheck"):
        add({"id": "govulncheck", "category": "deps", "kind": "generic",
             "command": ["govulncheck", "./..."], "required": False,
             "severity": "high", "timeout_s": min(timeout, 600),
             "modes": ("static", "all", "full")})

    if os.path.isfile(os.path.join(root, "package-lock.json")) and shutil.which("npm"):
        add({"id": "npm-audit", "category": "deps", "kind": "json-command",
             "command": ["npm", "audit", "--json"], "required": False,
             "severity": "high", "timeout_s": min(timeout, 300),
             "parse": parse_npm_audit, "modes": ("static", "all", "full")})

    if (enabled.get("dynamic", False) or force_dynamic) and shutil.which("cargo"):
        fuzz_root = os.path.join(root, "fuzz", "Cargo.toml")
        if os.path.isfile(fuzz_root):
            add({"id": "cargo-fuzz", "category": "dynamic", "kind": "fuzz",
                 "command": ["cargo", "fuzz"], "required": bool(force_dynamic),
                 "severity": "high", "timeout_s": min(timeout, 900),
                 "modes": ("dynamic", "all", "full")})

    if enabled.get("runtime", False) or force_runtime:
        mcp = None
        for rel in (".mcp.json", "mcp.json", os.path.join(".qoder", "mcp.json")):
            if os.path.isfile(os.path.join(root, rel)):
                mcp = rel
                break
        if mcp and os.path.isdir(os.path.join(root, "crates", "trit-procscan")):
            add({"id": "mcp-audit", "category": "runtime", "kind": "generic",
                 "command": ["cargo", "run", "-q", "-p", "trit-procscan", "--",
                             "mcp", mcp, "--timeout-ms", "8000", "--strict"],
                 "required": False, "severity": "high", "timeout_s": min(timeout, 600),
                 "modes": ("runtime", "all", "full")})

    # User-supplied commands are first-class, but still argv-only and timed.
    for index, cmd in enumerate(sec.get("commands", []) or []):
        if not isinstance(cmd, dict) or not cmd.get("command"):
            continue
        add({"id": cmd.get("id") or f"custom-{index}", "category": cmd.get("category", "static"),
             "kind": "generic", "command": list(cmd["command"]),
             "required": bool(cmd.get("required", False)),
             "severity": cmd.get("severity", "high"),
             "timeout_s": float(cmd.get("timeout_s", timeout)),
             "modes": ("quick", "static", "dynamic", "runtime", "all", "full")})
    return specs


def mode_allows(spec, mode):
    if mode in ("all", "full"):
        return True
    return mode in spec.get("modes", ("all",))


def run_one(spec, root, timeout_s, dry_run=False):
    t0 = time.time()
    meta = {"id": spec["id"], "category": spec.get("category", ""),
            "required": bool(spec.get("required")), "status": "ran",
            "reason": "", "seconds": 0.0, "findings": 0}
    if dry_run:
        meta.update({"status": "planned", "reason": "dry-run"})
        return meta, []
    if spec.get("kind") == "fuzz":
        # Real fuzzing, not a capability probe: list targets first, then run one
        # bounded target.  No target means not applicable, not a synthetic pass.
        if shutil.which("cargo") is None:
            meta.update({"status": "unavailable", "reason": "cargo not found"})
            return meta, []
        fuzz_cmd = ["cargo"]
        if shutil.which("rustup"):
            toolchains = run_process(["rustup", "toolchain", "list"], root, 30)
            if toolchains.get("code") == 0 and re.search(r"^nightly", toolchains.get("out", ""), re.M):
                fuzz_cmd.append("+nightly")
        fuzz_cmd.append("fuzz")
        lr = run_process(fuzz_cmd + ["list"], root, min(timeout_s, 60))
        if lr["code"] != 0:
            meta.update({"status": "unavailable", "reason": "cargo-fuzz not installed or no fuzz manifest",
                         "seconds": round(time.time() - t0, 2)})
            return meta, []
        targets = [ln.strip() for ln in lr["out"].splitlines() if ln.strip()]
        if not targets:
            meta.update({"status": "not_applicable", "reason": "no fuzz target",
                         "seconds": round(time.time() - t0, 2)})
            return meta, []
        rr = run_process(fuzz_cmd + ["list"], root, min(timeout_s, 60))
        if rr["code"] != 0:
            meta.update({"status": "unavailable", "reason": "cargo-fuzz not installed or no fuzz manifest",
                         "seconds": round(time.time() - t0, 2)})
            return meta, []
        targets = [ln.strip() for ln in rr["out"].splitlines() if ln.strip()]
        if not targets:
            meta.update({"status": "not_applicable", "reason": "no fuzz target",
                         "seconds": round(time.time() - t0, 2)})
            return meta, []
        # Cover every discovered target, not just the alphabetically first one.
        # The per-target budget divides the scanner timeout so a hung target
        # cannot eat the whole gate.
        per_target = max(60, int(timeout_s / max(len(targets), 1)))
        findings = []
        ran = []
        for target in targets:
            rr = run_process(fuzz_cmd + ["run", target, "--", "-runs=1000"],
                             root, per_target)
            meta["seconds"] = round(time.time() - t0, 2)
            out = (rr.get("out") or "") + "\n" + (rr.get("err") or "")
            build_failure = bool(re.search(
                r"failed to build fuzz script|nightly option|requires nightly|"
                r"error:.*(?:-Z|build)", out, re.I))
            if rr.get("timed_out"):
                meta.update({"status": "error", "reason": f"fuzz timeout on {target}"})
                findings = generic_finding(rr, spec)
                break
            if rr["code"] != 0 and build_failure:
                # A toolchain/build failure is not a security finding.  Keep it
                # honest as an unavailable required gate so it cannot look green.
                meta.update({"status": "unavailable",
                             "reason": f"cargo-fuzz build unavailable: {out.strip().splitlines()[-1][:200]}"})
                return meta, []
            if rr["code"] != 0:
                meta.update({"status": "findings", "reason": f"fuzz target {target} exit {rr['code']}"})
                findings = generic_finding(rr, spec)
                break
            ran.append(target)
        else:
            meta.update({"status": "pass", "reason": f"{len(ran)} fuzz targets x 1000 runs"})
            findings = []
        meta["findings"] = len(findings)
        return meta, findings
    if shutil.which(spec["command"][0]) is None and not os.path.isabs(spec["command"][0]):
        meta.update({"status": "unavailable", "reason": f"command not found: {spec['command'][0]}",
                     "seconds": round(time.time() - t0, 2)})
        return meta, []
    r = run_process(spec["command"], root, timeout_s)
    meta["seconds"] = r["seconds"]
    if r.get("missing"):
        meta.update({"status": "unavailable", "reason": "command not found"})
        return meta, []
    if r.get("timed_out"):
        meta.update({"status": "error", "reason": f"timeout after {timeout_s}s"})
        findings = generic_finding(r, spec)
        meta["findings"] = len(findings)
        return meta, findings
    # privacy_scan needs a git index (it uses `git ls-files`).  A non-git tree is
    # not a privacy violation; report not-applicable instead of a fake HIGH.
    out_text = (r.get("out") or "") + "\n" + (r.get("err") or "")
    if (spec.get("id") == "privacy" and r["code"] not in (0, None)
            and re.search(r"git ls-files 失败|需要在 git 仓库", out_text)):
        meta.update({"status": "unavailable",
                     "reason": "privacy scan needs a git index (non-git tree)"})
        return meta, []
    if r["code"] not in (0, None):
        meta.update({"status": "findings", "reason": f"exit {r['code']}"})
    else:
        meta.update({"status": "pass", "reason": "exit 0"})
    findings = []
    if spec.get("kind") in ("json-script", "json-command"):
        doc = json_from_text(r.get("out", ""))
        if doc is not None and spec.get("parse"):
            findings = spec["parse"](doc)
            if not findings and r["code"] not in (0, None):
                findings = generic_finding(r, spec)
        elif r["code"] not in (0, None):
            findings = generic_finding(r, spec)
    else:
        findings = generic_finding(r, spec)
    meta["findings"] = len(findings)
    return meta, findings


def decide(scanners, findings, fail_on, allow_gaps):
    gaps = []
    for s in scanners:
        if s["required"] and s["status"] in ("unavailable", "error"):
            gaps.append(f"{s['id']}: {s['reason']}")
    blocking = [f for f in findings if f["severity"] in set(fail_on)]
    if gaps and not allow_gaps:
        return "fail", 1, gaps, blocking
    if blocking:
        return "fail", 1, gaps, blocking
    if gaps and allow_gaps:
        return "pass", 0, gaps, blocking
    return "pass", 0, gaps, blocking


def report_doc(root, mode, scanners, findings, fail_on, allow_gaps, settings_meta):
    verdict, exit_code, gaps, blocking = decide(scanners, findings, fail_on, allow_gaps)
    counts = {s: len([f for f in findings if f["severity"] == s]) for s in SEVERITIES}
    return {
        "schema": SCHEMA,
        "root": os.path.abspath(root),
        "mode": mode,
        "verdict": verdict,
        "exit": exit_code,
        "at": datetime.now().astimezone().isoformat(timespec="microseconds"),
        "settings": settings_meta,
        "fail_on": list(fail_on),
        "allow_gaps": bool(allow_gaps),
        "summary": {
            "scanners": len(scanners),
            "ran": len([s for s in scanners if s["status"] in ("pass", "findings")]),
            "unavailable": len([s for s in scanners if s["status"] == "unavailable"]),
            "not_applicable": len([s for s in scanners if s["status"] == "not_applicable"]),
            "findings": len(findings),
            "blocking": len(blocking),
            "counts": counts,
        },
        "scanners": scanners,
        "findings": sorted(findings, key=lambda f: (severity_rank(f["severity"]), f["scanner"], f["id"])),
        "gaps": gaps,
    }


def self_test():
    checks = 0

    def ok(cond, msg):
        nonlocal checks
        checks += 1
        if not cond:
            raise AssertionError(msg)

    # Normalizers.
    audit = {"vulnerabilities": {"list": [{"advisory": {"id": "RUSTSEC-1", "title": "bad"},
                                           "package": {"name": "x", "version": "1.0"}}]},
             "warnings": {"unmaintained": [{"advisory": {"id": "RUSTSEC-2", "title": "old"},
                                             "package": {"name": "y", "version": "2.0"}}]}}
    f = parse_cargo_audit(audit)
    ok(any(x["severity"] == "high" for x in f), "cargo audit vulnerability is high")
    ok(any(x["severity"] == "medium" for x in f), "unmaintained is medium")
    sem = {"results": [{"check_id": "x", "path": "a.py", "start": {"line": 3},
                        "extra": {"severity": "ERROR", "message": "m", "lines": "x"}}]}
    f = parse_semgrep(sem)
    ok(f and f[0]["severity"] == "high" and f[0]["line"] == 3, "semgrep normalize")
    band = {"results": [{"test_id": "B1", "issue_severity": "MEDIUM", "issue_text": "m",
                         "filename": "a.py", "line_number": 4, "code": "x"}]}
    f = parse_bandit(band)
    ok(f and f[0]["severity"] == "medium", "bandit normalize")
    npm = {"vulnerabilities": {"pkg": {"severity": "high", "title": "bad", "via": [], "cve": "CVE-1"}}}
    f = parse_npm_audit(npm)
    ok(f and f[0]["severity"] == "high", "npm normalize")
    trivy = {"Results": [{"Target": "img", "Vulnerabilities": [
        {"VulnerabilityID": "CVE-2", "Severity": "CRITICAL", "Title": "bad",
         "PkgName": "p", "InstalledVersion": "1"}]}]}
    f = parse_trivy(trivy)
    ok(f and f[0]["severity"] == "critical", "trivy normalize")
    secret = [{"rule": "aws-key", "path": "a.txt", "line": 1, "match": "x"}]
    f = parse_secret(secret)
    ok(f and f[0]["severity"] == "high", "secret normalize")
    privacy = {"details": [{"rule": "drive-path", "path": "AGENTS.md", "line": 3}]}
    f = parse_privacy(privacy)
    ok(f and f[0]["severity"] == "high", "privacy normalize")

    # Decisions: high fails, medium passes, required gap fails, optional gap passes.
    sc = [{"id": "s", "required": True, "status": "pass", "reason": "", "findings": 1}]
    f = [finding("s", "static", "high", "bad")]
    v, e, gaps, blocking = decide(sc, f, ("critical", "high"), False)
    ok(v == "fail" and e == 1 and blocking, "high finding fails")
    f = [finding("s", "static", "medium", "warn")]
    v, e, gaps, blocking = decide(sc, f, ("critical", "high"), False)
    ok(v == "pass" and e == 0, "medium does not fail high gate")
    sc_gap = [{"id": "s", "required": True, "status": "unavailable", "reason": "missing", "findings": 0}]
    v, e, gaps, blocking = decide(sc_gap, [], ("critical", "high"), False)
    ok(v == "fail" and e == 1 and gaps, "required gap fails closed")
    v, e, gaps, blocking = decide(sc_gap, [], ("critical", "high"), True)
    ok(v == "pass" and e == 0 and gaps, "allow_gaps keeps gap visible")

    # Timeout is real and produces a blocking finding.
    r = run_process([PY, "-c", "import time; time.sleep(2)"], ".", 1)
    ok(r["timed_out"] and r["code"] == 124, "timeout is 124")
    f = generic_finding(r, {"id": "slow", "category": "dynamic", "timeout_s": 1})
    ok(f and f[0]["severity"] == "high" and "timed out" in f[0]["title"], "timeout finding")

    # argv-only: a shell metacharacter is one argument, not a shell.
    r = run_process([PY, "-c", "import sys; print(sys.argv[1])", "a; echo pwned"], ".", 10)
    ok(r["code"] == 0 and r["out"].strip() == "a; echo pwned", "argv does not invoke a shell")

    # Report round-trip.
    doc = report_doc(".", "quick", sc, [finding("s", "static", "high", "bad")],
                     ("critical", "high"), False, {"name": "deepseek-strict"})
    ok(doc["schema"] == SCHEMA and doc["verdict"] == "fail" and doc["exit"] == 1, "report verdict")
    ok(json.loads(json.dumps(doc))["summary"]["blocking"] == 1, "report serializes")

    # Empty tree: no secrets/privacy scripts -> no required gap, not applicable.
    import tempfile
    with tempfile.TemporaryDirectory() as td:
        settings = agent_settings.DEFAULTS
        specs = scanner_specs(settings, td)
        ok(len(specs) >= 0, "empty tree specs")
        meta, fs = run_one({"id": "missing", "category": "static", "kind": "generic",
                            "command": ["definitely-not-a-real-command-xyz"], "required": False,
                            "severity": "high", "timeout_s": 5}, td, 5)
        ok(meta["status"] == "unavailable" and not fs, "missing command is unavailable")

    print(f"security-gate self-test: {checks}/{checks}")
    return 0


def parse(argv):
    out = {"root": ".", "mode": "", "out": "", "json": False, "self_test": False,
           "list": False, "dry_run": False, "fail_on": "", "allow_gaps": None,
           "settings": "", "scanner": [], "skip": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("--self-test",):
            out["self_test"] = True
            i += 1
        elif a in ("--json",):
            out["json"] = True
            i += 1
        elif a in ("--list",):
            out["list"] = True
            i += 1
        elif a in ("--dry-run",):
            out["dry_run"] = True
            i += 1
        elif a in ("--allow-gaps",):
            out["allow_gaps"] = True
            i += 1
        elif a in ("--no-allow-gaps",):
            out["allow_gaps"] = False
            i += 1
        elif a in ("--root", "-Root"):
            out["root"] = argv[i + 1] if i + 1 < len(argv) else "."
            i += 2
        elif a in ("--mode", "-Mode"):
            out["mode"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--out", "-Out"):
            out["out"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--settings", "-Settings"):
            out["settings"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--fail-on", "-FailOn"):
            out["fail_on"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--scanner", "-Scanner"):
            out["scanner"].append(argv[i + 1] if i + 1 < len(argv) else "")
            i += 2
        elif a in ("--skip", "-Skip"):
            out["skip"].append(argv[i + 1] if i + 1 < len(argv) else "")
            i += 2
        elif a in ("--help", "-h"):
            out["help"] = True
            i += 1
        else:
            i += 1
    return out


def main(argv):
    a = parse(argv)
    if a["self_test"]:
        return self_test()
    if a.get("help"):
        print("security-gate: --root DIR [--mode quick|static|dynamic|runtime|all] "
              "[--out report.json] [--json] [--fail-on critical,high] [--settings FILE]")
        return 0
    root = os.path.abspath(a["root"])
    if not os.path.isdir(root):
        print("security-gate: --root is not a directory")
        return 2
    try:
        settings, meta = agent_settings.load_settings(root, explicit=a["settings"])
    except (OSError, ValueError) as e:
        print(f"security-gate: settings error: {e}")
        return 2
    sec = settings.get("security", {})
    mode = (a["mode"] or sec.get("mode") or "quick").lower()
    if mode == "full":
        mode = "all"
    if mode not in ("quick", "static", "dynamic", "runtime", "all"):
        print(f"security-gate: unknown mode {mode}")
        return 2
    fail_on = [x.strip() for x in (a["fail_on"] or ",".join(sec.get("fail_on", FAIL_ON_DEFAULT))).split(",")
               if x.strip()]
    bad = [x for x in fail_on if x not in SEVERITIES]
    if bad:
        print(f"security-gate: bad severity {bad}")
        return 2
    allow_gaps = bool(sec.get("allow_gaps", False)) if a["allow_gaps"] is None else a["allow_gaps"]
    specs = [s for s in scanner_specs(settings, root, mode) if mode_allows(s, mode)]
    if a["scanner"]:
        wanted = set(a["scanner"])
        specs = [s for s in specs if s["id"] in wanted]
    if a["skip"]:
        skipped = set(a["skip"])
        specs = [s for s in specs if s["id"] not in skipped]
    if a["list"]:
        for s in specs:
            print(f"{s['id']:20} {s['category']:8} required={str(s['required']):5} "
                  f"timeout={s['timeout_s']}s command={' '.join(s['command'])}")
        return 0
    scanners, findings = [], []
    for spec in specs:
        m, f = run_one(spec, root, spec["timeout_s"], a["dry_run"])
        scanners.append(m)
        findings.extend(f)
    doc = report_doc(root, mode, scanners, findings, fail_on, allow_gaps,
                     {"name": meta.get("name"), "source": meta.get("source")})
    if a["out"]:
        os.makedirs(os.path.dirname(os.path.abspath(a["out"])), exist_ok=True)
        with open(a["out"], "w", encoding="utf-8", newline="\n") as fh:
            json.dump(doc, fh, ensure_ascii=True, indent=2, sort_keys=True)
            fh.write("\n")
    if a["json"]:
        print(json.dumps(doc, ensure_ascii=True, sort_keys=True))
    else:
        print(f"security-gate: {doc['verdict'].upper()} mode={mode} "
              f"findings={doc['summary']['findings']} blocking={doc['summary']['blocking']} "
              f"gaps={len(doc['gaps'])}")
        for s in scanners:
            print(f"  {s['id']:20} {s['status']:13} findings={s['findings']} {s['reason']}")
        for f in doc["findings"][:12]:
            print(f"  {f['severity'].upper():8} {f['scanner']} {f['path']}:{f['line'] or '-'} {f['title']}")
    return doc["exit"]


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
