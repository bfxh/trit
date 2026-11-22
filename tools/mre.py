#!/usr/bin/env python3
"""mre: math-reasoning engine -- a derivation-graph re-executor in exact rational
arithmetic.

    mre.py -Derivation <derivation.json>

A model that states a number should be able to show its work. mre takes that work as a
derivation graph -- steps of exact arithmetic -- and RE-EXECUTES every step, comparing
each declared output against recomputation. The claim is not believed; it is re-derived.

DERIVATION SHAPE (JSON)

    {"id": "overhead",
     "claim": "the ratio is 2.35",
     "target": {"value": "2.35"},
     "steps": [
       {"op": "add", "inputs": [40, 7], "output_id": "s1", "output": "47",
        "note": "40 + 7"},
       {"op": "div", "inputs": ["s1", 20], "output_id": "s2", "output": "2.35",
        "note": "47/20"}]}

  steps[].op      add | sub | mul | div | sqrt | pow
                  add/mul fold over >= 2 inputs; sub/div/pow take exactly 2; sqrt exactly 1
  steps[].inputs  literal numbers (int, decimal string, ratio string like "3/7") or the
                  output_id of another step -- a string that matches some output_id is a
                  reference, so do not name output_ids like bare numbers
  steps[].output  the declared value of the step; the thing re-execution is checked against
  steps[].tol     optional tolerance; REQUIRED exactly when the step's result is non-exact
  steps[].note    REQUIRED -- a derivation without per-step notes is not auditable process
                  data (UNVERIFIED); that is the point of the field
  target          the claim itself, checked against the LAST step's recomputed value;
                  {"value", "tol"?} -- a target without tol demands exact equality

EXACTNESS (no float lies)
  add/sub/mul/div/integer-exponent pow run in Fraction arithmetic and must match exactly
  (a declared tol widens the check to |declared - computed| <= tol). sqrt and fractional
  pow are irrational in general: the engine brackets the true value between two rational
  bounds (bisecting in exact rational arithmetic until the bracket is no wider than the
  declared tol) and requires the declared value to lie inside the bracket. The precision
  boundary is exactly the declared tolerance -- the engine adds no precision of its own,
  and a non-exact step without a declared tol is REJECTED, never guessed. Declared values
  are read as decimal literals (a JSON float is decoded through its repr, the shortest
  round-tripping decimal); exact values beyond float precision must be quoted strings.
  Values are propagated as intervals: a non-exact step poisons nothing silently -- its
  interval flows into downstream steps and every downstream check stays sound.

THREE-STATE OUTPUT (exit 0/1/2; refusals are 3)

  VERIFIED     every step re-executed and matched, target matched (checklist printed)
  UNVERIFIED   no derivation graph (no/empty steps), no target, or a step without a note --
               nothing was checked, nothing may be taken as true
  REJECTED     recomputation contradicts a declared value, or an operation is undefined at
               run time (division by zero, sqrt of a negative, fractional power of a
               negative base, a non-exact result without a declared tolerance): the FIRST
               failing step is named -- more specific than a bare fabrication
  REFUSED (3)  the artifact is not a well-formed derivation: unreadable, bad JSON, not an
               object, unknown op, bad arity, missing/duplicate output_id, unresolved input
               reference, a reference cycle, a declaration that is not a number, negative
               tolerance. Nothing is believed from it.

SCALE GUARDS (refuse, exit 3 -- a graph that cannot be checked cheaply is not checked)
  steps <= 10000; <= 256 inputs per step; literals <= 4096 chars; computed values and
  bracket pre-products <= 100000 digits; bracketing <= 4096 iterations; fractional
  exponent denominator <= 1024. DAG rules: references may point at any step (execution is
  topological); a reference cycle refuses, naming the unresolved steps.

HONEST BOUNDARY
  mre verifies DERIVATION CONSISTENCY: the given steps do or do not compute the claimed
  result. It does NOT verify the truth of the premises -- whether the inputs are real
  measurements is upstream evidence's job (claim_audit's kind=number artifact
  provenance). A derivation can be perfectly consistent and still start from a false
  premise; that is exactly why a derivation-verified number gains a SECOND evidence path
  beside the artifact, never a replacement for it. Role separation vs proof assistants:
  Lean-style systems accept PROPOSITIONS (a statement, under axioms); mre accepts
  ARITHMETIC (steps, under exact recomputation). Neither certifies the other's layer.

CLAIM_AUDIT INTEGRATION
  a kind=number claim may carry "derivation": a root-relative path to a derivation file
  or an inline derivation object. Re-execution VERIFIED gives the number a second
  evidence path (artifact provenance + derivation re-execution) and second-opinion may
  upgrade it to dual-source; REJECTED refuses the claim as FABRICATED with the
  mismatching step; a number without a derivation behaves exactly as before.

SOURCES (industry)
  - industry process-supervision evidence, "Let's Verify Step by Step" (2023): per-step
    judgment beats result-only judgment; a derivation trace with per-step notes and
    verdicts is that training-data shape.
  - industry exploration of the Navier--Stokes blow-up claim with a Lean 4 formalization
    (Alpoge--Buckmaster line, assumption-state exploration): the reusable engineering
    shape is "turn the claim into a machine-checkable object". This engine depends on
    neither the truth nor the falsehood of that claim.

Output is pure ASCII. Python 3 stdlib only (fractions).
"""

import json
import os
import re
import sys
from collections import deque
from fractions import Fraction

MAX_STEPS = 10000
MAX_INPUTS = 256
MAX_LITERAL_CHARS = 4096
MAX_LITERAL_EXP = 100000
MAX_VALUE_DIGITS = 100000
MAX_BRACKET_ITERS = 4096
MAX_EXP_DENOM = 1024

LITERAL_EXP_RX = re.compile(r"[eE]([+-]?\d+)")

OPS = ("add", "sub", "mul", "div", "sqrt", "pow")
ARITY = {"add": (2, None), "mul": (2, None), "sub": (2, 2), "div": (2, 2),
         "sqrt": (1, 1), "pow": (2, 2)}

VERIFIED, UNVERIFIED, REJECTED, REFUSED = "VERIFIED", "UNVERIFIED", "REJECTED", "REFUSED"


class Refuse(Exception):
    """Structural: the artifact is not a well-formed derivation (exit 3)."""


class Reject(Exception):
    """Run-time arithmetic failure in a named step (exit 2)."""


def ascii_safe(text, limit=160):
    t = str(text).encode("ascii", "backslashreplace").decode("ascii")
    return t if len(t) <= limit else t[:limit - 3] + "..."


def to_fraction(x):
    """Decimal-literal semantics, never binary floats: a float goes through repr (the
    shortest decimal that round-trips), ints and strings go to Fraction directly."""
    if isinstance(x, bool) or x is None:
        raise ValueError("not a number")
    if isinstance(x, Fraction):
        return x
    if isinstance(x, int):
        return Fraction(x)
    if isinstance(x, float):
        if x != x or x in (float("inf"), float("-inf")):
            raise ValueError("not a finite number")
        return Fraction(repr(x))
    if isinstance(x, str):
        s = x.strip()
        if not s or len(s) > MAX_LITERAL_CHARS:
            raise ValueError("bad numeric literal")
        for m in LITERAL_EXP_RX.finditer(s):
            if abs(int(m.group(1))) > MAX_LITERAL_EXP:
                # 10**10^9 inside the Fraction constructor would chew memory before any
                # digit guard could run -- the exponent is refused at the door instead.
                raise ValueError("literal exponent beyond the guard")
        try:
            return Fraction(s)
        except (ValueError, ZeroDivisionError):
            raise ValueError(f"bad numeric literal: {s[:48]}")
    raise ValueError("not a number")


def digits_of(n):
    return len(str(abs(n)))


def digit_guard(fr, what):
    total = digits_of(fr.numerator) + digits_of(fr.denominator)
    if total > MAX_VALUE_DIGITS:
        raise Refuse(f"magnitude guard: {what} needs {total} digits (cap {MAX_VALUE_DIGITS})")


def iroot(n, q):
    """floor(n ** (1/q)) for integer n >= 0, q >= 1 -- pure integer binary search."""
    if q == 1 or n < 2:
        return n
    hi = 1 << ((n.bit_length() + q - 1) // q + 1)
    lo = 0
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if mid ** q <= n:
            lo = mid
        else:
            hi = mid - 1
    return lo


def exact_root(t, q):
    """The exact q-th root of rational t >= 0, or None when t is not a perfect q-th power."""
    if t < 0:
        raise Reject("q-th root of a negative value")
    if t == 0:
        return Fraction(0)
    d = t.denominator
    pre = digits_of(t.numerator) + (q - 1) * digits_of(d)
    if pre > MAX_VALUE_DIGITS:
        raise Refuse(f"magnitude guard: q-th-root pre-product needs {pre} digits (cap {MAX_VALUE_DIGITS})")
    m = iroot(t.numerator * d ** (q - 1), q)
    f = Fraction(m, d)
    return f if f ** q == t else None


def root_bracket(t, q, tol):
    """Rational bracket [f, c] of the q-th root of exact rational t >= 0:
    f^q <= t <= c^q and c - f <= tol (an exact point when t is a perfect q-th power)."""
    r = exact_root(t, q)
    if r is not None:
        return r, r
    d = t.denominator
    m = iroot(t.numerator * d ** (q - 1), q)
    f = Fraction(m, d)
    c = Fraction(m + 1, d)
    it = 0
    while c - f > tol:
        it += 1
        if it > MAX_BRACKET_ITERS:
            raise Refuse("bracketing iteration guard tripped (declared tolerance finer "
                         "than the engine will bisect)")
        mid = (f + c) / 2
        if mid ** q <= t:
            f = mid
        else:
            c = mid
    return f, c


def ival_add(a, b):
    return a[0] + b[0], a[1] + b[1]


def ival_sub(a, b):
    return a[0] - b[1], a[1] - b[0]


def ival_mul(a, b):
    c = (a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1])
    return min(c), max(c)


def ival_div(a, b):
    if b[0] <= 0 <= b[1]:
        raise Reject("division by zero (the denominator is or could be zero)")
    c = (a[0] / b[0], a[0] / b[1], a[1] / b[0], a[1] / b[1])
    return min(c), max(c)


def ival_sqrt(x, tol):
    """Bracket of sqrt over interval x >= 0. Exact input + perfect square -> exact point
    (no tol needed); otherwise tol is required and each side's slack is <= tol/2, so an
    exact input yields a bracket no wider than tol."""
    if x[0] < 0:
        raise Reject("sqrt of a negative (or possibly negative) value")
    if x[0] == x[1]:
        r = exact_root(x[0], 2)
        if r is not None:
            return r, r
    if tol is None:
        raise Reject("non-exact op sqrt requires a declared tol (the precision boundary "
                     "must be stated, not implied)")
    half = tol / 2
    return root_bracket(x[0], 2, half)[0], root_bracket(x[1], 2, half)[1]


def _root_of_power(x, p, q, half):
    """Bracket of x^(p/q) for exact rational x >= 0; slack <= half per side."""
    if x == 0:
        if p <= 0:
            raise Reject("zero base with a non-positive exponent")
        return Fraction(0), Fraction(0)
    est = abs(p) * max(digits_of(x.numerator), digits_of(x.denominator))
    if est > MAX_VALUE_DIGITS:
        raise Refuse(f"magnitude guard: base**{p} pre-estimate {est} digits (cap {MAX_VALUE_DIGITS})")
    return root_bracket(x ** p, q, half)


def ival_pow(b, e, tol):
    if e[0] != e[1]:
        raise Reject("pow exponent must be an exact value (interval exponents are not supported)")
    k = e[0]
    if k.denominator == 1:
        n = k.numerator
        if n == 0:
            if b[0] <= 0 <= b[1]:
                raise Reject("0^0 is undefined (the base is or could be zero)")
            return Fraction(1), Fraction(1)
        for v in b:
            est = abs(n) * max(digits_of(v.numerator), digits_of(v.denominator))
            if est > MAX_VALUE_DIGITS:
                raise Refuse(f"magnitude guard: base**{n} pre-estimate {est} digits (cap {MAX_VALUE_DIGITS})")
        if n < 0 and b[0] <= 0 <= b[1]:
            raise Reject("division by zero (negative exponent, the base is or could be zero)")
        vals = [b[0] ** n, b[1] ** n]
        if n > 0 and b[0] <= 0 <= b[1]:
            vals.append(Fraction(0))
        lo, hi = min(vals), max(vals)
        digit_guard(lo, "pow result")
        digit_guard(hi, "pow result")
        return lo, hi
    p, q = k.numerator, k.denominator
    if q > MAX_EXP_DENOM:
        raise Refuse(f"fractional exponent denominator {q} exceeds the guard {MAX_EXP_DENOM}")
    if b[0] < 0:
        raise Reject("fractional power of a negative (or possibly negative) base")
    if tol is None:
        if b[0] != b[1]:
            raise Reject("non-exact op pow requires a declared tol (the precision boundary "
                         "must be stated, not implied)")
        if b[0] == 0:
            if p <= 0:
                raise Reject("zero base with a non-positive exponent")
            return Fraction(0), Fraction(0)
        est = abs(p) * max(digits_of(b[0].numerator), digits_of(b[0].denominator))
        if est > MAX_VALUE_DIGITS:
            raise Refuse(f"magnitude guard: base**{p} pre-estimate {est} digits (cap {MAX_VALUE_DIGITS})")
        r = exact_root(b[0] ** p, q)
        if r is None:
            raise Reject("non-exact op pow requires a declared tol (the precision boundary "
                         "must be stated, not implied)")
        return r, r
    half = tol / 2
    lo_b = _root_of_power(b[0], p, q, half)
    hi_b = _root_of_power(b[1], p, q, half)
    if p >= 0:
        return lo_b[0], hi_b[1]
    return hi_b[0], lo_b[1]


def parse_tol(raw, what):
    if raw is None:
        return None
    try:
        tol = to_fraction(raw)
    except ValueError:
        raise Refuse(f"{what} tol {raw!r} is not a number")
    if tol < 0:
        raise Refuse(f"{what} tol must be >= 0")
    digit_guard(tol, f"{what} tol")
    return tol


def parse_declared(raw, what):
    try:
        v = to_fraction(raw)
    except ValueError:
        raise Refuse(f"{what} declared value {raw!r} is not a number")
    digit_guard(v, f"{what} declared value")
    return v


def fmt(fr):
    return str(fr)


def fmt_ival(iv):
    lo, hi = iv
    return fmt(lo) if lo == hi else f"[{fmt(lo)}, {fmt(hi)}]"


def step_check(declared, comp, tol):
    """(ok, why). Point result: exact equality, or the declared tol band. Interval result
    (non-exact arithmetic or inherited upstream slack): containment, plus the interval
    must honor the declared tolerance when one is stated."""
    lo, hi = comp
    if lo == hi:
        if tol is None:
            if declared != lo:
                return False, (f"declared {fmt(declared)} vs computed {fmt(lo)} "
                               f"(exact mismatch)")
            return True, ""
        if abs(declared - lo) > tol:
            return False, (f"declared {fmt(declared)} vs computed {fmt(lo)} "
                           f"(|diff| {fmt(abs(declared - lo))} > tol {fmt(tol)})")
        return True, ""
    if tol is not None and hi - lo > tol:
        return False, (f"re-executed interval width {fmt(hi - lo)} exceeds the declared "
                       f"tol {fmt(tol)} (upstream precision is looser than this step claims)")
    if not (lo <= declared <= hi):
        return False, (f"declared {fmt(declared)} outside the re-executed interval "
                       f"[{fmt(lo)}, {fmt(hi)}]")
    return True, ""


def compute_step(st, idx, env):
    """Recompute one step -> interval. Refuse on structure, Reject on run-time math."""
    op = st["op"]
    vals = []
    for inp in st["inputs"]:
        if isinstance(inp, str) and inp in env:
            vals.append(env[inp])
        else:
            try:
                v = to_fraction(inp)
            except ValueError:
                raise Refuse(f"input {ascii_safe(inp, 60)!r} is neither a literal number "
                             f"nor a step output_id")
            digit_guard(v, "literal input")
            vals.append((v, v))
    tol = parse_tol(st.get("tol"), f"step {idx}")
    if op == "add":
        lo, hi = vals[0]
        for v in vals[1:]:
            lo, hi = lo + v[0], hi + v[1]
        return lo, hi
    if op == "sub":
        return ival_sub(vals[0], vals[1])
    if op == "mul":
        lo, hi = vals[0]
        for v in vals[1:]:
            lo, hi = ival_mul((lo, hi), v)
        return lo, hi
    if op == "div":
        comp = vals[0]
        for v in vals[1:]:
            comp = ival_div(comp, v)
        return comp
    if op == "sqrt":
        return ival_sqrt(vals[0], tol)
    return ival_pow(vals[0], vals[1], tol)


def _result(verdict, reason, steps):
    return {"verdict": verdict, "reason": reason, "steps": steps, "n_steps": len(steps)}


def evaluate_doc(doc):
    """Full derivation evaluation -> result dict. Raises Refuse on structural problems."""
    if not isinstance(doc, dict):
        raise Refuse("derivation is not a JSON object")
    steps = doc.get("steps")
    if steps is None:
        return _result(UNVERIFIED, "no derivation graph (no steps) -- nothing to re-execute", [])
    if not isinstance(steps, list):
        raise Refuse("steps must be a list")
    if len(steps) > MAX_STEPS:
        raise Refuse(f"steps {len(steps)} exceeds the scale guard {MAX_STEPS}")
    if not steps:
        return _result(UNVERIFIED, "empty step list -- nothing to re-execute", [])

    ids = []
    seen = set()
    for i, st in enumerate(steps, 1):
        if not isinstance(st, dict):
            raise Refuse(f"step {i} is not an object")
        op = st.get("op")
        if op not in ARITY:
            raise Refuse(f"step {i}: unknown op {ascii_safe(op, 40)!r}")
        inputs = st.get("inputs")
        if not isinstance(inputs, list):
            raise Refuse(f"step {i}: inputs must be a list")
        low, high = ARITY[op]
        if len(inputs) < low or (high is not None and len(inputs) != high):
            want = f">= {low}" if high is None else f"exactly {low}"
            raise Refuse(f"step {i}: op {op} needs {want} input(s), got {len(inputs)}")
        if len(inputs) > MAX_INPUTS:
            raise Refuse(f"step {i}: {len(inputs)} inputs exceeds the guard {MAX_INPUTS}")
        oid = st.get("output_id")
        if not isinstance(oid, str) or not oid.strip():
            raise Refuse(f"step {i}: missing output_id")
        if oid in seen:
            raise Refuse(f"step {i}: duplicate output_id {ascii_safe(oid, 60)!r}")
        seen.add(oid)
        ids.append(oid)

    for i, st in enumerate(steps, 1):
        note = st.get("note")
        if not isinstance(note, str) or not note.strip():
            return _result(UNVERIFIED, f"step {i} carries no note -- a derivation without "
                                       f"per-step notes is not auditable process data", [])

    target = doc.get("target")
    if not isinstance(target, dict) or "value" not in target:
        return _result(UNVERIFIED, "steps present but no target declared -- nothing to "
                                   "verify the re-execution against", [])
    tval = parse_declared(target["value"], "target")
    ttol = parse_tol(target.get("tol"), "target")

    # DAG validation + deterministic topological order (Kahn, ready set in list order).
    id_pos = {oid: i for i, oid in enumerate(ids)}
    dependents = [[] for _ in steps]
    indeg = [0] * len(steps)
    for i, st in enumerate(steps):
        for inp in st["inputs"]:
            j = id_pos.get(inp) if isinstance(inp, str) else None
            if j is not None:
                indeg[i] += 1
                dependents[j].append(i)
    ready = deque(i for i in range(len(steps)) if indeg[i] == 0)
    order = []
    while ready:
        i = ready.popleft()
        order.append(i)
        for w in dependents[i]:
            indeg[w] -= 1
            if indeg[w] == 0:
                ready.append(w)
    if len(order) != len(steps):
        stuck = ", ".join(ascii_safe(ids[i], 40) for i in range(len(steps)) if indeg[i] > 0)[:200]
        raise Refuse(f"reference cycle detected: unresolved step(s) {stuck}")

    env = {}
    checklist = []
    for i in order:
        st = steps[i]
        rec = {"step": i + 1, "op": st["op"], "output_id": ids[i],
               "inputs": [ascii_safe(x, 40) for x in st["inputs"]], "status": "ok"}
        try:
            comp = compute_step(st, i + 1, env)
            digit_guard(comp[0], f"step {i + 1} result")
            digit_guard(comp[1], f"step {i + 1} result")
            declared = parse_declared(st.get("output", ""), f"step {i + 1}")
            tol = parse_tol(st.get("tol"), f"step {i + 1}")
        except Reject as ex:
            rec["status"] = "UNDEFINED"
            rec["detail"] = ascii_safe(str(ex))
            checklist.append(rec)
            return _result(REJECTED, f"step {i + 1} ({ascii_safe(ids[i], 60)}): {ex}", checklist)
        ok, why = step_check(declared, comp, tol)
        if not ok:
            rec["status"] = "MISMATCH"
            rec["declared"] = fmt(declared)
            rec["computed"] = fmt_ival(comp)
            rec["detail"] = ascii_safe(why)
            checklist.append(rec)
            return _result(REJECTED, f"step {i + 1} ({ascii_safe(ids[i], 60)}): {why}", checklist)
        rec["declared"] = fmt(declared)
        rec["computed"] = fmt_ival(comp)
        env[ids[i]] = comp
        checklist.append(rec)

    lo, hi = env[ids[-1]]
    tname = f"target {fmt(tval)}"
    if lo == hi:
        if ttol is None:
            ok = tval == lo
            why = f"{tname} vs recomputed {fmt(lo)} (exact mismatch)" if not ok else ""
        else:
            ok = abs(tval - lo) <= ttol
            why = (f"{tname} vs recomputed {fmt(lo)} (|diff| {fmt(abs(tval - lo))} > "
                   f"target tol {fmt(ttol)})") if not ok else ""
    elif ttol is None:
        ok = False
        why = ("target demands exact equality but the re-executed result is an interval "
               "(declare a tolerance)")
    elif hi - lo > ttol:
        ok = False
        why = (f"re-executed interval width {fmt(hi - lo)} exceeds the target tol "
               f"{fmt(ttol)}")
    else:
        ok = lo <= tval <= hi
        why = "" if ok else (f"{tname} outside the re-executed interval "
                             f"[{fmt(lo)}, {fmt(hi)}]")
    if not ok:
        return _result(REJECTED, why, checklist)
    return _result(VERIFIED, f"{len(steps)} step(s) re-executed in exact rational "
                             f"arithmetic; target matches ({fmt_ival((lo, hi))})", checklist)


def evaluate(doc):
    """evaluate_doc with structural refusals folded into the result (REFUSED verdict)."""
    try:
        return evaluate_doc(doc)
    except Refuse as ex:
        return _result(REFUSED, str(ex), [])


USAGE = """mre: verify a derivation file by re-executing it in exact rational arithmetic.

  mre.py -Derivation <derivation.json>

Exit codes: 0 VERIFIED; 1 UNVERIFIED (no steps / no target / a step without note);
2 REJECTED (first mismatching step named); 3 REFUSED (bad JSON, bad shape, cycle,
scale guard, magnitude guard). See the module docstring for the full contract.
"""


def main(argv):
    der = ""
    i = 0
    while i < len(argv):
        a = argv[i]
        name = a.lstrip("-").lower()
        if name in ("derivation", "d"):
            der = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
            continue
        if a.startswith("-"):
            print(f"mre: unknown flag {a}")
            return 3
        print(f"mre: unexpected argument {a}")
        return 3
    if not der:
        print(USAGE)
        return 3
    if not os.path.isfile(der):
        print(f"mre: REFUSED -- derivation file does not exist: {ascii_safe(der)}")
        return 3
    try:
        with open(der, "r", encoding="utf-8-sig") as fh:
            doc = json.load(fh)
    except (OSError, ValueError) as ex:
        print(f"mre: REFUSED -- derivation unreadable ({ascii_safe(str(ex), 120)})")
        return 3
    res = evaluate(doc)
    v = res["verdict"]
    print(f"mre: {v} -- {ascii_safe(res['reason'], 400)}")
    for s in res["steps"]:
        line = (f"  step {s['step']} {s['op']} -> {ascii_safe(s['output_id'], 40)}  "
                f"declared {s.get('declared', '-')}  computed {s.get('computed', '-')}")
        if s["status"] != "ok":
            line += f"  {s['status']}: {s.get('detail', '')}"
        print(line)
    return {"VERIFIED": 0, "UNVERIFIED": 1, "REJECTED": 2, "REFUSED": 3}[v]


if __name__ == "__main__":
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
        except Exception:
            pass
    sys.exit(main(sys.argv[1:]))
