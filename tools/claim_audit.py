#!/usr/bin/env python3
"""claim_audit: model statements are checked against reality, not believed.

    claim_audit.py -Claims <claims.jsonl> -Root <dir> [-Receipts <glob>...] 

Claims file: JSON lines, one claim per line:

    {"claim": "tests/claim_edges.rs all green", "kind": "test-pass"}
    {"claim": "pkg/core.py exists", "kind": "file-exists", "file": "pkg/core.py"}
    {"claim": "overhead 2.35x", "kind": "number", "source": {"path": "artifacts/bench.txt"}}
    {"claim": "...", "kind": "command-ran", "command": "python -m pytest tests/..."}

What the model says carries zero evidential weight (the whole point of this tool).
Every claim is classified against evidence on disk and in recorded runs:

    VERIFIED     evidence found and agrees (a pointer to it is printed)
    UNVERIFIED   no evidence either way -- must not be taken as true
    FABRICATED   contradicts reality: the claimed file/test/artifact does not
                 exist, or the most recent recorded verdict is red, or the
                 claimed number is absent from its own cited artifact

Per-kind verifiers:
    test-pass    the named test file must exist under root; the most recent
                 receipt (-Receipts globs, newest first) whose run[] mentions it
                 decides: exit 0 -> VERIFIED, exit != 0 -> FABRICATED (claimed
                 green while reality is red), no receipt mentions it -> UNVERIFIED
    file-exists  real file on disk under root -> VERIFIED with sha256 evidence;
                 absent -> FABRICATED
    number       every number in the claim must be found in the cited source
                 artifact (bench output / ledger line); no source -> UNVERIFIED;
                 source missing -> FABRICATED; number absent from it -> FABRICATED
    command-ran  exact (whitespace-normalized) match in some receipt's run[] ->
                 VERIFIED; no record -> UNVERIFIED (absence of evidence is not
                 contradiction -- the command may have run outside the chain)
    anything else  UNVERIFIED (a verifier that does not exist cannot certify)

Exit codes: 0 all VERIFIED; 1 contains UNVERIFIED; 2 contains FABRICATED -- the
loudest alarm, treat the whole claim set as hostile; 3 bad input (malformed claims
line, machine path in a claim, bad invocation -- fail-closed, nothing is believed
from an unreadable claim set). Receipts that fail to parse are counted and noted,
never used as evidence.

Output is pure ASCII (claims are printed backslash-escaped). Machine-path shapes
in any claim field refuse the whole audit: receipts and audits may be committed,
and a committed file must not carry local paths.
"""

import glob
import hashlib
import json
import os
import re
import sys

# Same shapes privacy_scan hunts for; a claims file quoting local paths would see
# them echoed into receipts. Scan PARSED string values, not raw lines -- JSON
# doubles backslashes, so raw-line scanning is structurally blind to drive paths
# (docs/ERRORS.md L110).
MACHINE_PATH_RX = re.compile(
    r"[A-Za-z]:[\\/]|/Users/[A-Za-z0-9._-]+/|/home/[A-Za-z0-9._-]+/|/mnt/[a-z]/")

TEST_FILE_RX = re.compile(r"[A-Za-z0-9_.\-/\\]+\.(?:rs|py|go|js|ts|mjs)")
PATH_LIKE_RX = re.compile(r"[A-Za-z0-9_.\-/\\]+\.[A-Za-z0-9]{1,8}")
NUMBER_RX = re.compile(r"(?<![\d.])-?\d+(?:\.\d+)?(?![\d.])")

VERIFIED, UNVERIFIED, FABRICATED = "VERIFIED", "UNVERIFIED", "FABRICATED"


def parse_args(argv):
    out = {"claims": "", "root": "", "receipts": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if name == "receipts":
            out["receipts"].extend([x for x in val.split(",") if x.strip()])
        elif name in ("claims", "root"):
            out[name] = val
        else:
            return None, f"unknown flag {a}"
        i += 2
    return out, None


def ascii_safe(text, limit=72):
    t = str(text).encode("ascii", "backslashreplace").decode("ascii")
    return t if len(t) <= limit else t[:limit - 3] + "..."


def sha12(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()[:12]


def under_root(root, rel):
    """Join and refuse escapes: a claim pointing outside root is not evidence, it is
    a traversal (and would let 'exists' bless files the root never contained)."""
    rel_n = str(rel).replace("\\", "/").lstrip("/")
    full = os.path.abspath(os.path.join(root, *rel_n.split("/")))
    root_n = os.path.abspath(root)
    if full != root_n and not full.startswith(root_n + os.sep):
        return None
    return full


def load_receipts(patterns):
    """(receipts newest-first, unreadable_count). A receipt is any JSON dict with a
    'run' list of {command, exit}; anything else is not evidence."""
    receipts = []
    unreadable = 0
    seen = set()
    for pat in patterns:
        matches = [pat] if os.path.isfile(pat) else sorted(glob.glob(pat))
        if not os.path.isfile(pat) and not matches:
            unreadable += 1
            continue
        for p in matches:
            if p in seen or not os.path.isfile(p):
                continue
            seen.add(p)
            try:
                with open(p, "r", encoding="utf-8-sig") as fh:
                    doc = json.load(fh)
                runs = [(r.get("command", ""), r.get("exit")) for r in doc.get("run", [])
                        if isinstance(r, dict)]
            except (OSError, ValueError):
                unreadable += 1
                continue
            if not isinstance(doc, dict) or not runs:
                continue
            receipts.append({"path": p, "mtime": os.path.getmtime(p), "runs": runs})
    receipts.sort(key=lambda r: -r["mtime"])
    return receipts, unreadable


def find_test_target(claim):
    t = claim.get("test")
    if isinstance(t, str) and t.strip():
        return t.strip()
    for tok in TEST_FILE_RX.findall(str(claim.get("claim", ""))):
        if "test" in tok.lower():
            return tok
    return ""


def normalize_cmd(s):
    return " ".join(str(s).split())


def verify_test_pass(claim, root, receipts):
    target = find_test_target(claim)
    if not target:
        return UNVERIFIED, "no test file identifiable in the claim"
    full = under_root(root, target)
    if not full or not os.path.isfile(full):
        return FABRICATED, f"claimed test file does not exist under root: {target}"
    tn = target.replace("\\", "/")
    for rc in receipts:
        for cmd, exit_code in rc["runs"]:
            if tn in normalize_cmd(cmd).replace("\\", "/"):
                if exit_code == 0:
                    return VERIFIED, (f"receipt {os.path.basename(rc['path'])} "
                                      f"run exit 0: {normalize_cmd(cmd)}")
                return FABRICATED, (f"most recent recorded verdict for this test is RED "
                                    f"(exit {exit_code}) in receipt "
                                    f"{os.path.basename(rc['path'])}: {normalize_cmd(cmd)}")
    return UNVERIFIED, (f"test file exists but no receipt records a verdict for it "
                        f"(not green just because it is claimed)")


def verify_file_exists(claim, root):
    target = claim.get("file")
    if not (isinstance(target, str) and target.strip()):
        for tok in PATH_LIKE_RX.findall(str(claim.get("claim", ""))):
            target = tok
            break
    if not (isinstance(target, str) and target.strip()):
        return UNVERIFIED, "no file path identifiable in the claim"
    full = under_root(root, target)
    if not full or not os.path.isfile(full):
        return FABRICATED, f"claimed file does not exist on disk under root: {target}"
    return VERIFIED, f"sha256:{sha12(full)} {target}"


def verify_number(claim, root):
    text = str(claim.get("claim", ""))
    numbers = []
    v = claim.get("value")
    if isinstance(v, (int, float)) and not isinstance(v, bool):
        numbers.append(str(v))
    else:
        numbers.extend(NUMBER_RX.findall(text))
    if not numbers:
        return UNVERIFIED, "no number to trace in the claim"
    src = claim.get("source")
    if not (isinstance(src, dict) and isinstance(src.get("path"), str) and src["path"].strip()):
        return UNVERIFIED, ("number carries no traceable source artifact "
                            "(a number without provenance is not a measurement)")
    full = under_root(root, src["path"])
    if not full or not os.path.isfile(full):
        return FABRICATED, f"claimed source artifact does not exist: {src['path']}"
    with open(full, "r", encoding="utf-8", errors="replace") as fh:
        artifact = fh.read()
    missing = [n for n in numbers
               if not re.search(r"(?<![\d.])" + re.escape(n) + r"(?![\d.])", artifact)]
    if missing:
        return FABRICATED, (f"number(s) {', '.join(missing)} not found in cited artifact "
                            f"{src['path']}")
    return VERIFIED, f"sha256:{sha12(full)} {src['path']} (all {len(numbers)} number(s) found)"


def verify_command_ran(claim, receipts):
    cmd = claim.get("command")
    if not (isinstance(cmd, str) and cmd.strip()):
        cmd = str(claim.get("claim", ""))
    cmd = normalize_cmd(cmd)
    if not cmd:
        return UNVERIFIED, "no command identifiable in the claim"
    for rc in receipts:
        for c, exit_code in rc["runs"]:
            if normalize_cmd(c) == cmd:
                return VERIFIED, (f"receipt {os.path.basename(rc['path'])} run exit "
                                  f"{exit_code}: {normalize_cmd(c)}")
    return UNVERIFIED, "no execution record for this command in receipts (absence of evidence is not proof it ran)"


def load_claims(path):
    """(claims, "") or (None, reason). A bad line refuses the WHOLE file: half-read
    claim sets let the readable half pose as audited (fail-closed, line named)."""
    claims = []
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            for ln, raw in enumerate(fh, start=1):
                raw = raw.strip()
                if not raw:
                    continue
                try:
                    obj = json.loads(raw)
                except ValueError as ex:
                    return None, f"line {ln} is not valid JSON ({ex})"
                if not isinstance(obj, dict):
                    return None, f"line {ln} is not a JSON object"
                c = obj.get("claim")
                k = obj.get("kind")
                if not isinstance(c, str) or not c.strip():
                    return None, f"line {ln}: 'claim' must be a non-empty string"
                if not isinstance(k, str) or not k.strip():
                    return None, f"line {ln}: 'kind' must be a non-empty string"
                strings = [c, k]
                stack = [obj.get("source"), obj.get("test"), obj.get("file"),
                         obj.get("command")]
                while stack:
                    item = stack.pop()
                    if isinstance(item, str):
                        strings.append(item)
                    elif isinstance(item, dict):
                        stack.extend(item.values())
                    elif isinstance(item, list):
                        stack.extend(item)
                for s in strings:
                    if MACHINE_PATH_RX.search(s):
                        return None, (f"line {ln}: machine path shape in a claim "
                                      f"(audits may be committed; use root-relative paths)")
                claims.append(obj)
    except OSError as ex:
        return None, f"unreadable ({ex})"
    return claims, ""


def main(argv):
    a, err = parse_args(argv)
    if err:
        print(f"claim_audit: {err}")
        return 3
    if not a["claims"] or not a["root"]:
        print("claim_audit: -Claims and -Root are required")
        return 3
    if not os.path.isdir(a["root"]):
        print("claim_audit: -Root must be an existing directory (evidence lives under it)")
        return 3
    claims, err = load_claims(a["claims"])
    if claims is None:
        print(f"claim_audit: REFUSED -- claims file untrustworthy: {err} "
              f"(fail-closed: nothing is believed from a half-readable claim set)")
        return 3
    receipts, unreadable = load_receipts(a["receipts"])

    print(f"claim_audit: {len(claims)} claim(s) from {os.path.basename(a['claims'])}, "
          f"{len(receipts)} receipt(s) scanned as evidence")
    counts = {VERIFIED: 0, UNVERIFIED: 0, FABRICATED: 0}
    for idx, claim in enumerate(claims, start=1):
        kind = claim["kind"].strip().lower()
        if kind == "test-pass":
            verdict, ev = verify_test_pass(claim, a["root"], receipts)
        elif kind == "file-exists":
            verdict, ev = verify_file_exists(claim, a["root"])
        elif kind == "number":
            verdict, ev = verify_number(claim, a["root"])
        elif kind == "command-ran":
            verdict, ev = verify_command_ran(claim, receipts)
        else:
            verdict, ev = UNVERIFIED, f"no verifier for kind '{claim['kind']}' (unknown kinds certify nothing)"
        counts[verdict] += 1
        print(f"  [{idx}] {verdict:<10} {ascii_safe(claim['claim'])}")
        print(f"      {verdict}: {ascii_safe(ev, 160)}")
    if unreadable:
        print(f"note: {unreadable} receipt path(s) unreadable or not receipts -- "
              f"counted, never used as evidence")
    print(f"summary: total={len(claims)} verified={counts[VERIFIED]} "
          f"unverified={counts[UNVERIFIED]} fabricated={counts[FABRICATED]}")
    if counts[FABRICATED]:
        print("verdict: FABRICATED present -- claims contradict reality; treat the "
              "whole claim set as hostile")
        return 2
    if counts[UNVERIFIED]:
        print("verdict: UNVERIFIED present -- claims without evidence must not be "
              "taken as true")
        return 1
    print("verdict: all claims verified against recorded evidence")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
