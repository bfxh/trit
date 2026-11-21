#!/usr/bin/env python3
"""claim_audit: model statements are checked against reality, not believed.

    claim_audit.py -Claims <claims.jsonl> -Root <dir> [-Receipts <glob>...]
    claim_audit.py second-opinion -Claims <f> -Root <dir> [-Receipts <g>...] [-Ledger <cost-ledger.json>]

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

Aggregate claims (kind=aggregate) -- a big statement is not evidence. "All green",
"everything works" must carry "decomposition": a non-empty list of atomic claims,
each with its own kind + claim (+ the fields that kind needs). The aggregate is
judged ONLY through its atoms, rules pinned:

    any atom FABRICATED                                -> aggregate FABRICATED
    all atoms VERIFIED (the aggregate is a conjunction) -> aggregate VERIFIED
    any atom UNVERIFIED, or no/empty decomposition      -> aggregate UNVERIFIED

An aggregate without decomposition can never be VERIFIED -- the sentence "all
green" has no standing at this gate until it is broken into checkable steps.
Atoms must be atomic kinds: a kind=aggregate atom has no verifier (UNVERIFIED,
decomposition does not recurse); a decomposition that is present but malformed
(non-list, atom without claim/kind) refuses the whole file (exit 3, fail-closed).
Attribution: judging the steps beats judging the summary -- OpenAI, "Let's Verify
Step by Step" (2023), process supervision over result supervision.

Second opinion -- independent re-derivation, not a helper's nod:

    claim_audit.py second-opinion -Claims <f> -Root <dir> [-Receipts <glob>...]
                                  [-Receipt <recent.json>] [-Ledger <cost-ledger.json>]

The same claims are re-derived through two INDEPENDENT evidence paths and diffed;
both paths must walk through, and the third party re-derives rather than re-reads
the first audit:

    src1  the freshest receipt alone (what the newest recorded run testifies;
          receipts are sorted by mtime, newest wins)
    src2  reality: the file/test present on disk under root + the cost ledger's own
          record that the command actually ran (entry key carries sha8(command),
          the same derivation agent_verify writes when a chain runs); for
          file-exists the receipt's journal changes[] (added/modified/deleted) is
          the second capture mechanism instead of bare disk

    both paths produce agreeing evidence   -> VERIFIED, annotated dual-source
    either path contradicts reality        -> FABRICATED (both evidences printed)
    either path produces no evidence       -> UNVERIFIED (never upgraded on absence)

kind=number has no second path (the cited artifact is the only source) and never
upgrades. A receipt or ledger that was named but cannot be parsed refuses the mode
(exit 3, fail-closed) -- a second opinion that cannot trust its own sources is not
an opinion. Attribution: the third party re-derives and diffs, it does not "take
another look" -- independent recomputation, arXiv:2610.12234-style.

Exit codes: 0 all VERIFIED; 1 contains UNVERIFIED; 2 contains FABRICATED -- the
loudest alarm, treat the whole claim set as hostile; 3 bad input (malformed claims
line, machine path in a claim, bad invocation -- fail-closed, nothing is believed
from an unreadable claim set). Both modes share these codes. Receipts that fail to
parse are counted and noted, never used as evidence (audit mode); named second-
opinion sources refuse instead (fail-closed where the mode's whole point is
cross-checking).

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

try:
    import ledger  # same tools directory; the second source reads the cost ledger's run keys
except ImportError:
    # A traceback exit would masquerade as exit 1 (UNVERIFIED) and silently pass gates;
    # degrading here keeps the tri-state contract, and -Ledger then refuses loudly.
    ledger = None

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
    out = {"claims": "", "root": "", "receipts": [], "receipt": "", "ledger": ""}
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
        elif name in ("claims", "root", "receipt", "ledger",
                      "claims-file", "claimsfile"):
            out[{"claims-file": "claims", "claimsfile": "claims"}.get(name, name)] = val
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


def file_target(claim):
    target = claim.get("file")
    if not (isinstance(target, str) and target.strip()):
        for tok in PATH_LIKE_RX.findall(str(claim.get("claim", ""))):
            target = tok
            break
    return target if isinstance(target, str) and target.strip() else ""


def verify_file_exists(claim, root):
    target = file_target(claim)
    if not target:
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


AGGREGATE_KIND = "aggregate"


def verify_one(kind, claim, root, receipts):
    """The per-kind dispatch every verdict (top-level claim or decomposition atom)
    goes through. In audit mode `receipts` is the receipt corpus; in second-opinion
    mode it is the single freshest receipt and so_verdict wraps this call."""
    if kind == "test-pass":
        return verify_test_pass(claim, root, receipts)
    if kind == "file-exists":
        return verify_file_exists(claim, root)
    if kind == "number":
        return verify_number(claim, root)
    if kind == "command-ran":
        return verify_command_ran(claim, receipts)
    return UNVERIFIED, f"no verifier for kind '{claim['kind']}' (unknown kinds certify nothing)"


def verify_atom(atom, root, receipts, so_ctx=None):
    kind = str(atom.get("kind", "")).strip().lower()
    if kind == AGGREGATE_KIND:
        return UNVERIFIED, ("aggregate cannot serve as its own atom "
                            "(decomposition does not recurse)")
    if so_ctx is not None:
        return so_verdict(kind, atom, root, so_ctx[0], so_ctx[1])
    return verify_one(kind, atom, root, receipts)


def verify_aggregate(claim, root, receipts, so_ctx=None):
    """Process supervision: the aggregate verdict is computed from atom verdicts,
    never from the aggregate sentence itself. Rules pinned (see usage):
    any F -> F; all V -> V; any U (incl. missing/empty decomposition) -> U."""
    dec = claim.get("decomposition")
    if not isinstance(dec, list) or not dec:
        return UNVERIFIED, ("aggregate without decomposition -- an undecomposed "
                            "aggregate claim cannot be VERIFIED (no decomposition, "
                            "no pass: 'all green' has no standing as one sentence)"), []
    subs = []
    for i, atom in enumerate(dec, start=1):
        v, ev = verify_atom(atom, root, receipts, so_ctx)
        subs.append((i, str(atom.get("claim", "")), v, ev))
    f_idx = [str(i) for i, _t, v, _e in subs if v == FABRICATED]
    if f_idx:
        return FABRICATED, (f"atom(s) {', '.join(f_idx)} contradict reality -- any "
                            f"FABRICATED atom makes the aggregate FABRICATED"), subs
    u_idx = [str(i) for i, _t, v, _e in subs if v == UNVERIFIED]
    if u_idx:
        return UNVERIFIED, (f"atom(s) {', '.join(u_idx)} lack evidence -- an aggregate "
                            f"with an unevidenced atom is UNVERIFIED"), subs
    tail = " (dual-source)" if so_ctx is not None else ""
    return VERIFIED, f"conjunction of {len(dec)} atom(s), all VERIFIED{tail}", subs


def cmd_sha8(text):
    """Same derivation agent_verify writes into cost-ledger run keys
    (av|<runid>|cmd<idx>|sha8(command)) -- the ledger can therefore testify that a
    command actually ran without storing the command text itself."""
    return hashlib.sha256(str(text).encode("utf-8", "replace")).hexdigest()[:8]


def ledger_records(keys, command):
    """The ledger's own key embeds sha8(command) as its command-identifying part,
    so corroboration is containment, not equality."""
    h = cmd_sha8(command)
    return any(h in k for k in keys)


def load_so_receipts(a):
    """(receipts newest-first, "") for the second opinion, or (None, reason).

    Unlike load_receipts, a receipt without run[] is still loaded (its journal
    changes[] can testify for file-exists). A source that was named but cannot be
    parsed refuses -- the mode's whole point is cross-checking, and a second
    opinion must not silently run on fewer sources than it was given."""
    paths = []
    if a.get("receipt"):
        paths.append(a["receipt"])
    for pat in a.get("receipts", []):
        matches = [pat] if os.path.isfile(pat) else sorted(glob.glob(pat))
        if not matches:
            return None, f"receipt path/glob matched nothing: {pat}"
        paths.extend(matches)
    out = []
    for p in dict.fromkeys(paths):
        try:
            with open(p, "r", encoding="utf-8-sig") as fh:
                doc = json.load(fh)
        except (OSError, ValueError) as ex:
            return None, f"receipt unreadable ({os.path.basename(p)}): {ex}"
        if not isinstance(doc, dict):
            return None, f"receipt is not a JSON object: {os.path.basename(p)}"
        runs = [(r.get("command", ""), r.get("exit")) for r in doc.get("run", [])
                if isinstance(r, dict)]
        chg = []
        changes = doc.get("changes")
        if isinstance(changes, list):
            for c in changes:
                m = (re.match(r"\s*journal:\s+(added|deleted|modified)\s+(\S+)", c)
                     if isinstance(c, str) else None)
                if m:
                    chg.append((m.group(1), m.group(2).replace("\\", "/")))
        out.append({"path": p, "mtime": os.path.getmtime(p), "runs": runs, "changes": chg})
    out.sort(key=lambda r: -r["mtime"])
    return out, ""


def load_ledger_keys(path):
    """(key set, "") from the cost ledger; (None, "") when no -Ledger was given
    (the second source then cannot corroborate command-ran/test-pass runs);
    (None, reason) when the ledger exists but is corrupt (caller refuses)."""
    if not path:
        return None, ""
    if ledger is None:
        return None, "ledger module unavailable (second-opinion expects tools/ledger.py beside it)"
    entries = ledger.load(path)
    if entries is None:
        return None, "cost ledger is corrupt -- refusing (a broken book certifies nothing)"
    return {str(e.get("key", "")) for e in entries if isinstance(e, dict)}, ""


def so_rederive(kind, claim, root, rc, keys):
    """Source 2 re-derivation: disk reality + ledger run records (or the receipt's
    journal changes for file-exists). VERIFIED only from positive evidence."""
    if kind == "test-pass":
        target = find_test_target(claim)
        if not target:
            return UNVERIFIED, "src2: no test file identifiable in the claim"
        full = under_root(root, target)
        if not full or not os.path.isfile(full):
            return FABRICATED, f"src2: disk -- claimed test file does not exist under root: {target}"
        if rc is None:
            return UNVERIFIED, "src2: ledger -- no receipt given, no command to trace"
        tn = target.replace("\\", "/")
        cmds = [c for c, _e in rc["runs"] if tn in normalize_cmd(c).replace("\\", "/")]
        if not cmds:
            return UNVERIFIED, "src2: ledger -- the receipt records no command for this test"
        if keys is None:
            return UNVERIFIED, "src2: ledger -- none given (pass -Ledger cost-ledger.json)"
        hits = [c for c in cmds if ledger_records(keys, c)]
        if not hits:
            return UNVERIFIED, ("src2: ledger -- does not record the receipt's test "
                                "command (the run left no ledger trace)")
        return VERIFIED, (f"src2: disk+ledger -- file present, ledger records "
                          f"sha8 {cmd_sha8(hits[0])}")
    if kind == "command-ran":
        cmd = claim.get("command")
        if not (isinstance(cmd, str) and cmd.strip()):
            cmd = str(claim.get("claim", ""))
        cmd = normalize_cmd(cmd)
        if not cmd:
            return UNVERIFIED, "src2: no command identifiable in the claim"
        if keys is None:
            return UNVERIFIED, "src2: ledger -- none given (pass -Ledger cost-ledger.json)"
        if ledger_records(keys, cmd):
            return VERIFIED, f"src2: ledger -- records sha8 {cmd_sha8(cmd)} (the chain ran it)"
        return UNVERIFIED, "src2: ledger -- does not record this command"
    if kind == "file-exists":
        target = file_target(claim)
        if not target:
            return UNVERIFIED, "src2: no file path identifiable in the claim"
        full = under_root(root, target)
        if not full or not os.path.isfile(full):
            return FABRICATED, f"src2: disk -- claimed file does not exist under root: {target}"
        if rc is None:
            return UNVERIFIED, "src2: journal -- no receipt given, nothing to cross-check"
        tn = target.replace("\\", "/")
        acts = [(ac, p) for ac, p in rc["changes"]
                if p == tn or p.endswith("/" + tn) or tn.endswith("/" + p)]
        if not acts:
            return UNVERIFIED, "src2: journal -- the receipt records nothing for this file"
        if any(ac == "deleted" for ac, _p in acts):
            return FABRICATED, "src2: journal -- the receipt records this file DELETED"
        return VERIFIED, ("src2: journal -- the receipt records "
                          + "/".join(sorted({ac for ac, _p in acts})) + " for this file")
    if kind == "number":
        return UNVERIFIED, ("src2: no independent second path exists for kind number "
                            "(the cited artifact is the only source)")
    return UNVERIFIED, f"src2: no verifier for kind '{claim.get('kind')}'"


def so_verdict(kind, claim, root, rc, keys):
    """Diff of two independent re-derivations. VERIFIED only when BOTH sources
    produce agreeing evidence (dual-source); any contradiction is FABRICATED with
    both evidences attached; absence degrades to UNVERIFIED, never an upgrade."""
    v1, ev1 = verify_one(kind, claim, root, [rc] if rc is not None else [])
    v2, ev2 = so_rederive(kind, claim, root, rc, keys)
    if v1 == FABRICATED or v2 == FABRICATED:
        return FABRICATED, f"downgraded -- src1: {ev1} | {ev2}"
    if v1 == VERIFIED and v2 == VERIFIED:
        return VERIFIED, f"dual-source -- src1: {ev1} | {ev2}"
    return UNVERIFIED, f"not corroborated -- src1: {ev1} | {ev2}"


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
                if k.strip().lower() == AGGREGATE_KIND:
                    dec = obj.get("decomposition")
                    # Present-but-malformed refuses the whole file; absent/empty is a
                    # verdict (aggregate UNVERIFIED), not a syntax error.
                    if isinstance(dec, list) and dec and not all(
                            isinstance(x, dict)
                            and isinstance(x.get("claim"), str) and x["claim"].strip()
                            and isinstance(x.get("kind"), str) and x["kind"].strip()
                            for x in dec):
                        return None, (f"line {ln}: aggregate decomposition must be a "
                                      f"list of atoms, each with claim+kind")
                strings = [c, k]
                stack = [obj.get("source"), obj.get("test"), obj.get("file"),
                         obj.get("command"), obj.get("decomposition")]
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
    mode = "audit"
    rest = list(argv)
    if rest and rest[0].lstrip("-").lower() in ("second-opinion", "secondopinion"):
        mode = "second-opinion"
        rest = rest[1:]
    a, err = parse_args(rest)
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
    if mode == "audit":
        return run_audit(a, claims)
    return run_second_opinion(a, claims)


def run_audit(a, claims):
    receipts, unreadable = load_receipts(a["receipts"])

    print(f"claim_audit: {len(claims)} claim(s) from {os.path.basename(a['claims'])}, "
          f"{len(receipts)} receipt(s) scanned as evidence")
    counts = {VERIFIED: 0, UNVERIFIED: 0, FABRICATED: 0}
    agg_count = 0
    atom_total = 0
    for idx, claim in enumerate(claims, start=1):
        kind = claim["kind"].strip().lower()
        subs = []
        if kind == AGGREGATE_KIND:
            verdict, ev, subs = verify_aggregate(claim, a["root"], receipts)
            agg_count += 1
            atom_total += len(subs)
        else:
            verdict, ev = verify_one(kind, claim, a["root"], receipts)
        counts[verdict] += 1
        print(f"  [{idx}] {verdict:<10} {ascii_safe(claim['claim'])}")
        print(f"      {verdict}: {ascii_safe(ev, 160)}")
        for si, stext, sv, sev in subs:
            print(f"      atom[{si}] {sv:<10} {ascii_safe(stext, 60)}")
            print(f"        {sv}: {ascii_safe(sev, 140)}")
    if unreadable:
        print(f"note: {unreadable} receipt path(s) unreadable or not receipts -- "
              f"counted, never used as evidence")
    print(f"summary: total={len(claims)} verified={counts[VERIFIED]} "
          f"unverified={counts[UNVERIFIED]} fabricated={counts[FABRICATED]}")
    if agg_count:
        print(f"summary: aggregates={agg_count} atoms={atom_total}")
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


def run_second_opinion(a, claims):
    rcs, err = load_so_receipts(a)
    if rcs is None:
        print(f"claim_audit: REFUSED -- second-opinion source unusable: {err}")
        return 3
    keys, err = load_ledger_keys(a["ledger"])
    if keys is None and err:
        print(f"claim_audit: REFUSED -- second-opinion source unusable: {err}")
        return 3
    rc = rcs[0] if rcs else None
    src1 = os.path.basename(rc["path"]) if rc else "none"
    src2 = os.path.basename(a["ledger"]) if a["ledger"] else "none"
    print(f"claim_audit second-opinion: {len(claims)} claim(s) from "
          f"{os.path.basename(a['claims'])}, src1 receipt={src1} (freshest), "
          f"src2 ledger={src2} + disk under root")
    counts = {VERIFIED: 0, UNVERIFIED: 0, FABRICATED: 0}
    dual = 0
    agg_count = 0
    atom_total = 0
    for idx, claim in enumerate(claims, start=1):
        kind = claim["kind"].strip().lower()
        subs = []
        if kind == AGGREGATE_KIND:
            verdict, ev, subs = verify_aggregate(claim, a["root"], [], (rc, keys))
            agg_count += 1
            atom_total += len(subs)
        else:
            verdict, ev = so_verdict(kind, claim, a["root"], rc, keys)
        counts[verdict] += 1
        if verdict == VERIFIED and "dual-source" in ev:
            dual += 1
        print(f"  [{idx}] {verdict:<10} {ascii_safe(claim['claim'])}")
        # 240, not the audit mode's 160: the two-source evidence must survive printing
        # whole -- a truncated reason reads as no reason.
        print(f"      {verdict}: {ascii_safe(ev, 240)}")
        for si, stext, sv, sev in subs:
            print(f"      atom[{si}] {sv:<10} {ascii_safe(stext, 60)}")
            print(f"        {sv}: {ascii_safe(sev, 200)}")
    print(f"summary: total={len(claims)} verified={counts[VERIFIED]} "
          f"unverified={counts[UNVERIFIED]} fabricated={counts[FABRICATED]}")
    extra = (f" aggregates={agg_count} atoms={atom_total}" if agg_count else "")
    print(f"summary: dual-source={dual}{extra}")
    if counts[FABRICATED]:
        print("verdict: FABRICATED present -- the second source contradicts the "
              "claim(s); treat the whole claim set as hostile")
        return 2
    if counts[UNVERIFIED]:
        print("verdict: UNVERIFIED present -- claims the independent re-derivation "
              "could not double-confirm must not be taken as true")
        return 1
    print("verdict: all claims re-derived through two independent evidence paths")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
