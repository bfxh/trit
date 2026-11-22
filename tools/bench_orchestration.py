#!/usr/bin/env python3
"""bench_orchestration: what the agent_verify chain costs, measured, not claimed.

Core numbers, each a median over repeated rounds on a throwaway fixture:

  1. guard overhead ratio   full chain wall vs running the same plan directly
  2. receipt token proxy    full vs compact receipt bytes; proxy tokens = bytes/4 --
                            a context-size PROXY, not a model-side token count
  3. parallel-gate speedup  chain wall with the three read-only gates serial vs
                            concurrent (--parallel-gates)
  4. crash recovery         kill the chain mid-run; then dirty-state detection wall
                            (next run refuses on the chain lock) + journal restore wall

Token-economics numbers (L123, prefix-stable receipts; industry prompt caching --
Prompt Cache MLSys'24 arXiv:2311.04934, CacheBlend EuroSys'25: provider discounts on
cached input 50-90%, agentic deployments report 41-80% API-cost savings; a hit
depends on the artifact's prefix stability, which is what the tool layer controls):

  5. lcp_ratio              longest common byte prefix of receipt 2..N vs receipt 1,
                            as a share of each receipt (same task, consecutive runs)
  6. cacheable_bytes_ratio  that shared prefix as a share of the base receipt -- the
                            bytes a provider could serve from cache. HONEST BOUNDARY:
                            the tool layer manufactures the cacheable prefix; whether
                            and how much a provider discounts it is the provider's
                            decision and is NOT measurable locally -- hit rate is
                            claimed nowhere.
  7. delta_receipt_ratio    --delta --against <prev> receipt bytes vs the full receipt
                            bytes of the same run (subagent-chained scenario)
  8. policy_batch_wall      N policy subprocess spawns (the old orchestration shape)
                            vs ONE -CommandsFile batch call, same commands -- the
                            wall-clock fix behind the guard overhead ratio.

Plus an adversarial section (self-planted attacks, programmatic -- honest boundary:
real attackers are more cunning, this bounds nothing about the wild):

  9. fake-claim capture     N=100 planted false claims (nonexistent files, numbers
                            with no or lying provenance, test-pass against a receipt
                            that recorded RED) -> claim_audit capture rate and the
                            leaked-sample list (anything classified VERIFIED)
 10. duplicate-spend attack the same spend appended to the attest chain N times ->
                            every replay refused, chain verify intact, balance unchanged

Honesty rules: a warm-up round is run and excluded; each round is measured fresh and
the median is derived, never accumulated (same accounting rule as costprobe/ledger);
commands are `true`, so there is no build cache that could lie about cold/warm; the
journal hashes content, not mtime. Fixtures are built from scratch per invocation.
The adversarial receipts are synthetic files written by this bench, not real runs.

  python tools/bench_orchestration.py [-Rounds 3] [-Commands 40] [-Keep]
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

TOOLS = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
AV = os.path.join(TOOLS, "agent_verify.py")
JOURNAL = os.path.join(TOOLS, "journal.py")
ATTEST = os.path.join(TOOLS, "attest.py")
CLAIM_AUDIT = os.path.join(TOOLS, "claim_audit.py")
POLICY = os.path.join(TOOLS, "policy.py")


def parse_args(argv):
    out = {"rounds": 3, "commands": 40, "keep": False}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if name == "keep":
            out["keep"] = True
            i += 1
            continue
        if name in ("rounds", "commands"):
            out[name] = max(1, int(val))
        i += 2
    return out


def sh(cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True,
                          encoding="utf-8", errors="replace")


def build_fixture(root, n_cmds):
    os.makedirs(os.path.join(root, "pkg"), exist_ok=True)
    os.makedirs(os.path.join(root, "tests"), exist_ok=True)
    with open(os.path.join(root, "pkg", "core.py"), "w", newline="\n") as fh:
        fh.write("def f():\n    return 1\n")
    with open(os.path.join(root, "pkg", "__init__.py"), "w", newline="\n") as fh:
        fh.write("")
    with open(os.path.join(root, "tests", "test_core.py"), "w", newline="\n") as fh:
        fh.write("from pkg.core import f\n\n\ndef test_f():\n    assert f() == 1\n")
    with open(os.path.join(root, "conftest.py"), "w", newline="\n") as fh:
        fh.write("")
    sh(["git", "init", "-q"], cwd=root)
    sh(["git", "add", "-A"], cwd=root)
    sh(["git", "-c", "user.email=b@ench", "-c", "user.name=bench", "commit", "-qm", "init"],
       cwd=root)
    plan = {"proven": True, "commands": ["true"] * n_cmds, "unproven": []}
    with open(os.path.join(root, "plan.json"), "w", newline="\n") as fh:
        json.dump(plan, fh)
    slow = {"proven": True, "commands": ["sleep 6"], "unproven": []}
    with open(os.path.join(root, "plan-slow.json"), "w", newline="\n") as fh:
        json.dump(slow, fh)
    return os.path.join(root, "plan.json"), os.path.join(root, "plan-slow.json")


def pick_bash():
    """Same shell-picking rule as _run.py, one purpose: the direct baseline must run on
    the SAME binary the chain's runner uses. PATH resolution lands on usr/bin/bash
    (msys runtime), which pays ~0.5s startup per spawn vs ~0.05s for Git\\bin\\bash --
    a slow-shell denominator would fake a 10x guard overhead saving."""
    cands = [os.environ.get("BASH"),
             r"C:\Program Files\Git\bin\bash.exe",
             r"C:\Program Files\Git\usr\bin\bash.exe",
             r"C:\Program Files (x86)\Git\bin\bash.exe"]
    git = shutil.which("git")
    if git:
        cands.append(os.path.join(os.path.dirname(os.path.dirname(git)), "bin", "bash.exe"))
    for c in cands:
        if c and os.path.exists(c):
            return c
    return shutil.which("bash")


def direct_wall(fixture, commands, bash):
    """Run the plan's commands the way the chain's runner ultimately does: bash -c, in
    sequence, no guards. This is the denominator of the guard overhead ratio."""
    t0 = time.time()
    for cmd in commands:
        sh([bash, "-c", cmd], cwd=fixture)
    return time.time() - t0


def chain_wall(fixture, plan_path, store, receipt, extra=()):
    args = [PY, AV, "-Root", fixture, "-Changed", "pkg/core.py", "-Lang", "python",
            "-Plan", plan_path, "-Store", store, "-Receipt", receipt] + list(extra)
    t0 = time.time()
    p = sh(args)
    return time.time() - t0, p.returncode


def non_gate_seconds(receipt_path):
    """Sum of recorded step walls EXCLUDING the three gates -- used to derive the gates
    segment wall (chain wall minus everything the receipt did record). Reads the
    prefix-stable shape (volatile.steps_seconds index-aligned) and the legacy shape."""
    with open(receipt_path, "r", encoding="ascii") as fh:
        doc = json.load(fh)
    gates = {"affected", "policy", "constraints"}
    steps = doc.get("steps", [])
    secs = doc.get("volatile", {}).get("steps_seconds")
    if secs is None:
        secs = [s.get("seconds", 0) for s in steps]
    return sum(sec for s, sec in zip(steps, secs) if s.get("step") not in gates)


def median(values):
    s = sorted(values)
    return s[len(s) // 2] if s else 0.0


def lcp_bytes(b1, b2):
    n = min(len(b1), len(b2))
    i = 0
    while i < n and b1[i] == b2[i]:
        i += 1
    return i


def policy_walls(fixture, sample):
    """(serial wall, batch wall) for the same commands: serial = one policy.py spawn
    per command (the old orchestration shape, the 4.5x-overhead dominant term);
    batch = one -CommandsFile spawn. Per-command results are equivalent by selftest;
    here only the wall is measured."""
    t0 = time.time()
    for cmd in sample:
        sh([PY, POLICY, "-Profile", "workspace-write", "-Root", fixture, "-Command", cmd])
    serial = time.time() - t0
    bf = os.path.join(fixture, "bench-batch.json")
    with open(bf, "w", encoding="ascii", newline="\n") as fh:
        json.dump({"commands": sample}, fh, ensure_ascii=True)
    t0 = time.time()
    p = sh([PY, POLICY, "-Profile", "workspace-write", "-Root", fixture,
            "-CommandsFile", bf])
    batch = time.time() - t0
    assert p.returncode == 0, f"policy batch exit {p.returncode}"
    return serial, batch


def crash_round(fixture, slow_plan, store, py):
    """Kill the chain mid-run, then measure detection and restore. Returns
    (detect_s, restore_s) or None when the crash did not leave the expected dirt."""
    out = open(os.path.join(fixture, "bench-crash.out"), "w")
    proc = subprocess.Popen(
        [py, AV, "-Root", fixture, "-Changed", "pkg/core.py", "-Lang", "python",
         "-Plan", slow_plan, "-Store", store, "-Receipt",
         os.path.join(fixture, "rc-crash.json")],
        stdout=out, stderr=subprocess.STDOUT)
    time.sleep(2.5)
    alive = proc.poll() is None
    proc.kill()
    proc.wait()
    out.close()
    lock = os.path.join(fixture, ".journal", "chain.lock")
    if not alive or not os.path.exists(lock):
        return None
    t0 = time.time()
    p = sh([py, AV, "-Root", fixture, "-Changed", "pkg/core.py", "-Lang", "python",
            "-Plan", slow_plan, "-Store", store, "-Receipt",
            os.path.join(fixture, "rc-detect.json")])
    detect_s = time.time() - t0
    if p.returncode != 1:
        return None
    os.remove(lock)
    t0 = time.time()
    r = sh([py, JOURNAL, "-Mode", "restore", "-Root", fixture, "-Store", store])
    restore_s = time.time() - t0
    if r.returncode != 0:
        return None
    return detect_s, restore_s


def fmt(v):
    return f"{v:.2f}"


def build_adversarial(work, n_claims=100):
    """Plant a fake-claim set and the receipt evidence to catch part of it with.

    Three classes, all self-made and labeled as such: nonexistent files (file-exists),
    unsourceable or lying numbers (number), test-pass against a receipt that recorded
    RED for the exact test. The receipts are synthetic JSON in agent-verify receipt
    shape -- no real runs happen here, and the honesty note in the report says so.
    """
    root = os.path.join(work, "adv-root")
    rc_dir = os.path.join(work, "adv-receipts")
    os.makedirs(os.path.join(root, "pkg"), exist_ok=True)
    os.makedirs(os.path.join(root, "tests"), exist_ok=True)
    os.makedirs(os.path.join(root, "artifacts"), exist_ok=True)
    os.makedirs(rc_dir, exist_ok=True)
    with open(os.path.join(root, "pkg", "core.py"), "w", newline="\n") as fh:
        fh.write("def f(): return 1\n")
    with open(os.path.join(root, "tests", "test_core.py"), "w", newline="\n") as fh:
        fh.write("def test_f(): assert f()\n")
    with open(os.path.join(root, "tests", "test_red.py"), "w", newline="\n") as fh:
        fh.write("def test_r(): assert 0\n")
    with open(os.path.join(root, "artifacts", "bench.txt"), "w", newline="\n") as fh:
        fh.write("guard_overhead_ratio 2.35x\nreceipt_full_bytes 4800\n")
    with open(os.path.join(rc_dir, "rc-green.json"), "w", newline="\n") as fh:
        json.dump({"at": "bench-synthetic", "run": [
            {"command": "python -m pytest tests/test_core.py -q", "exit": 0}]}, fh)
    with open(os.path.join(rc_dir, "rc-red.json"), "w", newline="\n") as fh:
        json.dump({"at": "bench-synthetic", "run": [
            {"command": "python -m pytest tests/test_red.py -q", "exit": 1}]}, fh)

    # deterministic split: nonexistent files, numbers with lying provenance, numbers
    # with no provenance, test-pass against a RED receipt (sourceless numbers land in
    # UNVERIFIED by design -- a number without provenance is not provably false, it is
    # just not believable; the other three classes are hard FABRICATED)
    n_files = n_claims // 3 + 1
    n_rest = n_claims - n_files
    n_red = n_rest // 2
    n_lie_num = (n_rest - n_red) // 2
    n_no_src = n_rest - n_red - n_lie_num
    lines = []
    for i in range(n_files):
        lines.append({"claim": f"tests/ghost_{i:03d}.py exists on disk",
                      "kind": "file-exists"})
    for i in range(n_lie_num):
        lines.append({"claim": f"overhead ratio {7.77 + i:.2f}x measured in bench",
                      "kind": "number", "source": {"path": "artifacts/bench.txt"}})
    for i in range(n_no_src):
        lines.append({"claim": f"session spent {500 + i} tokens total",
                      "kind": "number"})
    for i in range(n_red):
        lines.append({"claim": f"tests/test_red.py all green (batch {i:03d})",
                      "kind": "test-pass"})
    claims_path = os.path.join(work, "adv-claims.jsonl")
    with open(claims_path, "w", encoding="utf-8", newline="\n") as fh:
        for ln in lines:
            fh.write(json.dumps(ln, ensure_ascii=True) + "\n")
    return root, rc_dir, claims_path, n_claims


def adversarial_round(work):
    """Run both attacks. Returns a dict of numbers; asserts only that the harness
    itself worked (a capture rate below 1.0 is a REPORTED finding, not a bench crash).
    """
    root, rc_dir, claims_path, total = build_adversarial(work)
    p = sh([PY, CLAIM_AUDIT, "-Claims", claims_path, "-Root", root,
            "-Receipts", os.path.join(rc_dir, "*.json")])
    assert p.returncode == 2, f"adversarial claim audit exit {p.returncode} (setup broken)"
    m = re.search(r"summary: total=(\d+) verified=(\d+) unverified=(\d+) fabricated=(\d+)",
                  p.stdout)
    assert m, "claim_audit summary line not found"
    t, v, u, f = (int(x) for x in m.groups())
    assert t == total, f"claim count mismatch {t} != {total}"
    leaked = []
    for ln in p.stdout.splitlines():
        lm = re.match(r"\s+\[(\d+)\] VERIFIED\s+(.*)$", ln)
        if lm:
            leaked.append(lm.group(2))

    # duplicate-spend attack: one honest append, then N replays of the SAME identity
    store = os.path.join(work, "adv-store")
    base = [PY, ATTEST, "append", "--store", store, "--kind", "spend",
            "--task-id", "t1", "--agent-id", "attacker", "--runid", "r1",
            "--claimed-tokens", "500"]
    first = sh(base)
    assert first.returncode == 0, f"first append failed: {first.stdout}{first.stderr}"
    accepted = 0
    for _ in range(20):
        if sh(base).returncode == 0:
            accepted += 1
    pv = sh([PY, ATTEST, "verify", "--store", store])
    assert pv.returncode == 0, "chain broken after duplicate-spend attack"
    pb = sh([PY, ATTEST, "balance", "--store", store, "--task", "t1"])
    bm = re.search(r"spent: (\S+)", pb.stdout)
    assert bm, "balance output not found"
    spent_after = float(bm.group(1))
    assert spent_after == 500.0, f"balance drifted after attack: {spent_after}"
    return {"total": total, "verified": v, "unverified": u, "fabricated": f,
            "leaked": leaked, "replays": 20, "accepted": accepted,
            "spent_after": spent_after}


def main(argv):
    a = parse_args(argv)
    work = tempfile.mkdtemp(prefix="bench-orch-")
    fixture = os.path.join(work, "fixture")
    os.makedirs(fixture)
    plan_path, slow_plan = build_fixture(fixture, a["commands"])
    store = os.path.join(work, "store")
    rc_full = os.path.join(work, "rc-full.json")
    rc_compact = os.path.join(work, "rc-compact.json")
    rc_serial = os.path.join(work, "rc-serial.json")
    rc_parallel = os.path.join(work, "rc-parallel.json")

    with open(os.path.join(fixture, "plan.json"), "r", encoding="utf-8") as fh:
        commands = json.load(fh)["commands"]

    # warm-up: one full serial + one parallel chain, excluded from every median
    chain_wall(fixture, plan_path, store, rc_serial)
    chain_wall(fixture, plan_path, store, rc_parallel, extra=("--parallel-gates",))

    bash = pick_bash()
    direct = []
    serial_pairs, parallel_pairs = [], []  # (round wall, non-gate seconds of the SAME run)
    for i in range(a["rounds"]):
        direct.append(direct_wall(fixture, commands, bash))
        w, code = chain_wall(fixture, plan_path, store, rc_serial)
        assert code == 0, f"serial chain exit {code}"
        serial_pairs.append((w, non_gate_seconds(rc_serial)))
        w, code = chain_wall(fixture, plan_path, store, rc_parallel,
                             extra=("--parallel-gates",))
        assert code == 0, f"parallel chain exit {code}"
        parallel_pairs.append((w, non_gate_seconds(rc_parallel)))

    d_med = median(direct)
    s_med = median([w for w, _ in serial_pairs])
    p_med = median([w for w, _ in parallel_pairs])
    overhead_ratio = s_med / d_med if d_med else 0.0

    _, _ = chain_wall(fixture, plan_path, store, rc_full)
    _, _ = chain_wall(fixture, plan_path, store, rc_compact, extra=("--compact",))
    fb = os.path.getsize(rc_full)
    cb = os.path.getsize(rc_compact)

    # gates segment wall, derived per round: chain wall minus what the receipt recorded
    # for the non-gate steps. (Round-matched on purpose: a median wall minus a
    # last-round receipt is how two different runs get subtracted into fiction.)
    gates_serial = median([max(0.0, w - ng) for w, ng in serial_pairs])
    gates_parallel = median([max(0.0, w - ng) for w, ng in parallel_pairs])
    gates_ratio = gates_serial / gates_parallel if gates_parallel else 0.0
    chain_ratio = s_med / p_med if p_med else 0.0

    crashes = []
    for _ in range(2):
        r = crash_round(fixture, slow_plan, store, PY)
        if r:
            crashes.append(r)
    det = median([c[0] for c in crashes]) if crashes else 0.0
    res = median([c[1] for c in crashes]) if crashes else 0.0

    # L123 token economics: prefix-stable receipts measured on consecutive full runs,
    # then delta rounds against the previous run's full receipt, then the policy batch
    # wall on a sample of the plan's commands.
    lcp_blobs = []
    for i in range(3):
        rc_l = os.path.join(work, f"rc-lcp-{i}.json")
        _, code = chain_wall(fixture, plan_path, store, rc_l)
        assert code == 0, f"lcp round {i} exit {code}"
        with open(rc_l, "rb") as fh:
            lcp_blobs.append(fh.read())
    base_blob = lcp_blobs[0]
    lcp_shares, cache_shares = [], []
    for b in lcp_blobs[1:]:
        l = lcp_bytes(base_blob, b)
        lcp_shares.append(l / len(b) if b else 0.0)
        cache_shares.append(l / len(base_blob) if base_blob else 0.0)
    lcp_ratio = median(lcp_shares)
    cache_ratio = median(cache_shares)

    delta_shares = []
    prev_rc = os.path.join(work, "rc-lcp-2.json")
    for i in range(2):
        d_path = os.path.join(work, f"rc-delta-{i}.json")
        f_path = os.path.join(work, f"rc-delta-full-{i}.json")
        _, code = chain_wall(fixture, plan_path, store, d_path,
                             extra=("--delta", "--against", prev_rc,
                                    "--delta-full", f_path))
        assert code == 0, f"delta round {i} exit {code}"
        fb = os.path.getsize(f_path)
        delta_shares.append(os.path.getsize(d_path) / fb if fb else 0.0)
        prev_rc = f_path
    delta_ratio = median(delta_shares)

    pol_serial, pol_batch = policy_walls(fixture, commands[:10])
    pol_speedup = pol_serial / pol_batch if pol_batch else 0.0

    # adversarial section: self-planted attacks against claim_audit and the attest
    # chain. These numbers describe THIS generator's attacks only -- a self-made
    # adversarial set is a floor-check of the auditors, never a bound on real ones.
    adv = adversarial_round(work)
    adv_captured = adv["unverified"] + adv["fabricated"]
    adv_rate = adv_captured / adv["total"] if adv["total"] else 0.0
    adv_accept_rate = adv["accepted"] / adv["replays"] if adv["replays"] else 0.0

    print()
    print(f"bench_orchestration  commands={a['commands']}  rounds={a['rounds']} "
          f"(median, warm-up excluded)  fixture={work}")
    print()
    print(f"{'metric':<30}{'value':>10}   detail")
    print("-" * 96)
    print(f"{'guard_overhead_ratio':<30}{overhead_ratio:>9.2f}x   "
          f"chain {fmt(s_med)}s vs direct {fmt(d_med)}s (same plan, wall; policy "
          f"batched since L123)")
    print(f"{'receipt_full_bytes':<30}{fb:>10}   proxy tokens {fb // 4} "
          f"(bytes/4 -- proxy, NOT a model-side count)")
    print(f"{'receipt_compact_bytes':<30}{cb:>10}   proxy tokens {cb // 4}")
    print(f"{'compact_ratio':<30}{cb / fb:>9.3f}x   "
          f"compact saves {100 * (1 - cb / fb):.1f}% of receipt bytes")
    print(f"{'lcp_ratio':<30}{lcp_ratio:>9.3f}   longest common byte prefix, receipt "
          f"2..N vs receipt 1 (same task, consecutive full runs)")
    print(f"{'cacheable_bytes_ratio':<30}{cache_ratio:>9.3f}   shared prefix / base "
          f"receipt bytes -- the cacheable prefix is manufactured HERE; provider "
          f"discount is the provider's decision (hit rate not locally measurable)")
    print(f"{'delta_receipt_ratio':<30}{delta_ratio:>9.3f}   --delta --against <prev> "
          f"bytes / full receipt bytes of the same run (subagent chain)")
    print(f"{'policy_serial_wall':<30}{fmt(pol_serial):>9}s   "
          f"{min(10, len(commands))} commands, one spawn each (old orchestration shape)")
    print(f"{'policy_batch_wall':<30}{fmt(pol_batch):>9}s   same commands via ONE "
          f"-CommandsFile spawn, speedup {pol_speedup:.1f}x (per-command results "
          f"equivalent by selftest)")
    print(f"{'gates_wall_serial':<30}{fmt(gates_serial):>9}s   "
          f"derived: chain wall minus recorded non-gate step walls")
    print(f"{'gates_wall_parallel':<30}{fmt(gates_parallel):>9}s   "
          f"parallel-gate speedup {gates_ratio:.2f}x (whole-chain ratio {chain_ratio:.2f}x)")
    print(f"{'crash_detect_wall':<30}{fmt(det):>9}s   "
          f"next run refuses on .journal/chain.lock ({len(crashes)}/{2} rounds usable)")
    print(f"{'crash_restore_wall':<30}{fmt(res):>9}s   journal restore after lock removal")
    print()
    print(f"adversarial (self-planted, programmatic; receipts are synthetic bench files, "
          f"NOT real runs)")
    print("-" * 96)
    print(f"{'fake_claim_capture_rate':<30}{adv_rate:>9.3f}   "
          f"{adv_captured}/{adv['total']} planted fakes not believed "
          f"(fabricated {adv['fabricated']}, unverified {adv['unverified']})")
    leaked_txt = ("audit holes leak here -- do not ignore"
                  if adv["leaked"] else "no claim escaped as VERIFIED")
    print(f"{'fake_claim_leaked':<30}{len(adv['leaked']):>10}   {leaked_txt}")
    for s in adv["leaked"][:5]:
        print(f"{'':<42}leaked: {s.encode('ascii', 'backslashreplace').decode('ascii')}")
    print(f"{'duplicate_spend_accepted':<30}{adv_accept_rate:>9.3f}   "
          f"{adv['accepted']}/{adv['replays']} replays accepted; chain verify intact, "
          f"balance still {adv['spent_after']:.0f}")
    print()
    print("honesty: proxy tokens are context-size bytes/4, not model-side token counts; "
          "commands are `true` (no build cache); medians over fresh rounds, nothing "
          "accumulated; the adversarial set is self-made and programmatic -- real "
          "attackers are more cunning, so the capture rate bounds nothing about the "
          "wild; LCP/cacheable numbers describe the tool layer's prefix stability only "
          "-- provider cache discounts are the provider's decision and are not "
          "measured or claimed here (industry figures: Prompt Cache MLSys'24 "
          "arXiv:2311.04934, CacheBlend EuroSys'25; 50-90% cached-input discounts, "
          "41-80% agentic API-cost savings)")
    if not a["keep"]:
        shutil.rmtree(work, ignore_errors=True)
    else:
        print(f"fixture kept: {work}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
