#!/usr/bin/env python3
"""attest: a hash-chained token ledger -- tamper-evident bookkeeping with structural
double-count prevention.

    attest.py append  --store <dir> --kind spend|grant|correction --runid <id>
                      [--task-id t] [--agent-id a] --claimed-tokens N [--note s]
    attest.py verify  --store <dir>
    attest.py balance --store <dir> [--task t] [--agent a]
    attest.py import  --store <dir> --from-ledger <cost-ledger.json>

Storage: <Store>/attest-ledger.jsonl -- OUTSIDE the repo, same site convention as
cost-ledger.json and the journal store. One JSON object per line:

    {seq, ts, agent_id, task_id, runid, claimed_tokens, kind, note, prev_hash, payload_hash}

payload_hash = sha256(canonical JSON of every field EXCEPT prev_hash); prev_hash =
the previous entry's payload_hash; the genesis row's prev_hash is a fixed constant
(64 zeros). Rewriting any recorded field breaks the recomputation; deleting or
reordering lines breaks seq/prev linkage -- verify reports the FIRST break.

What authentication means here (honest boundary, do not shortcut): this chain is
TAMPER-EVIDENT BOOKKEEPING AND DEDUP, not model-side ground truth. The tool layer
can never measure the model's real token usage; claimed_tokens is still the
CALLER'S DECLARATION. The chain guarantees exactly three things: a declared entry
cannot be silently altered, cannot vanish, and cannot be counted twice.

Double-count prevention (the known industry failure: a resumed session replays its
/cost totals): the dedup key (agent_id, task_id, runid, kind) is unique across the
whole chain, and payload_hash is unique too. A repeated spend is REJECTED at
append, not merged or warned -- duplicate bookkeeping is structurally impossible,
the same rule ledger.py applies to keys and costprobe applies to measurements.
`import --from-ledger` brings already-recorded cost-ledger spent entries onto the
chain under the SAME identity the live append would have used, so re-importing is
a no-op (already-recorded spend cannot vanish and cannot double).

Concurrency: appends take <store>/attest-ledger.lock (O_CREAT|O_EXCL) with a
bounded retry; a holder that will not release within the window makes appenders
refuse (fail-closed) -- the lock is never broken automatically, same discipline as
agent_verify's chain.lock.

Durability (crash consistency, honest Windows boundary): an append is one
full write of the line plus its newline, followed by os.fsync on the file
handle before close. os.fsync on Windows is FlushFileBuffers on that handle --
the closest stdlib equivalent of fsync(2); a directory-level fsync does not
exist in the Windows stdlib surface (there is no openable directory fd), so
name-entry durability after a power cut is a stated boundary, not a promise.
A process kill does not lose OS-buffered writes, so within-simulator kills
already preserve every confirmed line; fsync covers the power-loss class.

Recovery scan (load time, physical truncation only): a load that finds the
LAST line unparseable AND without its terminating newline treats it as a
truncated tail (the crash/power-loss signature -- a full write persists any
prefix of "line\n", so a real truncation artifact never carries the newline).
The bad tail is quarantined whole-line into the recycle bin
(<store>/trash, via trash.put_paths -- never raw-deleted), the ledger file is
truncated to the last intact line, and the load prints
"recovered: truncated N bytes (quarantined at <path>)". Nothing is silently
dropped and a bad tail never wedges the book forever; the recovered prefix is
still fully re-verified by verify_chain, and an append may continue on it.
A bad line anywhere BEFORE the tail, or a bad tail line that still has its
newline, is logical corruption -- no recovery, the load refuses (fail-closed)
and verify keeps gatekeeping (that division is deliberate: the scan repairs
physics, the chain verifies logic).

Exit codes: append 0 recorded / 1 refused (broken chain, duplicate, lock, corrupt
file) / 2 bad invocation; verify 0 intact / 1 broken or missing (an audit refuses
to say OK about nothing); balance 0 (zero is an honest answer, message says why)
/ 1 untrustworthy book / 2 bad invocation; import 0 (with imported/skipped counts)
/ 1 refused. Output is pure ASCII.

budget-gate contract (consumed by agent_verify): when the chain exists it is the
book of record for --spent-token; recorded scope spend = sum(spend) + sum
(correction, signed). No chain file -> the gate keeps declaration mode and labels
it declared-only. A chain that fails verify -> the gate refuses outright; a gate
that cannot trust its own books must not silently degrade.
"""

import hashlib
import json
import os
import sys
import time
from datetime import datetime

import ledger  # same tools directory; one loader for the cost-ledger shape on import
import trash   # same tools directory; the recovery scan quarantines bad tails, never raw-deletes

GENESIS_PREV = "0" * 64
KINDS = ("spend", "grant", "correction")
LEDGER_NAME = "attest-ledger.jsonl"
LOCK_NAME = "attest-ledger.lock"
LOCK_RETRY_S = 20.0
PAYLOAD_FIELDS = ("seq", "ts", "agent_id", "task_id", "runid", "claimed_tokens",
                  "kind", "note")


def parse_args(argv):
    out = {"mode": "", "store": "", "kind": "", "task_id": "", "agent_id": "",
           "runid": "", "claimed_tokens": "", "note": "", "from_ledger": ""}
    positional = [x for x in argv if not x.startswith("-")]
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        val = argv[i + 1] if i + 1 < len(argv) else ""
        out[{"store": "store", "kind": "kind", "task-id": "task_id", "taskid": "task_id",
             "task": "task_id", "agent-id": "agent_id", "agentid": "agent_id",
             "agent": "agent_id", "runid": "runid",
             "claimed-tokens": "claimed_tokens", "claimedtokens": "claimed_tokens",
             "note": "note", "from-ledger": "from_ledger", "fromledger": "from_ledger",
             "append": "mode", "verify": "mode", "balance": "mode",
             "import": "mode"}.get(name, name)] = val
        i += 2
    if positional and positional[0] in ("append", "verify", "balance", "import"):
        out["mode"] = positional[0]
    return out


def canonical_payload(entry):
    body = {k: entry[k] for k in PAYLOAD_FIELDS}
    return json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def payload_hash_of(entry):
    return hashlib.sha256(canonical_payload(entry).encode("ascii")).hexdigest()


def now_iso():
    return datetime.now().astimezone().isoformat(timespec="microseconds")


def ledger_path(store):
    return os.path.join(store, LEDGER_NAME)


def _quarantine_bytes(site_root, payload, tag):
    """(bin_relpath, byte_count, "") or ("", 0, error): move the bad-tail bytes into the
    recycle bin under <site_root>/trash -- the bin's iron rule, never a raw delete. The
    bytes land as one .part file, ledgered with a reason, so nothing vanishes."""
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    tmp = os.path.join(site_root, f"{tag}.tail-{stamp}-{os.getpid()}.part")
    try:
        with open(tmp, "wb") as fh:
            fh.write(payload)
            fh.flush()
            os.fsync(fh.fileno())
        entries, err = trash.put_paths(site_root, site_root, [tmp],
                                       f"{tag}-tail-recovery:{now_iso()}")
        if err:
            try:
                os.remove(tmp)
            except OSError:
                pass
            return "", 0, err
        e = entries[0]
        return f"trash/{e['blob']}", len(payload), ""
    except OSError as ex:
        try:
            if os.path.exists(tmp):
                os.remove(tmp)
        except OSError:
            pass
        return "", 0, f"quarantine write failed ({ex})"


def _truncate_file(path, keep_bytes):
    """Cut the file down to keep_bytes and flush that fact to the handle (fsync)."""
    fd = os.open(path, os.O_WRONLY)
    try:
        os.ftruncate(fd, keep_bytes)
        os.fsync(fd)
    finally:
        os.close(fd)


def load_chain(path):
    """(entries, "") or (None, reason). One bad line refuses the WHOLE file -- a
    partially-read chain would let a verifier bless the intact prefix while the tail
    rots, which is the exact lie the chain exists to prevent. The one exception is
    physical truncation (see module docstring): an unparseable LAST line with no
    terminating newline is quarantined whole into the bin, the file is truncated to
    the last intact line, and the load proceeds on the verified prefix.

    Read in binary and decoded per line: a bit flip that breaks UTF-8 must be a clean
    per-line refusal (or tail recovery), never a traceback from a whole-file decode.
    """
    if not os.path.exists(path):
        return [], ""
    try:
        with open(path, "rb") as fh:
            raw = fh.read()
    except OSError as ex:
        return None, f"unreadable ({ex})"
    pieces = raw.split(b"\n")
    if pieces and pieces[-1] == b"":
        pieces.pop()  # a trailing newline ends the last real line; the empty piece is not one
    entries = []
    for idx, chunk in enumerate(pieces):
        ln = idx + 1
        text = chunk.strip()
        if not text:
            continue
        try:
            line_text = text.decode("utf-8-sig")
        except UnicodeDecodeError as ex:
            return _load_chain_on_bad_line(path, raw, pieces, idx, ln,
                                           f"line {ln} is not decodable ({ex})")
        try:
            e = json.loads(line_text)
        except ValueError as ex:
            return _load_chain_on_bad_line(path, raw, pieces, idx, ln,
                                           f"line {ln} is not valid JSON ({ex})")
        if not isinstance(e, dict) or "payload_hash" not in e:
            return None, f"line {ln} is not a chain entry"
        entries.append(e)
    return entries, ""


def _load_chain_on_bad_line(path, raw, pieces, idx, ln, reason):
    """Route one bad line: physical truncation of the tail -> quarantine + truncate +
    reload; anything else -> refuse with the line named (fail-closed).

    Truncation signature: the bad line is the LAST line AND the file does not end with
    a newline. A newline-terminated bad line was written whole by something, not cut
    by physics -- that is logical corruption and stays refuse-only.
    """
    is_last_line = idx == len(pieces) - 1
    ends_with_newline = raw.endswith(b"\n")
    if not (is_last_line and not ends_with_newline):
        return None, reason
    keep = sum(len(p) + 1 for p in pieces[:idx])  # bytes of every intact line plus its newline
    bad = raw[keep:]
    if not bad.strip():
        return None, reason
    site_root = os.path.dirname(os.path.abspath(path)) or "."
    os.makedirs(site_root, exist_ok=True)
    rel, n, err = _quarantine_bytes(site_root, bad, "attest-ledger")
    if err:
        return None, (f"line {ln} looks like a truncated tail but the recycle bin refused "
                      f"the quarantine ({err}) -- load refuses, bytes untouched (fail-closed)")
    try:
        _truncate_file(path, keep)
    except OSError as ex:
        return None, (f"truncated tail seen but the file could not be truncated back "
                      f"({ex}) -- load refuses, quarantine entry {rel} holds the bytes")
    print(f"attest: recovered: truncated {n} bytes (quarantined at {rel}) -- "
          f"load continues on the intact prefix, verify still gatekeeps it")
    return load_chain(path)


def verify_chain(entries):
    """(True, "") when intact; (False, reason-with-first-break) otherwise.

    Full recomputation, first break wins: genesis prev_hash, seq continuity,
    payload_hash recompute, prev linkage, duplicate payload, duplicate dedup key.
    """
    seen_payload = {}
    seen_key = {}
    prev = GENESIS_PREV
    for pos, e in enumerate(entries, start=1):
        seq = e.get("seq")
        if seq != pos:
            return False, f"seq {seq} out of order at position {pos}"
        if e.get("prev_hash") != prev:
            return False, f"prev_hash mismatch at seq {pos}"
        ph = hashlib.sha256(canonical_payload(e).encode("ascii")).hexdigest()
        if e.get("payload_hash") != ph:
            return False, f"payload_hash mismatch at seq {pos} (recorded fields were altered)"
        if ph in seen_payload:
            return False, f"duplicate payload at seq {pos} (replay of seq {seen_payload[ph]})"
        key = (e.get("agent_id", ""), e.get("task_id", ""), e.get("runid", ""), e.get("kind", ""))
        if key in seen_key:
            return False, (f"duplicate bookkeeping key at seq {pos} "
                           f"(agent={key[0]} task={key[1]} runid={key[2]} kind={key[3]} "
                           f"already recorded at seq {seen_key[key]})")
        seen_payload[ph] = pos
        seen_key[key] = pos
        prev = ph
    return True, ""


def _num(v):
    v = round(v, 2)
    return int(v) if float(v).is_integer() else v


def balance_of(entries, task=None, agent=None):
    """Scope filter mirrors cost_scope: --task wins, --agent is the fallback, no flag
    = whole chain. spent_net is what the budget gate subtracts."""
    spent = granted = corrected = 0.0
    count = 0
    for e in entries:
        if task is not None and e.get("task_id", "") != task:
            continue
        if agent is not None and e.get("agent_id", "") != agent:
            continue
        v = e.get("claimed_tokens") or 0
        count += 1
        if e.get("kind") == "spend":
            spent += v
        elif e.get("kind") == "grant":
            granted += v
        elif e.get("kind") == "correction":
            corrected += v
    return {"entries": count, "spent": _num(spent), "granted": _num(granted),
            "corrections": _num(corrected),
            "spent_net": _num(spent + corrected),
            "net": _num(granted - spent + corrected)}


def spent_net_for(entries, task_id, agent_id):
    """Gate-facing recorded spend for a scope, or None when the scope has no entries
    (pure declaration -- same None contract as ledger_tokens_for)."""
    if task_id:
        task, agent = task_id, None
    elif agent_id:
        task, agent = None, agent_id
    else:
        return None
    b = balance_of(entries, task=task, agent=agent)
    return b["spent_net"] if b["entries"] else None


def acquire_lock(store):
    lock = os.path.join(store, LOCK_NAME)
    try:
        os.makedirs(store, exist_ok=True)
    except OSError as ex:
        return None, f"store directory unavailable ({ex})"
    deadline = time.time() + LOCK_RETRY_S
    while True:
        try:
            fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            os.write(fd, json.dumps({"pid": os.getpid(),
                                     "at": now_iso()}).encode("ascii"))
            os.close(fd)
            return lock, ""
        except FileExistsError:
            if time.time() >= deadline:
                return None, (f"lock {LOCK_NAME} not released within {int(LOCK_RETRY_S)}s "
                              f"(another appender is stuck or a killed process left it -- "
                              f"inspect, then remove it by hand; it is never broken "
                              f"automatically)")
        except OSError as ex:
            return None, f"lock unavailable ({ex})"
        time.sleep(0.05)


def release_lock(lock):
    if not lock:
        return
    try:
        os.remove(lock)
    except OSError:
        pass


def append_entry(store, kind, agent_id, task_id, runid, claimed_tokens, note=""):
    """(entry, "") or (None, reason).

    LANDMINE: load -> verify -> dedup -> seq derivation ALL happen inside the lock.
    Doing them first and locking only for the write would let two concurrent
    appenders both read the same tail and both write the same seq with the same
    prev_hash -- the chain would verify-broken silently some unlucky day, and the
    selftest's process-spawn jitter would hide it most days. The whole
    read-modify-write is the critical section, not the final line write.
    """
    lock, err = acquire_lock(store)
    if lock is None:
        return None, f"cannot append: {err}"
    try:
        path = ledger_path(store)
        entries, err = load_chain(path)
        if entries is None:
            return None, f"existing ledger untrustworthy: {err} (fail-closed, refuse to build on it)"
        ok, brk = verify_chain(entries)
        if not ok:
            return None, f"existing chain is broken: {brk} (fail-closed)"
        key = (agent_id, task_id, runid, kind)
        for e in entries:
            if (e.get("agent_id", ""), e.get("task_id", ""), e.get("runid", ""),
                    e.get("kind", "")) == key:
                return None, (f"duplicate bookkeeping refused: agent={agent_id} task={task_id} "
                              f"runid={runid} kind={kind} already recorded at seq "
                              f"{e.get('seq')} (double counting is structurally impossible)")
        entry = {"seq": len(entries) + 1, "ts": now_iso(), "agent_id": agent_id,
                 "task_id": task_id, "runid": runid, "claimed_tokens": claimed_tokens,
                 "kind": kind, "note": note,
                 "prev_hash": entries[-1]["payload_hash"] if entries else GENESIS_PREV}
        entry["payload_hash"] = payload_hash_of(entry)
        try:
            line = json.dumps(entry, sort_keys=True, separators=(",", ":"),
                              ensure_ascii=True) + "\n"
            fd = os.open(path, os.O_APPEND | os.O_CREAT | os.O_WRONLY)
            try:
                data = line.encode("ascii")
                while data:
                    # full-write loop: a short write would splice the next line into
                    # the middle of this one (same failure shape as L115's two-write
                    # writeln); os.fsync on the handle is the Windows FlushFileBuffers
                    # semantics -- the power-loss boundary the module docstring states
                    n = os.write(fd, data)
                    data = data[n:]
                os.fsync(fd)
            finally:
                os.close(fd)
        except OSError as ex:
            return None, f"append write failed ({ex})"
    finally:
        release_lock(lock)
    return entry, ""


def fmt_num(v):
    if float(v).is_integer():
        return str(int(v))
    return repr(float(v))


def check_entry_args(a):
    if not a["store"]:
        return "--store is required"
    if a["kind"] not in KINDS:
        return f"--kind must be one of {list(KINDS)}"
    if not a["runid"]:
        return "--runid is required (the dedup key needs it; pick a fresh id per bookkeeping action)"
    if not a["task_id"] and not a["agent_id"]:
        return "--task-id and/or --agent-id is required (an entry must say whose it is)"
    if a["claimed_tokens"] == "":
        return "--claimed-tokens is required"
    try:
        v = float(a["claimed_tokens"])
    except ValueError:
        return "--claimed-tokens must be a number"
    if a["kind"] in ("spend", "grant") and v <= 0:
        return f"--claimed-tokens must be > 0 for {a['kind']} (a negative spend is a correction)"
    if a["kind"] == "correction" and v == 0:
        return "--claimed-tokens must be non-zero for correction (a no-op is not an event)"
    return ""


def cmd_append(a):
    err = check_entry_args(a)
    if err:
        print(f"attest: {err}")
        return 2
    v = float(a["claimed_tokens"])
    if v.is_integer():
        v = int(v)
    entry, err = append_entry(a["store"], a["kind"], a["agent_id"], a["task_id"],
                              a["runid"], v, a["note"])
    if entry is None:
        print(f"attest: REFUSED -- {err}")
        return 1
    print(f"attest: recorded seq={entry['seq']} {entry['kind']} {fmt_num(entry['claimed_tokens'])} "
          f"(task={entry['task_id'] or '-'} agent={entry['agent_id'] or '-'} "
          f"runid={entry['runid']}) head={entry['payload_hash'][:12]}")
    return 0


def cmd_verify(a):
    if not a["store"]:
        print("attest: --store is required")
        return 2
    path = ledger_path(a["store"])
    if not os.path.exists(path):
        print(f"attest: no ledger yet at {LEDGER_NAME} -- nothing is authenticated "
              f"(verify refuses to say OK about nothing)")
        return 1
    entries, err = load_chain(path)
    if entries is None:
        print(f"attest: chain BROKEN: {err}")
        return 1
    ok, reason = verify_chain(entries)
    if not ok:
        print(f"attest: chain BROKEN: {reason}")
        return 1
    head = entries[-1]["payload_hash"][:12] if entries else GENESIS_PREV[:12]
    print(f"attest: chain OK ({len(entries)} entries, head={head}) -- every payload "
          f"recomputed, linkage and dedup keys intact")
    return 0


def cmd_balance(a):
    if not a["store"]:
        print("attest: --store is required")
        return 2
    task = a["task_id"] or None
    agent = a["agent_id"] or None
    path = ledger_path(a["store"])
    entries, err = load_chain(path)
    if entries is None:
        print(f"attest: balance refused -- ledger untrustworthy: {err}")
        return 1
    if not entries:
        print(f"attest: no ledger yet at {LEDGER_NAME} (nothing authenticated) -- balance 0")
        return 0
    ok, reason = verify_chain(entries)
    if not ok:
        print(f"attest: balance refused -- chain broken: {reason} (numbers off a broken "
              f"book are not numbers)")
        return 1
    scope = f"task '{task}'" if task else (f"agent '{agent}'" if agent else "whole chain")
    b = balance_of(entries, task=task, agent=agent)
    print(f"attest: balance for {scope}")
    print(f"  entries: {b['entries']}  spent: {fmt_num(b['spent'])}  "
          f"granted: {fmt_num(b['granted'])}  corrections: {fmt_num(b['corrections'])}")
    print(f"  spent_net (budget-gate facing): {fmt_num(b['spent_net'])}  "
          f"net (granted - spent + corrections): {fmt_num(b['net'])}")
    return 0


def cmd_import(a):
    if not a["store"]:
        print("attest: --store is required")
        return 2
    if not a["from_ledger"]:
        print("attest: --from-ledger is required")
        return 2
    entries = ledger.load(a["from_ledger"])
    if entries is None:
        print("attest: REFUSED -- cost ledger corrupt (fail-closed, importing from a "
              "half-written book would launder it)")
        return 1
    imported = skipped = 0
    for e in entries:
        if e.get("tokens") is None:
            continue  # wall-only rows are not token accounting; they stay out of the chain
        tag = str(e.get("task", ""))
        task_id = tag[3:] if tag.startswith("av|") else tag
        agent_id = str(e.get("agent_id", "") or "")
        key = str(e.get("key", ""))
        runid = key.split("|")[1] if key.startswith("av|") and key.count("|") >= 2 else key
        note = f"imported from cost-ledger (recorded there at {e.get('at', '?')})"
        entry, err = append_entry(a["store"], "spend", agent_id, task_id, runid,
                                  e["tokens"], note)
        if entry is None:
            if "already recorded" in err:
                skipped += 1
                continue
            print(f"attest: REFUSED -- {err}")
            return 1
        imported += 1
    print(f"attest: import -- imported={imported} skipped={skipped} "
          f"(already in chain, counted once) scanned={len(entries)} cost-ledger entries")
    return 0


def main(argv):
    a = parse_args(argv)
    if not a["mode"]:
        print(f"attest: mode must be one of {['append', 'verify', 'balance', 'import']}")
        return 2
    return {"append": cmd_append, "verify": cmd_verify,
            "balance": cmd_balance, "import": cmd_import}[a["mode"]](a)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
